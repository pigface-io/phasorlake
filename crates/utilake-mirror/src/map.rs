//! A map of every file of the mirror: every folder and file the crawl
//! listed, every archived file nemweb no longer lists, and inside every zip
//! (zips within zips too) every folder and file, as one tree.
//!
//! The map is built offline from the crawl record and the archive. It is
//! written twice: `map/map.ndjson`, one node per line, and `map/tree.txt`,
//! the same nodes indented as a tree. Both are in tree order (a folder
//! before what it holds, names in byte order), so two maps of the same
//! archive are the same bytes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, Write as _};
use std::path::{Path, PathBuf};

use crate::net::Archive;
use serde::Serialize;
use url::Url;
use zip::ZipArchive;

use crate::compare::Removed;
use crate::crawl::{Mirror, RemoteKind};
use crate::error::MirrorError;
use crate::hosts::Offsets;
use crate::peek::{Peeked, current, latest};
use crate::stamp::{AEST, Stamp};
use crate::zipdir::Entry;

/// A nested zip up to this size is read into memory; a bigger one is
/// spooled to a temporary file.
const NESTED_IN_MEMORY: u64 = 64 << 20;
/// Zips nested deeper than this are not opened (`zip.nesting_too_deep`).
pub const MAX_DEPTH: usize = 8;
/// A zip inside a zip bigger than this, as declared, is not unpacked
/// (`zip.too_big`).
pub const MAX_NESTED: u64 = 16 << 30;
/// Entries listed per archived file, zips within it included; a zip that
/// would pass it is not listed (`zip.too_many_entries`).
pub const MAX_ENTRIES: u64 = 10_000_000;
/// The version of the catalog's format, on its first line.
pub const CATALOG_FORMAT: u32 = 3;
/// The bytes every zip with at least one entry starts with, and an empty one.
const ZIP_MAGIC: [&[u8; 4]; 2] = [b"PK\x03\x04", b"PK\x05\x06"];
/// The buffer of a zip read from disk, which is read by seeking: small,
/// since every seek drops it.
const SEEK_BUFFER: usize = 64 << 10;
/// The buffer of each map file written.
const WRITE_BUFFER: usize = 1 << 20;

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A folder on nemweb, or inside a zip.
    Dir,
    /// A file that is not a zip.
    File,
    /// A zip: a file that is also a folder of what it holds.
    Zip,
}

/// Where a file of nemweb stands against the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Listed and archived.
    Archived,
    /// Listed, not yet archived.
    NotArchived,
    /// Archived, no longer listed by nemweb.
    GoneUpstream,
    /// Removed from the archive: a zip found the same as its folder.
    Removed,
    /// Another version of a file nemweb rewrote in place, kept beside it
    /// as `{name}@{at}` (`docs/archive.md`).
    Version,
}

/// Why a file or zip could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeError {
    /// The diagnostic code.
    pub code: &'static str,
    /// What happened.
    pub message: String,
}

/// One folder or file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    /// The file or folder nemweb lists, at its path under the archive
    /// (`docs/archive.md`); for an entry inside a zip, the zip's.
    pub file: String,
    /// The exact URL of `file`, as crawled or as its sidecar records it;
    /// none only for the folders above the roots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Where inside `file` it is: one path per zip, outermost first, each
    /// with folders separated by `/`; empty for `file` itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inside: Vec<String>,
    /// Folder, file or zip.
    pub kind: NodeKind,
    /// Size in bytes (uncompressed inside a zip).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Compressed size, inside a zip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed: Option<u64>,
    /// CRC-32: as the zip records it for an entry; from its sidecar for a
    /// downloaded file, when recorded there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crc32: Option<u32>,
    /// sha256 hex of a downloaded file, from its sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// The server's `ETag`: from the sidecar, or from the peek of a zip not
    /// downloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The server's `Last-Modified`, from the same.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// `last_modified` as an instant (UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified_at: Option<Stamp>,
    /// When a zip not downloaded had its listing read (UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peeked_at: Option<Stamp>,
    /// For a removed zip: how many of its files were found the same as the
    /// folder's, each listed under it with its CRC-32.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<usize>,
    /// For a removed zip: when it was found the same (UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<Stamp>,
    /// Date and time as nemweb lists it, or as the zip records it; for a
    /// file nemweb no longer lists, the server's `Last-Modified`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// `modified` as an instant: AEST for a listing's or a zip's time, UTC
    /// for a `Last-Modified`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Stamp>,
    /// For a file nemweb lists or listed: archived or not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,
    /// For a removed zip, the folder that holds the same files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub same_as: Option<String>,
    /// `needs_download`: a zip inside a zip known only from its listing,
    /// whose own entries need the outer zip downloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contents: Option<&'static str>,
    /// Why it could not be read, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<NodeError>,
    /// Depth in the tree, the host at 0.
    #[serde(skip)]
    pub depth: usize,
}

impl Node {
    fn new(file: String, kind: NodeKind, depth: usize) -> Self {
        Self {
            file,
            url: None,
            inside: Vec::new(),
            kind,
            size: None,
            compressed: None,
            crc32: None,
            sha256: None,
            etag: None,
            last_modified: None,
            last_modified_at: None,
            peeked_at: None,
            matched: None,
            removed_at: None,
            modified: None,
            modified_at: None,
            state: None,
            same_as: None,
            contents: None,
            error: None,
            depth,
        }
    }
}

/// What the map holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MapSummary {
    /// Folders, on nemweb and inside zips.
    pub dirs: usize,
    /// Files that are not zips.
    pub files: usize,
    /// Zips, nested ones included.
    pub zips: usize,
    /// Listed files not yet archived.
    pub not_archived: usize,
    /// Archived files nemweb no longer lists.
    pub gone_upstream: usize,
    /// Zips removed as the same as their folders.
    pub removed: usize,
    /// Later versions of files nemweb rewrote in place.
    pub versions: usize,
    /// Files or zips that could not be read.
    pub errors: usize,
    /// Bytes of the archived files nemweb lists or listed.
    pub archived_bytes: u64,
}

/// A file or folder nemweb lists, or an archived file it listed once.
#[derive(Debug, Clone)]
struct Top {
    /// The URL, for what the crawl or the archive knows it by.
    url: Option<String>,
    kind: NodeKind,
    size: Option<u64>,
    modified: Option<String>,
    modified_at: Option<Stamp>,
    state: Option<State>,
    same_as: Option<String>,
    /// What the zip holds, from its listing, when it is not downloaded.
    peek: Option<Peeked>,
    /// The record of its removal, when it was removed.
    removed: Option<Removed>,
}

/// A node's place in the tree: its archive path components, and, for a
/// file, its URL, since two URLs can share one path (both listings of a
/// report folder are one folder) and neither may replace the other.
type TopKey = (Vec<String>, String);

/// Writes the map of the crawl record and the archive under `roots`,
/// reading zips on `workers` threads.
pub fn map(
    mirror: &Mirror,
    archive: &Archive,
    roots: &[Url],
    workers: usize,
    offsets: &Offsets,
) -> Result<MapSummary, MirrorError> {
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.crawl_dir().display().to_string(),
        });
    }
    let tops = tops(mirror, archive, roots)?;
    let dir = mirror.dir().join("map");
    let tmp = mirror.dir().join("tmp");
    for d in [&dir, &tmp] {
        std::fs::create_dir_all(d).map_err(MirrorError::io(d))?;
    }
    let ndjson_path = dir.join("map.ndjson");
    let tree_path = dir.join("tree.txt");
    let header = serde_json::json!({
        "catalog_format": CATALOG_FORMAT,
        "roots": roots.iter().map(Url::as_str).collect::<Vec<_>>(),
    });
    let mut out = Output::create(&ndjson_path, &tree_path, &header)?;
    let mut listers: Vec<Lister> = (0..workers.max(1))
        .map(|_| Lister::new(tmp.clone()))
        .collect();
    let jobs: Vec<(TopKey, Top)> = tops.into_iter().collect();
    for batch in jobs.chunks(listers.len() * 4) {
        let results = list_batch(archive, batch, &mut listers, offsets);
        for nodes in results {
            for node in &nodes {
                out.write(node)?;
            }
        }
    }
    let summary = out.finish(&ndjson_path, &tree_path)?;
    let _ = std::fs::remove_dir(&tmp);
    Ok(summary)
}

/// Every folder and file of the crawl under the roots, every archived file
/// under the roots the crawl no longer lists, and every folder above them,
/// keyed by archive path components, which orders them as a tree.
fn tops(
    mirror: &Mirror,
    archive: &Archive,
    roots: &[Url],
) -> Result<BTreeMap<TopKey, Top>, MirrorError> {
    let path_key = |path: &Path| -> Vec<String> {
        path.strip_prefix(archive.root())
            .unwrap_or(path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    let key = |url: &str| path_key(&archive.path_for(url));
    let mut tops = BTreeMap::new();
    let mut listed = HashSet::new();
    let removed = mirror.removed()?;
    let mut peeks: HashMap<String, Peeked> = current(mirror)?;
    let mut evidence: HashMap<String, Peeked> = latest(mirror)?;
    let record = mirror.record()?;
    // Paths two listed files share: each URL's file is found by its
    // sidecar there, the first file or a version beside it.
    let mut at_path: HashMap<PathBuf, usize> = HashMap::new();
    for r in record.iter().filter(|r| r.kind == RemoteKind::File) {
        *at_path.entry(archive.path_for(&r.url)).or_default() += 1;
    }
    let mut shown: HashSet<PathBuf> = HashSet::new();
    for r in record {
        let kind = match r.kind {
            RemoteKind::External => continue,
            RemoteKind::Dir => NodeKind::Dir,
            RemoteKind::File => NodeKind::File,
        };
        if !roots.iter().any(|root| r.url.starts_with(root.as_str())) {
            continue;
        }
        let gone = removed.get(&r.url);
        let path = archive.path_for(&r.url);
        let file = match kind == NodeKind::File && at_path.get(&path).is_some_and(|&n| n > 1) {
            true => {
                let found = archive.lookup(&r.url)?.map(|a| a.path);
                shown.extend(found.clone());
                Some(found)
            }
            false => None,
        };
        let state = (kind == NodeKind::File).then(|| {
            if gone.is_some() {
                State::Removed
            } else if file
                .as_ref()
                .map_or_else(|| path.is_file(), Option::is_some)
            {
                State::Archived
            } else {
                State::NotArchived
            }
        });
        let url_key = match kind {
            NodeKind::Dir => String::new(),
            _ => r.url.clone(),
        };
        if kind == NodeKind::Dir && tops.contains_key(&(key(&r.url), String::new())) {
            continue;
        }
        // A file held as a version beside another URL's is at its own path.
        let node = match file.as_ref().and_then(Option::as_deref) {
            Some(found) => path_key(found),
            None => key(&r.url),
        };
        tops.insert(
            (node, url_key),
            Top {
                url: Some(r.url.clone()),
                kind,
                size: r.size,
                modified: Some(r.modified),
                modified_at: r.modified_at,
                state,
                same_as: gone.map(|g| g.same_as.clone()),
                // A zip not downloaded shows its current listing; a removed
                // one the listing its removal stood on.
                peek: match state {
                    Some(State::NotArchived) => peeks.remove(&r.url),
                    Some(State::Removed) => evidence.remove(&r.url),
                    _ => None,
                },
                removed: gone.cloned(),
            },
        );
        listed.insert(r.url);
    }
    for root in roots {
        tops.entry((key(root.as_str()), String::new()))
            .or_insert(Top {
                url: Some(root.to_string()),
                kind: NodeKind::Dir,
                size: None,
                modified: None,
                modified_at: None,
                state: None,
                same_as: None,
                peek: None,
                removed: None,
            });
        for a in archive.entries_under(Some(&archive.path_for(root.as_str()))) {
            if shown.contains(&a.path) {
                continue;
            }
            // A version beside the URL's path, listed or not, is its own
            // node; the first version is the listed file, or one gone.
            let (modified, modified_at, state) = if a.path != archive.path_for(&a.sidecar.url) {
                let s = &a.sidecar;
                (
                    s.listed_modified.clone(),
                    s.listed_modified_at,
                    State::Version,
                )
            } else if !listed.contains(&a.sidecar.url) {
                let s = &a.sidecar;
                (
                    s.last_modified.clone(),
                    s.last_modified_at,
                    State::GoneUpstream,
                )
            } else {
                continue;
            };
            tops.insert(
                (path_key(&a.path), a.sidecar.url.clone()),
                Top {
                    url: Some(a.sidecar.url.clone()),
                    kind: NodeKind::File,
                    size: Some(a.sidecar.size),
                    modified,
                    modified_at,
                    state: Some(state),
                    same_as: None,
                    peek: None,
                    removed: None,
                },
            );
        }
    }
    let mut parents = Vec::new();
    for (k, _) in tops.keys() {
        for n in 1..k.len() {
            parents.push(k[..n].to_vec());
        }
    }
    for p in parents {
        tops.entry((p, String::new())).or_insert(Top {
            url: None,
            kind: NodeKind::Dir,
            size: None,
            modified: None,
            modified_at: None,
            state: None,
            same_as: None,
            peek: None,
            removed: None,
        });
    }
    Ok(tops)
}

/// The nodes of each item of a batch, in batch order; the archived files
/// are read on the listers' threads.
fn list_batch(
    archive: &Archive,
    batch: &[(TopKey, Top)],
    listers: &mut [Lister],
    offsets: &Offsets,
) -> Vec<Vec<Node>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<Vec<Node>> = vec![Vec::new(); batch.len()];
    std::thread::scope(|s| {
        let handles: Vec<_> = listers
            .iter_mut()
            .map(|lister| {
                let next = &next;
                s.spawn(move || {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((key, top)) = batch.get(i) else {
                            break done;
                        };
                        lister.offset = top.url.as_deref().map_or(AEST, |u| offsets.offset_of(u));
                        done.push((i, lister.top(archive, &key.0, top)));
                    }
                })
            })
            .collect();
        for h in handles {
            for (i, nodes) in h.join().expect("a lister does not panic") {
                results[i] = nodes;
            }
        }
    });
    results
}

/// One worker's buffers: a nested zip's bytes per level of nesting.
struct Lister {
    levels: Vec<Vec<u8>>,
    tmp: PathBuf,
    in_memory: u64,
    max_nested: u64,
    max_entries: u64,
    /// Entries this archived file may still list.
    entries_left: u64,
    /// Seconds east of UTC of the DOS times in the zip being read: its
    /// host's clock.
    offset: i32,
}

impl Lister {
    fn new(tmp: PathBuf) -> Self {
        Self {
            levels: Vec::new(),
            tmp,
            in_memory: NESTED_IN_MEMORY,
            max_nested: MAX_NESTED,
            max_entries: MAX_ENTRIES,
            entries_left: MAX_ENTRIES,
            offset: AEST,
        }
    }

    /// The buffer for a zip's bytes `level` zips deep: 0 for an archived
    /// file, 1 for a zip inside it, and so on.
    fn level(&mut self, level: usize) -> &mut Vec<u8> {
        if self.levels.len() <= level {
            self.levels.resize_with(level + 1, Vec::new);
        }
        &mut self.levels[level]
    }

    /// The node of a folder or file nemweb lists and, for an archived zip,
    /// the nodes of what it holds.
    fn top(&mut self, archive: &Archive, key: &[String], top: &Top) -> Vec<Node> {
        let mut node = Node::new(key.join("/"), top.kind, key.len() - 1);
        node.url.clone_from(&top.url);
        node.size = top.size;
        node.modified.clone_from(&top.modified);
        node.modified_at = top.modified_at;
        node.state = top.state;
        node.same_as.clone_from(&top.same_as);
        if let Some(r) = &top.removed {
            node.matched = Some(r.files);
            node.removed_at = Some(r.removed_at);
        }
        if let Some(p) = &top.peek {
            node.etag.clone_from(&p.etag);
            node.last_modified.clone_from(&p.last_modified);
            node.last_modified_at = p.last_modified_at;
            node.peeked_at = p.peeked_at;
        }
        if matches!(top.state, Some(State::NotArchived | State::Removed))
            || top.kind == NodeKind::Dir
        {
            let mut out = vec![node];
            if let Some(p) = &top.peek {
                listed(&mut out, p);
            }
            return out;
        }
        self.entries_left = self.max_entries;
        let file = archive.root().join(key.iter().collect::<PathBuf>());
        let mut out = vec![node];
        // A zip is read by seeking to each entry, and a `BufReader` drops
        // its buffer on every seek: a file up to the in-memory limit is read
        // once into the lister's top-level buffer instead, a bigger one
        // through a small buffer.
        let opened = File::open(&file).and_then(|mut f| {
            let size = f.metadata()?.len();
            if size <= self.in_memory {
                let mut buf = std::mem::take(self.level(0));
                buf.clear();
                f.read_to_end(&mut buf)?;
                Ok((size, Source::Memory(buf)))
            } else {
                Ok((size, Source::File(BufReader::with_capacity(SEEK_BUFFER, f))))
            }
        });
        let (size, source) = match opened {
            Ok(o) => o,
            Err(e) => {
                out[0].error = Some(NodeError {
                    code: "mirror.io",
                    message: format!("Reading {} failed: {e}", file.display()),
                });
                return out;
            }
        };
        out[0].size = Some(size);
        if let Ok(Some(s)) = Archive::read_sidecar(&file).map(|s| s.filter(|s| s.size == size)) {
            out[0].sha256 = Some(s.sha256);
            out[0].crc32 = s.crc32;
            out[0].etag = s.etag;
            out[0].last_modified = s.last_modified;
            out[0].last_modified_at = s.last_modified_at;
        }
        let at = out[0].clone();
        let listed = match source {
            Source::Memory(buf) => {
                let listed = starts_as_zip(&buf).then(|| {
                    out[0].kind = NodeKind::Zip;
                    self.zip(Cursor::new(&buf[..]), &at, 1, &mut out)
                });
                *self.level(0) = buf;
                listed
            }
            Source::File(mut r) => match is_zip(&mut r).and_then(|z| r.rewind().map(|()| z)) {
                Ok(true) => {
                    out[0].kind = NodeKind::Zip;
                    Some(self.zip(r, &at, 1, &mut out))
                }
                Ok(false) => None,
                Err(e) => Some(Err(NodeError {
                    code: "mirror.io",
                    message: format!("Reading {} failed: {e}", file.display()),
                })),
            },
        };
        if let Some(Err(e)) = listed {
            out[0].error = Some(e);
        }
        out
    }

    /// Appends every folder and file of the zip `r`, whose node is `at`,
    /// `level` zips deep.
    fn zip<R: Read + Seek>(
        &mut self,
        r: R,
        at: &Node,
        level: usize,
        out: &mut Vec<Node>,
    ) -> Result<(), NodeError> {
        if level > MAX_DEPTH {
            return Err(NodeError {
                code: "zip.nesting_too_deep",
                message: format!("Zips nested deeper than {MAX_DEPTH} are not opened."),
            });
        }
        let mut z = ZipArchive::new(r).map_err(|e| NodeError {
            code: "zip.open_failed",
            message: format!("The zip could not be opened: {e}"),
        })?;
        let n = z.len() as u64;
        if n > self.entries_left {
            return Err(NodeError {
                code: "zip.too_many_entries",
                message: format!(
                    "The zip lists {n} entries, more than the {} one archived file may list.",
                    self.max_entries
                ),
            });
        }
        self.entries_left -= n;
        // Every entry's name split into folders, in tree order; an entry
        // whose record cannot be read is reported after them.
        let mut entries: Vec<(Vec<String>, usize, bool)> = Vec::with_capacity(z.len());
        let mut unreadable = Vec::new();
        for i in 0..z.len() {
            match z.by_index_raw(i) {
                Ok(f) => {
                    // Zips made on Windows separate folders with `\`.
                    let raw = String::from_utf8_lossy(f.name_raw());
                    let parts: Vec<String> = raw
                        .split(['/', '\\'])
                        .filter(|p| !p.is_empty())
                        .map(str::to_owned)
                        .collect();
                    if !parts.is_empty() {
                        entries.push((parts, i, f.is_dir() || raw.ends_with('\\')));
                    }
                }
                Err(e) => unreadable.push(Node {
                    error: Some(NodeError {
                        code: "zip.bad_entry",
                        message: format!("Entry {i} of the zip could not be read: {e}"),
                    }),
                    ..child(at, &format!("#{i}"), NodeKind::File, 1)
                }),
            }
        }
        entries.sort();
        let mut dirs_done: HashSet<Vec<String>> = HashSet::new();
        for (parts, i, is_dir) in entries {
            folders(out, &mut dirs_done, at, &parts, is_dir);
            if is_dir {
                continue;
            }
            let node = child(at, &parts.join("/"), NodeKind::File, parts.len());
            let (mut node, nested) = self.entry(&mut z, i, node, level);
            let here = out.len();
            match nested {
                Ok(Some(Nested::Memory)) => {
                    node.kind = NodeKind::Zip;
                    out.push(node.clone());
                    let bytes = std::mem::take(&mut self.levels[level]);
                    if let Err(e) = self.zip(Cursor::new(&bytes[..]), &node, level + 1, out) {
                        out[here].error = Some(e);
                    }
                    self.levels[level] = bytes;
                }
                Ok(Some(Nested::Spooled(f))) => {
                    node.kind = NodeKind::Zip;
                    out.push(node.clone());
                    let r = BufReader::with_capacity(SEEK_BUFFER, f);
                    if let Err(e) = self.zip(r, &node, level + 1, out) {
                        out[here].error = Some(e);
                    }
                }
                Ok(None) => out.push(node),
                Err(e) => {
                    node.error = Some(e);
                    out.push(node);
                }
            }
        }
        out.append(&mut unreadable);
        Ok(())
    }

    /// The node of entry `i` and, when it is a zip itself, its bytes: in
    /// `self.levels[level]`, or spooled to a temporary file.
    fn entry<R: Read + Seek>(
        &mut self,
        z: &mut ZipArchive<R>,
        i: usize,
        mut node: Node,
        level: usize,
    ) -> (Node, Result<Option<Nested>, NodeError>) {
        let bad = |e: &dyn std::fmt::Display| NodeError {
            code: "zip.bad_entry",
            message: format!("The entry could not be read: {e}"),
        };
        let mut f = match z.by_index(i) {
            Ok(f) => f,
            Err(e) => return (node, Err(bad(&e))),
        };
        node.size = Some(f.size());
        node.compressed = Some(f.compressed_size());
        node.crc32 = Some(f.crc32());
        // The zip's DOS time, as AEMO's machines set it: the host's clock.
        if let Some(d) = f.last_modified() {
            node.modified = Some(d.to_string());
            node.modified_at = Stamp::at(
                i32::from(d.year()),
                d.month().into(),
                d.day().into(),
                d.hour().into(),
                d.minute().into(),
                d.second().into(),
                self.offset,
            );
        }
        let mut magic = [0u8; 4];
        let got = match read_up_to(&mut f, &mut magic) {
            Ok(n) => n,
            Err(e) => return (node, Err(bad(&e))),
        };
        if got < 4 || !ZIP_MAGIC.contains(&&magic) {
            return (node, Ok(None));
        }
        let size = f.size();
        if size > self.max_nested {
            return (
                node,
                Err(NodeError {
                    code: "zip.too_big",
                    message: format!(
                        "The zip inside declares {size} bytes, more than the {} unpacked to list it.",
                        self.max_nested
                    ),
                }),
            );
        }
        // Never more than the declared size, plus one byte to notice a lie.
        let mut f = f.take(size.saturating_sub(4) + 1);
        let read = if size <= self.in_memory {
            let buf = self.level(level);
            buf.clear();
            buf.extend_from_slice(&magic);
            f.read_to_end(buf).map(|n| (n, Nested::Memory))
        } else {
            tempfile::tempfile_in(&self.tmp).and_then(|mut t| {
                t.write_all(&magic)?;
                let n = std::io::copy(&mut f, &mut t)?;
                t.rewind()?;
                Ok((usize::try_from(n).unwrap_or(usize::MAX), Nested::Spooled(t)))
            })
        };
        // An entry that inflates past the size it declares is refused by
        // the zip reader itself, as a bad entry.
        match read {
            Err(e) => (node, Err(bad(&e))),
            Ok((_, nested)) => (node, Ok(Some(nested))),
        }
    }
}

/// A node inside the zip whose node is `at`: at `rel` within it, `deeper`
/// levels below it in the tree.
fn child(at: &Node, rel: &str, kind: NodeKind, deeper: usize) -> Node {
    let mut inside = at.inside.clone();
    inside.push(rel.to_owned());
    Node {
        url: at.url.clone(),
        inside,
        ..Node::new(at.file.clone(), kind, at.depth + deeper)
    }
}

/// The folder nodes above an entry `parts` of the zip whose node is `at`,
/// each once; the entry's own when it is a folder.
fn folders(
    out: &mut Vec<Node>,
    done: &mut HashSet<Vec<String>>,
    at: &Node,
    parts: &[String],
    is_dir: bool,
) {
    for n in 1..parts.len() + usize::from(is_dir) {
        let d = parts[..n].to_vec();
        if done.insert(d.clone()) {
            out.push(child(at, &d.join("/"), NodeKind::Dir, n));
        }
    }
}

/// What a zip not downloaded holds, from its listing, under its node
/// `out[0]`: its folders and files, a zip inside it marked as needing the
/// download to list further.
fn listed(out: &mut Vec<Node>, peek: &Peeked) {
    let Some(entries) = &peek.entries else {
        if let Some(e) = peek.error.as_ref().filter(|e| e.code != "zip.not_a_zip") {
            out[0].error = Some(NodeError {
                code: "zip.bad_directory",
                message: e.message.clone(),
            });
        }
        return;
    };
    out[0].kind = NodeKind::Zip;
    let at = out[0].clone();
    let mut sorted: Vec<(Vec<String>, &Entry)> = entries
        .iter()
        .map(|e| {
            let parts = e
                .name
                .split(['/', '\\'])
                .filter(|p| !p.is_empty())
                .map(str::to_owned);
            (parts.collect(), e)
        })
        .filter(|(parts, _): &(Vec<String>, &Entry)| !parts.is_empty())
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut done = HashSet::new();
    for (parts, e) in sorted {
        let is_dir = e.name.ends_with(['/', '\\']);
        folders(out, &mut done, &at, &parts, is_dir);
        if is_dir {
            continue;
        }
        let is_zip = e.name.len() >= 4 && e.name[e.name.len() - 4..].eq_ignore_ascii_case(".zip");
        let kind = if is_zip {
            NodeKind::Zip
        } else {
            NodeKind::File
        };
        let mut node = child(&at, &parts.join("/"), kind, parts.len());
        node.size = Some(e.size);
        node.compressed = Some(e.compressed);
        node.crc32 = Some(e.crc32);
        node.modified = Some(e.modified.clone());
        node.modified_at = e.modified_at;
        node.contents = is_zip.then_some("needs_download");
        out.push(node);
    }
}

/// An archived file to map: in the lister's top-level buffer, or on disk.
enum Source {
    Memory(Vec<u8>),
    File(BufReader<File>),
}

/// Where a nested zip's bytes are.
enum Nested {
    /// In the lister's buffer for its level.
    Memory,
    /// In a temporary file.
    Spooled(File),
}

/// Whether `b` starts as a zip does.
fn starts_as_zip(b: &[u8]) -> bool {
    b.len() >= 4 && ZIP_MAGIC.iter().any(|m| b.starts_with(&m[..]))
}

/// Whether `r` starts as a zip does.
fn is_zip(r: &mut impl Read) -> std::io::Result<bool> {
    let mut magic = [0u8; 4];
    let got = read_up_to(r, &mut magic)?;
    Ok(got == 4 && ZIP_MAGIC.contains(&&magic))
}

/// Reads until `buf` is full or the reader ends; the bytes read.
fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// The two map files, written beside their final names and renamed when
/// complete.
struct Output {
    ndjson: BufWriter<File>,
    tree: BufWriter<File>,
    summary: MapSummary,
    line: Vec<u8>,
}

fn part(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

impl Output {
    fn create(ndjson: &Path, tree: &Path, header: &serde_json::Value) -> Result<Self, MirrorError> {
        let open = |p: &Path| {
            let p = part(p);
            File::create(&p)
                .map(|f| BufWriter::with_capacity(WRITE_BUFFER, f))
                .map_err(MirrorError::io(&p))
        };
        let mut out = Self {
            ndjson: open(ndjson)?,
            tree: open(tree)?,
            summary: MapSummary::default(),
            line: Vec::with_capacity(512),
        };
        serde_json::to_writer(&mut out.line, header).expect("a header serialises");
        out.line.push(b'\n');
        out.ndjson
            .write_all(&out.line)
            .map_err(MirrorError::io(ndjson))?;
        Ok(out)
    }

    fn write(&mut self, node: &Node) -> Result<(), MirrorError> {
        let s = &mut self.summary;
        match node.kind {
            NodeKind::Dir => s.dirs += 1,
            NodeKind::File => s.files += 1,
            NodeKind::Zip => s.zips += 1,
        }
        match node.state {
            Some(State::NotArchived) => s.not_archived += 1,
            Some(State::GoneUpstream) => s.gone_upstream += 1,
            Some(State::Removed) => s.removed += 1,
            Some(State::Version) => s.versions += 1,
            _ => {}
        }
        if node
            .state
            .is_some_and(|st| matches!(st, State::Archived | State::GoneUpstream | State::Version))
        {
            s.archived_bytes += node.size.unwrap_or(0);
        }
        if node.error.is_some() {
            s.errors += 1;
        }
        self.line.clear();
        serde_json::to_writer(&mut self.line, node).expect("a Node always serialises");
        self.line.push(b'\n');
        self.ndjson
            .write_all(&self.line)
            .map_err(|e| MirrorError::io(Path::new("map.ndjson"))(e))?;
        self.tree
            .write_all(tree_line(node).as_bytes())
            .map_err(|e| MirrorError::io(Path::new("tree.txt"))(e))
    }

    fn finish(self, ndjson: &Path, tree: &Path) -> Result<MapSummary, MirrorError> {
        for (w, p) in [(self.ndjson, ndjson), (self.tree, tree)] {
            // Not synced: a map lost to a crash is written again, and the
            // rename keeps a half-written one from taking the final name.
            w.into_inner()
                .map_err(|e| MirrorError::io(p)(e.into_error()))?;
            std::fs::rename(part(p), p).map_err(MirrorError::io(p))?;
        }
        Ok(self.summary)
    }
}

/// A node as one indented line of the tree.
pub fn tree_line(node: &Node) -> String {
    let last = node.inside.last().unwrap_or(&node.file);
    let name = last.rsplit('/').next().unwrap_or(last);
    let mut line = "  ".repeat(node.depth);
    line.push_str(name);
    match node.kind {
        NodeKind::Dir => line.push('/'),
        NodeKind::Zip => line.push_str("/ (zip"),
        NodeKind::File => {}
    }
    if let Some(size) = node.size {
        line.push_str(if node.kind == NodeKind::Zip {
            ", "
        } else {
            "  "
        });
        line.push_str(&human(size));
        if let Some(c) = node.compressed {
            line.push_str(" (");
            line.push_str(&human(c));
            line.push_str(" packed)");
        }
    }
    if node.kind == NodeKind::Zip {
        line.push(')');
    }
    match node.state {
        Some(State::NotArchived) => line.push_str(" [not downloaded]"),
        Some(State::GoneUpstream) => line.push_str(" [no longer on nemweb]"),
        Some(State::Removed) => line.push_str(" [removed: same as its folder]"),
        Some(State::Version) => line.push_str(" [another version]"),
        _ => {}
    }
    if node.contents.is_some() {
        line.push_str(" [contents need the download]");
    }
    if let Some(e) = &node.error {
        line.push_str(" [");
        line.push_str(e.code);
        line.push(']');
    }
    line.push('\n');
    line
}

/// Bytes for people: B, KB, MB, GB, TB in powers of 1000.
pub(crate) fn human(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1000.0;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    /// A node as `file!inside!…`, for reading tests.
    fn shown(n: &Node) -> String {
        std::iter::once(n.file.as_str())
            .chain(n.inside.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("!")
    }

    use std::io::Write as _;

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::crawl::{Remote, root};

    /// A zip of `entries`, each a name and its bytes, deflated.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let at = zip::DateTime::from_date_and_time(2026, 10, 1, 12, 0, 0).unwrap();
        for (name, bytes) in entries {
            let opts = SimpleFileOptions::default().last_modified_time(at);
            if name.ends_with('/') {
                w.add_directory(*name, opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
        }
        w.finish().unwrap().into_inner()
    }

    struct Fixture {
        _home: tempfile::TempDir,
        mirror: Mirror,
        archive: Archive,
        roots: Vec<Url>,
    }

    /// `https://h/A/` lists a zip with folders and a zip in a zip in a zip,
    /// a text file, a file not downloaded, a broken zip, an empty zip and a
    /// zip nested too deep; the archive also holds a file no longer listed.
    fn fixture() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        let deeper = zip_of(&[("z.txt", b"zz")]);
        let inner = zip_of(&[("y.csv", b"y,y\n"), ("deeper.zip", &deeper)]);
        let a = zip_of(&[
            ("d/", b""),
            ("d/x.csv", b"x\n"),
            ("e/f/w.csv", b"w\n"),
            ("inner.zip", &inner),
        ]);
        let mut deep = zip_of(&[("bottom.txt", b"b")]);
        for _ in 0..MAX_DEPTH {
            deep = zip_of(&[("n.zip", &deep)]);
        }
        let files: [(&str, &[u8]); 6] = [
            ("a.zip", &a),
            ("b.txt", b"hello"),
            ("broken.zip", b"PK\x03\x04 not a zip at all"),
            ("empty.zip", &zip_of(&[])),
            ("deep.zip", &deep),
            ("sub/c.txt", b"c"),
        ];
        let mut record = vec![Remote {
            url: "https://h/A/sub/".into(),
            kind: RemoteKind::Dir,
            size: None,
            modified: "Monday, October 12, 2026 10:45 AM".into(),
            modified_at: None,
            listed_on: "https://h/A/".into(),
            crawled_at: None,
        }];
        for (name, bytes) in files {
            let url = format!("https://h/A/{name}");
            let p = archive.path_for(&url);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            record.push(Remote {
                url,
                kind: RemoteKind::File,
                size: Some(bytes.len() as u64),
                modified: "Monday, October 12, 2026 10:45 AM".into(),
                modified_at: None,
                listed_on: "https://h/A/".into(),
                crawled_at: None,
            });
        }
        for (url, kind) in [
            ("https://h/A/missing.zip", RemoteKind::File),
            ("https://o/x.zip", RemoteKind::External),
            ("https://h/Z/y.zip", RemoteKind::File),
        ] {
            record.push(Remote {
                url: url.into(),
                kind,
                size: Some(7),
                modified: "Monday, October 12, 2026 10:45 AM".into(),
                modified_at: None,
                listed_on: "https://h/A/".into(),
                crawled_at: None,
            });
        }
        let gone = archive.path_for("https://h/A/gone.txt");
        std::fs::write(&gone, b"old").unwrap();
        Archive::write_sidecar(
            &gone,
            &crate::net::Sidecar {
                url: "https://h/A/gone.txt".into(),
                size: 3,
                sha256: String::new(),
                crc32: None,
                fetched_at: "t".into(),
                etag: None,
                last_modified: Some("Mon, 12 Oct 2026 00:32:30 GMT".into()),
                last_modified_at: None,
                listed_size: None,
                listed_modified: None,
                listed_modified_at: None,
                tool: "nemlake".into(),
                also: Vec::new(),
                gone_at: None,
            },
        )
        .unwrap();
        // b.txt, rewritten in place, and its later version beside it.
        let later = Archive::version_path(
            &archive.path_for("https://h/A/b.txt"),
            crate::stamp::Stamp::from_listing(
                "Tuesday, October 13, 2026 11:53 AM",
                crate::stamp::AEST,
            )
            .unwrap(),
        );
        std::fs::write(&later, b"hello!").unwrap();
        Archive::write_sidecar(
            &later,
            &crate::net::Sidecar {
                url: "https://h/A/b.txt".into(),
                size: 6,
                sha256: String::new(),
                crc32: None,
                fetched_at: "t".into(),
                etag: None,
                last_modified: None,
                last_modified_at: None,
                listed_size: Some(6),
                listed_modified: Some("Tuesday, October 13, 2026 11:53 AM".into()),
                listed_modified_at: None,
                tool: "utilake-mirror".into(),
                also: Vec::new(),
                gone_at: None,
            },
        )
        .unwrap();
        std::fs::create_dir_all(mirror.crawl_dir()).unwrap();
        let lines: String = record
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        std::fs::write(mirror.record_path(), lines).unwrap();
        std::fs::write(mirror.done_path(), "").unwrap();
        Fixture {
            _home: home,
            mirror,
            archive,
            roots: vec![root("https://h/A/").unwrap()],
        }
    }

    #[test]
    fn maps_every_file_and_every_zip_as_a_folder() {
        let f = fixture();
        let s = map(&f.mirror, &f.archive, &f.roots, 3, &Offsets::default()).unwrap();
        let tree = std::fs::read_to_string(f.mirror.dir().join("map/tree.txt")).unwrap();
        let expected = "\
h/
  A/
    a.zip/ (zip, 578 B)
      d/
        x.csv  2 B (4 B packed)
      e/
        f/
          w.csv  2 B (4 B packed)
      inner.zip/ (zip, 283 B (190 B packed))
        deeper.zip/ (zip, 112 B (73 B packed))
          z.txt  2 B (4 B packed)
        y.csv  4 B (6 B packed)
    b.txt  5 B
    b.txt@20261013T115300+1000  6 B [another version]
    broken.zip/ (zip, 21 B) [zip.open_failed]
    deep.zip/ (zip, 796 B)
";
        assert!(tree.starts_with(expected), "{tree}");
        let tail = "
    empty.zip/ (zip, 22 B)
    gone.txt  3 B [no longer on nemweb]
    missing.zip  7 B [not downloaded]
    sub/
      c.txt  1 B
";
        assert!(tree.ends_with(tail), "{tree}");
        let deepest = format!(
            "{}n.zip/ (zip, 121 B (77 B packed)) [zip.nesting_too_deep]\n",
            "  ".repeat(2 + MAX_DEPTH)
        );
        assert!(tree.contains(&deepest), "{tree}");
        assert!(!tree.contains("bottom.txt"), "{tree}");
        assert!(
            !tree.contains(" y.zip") && !tree.contains(" x.zip"),
            "{tree}"
        );
        assert_eq!((s.not_archived, s.gone_upstream, s.errors), (1, 1, 2));
        assert_eq!(s.versions, 1);
        assert_eq!(s.zips, 6 + MAX_DEPTH);

        let ndjson = std::fs::read_to_string(f.mirror.dir().join("map/map.ndjson")).unwrap();
        let x = ndjson.lines().find(|l| l.contains("x.csv")).unwrap();
        let x: serde_json::Value = serde_json::from_str(x).unwrap();
        assert_eq!(x["file"], "h/A/a.zip");
        assert_eq!(x["inside"], serde_json::json!(["d/x.csv"]));
        assert_eq!(x["url"], "https://h/A/a.zip");
        assert_eq!(x["kind"], "file");
        assert_eq!(x["modified"], "2026-10-01 12:00:00");
        assert_eq!(x["modified_at"], "2026-10-01T12:00:00+10:00");
        assert_eq!(x["crc32"], crc32fast::hash(b"x\n"));
        let a = ndjson
            .lines()
            .find(|l| l.contains("\"h/A/a.zip\""))
            .unwrap();
        assert!(a.contains("\"state\":\"archived\""), "{a}");
        let a: serde_json::Value = serde_json::from_str(a).unwrap();
        assert_eq!(a["modified"], "Monday, October 12, 2026 10:45 AM");
        assert_eq!(a["modified_at"], "2026-10-12T10:45:00+10:00");
        // A file nemweb no longer lists carries its sidecar's Last-Modified.
        let gone = ndjson.lines().find(|l| l.contains("gone.txt")).unwrap();
        let gone: serde_json::Value = serde_json::from_str(gone).unwrap();
        assert_eq!(gone["modified"], "Mon, 12 Oct 2026 00:32:30 GMT");
        assert_eq!(gone["modified_at"], "2026-10-12T00:32:30Z");
        // A later version carries the time nemweb listed it with.
        let later = ndjson.lines().find(|l| l.contains("b.txt@")).unwrap();
        let later: serde_json::Value = serde_json::from_str(later).unwrap();
        assert_eq!(later["url"], "https://h/A/b.txt");
        assert_eq!(later["state"], "version");
        assert_eq!(later["modified_at"], "2026-10-13T11:53:00+10:00");
        // Every time field of the catalog is aware, or has its instant beside it.
        crate::stamp::check::assert_aware_lines(&ndjson);

        // The same archive maps to the same bytes, on any number of threads.
        map(&f.mirror, &f.archive, &f.roots, 1, &Offsets::default()).unwrap();
        assert_eq!(
            std::fs::read_to_string(f.mirror.dir().join("map/tree.txt")).unwrap(),
            tree
        );
        assert_eq!(
            std::fs::read_to_string(f.mirror.dir().join("map/map.ndjson")).unwrap(),
            ndjson
        );
    }

    #[test]
    fn two_listings_of_one_folder_each_keep_their_files() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        let (cur, arc) = (
            "https://www.nemweb.com.au/Reports/CURRENT/X/",
            "https://www.nemweb.com.au/Reports/ARCHIVE/X/",
        );
        let hold = |url: &str, path: PathBuf, bytes: &[u8]| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            let sc: crate::net::Sidecar = serde_json::from_value(serde_json::json!({
                "url": url, "size": bytes.len(), "sha256": "s", "fetched_at": "2026-10-05T00:00:00Z",
            }))
            .unwrap();
            Archive::write_sidecar(&path, &sc).unwrap();
        };
        let base = archive.path_for(&format!("{cur}m.txt"));
        hold(&format!("{cur}m.txt"), base.clone(), b"partial");
        let version = base.with_file_name("m.txt@20261013T115300+1000");
        hold(&format!("{arc}m.txt"), version.clone(), b"final!");
        let dir = |url: &str| Remote {
            url: url.into(),
            kind: RemoteKind::Dir,
            size: None,
            modified: "m".into(),
            modified_at: None,
            listed_on: "https://www.nemweb.com.au/Reports/".into(),
            crawled_at: None,
        };
        let file = |url: String, size| Remote {
            url,
            kind: RemoteKind::File,
            size: Some(size),
            modified: "m".into(),
            modified_at: None,
            listed_on: cur.into(),
            crawled_at: None,
        };
        let record = [
            dir(cur),
            dir(arc),
            file(format!("{cur}m.txt"), 7),
            file(format!("{arc}m.txt"), 6),
            file(format!("{cur}n.txt"), 1),
            file(format!("{arc}n.txt"), 1),
        ];
        std::fs::create_dir_all(mirror.crawl_dir()).unwrap();
        let lines: String = record
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        std::fs::write(mirror.record_path(), lines).unwrap();
        std::fs::write(mirror.done_path(), "").unwrap();
        let roots = [root("https://www.nemweb.com.au/Reports/").unwrap()];
        map(&mirror, &archive, &roots, 1, &Offsets::default()).unwrap();
        let ndjson = std::fs::read_to_string(mirror.dir().join("map/map.ndjson")).unwrap();
        let nodes: Vec<serde_json::Value> = ndjson
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let of = |url: String| {
            nodes
                .iter()
                .find(|n| n["url"] == url.as_str())
                .unwrap_or_else(|| panic!("{url} in {ndjson}"))
                .clone()
        };
        assert_eq!(of(format!("{cur}m.txt"))["state"], "archived");
        assert_eq!(of(format!("{arc}m.txt"))["state"], "archived");
        assert_eq!(of(format!("{arc}m.txt"))["size"], 6);
        assert_eq!(
            of(format!("{arc}m.txt"))["file"],
            "www.nemweb.com.au/Reports/X/m.txt@20261013T115300+1000"
        );
        assert_eq!(of(format!("{cur}n.txt"))["state"], "not_archived");
        // The version is the ARCHIVE file's node, not a node of its own.
        assert_eq!(
            nodes.iter().filter(|n| n["state"] == "version").count(),
            0,
            "{ndjson}"
        );
        // One folder for both listings.
        let folders = nodes
            .iter()
            .filter(|n| n["file"] == "www.nemweb.com.au/Reports/X")
            .count();
        assert_eq!(folders, 1, "{ndjson}");
    }

    #[test]
    fn a_big_nested_zip_is_spooled_to_disk() {
        let f = fixture();
        let mut l = Lister::new(f.mirror.dir().to_path_buf());
        l.in_memory = 0;
        let key: Vec<String> = ["h", "A", "a.zip"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let top = Top {
            url: None,
            kind: NodeKind::File,
            size: None,
            modified: None,
            modified_at: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
            removed: None,
        };
        let nodes = l.top(&f.archive, &key, &top);
        let paths: Vec<_> = nodes.iter().map(shown).collect();
        assert!(
            paths.contains(&"h/A/a.zip!inner.zip!deeper.zip!z.txt".to_owned()),
            "{paths:?}"
        );
        assert!(nodes.iter().all(|n| n.error.is_none()));

        let gone = Top {
            state: Some(State::GoneUpstream),
            ..top
        };
        let key: Vec<String> = ["h", "A", "nothing"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let nodes = l.top(&f.archive, &key, &gone);
        assert_eq!(nodes[0].error.as_ref().unwrap().code, "mirror.io");
    }

    #[test]
    fn needs_a_finished_crawl() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path());
        let err = map(
            &mirror,
            &Archive::new(home.path()),
            &[],
            1,
            &Offsets::default(),
        )
        .unwrap_err();
        assert!(matches!(err, MirrorError::CrawlIncomplete { .. }));
    }

    #[test]
    fn sizes_for_people() {
        assert_eq!(human(999), "999 B");
        assert_eq!(human(1_500), "1.5 KB");
        assert_eq!(human(28_670_000_000), "28.7 GB");
        assert_eq!(human(u64::MAX), "18446744.1 TB");
    }

    #[test]
    fn broken_zips_are_marked_and_the_map_carries_on() {
        use crate::testzip;
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let top = Top {
            url: None,
            kind: NodeKind::File,
            size: None,
            modified: None,
            modified_at: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
            removed: None,
        };
        let mut lister = Lister::new(dir.path().join("tmp"));
        let list = |name: &str, bytes: Option<&[u8]>, lister: &mut Lister| {
            let key = vec!["h".to_owned(), name.to_owned()];
            let path = dir.path().join("h").join(name);
            std::fs::create_dir_all(dir.path().join("h")).unwrap();
            match bytes {
                Some(b) => std::fs::write(&path, b).unwrap(),
                None => std::fs::create_dir_all(&path).unwrap(),
            }
            lister.top(&archive, &key, &top)
        };
        let codes = |nodes: &[Node]| -> Vec<Option<&'static str>> {
            nodes
                .iter()
                .map(|n| n.error.as_ref().map(|e| e.code))
                .collect()
        };
        // An entry that needs a password.
        let n = list(
            "secret.zip",
            Some(&testzip::encrypted(testzip::stored("u.bin", b"abcd"))),
            &mut lister,
        );
        assert_eq!(codes(&n), [None, Some("zip.bad_entry")]);
        // An entry whose compressed bytes deflate rejects.
        let text = vec![b'x'; 4096];
        let n = list(
            "data.zip",
            Some(&testzip::corrupt_data(testzip::deflated("c.bin", &text))),
            &mut lister,
        );
        assert_eq!(codes(&n), [None, Some("zip.bad_entry")]);
        // An entry the directory lists but whose header is not there.
        let n = list(
            "header.zip",
            Some(&testzip::bad_second_header()),
            &mut lister,
        );
        assert_eq!(codes(&n), [None, None, Some("zip.bad_entry")]);
        assert!(shown(&n[2]).ends_with("header.zip!#1"), "{}", shown(&n[2]));
        // A big entry that starts as a zip but is not one, spooled to disk;
        // and a folder where a file should be, read from disk.
        lister.in_memory = 0;
        std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
        let n = list(
            "nested.zip",
            Some(&testzip::stored("n.zip", b"PK\x03\x04 not a zip after all")),
            &mut lister,
        );
        assert_eq!(codes(&n), [None, Some("zip.open_failed")]);
        let n = list("plain.txt", Some(b"not a zip"), &mut lister);
        assert_eq!(codes(&n), [None]);
        let n = list("folder", None, &mut lister);
        assert_eq!(codes(&n), [Some("mirror.io")]);
    }

    #[test]
    fn the_catalog_says_its_format_and_lists_zips_not_downloaded() {
        let f = fixture();
        let entry = |name: &str| crate::zipdir::Entry {
            name: name.into(),
            size: 10,
            compressed: 4,
            crc32: 7,
            method: 8,
            modified: "2026-10-01 12:00:00".into(),
            modified_at: None,
        };
        let peeks = [
            crate::peek::Peeked {
                url: "https://h/A/missing.zip".into(),
                peeked_at: Some(Stamp::from_unix(1_791_765_900)),
                listed_size: Some(7),
                listed_modified: Some("Monday, October 12, 2026 10:45 AM".into()),
                last_modified: Some("Mon, 12 Oct 2026 00:32:30 GMT".into()),
                entries: Some(vec![
                    entry("in/"),
                    entry("in/a.csv"),
                    entry("in/b.ZIP"),
                    entry("/"),
                ]),
                error: None,
                ..Default::default()
            },
            crate::peek::Peeked {
                url: "https://h/A/b.txt".into(),
                entries: Some(vec![entry("ignored, it is archived")]),
                error: None,
                ..Default::default()
            },
        ];
        let lines: String = peeks
            .iter()
            .map(|p| serde_json::to_string(p).unwrap() + "\n")
            .collect();
        std::fs::write(f.mirror.peek_path(), lines).unwrap();
        map(&f.mirror, &f.archive, &f.roots, 1, &Offsets::default()).unwrap();
        let ndjson = std::fs::read_to_string(f.mirror.dir().join("map/map.ndjson")).unwrap();
        assert_eq!(
            ndjson.lines().next().unwrap(),
            r#"{"catalog_format":3,"roots":["https://h/A/"]}"#
        );
        let tree = std::fs::read_to_string(f.mirror.dir().join("map/tree.txt")).unwrap();
        let expected = "    missing.zip/ (zip, 7 B) [not downloaded]
      in/
        a.csv  10 B (4 B packed)
        b.ZIP/ (zip, 10 B (4 B packed)) [contents need the download]
";
        assert!(tree.contains(expected), "{tree}");
        assert!(!tree.contains("ignored"), "{tree}");
        // The peek's times, text and instant, and its entries' (the peek
        // record was written without the instants: the map derives them).
        let missing = ndjson
            .lines()
            .find(|l| l.contains("\"h/A/missing.zip\"") && !l.contains("inside"))
            .unwrap();
        let missing: serde_json::Value = serde_json::from_str(missing).unwrap();
        assert_eq!(missing["peeked_at"], "2026-10-12T00:45:00Z");
        assert_eq!(missing["last_modified"], "Mon, 12 Oct 2026 00:32:30 GMT");
        assert_eq!(missing["last_modified_at"], "2026-10-12T00:32:30Z");
        let in_a = ndjson.lines().find(|l| l.contains("in/a.csv")).unwrap();
        let in_a: serde_json::Value = serde_json::from_str(in_a).unwrap();
        assert_eq!(in_a["modified"], "2026-10-01 12:00:00");
        assert_eq!(in_a["modified_at"], "2026-10-01T12:00:00+10:00");
        crate::stamp::check::assert_aware_lines(&ndjson);
        let md = crate::summary(&f.mirror.dir().join("map/map.ndjson")).unwrap();
        assert!(md.contains("b.ZIP/ (zip)"), "{md}");

        // A listing that is damaged marks the zip; one of a file that is not
        // a zip leaves it a file.
        let bad = |code: &str| crate::peek::Peeked {
            url: "https://h/A/missing.zip".into(),
            listed_size: Some(7),
            listed_modified: Some("Monday, October 12, 2026 10:45 AM".into()),
            entries: None,
            error: Some(crate::peek::PeekError {
                code: code.into(),
                message: "m".into(),
            }),
            ..Default::default()
        };
        for (code, line) in [
            (
                "zip.bad_directory",
                "    missing.zip  7 B [not downloaded] [zip.bad_directory]\n",
            ),
            ("zip.not_a_zip", "    missing.zip  7 B [not downloaded]\n"),
        ] {
            std::fs::write(
                f.mirror.peek_path(),
                serde_json::to_string(&bad(code)).unwrap() + "\n",
            )
            .unwrap();
            map(&f.mirror, &f.archive, &f.roots, 1, &Offsets::default()).unwrap();
            let tree = std::fs::read_to_string(f.mirror.dir().join("map/tree.txt")).unwrap();
            assert!(tree.contains(line), "{tree}");
        }
    }

    #[test]
    fn zips_that_would_cost_too_much_are_not_unpacked() {
        use crate::testzip;
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let top = Top {
            url: None,
            kind: NodeKind::File,
            size: None,
            modified: None,
            modified_at: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
            removed: None,
        };
        let mut lister = Lister::new(dir.path().join("tmp"));
        std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
        let list = |name: &str, bytes: &[u8], lister: &mut Lister| {
            std::fs::create_dir_all(dir.path().join("h")).unwrap();
            std::fs::write(dir.path().join("h").join(name), bytes).unwrap();
            let key = vec!["h".to_owned(), name.to_owned()];
            let nodes = lister.top(&archive, &key, &top);
            nodes
                .iter()
                .map(|n| n.error.as_ref().map(|e| e.code))
                .collect::<Vec<_>>()
        };
        let inner = testzip::stored("x.csv", b"x");
        let outer = testzip::stored("n.zip", &inner);
        // Bigger than the limit, as declared.
        lister.max_nested = 10;
        assert_eq!(
            list("big.zip", &outer, &mut lister),
            [None, Some("zip.too_big")]
        );
        lister.max_nested = MAX_NESTED;
        // More entries than one archived file may list.
        lister.max_entries = 1;
        let two = testzip::bad_second_header();
        assert_eq!(
            list("many.zip", &two, &mut lister),
            [Some("zip.too_many_entries")]
        );
        assert_eq!(
            list("deep.zip", &outer, &mut lister),
            [None, Some("zip.too_many_entries")],
            "the zip inside would pass the limit"
        );
        lister.max_entries = MAX_ENTRIES;
        // An entry holding more than it declares, in memory and spooled:
        // the zip reader refuses it at the declared size.
        let lying = testzip::understated(&testzip::deflated("n.zip", &inner));
        assert_eq!(
            list("lie.zip", &lying, &mut lister),
            [None, Some("zip.bad_entry")]
        );
        lister.in_memory = 0;
        assert_eq!(
            list("lie2.zip", &lying, &mut lister),
            [None, Some("zip.bad_entry")]
        );
        // Nowhere to spool it.
        lister.tmp = dir.path().join("no-such-dir");
        assert_eq!(
            list("nowhere.zip", &outer, &mut lister),
            [None, Some("zip.bad_entry")]
        );
    }

    #[test]
    fn windows_folders_in_a_downloaded_zip_and_in_a_listing() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        std::fs::create_dir_all(dir.path().join("h")).unwrap();
        let z = crate::testzip::windows(&[("M|S|t.csv", b"x"), ("M|E|", b""), ("|", b"")]);
        std::fs::write(dir.path().join("h/w.zip"), &z).unwrap();
        let top = Top {
            url: None,
            kind: NodeKind::File,
            size: None,
            modified: None,
            modified_at: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
            removed: None,
        };
        let key = vec!["h".to_owned(), "w.zip".to_owned()];
        let nodes = Lister::new(dir.path().to_path_buf()).top(&archive, &key, &top);
        let paths: Vec<_> = nodes.iter().map(|n| (shown(n), n.kind)).collect();
        assert_eq!(
            paths,
            [
                ("h/w.zip".to_owned(), NodeKind::Zip),
                ("h/w.zip!M".to_owned(), NodeKind::Dir),
                ("h/w.zip!M/E".to_owned(), NodeKind::Dir),
                ("h/w.zip!M/S".to_owned(), NodeKind::Dir),
                ("h/w.zip!M/S/t.csv".to_owned(), NodeKind::File),
            ]
        );
        // A listing recorded before names were read with slashes.
        let entry = |name: &str| crate::zipdir::Entry {
            name: name.into(),
            size: 1,
            compressed: 1,
            crc32: 1,
            method: 0,
            modified: "m".into(),
            modified_at: None,
        };
        let peeked = crate::peek::Peeked {
            url: "u".into(),
            entries: Some(vec![entry("M\\S\\t.csv"), entry("M\\E\\")]),
            error: None,
            ..Default::default()
        };
        let mut out = vec![Node::new("h/p.zip".into(), NodeKind::File, 1)];
        listed(&mut out, &peeked);
        let paths: Vec<_> = out.iter().map(shown).collect();
        assert_eq!(
            paths,
            [
                "h/p.zip",
                "h/p.zip!M",
                "h/p.zip!M/E",
                "h/p.zip!M/S",
                "h/p.zip!M/S/t.csv"
            ]
        );
    }

    #[test]
    fn names_are_kept_as_they_are_and_files_carry_their_hashes() {
        let mut n = Node::new("h/z.zip".into(), NodeKind::File, 2);
        n.inside = vec!["a!b/50% c.csv".into()];
        n.size = Some(1);
        assert_eq!(tree_line(&n), "    50% c.csv  1 B\n");
        // A downloaded file shows its sidecar's hashes.
        let f = fixture();
        let path = f.archive.path_for("https://h/A/b.txt");
        Archive::write_sidecar(
            &path,
            &crate::net::Sidecar {
                url: "https://h/A/b.txt".into(),
                size: 5,
                sha256: "abc".into(),
                crc32: Some(9),
                fetched_at: "t".into(),
                etag: None,
                last_modified: None,
                last_modified_at: None,
                listed_size: None,
                listed_modified: None,
                listed_modified_at: None,
                tool: "x".into(),
                also: Vec::new(),
                gone_at: None,
            },
        )
        .unwrap();
        map(&f.mirror, &f.archive, &f.roots, 1, &Offsets::default()).unwrap();
        let ndjson = std::fs::read_to_string(f.mirror.dir().join("map/map.ndjson")).unwrap();
        let b = ndjson
            .lines()
            .find(|l| l.contains("\"h/A/b.txt\""))
            .unwrap();
        assert!(
            b.contains("\"crc32\":9") && b.contains("\"sha256\":\"abc\""),
            "{b}"
        );
        // Every node nemweb lists keeps its exact URL; the host above the
        // roots has none.
        assert!(b.contains("\"url\":\"https://h/A/b.txt\""), "{b}");
        let root = ndjson
            .lines()
            .find(|l| l.contains("\"file\":\"h/A\""))
            .unwrap();
        assert!(root.contains("\"url\":\"https://h/A/\""), "{root}");
        let host = ndjson
            .lines()
            .find(|l| l.contains("\"file\":\"h\""))
            .unwrap();
        assert!(!host.contains("url"), "{host}");
        let gone = ndjson.lines().find(|l| l.contains("gone.txt")).unwrap();
        assert!(gone.contains("\"url\":\"https://h/A/gone.txt\""), "{gone}");
    }
}
