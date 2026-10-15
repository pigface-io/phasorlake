//! Whether a zip nemweb publishes beside a folder of the same name holds
//! the same files, and removing it from the archive when it does.
//!
//! The archive publishes each month from 2015 on twice: as a folder of
//! per-table zips, and as one zip of the whole month. A pair is any file
//! `X.zip` the crawl lists beside a folder `X/`. Each file in the zip is
//! matched to the folder's file at the same path (the zip may put the
//! month's own folder, or something else, in front), first by size and
//! CRC-32, then, for a zip packed again, by the files it holds.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, Write as _};
use std::path::{Path, PathBuf};

use crate::net::Archive;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::crawl::{Mirror, RemoteKind, read_record};
use crate::error::MirrorError;
use crate::zipdir::Entry;

const READ_BUFFER: usize = 1 << 20;
/// A zip inside the zip up to this size is read into memory to list it; a
/// bigger one is spooled to a temporary file.
const NESTED_IN_MEMORY: u64 = 64 << 20;

/// A zip and the folder of the same name beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pair {
    /// The zip's URL.
    pub zip: String,
    /// The folder's URL.
    pub folder: String,
    /// The zip's listed size.
    pub zip_size: u64,
    /// The folder's files: URL and listed size.
    pub files: Vec<(String, u64)>,
}

/// What a comparison found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every file of the zip is in the folder, the same.
    Same,
    /// The zip holds files the folder lacks, or files that differ.
    Differs,
    /// Known only from the zip's listing, a file of it is not byte for byte
    /// the folder's: whether it is packed again needs the zip itself.
    NeedsDownload,
    /// Some file of the folder is not archived yet, or the zip is neither
    /// archived nor peeked.
    NotArchived,
}

/// The comparison of one pair.
#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    /// The zip's URL.
    pub zip: String,
    /// The folder's URL.
    pub folder: String,
    /// The zip's listed size.
    pub zip_size: u64,
    /// What it found.
    pub verdict: Verdict,
    /// Files of the zip whose bytes are the folder's.
    pub same: usize,
    /// Files of the zip packed again: other bytes, the same files inside.
    pub repacked: usize,
    /// Files of the zip with no file at the same path in the folder.
    pub only_in_zip: Vec<String>,
    /// Files of the zip whose content differs from the folder's.
    pub different: Vec<String>,
    /// Files of the zip, known from its listing, that are not byte for byte
    /// the folder's; the zip is needed to tell more.
    pub unsure: Vec<String>,
    /// Whether the zip was ruled on from its listing alone.
    pub from_listing: bool,
    /// Files of the folder the zip does not hold.
    pub only_in_folder: Vec<String>,
    /// Files of the pair not archived yet.
    pub not_archived: Vec<String>,
}

/// One removed duplicate: kept so the download does not fetch it again,
/// and so the map can still show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    /// The URL of the removed file.
    pub url: String,
    /// Its size.
    pub size: u64,
    /// Its sha256, from its sidecar; none for a zip never downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Whether it had been downloaded before it was found the same.
    #[serde(default = "downloaded")]
    pub downloaded: bool,
    /// The folder that holds the same files.
    pub same_as: String,
    /// When it was removed (UTC, RFC 3339).
    pub removed_at: String,
}

fn downloaded() -> bool {
    true
}

impl Mirror {
    /// The record of removed duplicates.
    pub fn removed_path(&self) -> PathBuf {
        self.dir().join("removed.ndjson")
    }

    /// Every removed duplicate, by URL.
    pub fn removed(&self) -> Result<HashMap<String, Removed>, MirrorError> {
        let path = self.removed_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(MirrorError::io(&path)(e)),
        };
        text.lines()
            .enumerate()
            .filter(|(_, l)| !l.is_empty())
            .map(|(i, l)| {
                serde_json::from_str::<Removed>(l)
                    .map(|r| (r.url.clone(), r))
                    .map_err(|e| MirrorError::Record {
                        path: path.display().to_string(),
                        line: i as u64 + 1,
                        message: e.to_string(),
                    })
            })
            .collect()
    }
}

/// Every zip of the crawl with a folder of the same name beside it.
pub fn pairs(mirror: &Mirror) -> Result<Vec<Pair>, MirrorError> {
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.crawl_dir().display().to_string(),
        });
    }
    let record = read_record(&mirror.record_path())?;
    let dirs: HashSet<&str> = record
        .iter()
        .filter(|r| r.kind == RemoteKind::Dir)
        .map(|r| r.url.as_str())
        .collect();
    let mut out = Vec::new();
    for r in record.iter().filter(|r| r.kind == RemoteKind::File) {
        let Some(stem) = r
            .url
            .len()
            .checked_sub(4)
            .filter(|&n| r.url[n..].eq_ignore_ascii_case(".zip"))
            .map(|n| &r.url[..n])
        else {
            continue;
        };
        let folder = format!("{stem}/");
        if !dirs.contains(folder.as_str()) {
            continue;
        }
        let files = record
            .iter()
            .filter(|f| f.kind == RemoteKind::File && f.url.starts_with(&folder))
            .map(|f| (f.url.clone(), f.size.unwrap_or(0)))
            .collect();
        out.push(Pair {
            zip: r.url.clone(),
            folder,
            zip_size: r.size.unwrap_or(0),
            files,
        });
    }
    Ok(out)
}

/// A file's size and CRC-32, or what a zip says of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sum {
    size: u64,
    crc32: u32,
}

/// One file of the zip: its name, size and CRC-32, and its index when the
/// zip itself is at hand.
struct ZipFile {
    name: String,
    sum: Sum,
    index: Option<usize>,
}

/// Compares the zip of `pair` with its folder. The folder's files come
/// from the archive; the zip's from the archive when it is there, else
/// from its peeked listing (`listing`).
pub fn compare(
    archive: &Archive,
    pair: &Pair,
    listing: Option<&[Entry]>,
    tmp: &Path,
) -> Result<Comparison, MirrorError> {
    let mut cmp = Comparison {
        zip: pair.zip.clone(),
        folder: pair.folder.clone(),
        zip_size: pair.zip_size,
        verdict: Verdict::Same,
        same: 0,
        repacked: 0,
        only_in_zip: Vec::new(),
        different: Vec::new(),
        unsure: Vec::new(),
        from_listing: false,
        only_in_folder: Vec::new(),
        not_archived: Vec::new(),
    };
    // The folder's files by their path under the folder.
    let mut folder: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (url, _) in &pair.files {
        let rel = percent_decoded(&url[pair.folder.len()..]);
        let path = archive.path_for(url);
        if path.is_file() {
            folder.insert(rel, path);
        } else {
            cmp.not_archived.push(rel);
        }
    }
    let zip_path = archive.path_for(&pair.zip);
    let mut zip = None;
    let files: Vec<ZipFile> = if zip_path.is_file() {
        let file = File::open(&zip_path).map_err(MirrorError::io(&zip_path))?;
        let bad = |e: zip::result::ZipError| MirrorError::Zip {
            path: zip_path.display().to_string(),
            message: e.to_string(),
        };
        let mut z = ZipArchive::new(BufReader::with_capacity(64 << 10, file)).map_err(bad)?;
        let mut files = Vec::with_capacity(z.len());
        for i in 0..z.len() {
            let f = z.by_index_raw(i).map_err(bad)?;
            if !f.is_dir() {
                files.push(ZipFile {
                    name: f.name().to_owned(),
                    sum: Sum {
                        size: f.size(),
                        crc32: f.crc32(),
                    },
                    index: Some(i),
                });
            }
        }
        zip = Some(z);
        files
    } else if let Some(entries) = listing {
        cmp.from_listing = true;
        entries
            .iter()
            .filter(|e| !e.name.ends_with('/'))
            .map(|e| ZipFile {
                name: e.name.clone(),
                sum: Sum {
                    size: e.size,
                    crc32: e.crc32,
                },
                index: None,
            })
            .collect()
    } else {
        cmp.not_archived.insert(0, pair.zip.clone());
        Vec::new()
    };
    if !cmp.not_archived.is_empty() {
        cmp.verdict = Verdict::NotArchived;
        return Ok(cmp);
    }
    let mut matched: HashSet<String> = HashSet::new();
    for f in files {
        let Some(rel) = suffix_in(&f.name, &folder) else {
            cmp.only_in_zip.push(f.name);
            continue;
        };
        let path = &folder[&rel];
        matched.insert(rel.clone());
        if sum_of(path).map_err(MirrorError::io(path))? == f.sum {
            cmp.same += 1;
            continue;
        }
        // Other bytes: the same files inside, packed again?
        let (Some(z), Some(i)) = (zip.as_mut(), f.index) else {
            cmp.unsure.push(rel);
            continue;
        };
        let inner = list_entry(z, i, tmp, NESTED_IN_MEMORY).ok();
        let theirs = File::open(path)
            .ok()
            .and_then(|f| list(BufReader::with_capacity(64 << 10, f)).ok());
        match (inner, theirs) {
            (Some(a), Some(b)) if a == b => cmp.repacked += 1,
            _ => cmp.different.push(rel),
        }
    }
    cmp.only_in_folder = folder
        .keys()
        .filter(|k| !matched.contains(*k))
        .cloned()
        .collect();
    if !cmp.only_in_zip.is_empty() || !cmp.different.is_empty() {
        cmp.verdict = Verdict::Differs;
    } else if !cmp.unsure.is_empty() {
        cmp.verdict = Verdict::NeedsDownload;
    }
    Ok(cmp)
}

/// Removes the zip of a comparison that found it the same as its folder,
/// recording it first: from the archive when it was downloaded, and from
/// the download to come when it was ruled on from its listing. Refuses any
/// other comparison.
pub fn remove(
    mirror: &Mirror,
    archive: &Archive,
    cmp: &Comparison,
) -> Result<Removed, MirrorError> {
    if cmp.verdict != Verdict::Same {
        return Err(MirrorError::NotSame {
            url: cmp.zip.clone(),
        });
    }
    let path = archive.path_for(&cmp.zip);
    let sidecar = Archive::read_sidecar(&path)?;
    if !cmp.from_listing && sidecar.is_none() {
        return Err(MirrorError::NotSame {
            url: cmp.zip.clone(),
        });
    }
    let removed = Removed {
        url: cmp.zip.clone(),
        size: sidecar.as_ref().map_or(cmp.zip_size, |s| s.size),
        downloaded: sidecar.is_some(),
        sha256: sidecar.map(|s| s.sha256),
        same_as: cmp.folder.clone(),
        removed_at: crate::clock::now_utc_rfc3339(),
    };
    let record = mirror.removed_path();
    let mut line = serde_json::to_vec(&removed).expect("a Removed always serialises");
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&record)
        .map_err(MirrorError::io(&record))?;
    f.write_all(&line).map_err(MirrorError::io(&record))?;
    f.sync_all().map_err(MirrorError::io(&record))?;
    if removed.downloaded {
        std::fs::remove_file(&path).map_err(MirrorError::io(&path))?;
        let sc = Archive::sidecar_path(&path);
        std::fs::remove_file(&sc).map_err(MirrorError::io(&sc))?;
    }
    Ok(removed)
}

/// The shortest tail of `name`, cut at a `/`, that is a path of `folder`.
fn suffix_in(name: &str, folder: &BTreeMap<String, PathBuf>) -> Option<String> {
    let mut tail = name;
    loop {
        if folder.contains_key(tail) {
            return Some(tail.to_owned());
        }
        tail = &tail[tail.find('/')? + 1..];
    }
}

/// The size and CRC-32 of a file, read in full.
fn sum_of(path: &Path) -> std::io::Result<Sum> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; READ_BUFFER];
    let mut h = crc32fast::Hasher::new();
    let mut size = 0u64;
    loop {
        match f.read(&mut buf)? {
            0 => break,
            n => {
                h.update(&buf[..n]);
                size += n as u64;
            }
        }
    }
    Ok(Sum {
        size,
        crc32: h.finalize(),
    })
}

/// What a zip holds: each file's name (without folders), size and CRC-32.
type Listing = BTreeMap<String, (u64, u32)>;

fn list<R: Read + Seek>(r: R) -> zip::result::ZipResult<Listing> {
    let mut z = ZipArchive::new(r)?;
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let f = z.by_index_raw(i)?;
        if !f.is_dir() {
            let name = f.name().rsplit('/').next().unwrap_or("").to_owned();
            out.insert(name, (f.size(), f.crc32()));
        }
    }
    Ok(out)
}

/// The listing of entry `i` of `z`, itself a zip.
fn list_entry<R: Read + Seek>(
    z: &mut ZipArchive<R>,
    i: usize,
    tmp: &Path,
    in_memory: u64,
) -> std::io::Result<Listing> {
    let mut f = z.by_index(i).map_err(std::io::Error::other)?;
    let listed = if f.size() <= in_memory {
        let mut buf = Vec::with_capacity(usize::try_from(f.size()).unwrap_or(0));
        f.read_to_end(&mut buf)?;
        list(Cursor::new(buf))
    } else {
        std::fs::create_dir_all(tmp)?;
        let mut t = tempfile::tempfile_in(tmp)?;
        std::io::copy(&mut f, &mut t)?;
        t.rewind()?;
        list(BufReader::with_capacity(64 << 10, t))
    };
    listed.map_err(std::io::Error::other)
}

/// `%XX` escapes decoded, as the archive decodes a URL's path.
fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::diag::Diagnose as _;
    use crate::net::transport::StaticTransport;
    use crate::net::{Fetcher, FetcherConfig, Sidecar};
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::crawl::{Remote, root};

    fn zip_of(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(method);
        for (name, bytes) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn archive_file(archive: &Archive, url: &str, bytes: &[u8]) {
        let p = archive.path_for(url);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        Archive::write_sidecar(
            &p,
            &Sidecar {
                url: url.into(),
                size: bytes.len() as u64,
                sha256: crate::net::archive::sha256_hex(bytes),
                fetched_at: "t".into(),
                etag: None,
                last_modified: None,
                tool: "nemlake".into(),
            },
        )
        .unwrap();
    }

    struct Fixture {
        _home: tempfile::TempDir,
        mirror: Mirror,
        archive: Archive,
    }

    /// `M/` holds two table zips and a control file; `M.zip` holds the
    /// first table zip byte for byte under `M/`, the second packed again,
    /// and, when `extra`, a file the folder lacks.
    fn fixture(extra: bool) -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        let t1 = zip_of(&[("T1.CSV", b"a,b\n")], zip::CompressionMethod::Deflated);
        let t2 = zip_of(&[("T2.CSV", b"c,d\n")], zip::CompressionMethod::Deflated);
        let t2_again = zip_of(&[("T2.CSV", b"c,d\n")], zip::CompressionMethod::Stored);
        let folder = [
            ("https://h/A/M/S/DATA/T1%231.zip", &t1[..]),
            ("https://h/A/M/S/DATA/T2.zip", &t2[..]),
            ("https://h/A/M/S/CTL/T1.ctl", b"ctl"),
        ];
        let mut inside: Vec<(&str, &[u8])> = vec![
            ("M/S/DATA/", b""),
            ("M/S/DATA/T1#1.zip", &t1),
            ("M/S/DATA/T2.zip", &t2_again),
        ];
        if extra {
            inside.push(("M/README.txt", b"only here"));
        }
        let whole = zip_of(&inside, zip::CompressionMethod::Stored);
        let mut record = vec![
            Remote {
                url: "https://h/A/M/".into(),
                kind: RemoteKind::Dir,
                size: None,
                modified: "m".into(),
                listed_on: "https://h/A/".into(),
            },
            Remote {
                url: "https://h/A/M.zip".into(),
                kind: RemoteKind::File,
                size: Some(whole.len() as u64),
                modified: "m".into(),
                listed_on: "https://h/A/".into(),
            },
            Remote {
                url: "https://h/A/lone.zip".into(),
                kind: RemoteKind::File,
                size: Some(1),
                modified: "m".into(),
                listed_on: "https://h/A/".into(),
            },
        ];
        archive_file(&archive, "https://h/A/M.zip", &whole);
        for (url, bytes) in folder {
            archive_file(&archive, url, bytes);
            record.push(Remote {
                url: url.into(),
                kind: RemoteKind::File,
                size: Some(bytes.len() as u64),
                modified: "m".into(),
                listed_on: "https://h/A/M/S/".into(),
            });
        }
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
        }
    }

    #[tokio::test]
    async fn a_zip_the_same_as_its_folder_is_removed_and_stays_mapped() {
        let f = fixture(false);
        let pairs = pairs(&f.mirror).unwrap();
        assert_eq!(pairs.len(), 1, "lone.zip has no folder");
        assert_eq!(pairs[0].folder, "https://h/A/M/");
        assert_eq!(pairs[0].files.len(), 3);
        let tmp = f.mirror.dir().join("tmp");
        let cmp = compare(&f.archive, &pairs[0], None, &tmp).unwrap();
        assert_eq!(cmp.verdict, Verdict::Same, "{cmp:?}");
        assert_eq!((cmp.same, cmp.repacked), (1, 1));
        assert_eq!(cmp.only_in_folder, ["S/CTL/T1.ctl"]);

        let removed = remove(&f.mirror, &f.archive, &cmp).unwrap();
        assert_eq!(removed.same_as, "https://h/A/M/");
        assert!(!f.archive.path_for("https://h/A/M.zip").exists());
        assert!(
            f.mirror
                .removed()
                .unwrap()
                .contains_key("https://h/A/M.zip")
        );

        // Not fetched again; counted as removed; mapped as removed.
        let t = Arc::new(StaticTransport::default());
        let fetcher = Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(f.archive.root()),
            FetcherConfig {
                attempts: 1,
                backoff: Duration::ZERO,
                max_backoff: Duration::ZERO,
                min_interval: Duration::ZERO,
            },
        );
        let s = crate::download(&fetcher, &f.mirror, 1, |_| {})
            .await
            .unwrap();
        assert_eq!((s.files, s.removed, s.cached), (5, 1, 3));
        assert!(
            !t.requests
                .lock()
                .unwrap()
                .iter()
                .any(|u| u.ends_with("M.zip"))
        );
        let roots = [root("https://h/A/").unwrap()];
        let st = crate::status(&f.mirror, &f.archive, &roots).unwrap();
        assert_eq!(
            (st.roots[0].removed_files, st.roots[0].archived_files),
            (1, 3)
        );
        let m = crate::map(&f.mirror, &f.archive, &roots, 1).unwrap();
        assert_eq!(m.removed, 1);
        let tree = std::fs::read_to_string(f.mirror.dir().join("map/tree.txt")).unwrap();
        assert!(tree.contains("M.zip  "), "{tree}");
        assert!(tree.contains("[removed: same as its folder]"), "{tree}");
        let ndjson = std::fs::read_to_string(f.mirror.dir().join("map/map.ndjson")).unwrap();
        let line = ndjson
            .lines()
            .find(|l| l.contains("\"h/A/M.zip\""))
            .unwrap();
        assert!(line.contains("\"same_as\":\"https://h/A/M/\""), "{line}");
    }

    #[test]
    fn a_zip_with_more_than_its_folder_is_kept() {
        let f = fixture(true);
        let pair = &pairs(&f.mirror).unwrap()[0];
        let cmp = compare(&f.archive, pair, None, &f.mirror.dir().join("tmp")).unwrap();
        assert_eq!(cmp.verdict, Verdict::Differs);
        assert_eq!(cmp.only_in_zip, ["M/README.txt"]);
        let e = remove(&f.mirror, &f.archive, &cmp).unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.not_same");
        assert!(f.archive.path_for(&pair.zip).exists());
    }

    #[test]
    fn a_different_file_and_files_not_yet_archived() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror).unwrap()[0];
        let t1 = f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip");
        std::fs::write(&t1, b"not the same").unwrap();
        let cmp = compare(&f.archive, pair, None, &f.mirror.dir().join("tmp")).unwrap();
        assert_eq!(cmp.verdict, Verdict::Differs);
        assert_eq!(cmp.different, ["S/DATA/T1#1.zip"]);

        std::fs::remove_file(&t1).unwrap();
        std::fs::remove_file(f.archive.path_for(&pair.zip)).unwrap();
        let cmp = compare(&f.archive, pair, None, &f.mirror.dir().join("tmp")).unwrap();
        assert_eq!(cmp.verdict, Verdict::NotArchived);
        assert_eq!(cmp.not_archived, ["https://h/A/M.zip", "S/DATA/T1#1.zip"]);
    }

    #[test]
    fn a_broken_zip_or_record_says_where() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror).unwrap()[0];
        std::fs::write(f.archive.path_for(&pair.zip), b"PK\x03\x04 broken").unwrap();
        let e = compare(&f.archive, pair, None, Path::new("/tmp")).unwrap_err();
        assert_eq!(e.diagnostic().code, "zip.open_failed");
        // The directory lists an entry whose header is not there.
        std::fs::write(
            f.archive.path_for(&pair.zip),
            crate::testzip::bad_second_header(),
        )
        .unwrap();
        let e = compare(&f.archive, pair, None, Path::new("/tmp")).unwrap_err();
        assert_eq!(e.diagnostic().code, "zip.open_failed");
        // A zip without its sidecar is not removed.
        std::fs::remove_file(f.archive.path_for(&pair.zip)).unwrap();
        let f2 = fixture(false);
        let pair2 = &pairs(&f2.mirror).unwrap()[0];
        let cmp = compare(&f2.archive, pair2, None, &f2.mirror.dir().join("tmp")).unwrap();
        std::fs::remove_file(Archive::sidecar_path(&f2.archive.path_for(&pair2.zip))).unwrap();
        let e = remove(&f2.mirror, &f2.archive, &cmp).unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.not_same");
        // A record of removals that is a folder cannot be read.
        std::fs::create_dir_all(f.mirror.removed_path()).unwrap();
        assert_eq!(
            f.mirror.removed().unwrap_err().diagnostic().code,
            "mirror.io"
        );
        std::fs::remove_dir(f.mirror.removed_path()).unwrap();
        std::fs::write(f.mirror.removed_path(), "{}\n").unwrap();
        assert_eq!(
            f.mirror.removed().unwrap_err().diagnostic().code,
            "mirror.record_corrupt"
        );
        std::fs::remove_file(f.mirror.done_path()).unwrap();
        assert_eq!(
            pairs(&f.mirror).unwrap_err().diagnostic().code,
            "mirror.crawl_incomplete"
        );
    }

    #[test]
    fn a_big_zip_inside_is_spooled_to_list_it() {
        let dir = tempfile::tempdir().unwrap();
        let inner = zip_of(&[("X.CSV", b"x")], zip::CompressionMethod::Stored);
        let outer = zip_of(&[("x.zip", &inner)], zip::CompressionMethod::Deflated);
        let mut z = ZipArchive::new(Cursor::new(outer)).unwrap();
        let a = list_entry(&mut z, 0, dir.path(), NESTED_IN_MEMORY).unwrap();
        let b = list_entry(&mut z, 0, &dir.path().join("spool"), 0).unwrap();
        assert_eq!(a["X.CSV"].0, 1);
        assert_eq!(a, b);
        assert_eq!(percent_decoded("a%23b%2"), "a#b%2");
    }

    #[test]
    fn a_zip_ruled_on_from_its_listing_is_never_downloaded() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror).unwrap()[0];
        let zip_path = f.archive.path_for(&pair.zip);
        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(Archive::sidecar_path(&zip_path)).unwrap();
        let entry = |name: &str, bytes: &[u8]| Entry {
            name: name.into(),
            size: bytes.len() as u64,
            compressed: bytes.len() as u64,
            crc32: crc32fast::hash(bytes),
            method: 0,
            modified: "m".into(),
        };
        let t1 = std::fs::read(f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip")).unwrap();
        let t2 = std::fs::read(f.archive.path_for("https://h/A/M/S/DATA/T2.zip")).unwrap();
        let tmp = f.mirror.dir().join("tmp");
        let mut listing = vec![
            entry("M/S/DATA/", b""),
            entry("M/S/DATA/T1#1.zip", &t1),
            entry("M/S/DATA/T2.zip", &t2),
        ];
        let cmp = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(
            (cmp.verdict, cmp.from_listing, cmp.same),
            (Verdict::Same, true, 2)
        );
        // Not byte for byte: only the zip itself could tell more.
        listing[2] = entry("M/S/DATA/T2.zip", b"other bytes");
        let unsure = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(unsure.verdict, Verdict::NeedsDownload);
        assert_eq!(unsure.unsure, ["S/DATA/T2.zip"]);
        assert!(remove(&f.mirror, &f.archive, &unsure).is_err());
        // A file the folder lacks settles it.
        listing.push(entry("M/README.txt", b"r"));
        let differs = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(differs.verdict, Verdict::Differs);

        let r = remove(&f.mirror, &f.archive, &cmp).unwrap();
        assert_eq!(
            (r.downloaded, r.sha256.as_deref(), r.size),
            (false, None, pair.zip_size)
        );
        let back = &f.mirror.removed().unwrap()[&pair.zip];
        assert!(!back.downloaded);
        // A record written before this field existed was of a downloaded zip.
        std::fs::write(
            f.mirror.removed_path(),
            "{\"url\":\"u\",\"size\":1,\"sha256\":\"s\",\"same_as\":\"f\",\"removed_at\":\"t\"}\n",
        )
        .unwrap();
        assert!(f.mirror.removed().unwrap()["u"].downloaded);
    }
}
