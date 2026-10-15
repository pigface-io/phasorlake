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

use crate::crawl::{Mirror, RemoteKind, read_record};
use crate::error::MirrorError;
use crate::peek::{Peeked, read_peeks};
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
pub const CATALOG_FORMAT: u32 = 1;
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
    /// The path: the archive path for what nemweb lists, then `!` and the
    /// name inside each zip.
    pub path: String,
    /// Folder, file or zip.
    pub kind: NodeKind,
    /// Size in bytes (uncompressed inside a zip).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Compressed size, inside a zip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed: Option<u64>,
    /// CRC-32 as the zip records it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crc32: Option<u32>,
    /// Date and time as nemweb lists it, or as the zip records it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
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
    fn new(path: String, kind: NodeKind, depth: usize) -> Self {
        Self {
            path,
            kind,
            size: None,
            compressed: None,
            crc32: None,
            modified: None,
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
    /// Files or zips that could not be read.
    pub errors: usize,
    /// Bytes of the archived files nemweb lists or listed.
    pub archived_bytes: u64,
}

/// A file or folder nemweb lists, or an archived file it listed once.
#[derive(Debug, Clone)]
struct Top {
    kind: NodeKind,
    size: Option<u64>,
    modified: Option<String>,
    state: Option<State>,
    same_as: Option<String>,
    /// What the zip holds, from its listing, when it is not downloaded.
    peek: Option<Peeked>,
}

/// Writes the map of the crawl record and the archive under `roots`,
/// reading zips on `workers` threads.
pub fn map(
    mirror: &Mirror,
    archive: &Archive,
    roots: &[Url],
    workers: usize,
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
    let jobs: Vec<(Vec<String>, Top)> = tops.into_iter().collect();
    for batch in jobs.chunks(listers.len() * 4) {
        let results = list_batch(archive, batch, &mut listers);
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
) -> Result<BTreeMap<Vec<String>, Top>, MirrorError> {
    let key = |url: &str| -> Vec<String> {
        let path = archive.path_for(url);
        path.strip_prefix(archive.root())
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    let mut tops = BTreeMap::new();
    let mut listed = HashSet::new();
    let removed = mirror.removed()?;
    let mut peeks: HashMap<String, Peeked> = read_peeks(&mirror.peek_path())?
        .into_iter()
        .map(|p| (p.url.clone(), p))
        .collect();
    for r in read_record(&mirror.record_path())? {
        let kind = match r.kind {
            RemoteKind::External => continue,
            RemoteKind::Dir => NodeKind::Dir,
            RemoteKind::File => NodeKind::File,
        };
        if !roots.iter().any(|root| r.url.starts_with(root.as_str())) {
            continue;
        }
        let gone = removed.get(&r.url);
        let state = (kind == NodeKind::File).then(|| {
            if gone.is_some() {
                State::Removed
            } else if archive.path_for(&r.url).is_file() {
                State::Archived
            } else {
                State::NotArchived
            }
        });
        tops.insert(
            key(&r.url),
            Top {
                kind,
                size: r.size,
                modified: Some(r.modified),
                state,
                same_as: gone.map(|g| g.same_as.clone()),
                peek: state
                    .filter(|s| matches!(s, State::NotArchived | State::Removed))
                    .and_then(|_| peeks.remove(&r.url)),
            },
        );
        listed.insert(r.url);
    }
    for root in roots {
        for a in archive.entries_under(Some(&archive.path_for(root.as_str()))) {
            if !listed.contains(&a.sidecar.url) {
                tops.insert(
                    key(&a.sidecar.url),
                    Top {
                        kind: NodeKind::File,
                        size: Some(a.sidecar.size),
                        modified: a.sidecar.last_modified,
                        state: Some(State::GoneUpstream),
                        same_as: None,
                        peek: None,
                    },
                );
            }
        }
    }
    let mut parents = Vec::new();
    for k in tops.keys() {
        for n in 1..k.len() {
            parents.push(k[..n].to_vec());
        }
    }
    for p in parents {
        tops.entry(p).or_insert(Top {
            kind: NodeKind::Dir,
            size: None,
            modified: None,
            state: None,
            same_as: None,
            peek: None,
        });
    }
    Ok(tops)
}

/// The nodes of each item of a batch, in batch order; the archived files
/// are read on the listers' threads.
fn list_batch(
    archive: &Archive,
    batch: &[(Vec<String>, Top)],
    listers: &mut [Lister],
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
                        done.push((i, lister.top(archive, key, top)));
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
        node.size = top.size;
        node.modified.clone_from(&top.modified);
        node.state = top.state;
        node.same_as.clone_from(&top.same_as);
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
        let (path, depth) = (out[0].path.clone(), out[0].depth);
        let listed = match source {
            Source::Memory(buf) => {
                let listed = starts_as_zip(&buf).then(|| {
                    out[0].kind = NodeKind::Zip;
                    self.zip(Cursor::new(&buf[..]), &path, depth, 1, &mut out)
                });
                *self.level(0) = buf;
                listed
            }
            Source::File(mut r) => match is_zip(&mut r).and_then(|z| r.rewind().map(|()| z)) {
                Ok(true) => {
                    out[0].kind = NodeKind::Zip;
                    Some(self.zip(r, &path, depth, 1, &mut out))
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

    /// Appends every folder and file of the zip `r`, whose node is at
    /// `path` and `depth`, `level` zips deep.
    fn zip<R: Read + Seek>(
        &mut self,
        r: R,
        path: &str,
        depth: usize,
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
                    let parts: Vec<String> = f
                        .name()
                        .split(['/', '\\'])
                        .filter(|p| !p.is_empty())
                        .map(str::to_owned)
                        .collect();
                    if !parts.is_empty() {
                        entries.push((parts, i, f.is_dir() || f.name().ends_with('\\')));
                    }
                }
                Err(e) => unreadable.push(Node {
                    error: Some(NodeError {
                        code: "zip.bad_entry",
                        message: format!("Entry {i} of the zip could not be read: {e}"),
                    }),
                    ..Node::new(format!("{path}!#{i}"), NodeKind::File, depth + 1)
                }),
            }
        }
        entries.sort();
        let mut dirs_done: HashSet<Vec<String>> = HashSet::new();
        for (parts, i, is_dir) in entries {
            folders(out, &mut dirs_done, path, depth, &parts, is_dir);
            if is_dir {
                continue;
            }
            let node_path = format!("{path}!{}", parts.join("/"));
            let node_depth = depth + parts.len();
            let (mut node, nested) = self.entry(&mut z, i, node_path, node_depth, level);
            let at = out.len();
            match nested {
                Ok(Some(Nested::Memory)) => {
                    node.kind = NodeKind::Zip;
                    out.push(node);
                    let bytes = std::mem::take(&mut self.levels[level]);
                    let (p, d) = (out[at].path.clone(), out[at].depth);
                    if let Err(e) = self.zip(Cursor::new(&bytes[..]), &p, d, level + 1, out) {
                        out[at].error = Some(e);
                    }
                    self.levels[level] = bytes;
                }
                Ok(Some(Nested::Spooled(f))) => {
                    node.kind = NodeKind::Zip;
                    out.push(node);
                    let (p, d) = (out[at].path.clone(), out[at].depth);
                    let r = BufReader::with_capacity(SEEK_BUFFER, f);
                    if let Err(e) = self.zip(r, &p, d, level + 1, out) {
                        out[at].error = Some(e);
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
        path: String,
        depth: usize,
        level: usize,
    ) -> (Node, Result<Option<Nested>, NodeError>) {
        let mut node = Node::new(path, NodeKind::File, depth);
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
        node.modified = f.last_modified().map(|d| d.to_string());
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
        match read {
            Err(e) => (node, Err(bad(&e))),
            Ok((n, _)) if n as u64 + 4 > size => (
                node,
                Err(NodeError {
                    code: "zip.size_mismatch",
                    message: format!("The entry holds more than the {size} bytes it declares."),
                }),
            ),
            Ok((_, nested)) => (node, Ok(Some(nested))),
        }
    }
}

/// The folder nodes above an entry `parts` of the zip at `path`, each once;
/// the entry's own when it is a folder.
fn folders(
    out: &mut Vec<Node>,
    done: &mut HashSet<Vec<String>>,
    path: &str,
    depth: usize,
    parts: &[String],
    is_dir: bool,
) {
    for n in 1..parts.len() + usize::from(is_dir) {
        let d = parts[..n].to_vec();
        if done.insert(d.clone()) {
            out.push(Node::new(
                format!("{path}!{}", d.join("/")),
                NodeKind::Dir,
                depth + n,
            ));
        }
    }
}

/// What a zip not downloaded holds, from its listing, under its node
/// `out[0]`: its folders and files, a zip inside it marked as needing the
/// download to list further.
fn listed(out: &mut Vec<Node>, peek: &Peeked) {
    let (path, depth) = (out[0].path.clone(), out[0].depth);
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
        folders(out, &mut done, &path, depth, &parts, is_dir);
        if is_dir {
            continue;
        }
        let is_zip = e.name.len() >= 4 && e.name[e.name.len() - 4..].eq_ignore_ascii_case(".zip");
        let mut node = Node::new(
            format!("{path}!{}", parts.join("/")),
            if is_zip {
                NodeKind::Zip
            } else {
                NodeKind::File
            },
            depth + parts.len(),
        );
        node.size = Some(e.size);
        node.compressed = Some(e.compressed);
        node.crc32 = Some(e.crc32);
        node.modified = Some(e.modified.clone());
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
            _ => {}
        }
        if node
            .state
            .is_some_and(|st| matches!(st, State::Archived | State::GoneUpstream))
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
    let name = node.path.rsplit(['/', '!']).next().unwrap_or(&node.path);
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
            modified: "m".into(),
            listed_on: "https://h/A/".into(),
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
                listed_on: "https://h/A/".into(),
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
                modified: "m".into(),
                listed_on: "https://h/A/".into(),
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
                fetched_at: "t".into(),
                etag: None,
                last_modified: Some("Mon, 12 Oct 2026 00:32:30 GMT".into()),
                tool: "nemlake".into(),
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
        let s = map(&f.mirror, &f.archive, &f.roots, 3).unwrap();
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
        assert_eq!(s.zips, 6 + MAX_DEPTH);

        let ndjson = std::fs::read_to_string(f.mirror.dir().join("map/map.ndjson")).unwrap();
        let x = ndjson.lines().find(|l| l.contains("x.csv")).unwrap();
        let x: serde_json::Value = serde_json::from_str(x).unwrap();
        assert_eq!(x["path"], "h/A/a.zip!d/x.csv");
        assert_eq!(x["kind"], "file");
        assert_eq!(x["modified"], "2026-10-01 12:00:00");
        assert_eq!(x["crc32"], crc32fast::hash(b"x\n"));
        let a = ndjson
            .lines()
            .find(|l| l.contains("\"h/A/a.zip\""))
            .unwrap();
        assert!(a.contains("\"state\":\"archived\""), "{a}");

        // The same archive maps to the same bytes, on any number of threads.
        map(&f.mirror, &f.archive, &f.roots, 1).unwrap();
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
    fn a_big_nested_zip_is_spooled_to_disk() {
        let f = fixture();
        let mut l = Lister::new(f.mirror.dir().to_path_buf());
        l.in_memory = 0;
        let key: Vec<String> = ["h", "A", "a.zip"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let top = Top {
            kind: NodeKind::File,
            size: None,
            modified: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
        };
        let nodes = l.top(&f.archive, &key, &top);
        let paths: Vec<_> = nodes.iter().map(|n| n.path.as_str()).collect();
        assert!(
            paths.contains(&"h/A/a.zip!inner.zip!deeper.zip!z.txt"),
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
        let err = map(&mirror, &Archive::new(home.path()), &[], 1).unwrap_err();
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
            kind: NodeKind::File,
            size: None,
            modified: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
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
        assert!(n[2].path.ends_with("header.zip!#1"), "{}", n[2].path);
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
        };
        let peeks = [
            crate::peek::Peeked {
                url: "https://h/A/missing.zip".into(),
                entries: Some(vec![
                    entry("in/"),
                    entry("in/a.csv"),
                    entry("in/b.ZIP"),
                    entry("/"),
                ]),
                error: None,
            },
            crate::peek::Peeked {
                url: "https://h/A/b.txt".into(),
                entries: Some(vec![entry("ignored, it is archived")]),
                error: None,
            },
        ];
        let lines: String = peeks
            .iter()
            .map(|p| serde_json::to_string(p).unwrap() + "\n")
            .collect();
        std::fs::write(f.mirror.peek_path(), lines).unwrap();
        map(&f.mirror, &f.archive, &f.roots, 1).unwrap();
        let ndjson = std::fs::read_to_string(f.mirror.dir().join("map/map.ndjson")).unwrap();
        assert_eq!(
            ndjson.lines().next().unwrap(),
            r#"{"catalog_format":1,"roots":["https://h/A/"]}"#
        );
        let tree = std::fs::read_to_string(f.mirror.dir().join("map/tree.txt")).unwrap();
        let expected = "    missing.zip/ (zip, 7 B) [not downloaded]
      in/
        a.csv  10 B (4 B packed)
        b.ZIP/ (zip, 10 B (4 B packed)) [contents need the download]
";
        assert!(tree.contains(expected), "{tree}");
        assert!(!tree.contains("ignored"), "{tree}");
        let md = crate::summary(&f.mirror.dir().join("map/map.ndjson")).unwrap();
        assert!(md.contains("b.ZIP/ (zip)"), "{md}");

        // A listing that is damaged marks the zip; one of a file that is not
        // a zip leaves it a file.
        let bad = |code: &str| crate::peek::Peeked {
            url: "https://h/A/missing.zip".into(),
            entries: None,
            error: Some(crate::peek::PeekError {
                code: code.into(),
                message: "m".into(),
            }),
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
            map(&f.mirror, &f.archive, &f.roots, 1).unwrap();
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
            kind: NodeKind::File,
            size: None,
            modified: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
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
        // deflate gives what it inflates, whatever the zip declared.
        let lying = testzip::understated(&testzip::deflated("n.zip", &inner));
        assert_eq!(
            list("lie.zip", &lying, &mut lister),
            [None, Some("zip.size_mismatch")]
        );
        lister.in_memory = 0;
        assert_eq!(
            list("lie2.zip", &lying, &mut lister),
            [None, Some("zip.size_mismatch")]
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
            kind: NodeKind::File,
            size: None,
            modified: None,
            state: Some(State::Archived),
            same_as: None,
            peek: None,
        };
        let key = vec!["h".to_owned(), "w.zip".to_owned()];
        let nodes = Lister::new(dir.path().to_path_buf()).top(&archive, &key, &top);
        let paths: Vec<_> = nodes.iter().map(|n| (n.path.as_str(), n.kind)).collect();
        assert_eq!(
            paths,
            [
                ("h/w.zip", NodeKind::Zip),
                ("h/w.zip!M", NodeKind::Dir),
                ("h/w.zip!M/E", NodeKind::Dir),
                ("h/w.zip!M/S", NodeKind::Dir),
                ("h/w.zip!M/S/t.csv", NodeKind::File),
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
        };
        let peeked = crate::peek::Peeked {
            url: "u".into(),
            entries: Some(vec![entry("M\\S\\t.csv"), entry("M\\E\\")]),
            error: None,
        };
        let mut out = vec![Node::new("h/p.zip".into(), NodeKind::File, 1)];
        listed(&mut out, &peeked);
        let paths: Vec<_> = out.iter().map(|n| n.path.as_str()).collect();
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
}
