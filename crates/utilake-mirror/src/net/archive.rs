//! The archive: every downloaded file, kept as the server sent it.
//!
//! A file lives at a path that mirrors its URL
//! (`{archive}/www.nemweb.com.au/Data_Archive/.../PUBLIC_DVD_X_202401010000.zip`)
//! beside a small JSON sidecar that records where it came from, when, and
//! what its bytes hash to. A file is downloaded once; nothing in utilake-mirror
//! replaces or deletes an archived file, which is the untransformed record
//! the census and the lake are derived from. To fetch one again, delete it.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::net::error::FetchError;
use crate::stamp::Stamp;

/// Hands a file's writes to the disk with `fsync`. On macOS
/// `File::sync_all` and `sync_data` also flush the drive's own cache
/// (`F_FULLFSYNC`), which on a USB disk costs most of a second per small
/// file. What that flush adds guards only against a power cut, after which
/// a file's bytes may not match the sha256 in its sidecar.
pub fn to_disk(file: &std::fs::File) -> std::io::Result<()> {
    rustix::fs::fsync(file).map_err(std::io::Error::from)
}

/// The names each directory holds, read once per directory. On exFAT a
/// lookup of a name that is not there scans the whole directory, so one
/// read of a large folder answers every miss in it.
#[derive(Debug, Default)]
pub struct Listed {
    dirs: HashMap<PathBuf, HashSet<OsString>>,
}

impl Listed {
    /// Whether `path`'s directory holds its name. A directory that cannot
    /// be read holds nothing.
    pub fn holds(&mut self, path: &Path) -> bool {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        self.dirs
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                std::fs::read_dir(dir)
                    .map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name()).collect())
                    .unwrap_or_default()
            })
            .contains(name)
    }
}

/// Where an archived file came from and what its bytes are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// The URL the file was downloaded from.
    pub url: String,
    /// Bytes.
    pub size: u64,
    /// sha256 hex of the bytes.
    pub sha256: String,
    /// CRC-32 of the bytes, as zips record their entries': written by the
    /// download, or by `compare` for a file downloaded before it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crc32: Option<u32>,
    /// When the download completed, RFC 3339 UTC.
    pub fetched_at: String,
    /// The server's `ETag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The server's `Last-Modified`: when AEMO published the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// `last_modified` as an instant (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_at: Option<Stamp>,
    /// The size the listing gave it when it was downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_size: Option<u64>,
    /// The date and time the listing gave it when it was downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified: Option<String>,
    /// `listed_modified` as an instant (AEST).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified_at: Option<Stamp>,
    /// The program and version that downloaded it, e.g. `utilake-mirror/0.1.0`.
    #[serde(default)]
    pub tool: String,
    /// Every other place AEMO serves these bytes, found after the
    /// download: the same path under the other listing, or an entry of a
    /// daily zip. The bytes never change, so only this list grows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<Also>,
    /// When the listing stopped naming `url` (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone_at: Option<Stamp>,
}

/// Another address of an archived file's bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Also {
    /// The URL: a file's, or `{zip URL}!{entry}` for an entry of a zip.
    pub url: String,
    /// How the bytes were found there.
    pub how: How,
    /// When they were found there (UTC).
    pub seen_at: Stamp,
    /// When the listing stopped naming the address (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone_at: Option<Stamp>,
    /// The date and time the listing gave the address when its bytes were
    /// found the same, so a later version under it is told apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_at: Option<Stamp>,
}

/// How another address was found to serve an archived file's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// The URL served the same bytes (sha256 equal).
    Same,
    /// An entry of a zip with the file's name, size and CRC-32.
    Entry,
}

impl Sidecar {
    /// Whether `url` is an address of these bytes: the URL they were
    /// downloaded from, or one of `also`.
    pub fn names(&self, url: &str) -> bool {
        self.url == url || self.also.iter().any(|a| a.url == url)
    }

    /// Adds `url` to `also`, unless it is an address already. Whether it
    /// was added.
    pub fn add_also(&mut self, url: &str, how: How, seen_at: Stamp) -> bool {
        self.add_listed(url, how, seen_at, None)
    }

    /// `add_also`, with the time the listing gave the address.
    pub fn add_listed(
        &mut self,
        url: &str,
        how: How,
        seen_at: Stamp,
        listed_at: Option<Stamp>,
    ) -> bool {
        if self.names(url) {
            return false;
        }
        self.also.push(Also {
            url: url.to_owned(),
            how,
            seen_at,
            gone_at: None,
            listed_at,
        });
        true
    }

    /// Notes that the listing no longer names the address `url`, unless
    /// it is noted already. Whether anything changed.
    pub fn mark_gone(&mut self, url: &str, at: Stamp) -> bool {
        if self.url == url {
            return self.gone_at.is_none() && self.gone_at.replace(at).is_none();
        }
        let mut changed = false;
        for a in self
            .also
            .iter_mut()
            .filter(|a| a.url == url && a.gone_at.is_none())
        {
            a.gone_at = Some(at);
            changed = true;
        }
        changed
    }

    /// Every address of these bytes: `url`, then each of `also`.
    pub fn addresses(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.url.as_str()).chain(self.also.iter().map(|a| a.url.as_str()))
    }

    /// `is_listed_as` for the address `url`: the download's listing when
    /// it is the URL downloaded from, else the listing `also` recorded.
    pub fn is_listed_at(&self, url: &str, size: Option<u64>, modified_at: Option<Stamp>) -> bool {
        if self.url == url {
            return self.is_listed_as(size, modified_at);
        }
        self.also.iter().filter(|a| a.url == url).any(|a| {
            size.is_none_or(|s| s == self.size)
                && match (a.listed_at, modified_at) {
                    (Some(x), Some(y)) => x == y,
                    _ => true,
                }
        })
    }

    /// Whether these are the bytes nemweb lists at `size` and
    /// `modified_at`: the same size, and the same listed time when both
    /// have one. nemweb rewrites some files in place under the same name;
    /// a listing that differs is a later version.
    pub fn is_listed_as(&self, size: Option<u64>, modified_at: Option<Stamp>) -> bool {
        size.is_none_or(|s| s == self.size)
            && match (self.listed_modified_at, modified_at) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }

    /// The sidecar with its `_at` instants derived from the text fields
    /// beside them, where a sidecar written before they were kept lacks
    /// them.
    pub fn derived(mut self) -> Self {
        if self.last_modified_at.is_none() {
            self.last_modified_at = self
                .last_modified
                .as_deref()
                .and_then(Stamp::from_http_date);
        }
        if self.listed_modified_at.is_none() {
            self.listed_modified_at = self
                .listed_modified
                .as_deref()
                .and_then(Stamp::from_listing);
        }
        self
    }
}

/// An archived file with its sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived {
    /// The bytes.
    pub path: PathBuf,
    /// What is known about them.
    pub sidecar: Sidecar,
}

/// The archive directory.
#[derive(Debug, Clone)]
pub struct Archive {
    root: PathBuf,
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// sha256 hex of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// sha256 hex and length of the file at `path`, read in 1 MiB pieces so a
/// file of any size takes the same memory. Blocking: call it off the
/// async runtime.
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    hashes_of(path).map(|h| (h.sha256, h.size))
}

/// A file's sha256, CRC-32 and size, from one read of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashes {
    /// sha256 hex.
    pub sha256: String,
    /// CRC-32.
    pub crc32: u32,
    /// Bytes.
    pub size: u64,
}

/// Reads the file at `path` once, in 1 MiB pieces, for its sha256 and
/// CRC-32. Blocking: call it off the async runtime.
pub fn hashes_of(path: &Path) -> std::io::Result<Hashes> {
    /// Both hashers as an `io::Write`, so `io::copy` drives them.
    struct Hashing(Sha256, crc32fast::Hasher);
    impl std::io::Write for Hashing {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.update(buf);
            self.1.update(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let f = std::fs::File::open(path)?;
    let mut hasher = Hashing(Sha256::new(), crc32fast::Hasher::new());
    let size = std::io::copy(
        &mut std::io::BufReader::with_capacity(1 << 20, f),
        &mut hasher,
    )?;
    std::io::Write::flush(&mut hasher)?;
    Ok(Hashes {
        sha256: hex(&hasher.0.finalize()),
        crc32: hasher.1.finalize(),
        size,
    })
}

/// Whether `path` names a macOS AppleDouble file (`._name`), which the
/// system writes beside a file on a volume without extended attributes.
/// It is not a download: every walk of the archive skips it.
pub fn is_apple_double(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("._"))
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> FetchError + '_ {
    move |source| FetchError::CacheIo {
        path: path.display().to_string(),
        source,
    }
}

impl Archive {
    /// The archive at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The archive directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a URL's bytes are kept (`utilake_provenance::layout`): the
    /// URL as a path under the archive, host first, one folder per report
    /// folder (a `CURRENT` or `ARCHIVE` listing segment dropped). A query
    /// string, which no archive URL has, is folded into the file name as a
    /// hash so two queries of one path stay apart; a fragment is not part
    /// of the file.
    pub fn path_for(&self, url: &str) -> PathBuf {
        let mut path = self.root.clone();
        for s in utilake_provenance::layout::local_segments(url) {
            path.push(s);
        }
        path
    }

    /// `url` as a path under the archive with nothing dropped
    /// (`utilake_provenance::layout::url_segments`): where a record of
    /// one listing is kept, so the two listings of a report folder keep a
    /// record each.
    pub fn url_path(&self, url: &str) -> PathBuf {
        let mut path = self.root.clone();
        for s in utilake_provenance::layout::url_segments(url) {
            path.push(s);
        }
        path
    }

    /// The sidecar of an archived path.
    pub fn sidecar_path(path: &Path) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(".json");
        PathBuf::from(s)
    }

    /// Where a later version of the file at `path` is kept: beside it, as
    /// `{name}@{at}`, `at` the time nemweb listed that version with
    /// (`20261021T115300+1000`). The first version downloaded keeps the
    /// plain name; nothing archived is moved or written over.
    pub fn version_path(path: &Path, at: Stamp) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(format!("@{}", at.compact()));
        PathBuf::from(s)
    }

    /// Every version of `url` archived: the files at `path` and each
    /// `{name}@{at}` beside it whose sidecar names `url`, in name order,
    /// so in the order nemweb listed them.
    pub fn versions(path: &Path, url: &str) -> Result<Vec<Archived>, FetchError> {
        Ok(Self::held_at(path)?
            .into_iter()
            .filter(|a| a.sidecar.names(url))
            .collect())
    }

    /// Every file held at `path`, whatever URL it came from: the file at
    /// `path`, then each `{name}@{at}` beside it, in name order. Two URLs
    /// can share a path (`layout`: both listings of a report folder are
    /// one folder); their files never replace each other.
    pub fn held_at(path: &Path) -> Result<Vec<Archived>, FetchError> {
        let mut out: Vec<Archived> = Self::held_file(path.to_path_buf())?.into_iter().collect();
        // A version is kept only beside a first file: with none there, the
        // folder (on exFAT, a scan of it) is not read.
        if std::fs::symlink_metadata(path).is_err() {
            return Ok(out);
        }
        let dir = path.parent().unwrap_or(Path::new("."));
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!("{name}@");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&prefix) && !n.ends_with(".json") && !n.ends_with(".part"))
            .collect();
        names.sort();
        for n in names {
            out.extend(Self::held_file(dir.join(n))?);
        }
        Ok(out)
    }

    /// The file at `path` with its sidecar, when both are there and agree
    /// on the size.
    fn held_file(path: PathBuf) -> Result<Option<Archived>, FetchError> {
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        let sidecar = match Self::read_sidecar(&path) {
            Ok(Some(s)) => s,
            Ok(None) | Err(FetchError::SidecarCorrupt { .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        if sidecar.size != meta.len() {
            return Ok(None);
        }
        Ok(Some(Archived { path, sidecar }))
    }

    /// The in-progress path of a download.
    pub fn part_path(path: &Path) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(".part");
        PathBuf::from(s)
    }

    /// The sidecar at `path`'s side, when there is one, with its `_at`
    /// instants.
    pub fn read_sidecar(path: &Path) -> Result<Option<Sidecar>, FetchError> {
        let sc = Self::sidecar_path(path);
        let text = match std::fs::read_to_string(&sc) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err(&sc)(e)),
        };
        serde_json::from_str::<Sidecar>(&text)
            .map(|s| Some(s.derived()))
            .map_err(|e| FetchError::SidecarCorrupt {
                path: sc.display().to_string(),
                message: e.to_string(),
            })
    }

    /// The archived file of a URL: the first file at its path, or a
    /// version beside it, whose sidecar names the URL (`Sidecar::names`)
    /// and size. A sidecar that does not read (cut short when a run was
    /// stopped as it wrote it) is no sidecar: the file is downloaded again
    /// and the sidecar rewritten.
    pub fn lookup(&self, url: &str) -> Result<Option<Archived>, FetchError> {
        let path = self.path_for(url);
        if let Some(first) = Self::lookup_at(path.clone(), url)? {
            return Ok(Some(first));
        }
        Ok(Self::versions(&path, url)?.into_iter().next())
    }

    /// The file at `path` with its sidecar, when it is archived from `url`.
    pub fn lookup_at(path: PathBuf, url: &str) -> Result<Option<Archived>, FetchError> {
        Ok(Self::held_file(path)?.filter(|a| a.sidecar.names(url)))
    }

    /// Hashes an archived file and compares it with its sidecar.
    pub fn verify(entry: &Archived) -> Result<(), FetchError> {
        let (observed, _) = sha256_file(&entry.path).map_err(io_err(&entry.path))?;
        if observed != entry.sidecar.sha256 {
            return Err(FetchError::Sha256Mismatch {
                path: entry.path.display().to_string(),
                expected: entry.sidecar.sha256.clone(),
                observed,
            });
        }
        Ok(())
    }

    /// Writes a sidecar whole or not at all: to a temporary file beside
    /// it, handed to the disk, then renamed over it.
    pub fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<(), FetchError> {
        use std::io::Write as _;
        let sc = Self::sidecar_path(path);
        let mut tmp = sc.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let text = serde_json::to_string_pretty(sidecar).expect("a Sidecar always serialises");
        let mut f = std::fs::File::create(&tmp).map_err(io_err(&tmp))?;
        f.write_all(text.as_bytes()).map_err(io_err(&tmp))?;
        to_disk(&f).map_err(io_err(&tmp))?;
        std::fs::rename(&tmp, &sc).map_err(io_err(&sc))
    }

    /// Notes in the sidecar of the file at `path` that the listing no
    /// longer names each of `urls` (`Sidecar::mark_gone`). Whether the
    /// sidecar changed.
    pub fn mark_gone<'a>(
        path: &Path,
        urls: impl IntoIterator<Item = &'a str>,
        at: Stamp,
    ) -> Result<bool, FetchError> {
        let Some(mut sidecar) = Self::read_sidecar(path)? else {
            return Ok(false);
        };
        let mut changed = false;
        for u in urls {
            changed |= sidecar.mark_gone(u, at);
        }
        if changed {
            Self::write_sidecar(path, &sidecar)?;
        }
        Ok(changed)
    }

    /// Notes, in every sidecar under the folder `folder` (a URL ending in
    /// `/`), that each address under it is gone. How many sidecars changed.
    pub fn folder_gone(&self, folder: &str, at: Stamp) -> Result<usize, FetchError> {
        let mut n = 0;
        for a in self.entries_under(Some(&self.path_for(folder))) {
            let gone: Vec<String> = a
                .sidecar
                .addresses()
                .filter(|u| u.starts_with(folder))
                .map(str::to_owned)
                .collect();
            if Self::mark_gone(&a.path, gone.iter().map(String::as_str), at)? {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Adds `url` to the `also` of the sidecar of the file at `path`, when
    /// it is not an address of it already. Whether the sidecar changed.
    pub fn add_also(path: &Path, url: &str, how: How, seen_at: Stamp) -> Result<bool, FetchError> {
        let Some(mut sidecar) = Self::read_sidecar(path)? else {
            return Ok(false);
        };
        if !sidecar.add_also(url, how, seen_at) {
            return Ok(false);
        }
        Self::write_sidecar(path, &sidecar)?;
        Ok(true)
    }

    /// Every archived file under `dir` (the archive itself by default),
    /// with its sidecar, in path order.
    pub fn entries_under(&self, dir: Option<&Path>) -> Vec<Archived> {
        let mut out = Vec::new();
        let mut stack = vec![dir.unwrap_or(&self.root).to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for p in rd.filter_map(Result::ok).map(|e| e.path()) {
                if is_apple_double(&p) {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "json") {
                    let data = p.with_extension("");
                    if data.is_file()
                        && let Ok(Some(sidecar)) = Self::read_sidecar(&data)
                    {
                        out.push(Archived {
                            path: data,
                            sidecar,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_reads_each_directory_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"").unwrap();
        let mut listed = Listed::default();
        assert!(listed.holds(&dir.path().join("a")));
        // Read once: a file made since is not seen.
        std::fs::write(dir.path().join("b"), b"").unwrap();
        assert!(!listed.holds(&dir.path().join("b")));
        assert!(!listed.holds(&dir.path().join("none/a")));
        assert!(!listed.holds(Path::new("/")));
    }
    use crate::diag::Diagnose as _;

    #[test]
    fn apple_double_files_are_not_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let a = Archive::new(dir.path());
        let url = "https://h/d/f.csv";
        let path = a.path_for(url);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"abc").unwrap();
        let sidecar = Sidecar {
            url: url.into(),
            size: 3,
            sha256: sha256_hex(b"abc"),
            crc32: None,
            fetched_at: "2026-10-05T00:00:00Z".into(),
            etag: None,
            last_modified: None,
            last_modified_at: None,
            listed_size: None,
            listed_modified: None,
            listed_modified_at: None,
            tool: "t".into(),
            also: Vec::new(),
            gone_at: None,
        };
        Archive::write_sidecar(&path, &sidecar).unwrap();
        // AppleDouble twins of the file and its sidecar, with a sidecar of
        // their own that reads.
        let twin = path.with_file_name("._f.csv");
        std::fs::write(&twin, b"abc").unwrap();
        Archive::write_sidecar(&twin, &sidecar).unwrap();
        std::fs::write(path.with_file_name("._f.csv.json.json"), b"{}").unwrap();
        let found = a.entries_under(None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, path);
        assert!(is_apple_double(&twin) && !is_apple_double(&path));
        // An address added once; the sidecar is rewritten whole.
        let at = Stamp::now();
        assert!(Archive::add_also(&path, "https://h/e/f.csv", How::Same, at).unwrap());
        assert!(!Archive::add_also(&path, "https://h/e/f.csv", How::Same, at).unwrap());
        assert!(!Archive::add_also(&path, url, How::Same, at).unwrap());
        assert!(!Archive::add_also(&dir.path().join("none"), url, How::Same, at).unwrap());
        let back = Archive::read_sidecar(&path).unwrap().unwrap();
        assert!(back.names("https://h/e/f.csv") && back.names(url) && !back.names("https://h/x"));
        assert_eq!(back.sha256, sidecar.sha256);
        // An address with no listing time recorded is any listing of it.
        assert!(back.is_listed_at("https://h/e/f.csv", Some(3), Some(at)));
        assert!(!back.is_listed_at("https://h/e/f.csv", Some(4), None));
        assert!(!back.is_listed_at("https://h/x", None, None));
        let text = std::fs::read_to_string(Archive::sidecar_path(&path)).unwrap();
        assert!(text.contains("\"how\": \"same\""), "{text}");
        assert!(
            !Archive::sidecar_path(&path)
                .with_extension("json.tmp")
                .exists()
        );
    }

    #[test]
    fn the_archive_mirrors_the_url() {
        let a = Archive::new("/home/archive");
        assert_eq!(a.root(), Path::new("/home/archive"));
        assert_eq!(
            a.path_for("https://www.nemweb.com.au/Data_Archive/X/PUBLIC_ARCHIVE%23T%23FILE01%23202608010000.zip"),
            Path::new("/home/archive/www.nemweb.com.au/Data_Archive/X/PUBLIC_ARCHIVE#T#FILE01#202608010000.zip")
        );
        assert_eq!(
            a.path_for("https://h/a/../b/?x=1"),
            Path::new(&format!(
                "/home/archive/h/a/_/b@{}",
                &sha256_hex(b"x=1")[..16]
            ))
        );
        assert_eq!(
            a.path_for("https://h/z.zip#member"),
            Path::new("/home/archive/h/z.zip")
        );
        assert_eq!(
            a.path_for("https://h/a:b"),
            Path::new("/home/archive/h/a_b")
        );
        // A report folder's two listings: one folder for their files, a
        // record each for what they list.
        let (c, r) = (
            "https://www.nemweb.com.au/Reports/CURRENT/X/",
            "https://www.nemweb.com.au/Reports/ARCHIVE/X/",
        );
        assert_eq!(a.path_for(c), a.path_for(r));
        assert_eq!(
            a.url_path(c),
            Path::new("/home/archive/www.nemweb.com.au/Reports/CURRENT/X")
        );
        assert_ne!(a.url_path(c), a.url_path(r));
        assert_eq!(
            Archive::part_path(Path::new("/x/y.zip")),
            Path::new("/x/y.zip.part")
        );
    }

    #[test]
    fn lookup_verify_and_entries() {
        let dir = tempfile::tempdir().unwrap();
        let a = Archive::new(dir.path());
        let url = "https://h/d/f.zip";
        assert!(a.lookup(url).unwrap().is_none());
        let path = a.path_for(url);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"abc").unwrap();
        // A file without its sidecar is not an archived download.
        assert!(a.lookup(url).unwrap().is_none());
        let (sha, len) = sha256_file(&path).unwrap();
        assert_eq!((sha.as_str(), len), (sha256_hex(b"abc").as_str(), 3));
        let sidecar = Sidecar {
            url: url.into(),
            size: 3,
            sha256: sha,
            crc32: None,
            fetched_at: "2026-10-05T00:00:00Z".into(),
            etag: None,
            last_modified: Some("Tue, 13 Feb 2024 08:27:00 GMT".into()),
            last_modified_at: None,
            listed_size: None,
            listed_modified: Some("Monday, October 12, 2026 10:45 AM".into()),
            listed_modified_at: None,
            tool: "nemlake/test".into(),
            also: Vec::new(),
            gone_at: None,
        };
        Archive::write_sidecar(&path, &sidecar).unwrap();
        // A sidecar written without the instants reads back with them,
        // derived from the text beside each; the text is as written.
        let found = a.lookup(url).unwrap().unwrap();
        assert_eq!(
            found.sidecar,
            Sidecar {
                last_modified_at: Some(Stamp::at(2024, 2, 13, 8, 27, 0, 0).unwrap()),
                listed_modified_at: Some(
                    Stamp::at(2026, 10, 12, 10, 45, 0, crate::stamp::AEST).unwrap()
                ),
                ..sidecar.clone()
            }
        );
        let text = std::fs::read_to_string(Archive::sidecar_path(&path)).unwrap();
        assert!(!text.contains("modified_at"), "{text}");
        Archive::write_sidecar(&path, &found.sidecar).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(Archive::sidecar_path(&path)).unwrap())
                .unwrap();
        assert_eq!(written["last_modified"], "Tue, 13 Feb 2024 08:27:00 GMT");
        assert_eq!(written["last_modified_at"], "2024-02-13T08:27:00Z");
        assert_eq!(written["listed_modified_at"], "2026-10-12T10:45:00+10:00");
        crate::stamp::check::assert_aware(&written);
        Archive::write_sidecar(&path, &sidecar).unwrap();
        let found = a.lookup(url).unwrap().unwrap();
        Archive::verify(&found).unwrap();
        assert_eq!(a.entries_under(None).len(), 1);
        assert_eq!(a.entries_under(Some(&dir.path().join("h/d"))).len(), 1);
        // Another size, or another URL, is not this download.
        std::fs::write(&path, b"abcd").unwrap();
        assert!(a.lookup(url).unwrap().is_none());
        let changed = Archived {
            path: path.clone(),
            sidecar,
        };
        assert_eq!(
            Archive::verify(&changed).unwrap_err().diagnostic().code,
            "cache.sha256_mismatch"
        );
        std::fs::write(Archive::sidecar_path(&path), b"{").unwrap();
        assert!(Archive::read_sidecar(&path).is_err());
        // A sidecar cut short is no sidecar: the file is not archived.
        std::fs::write(Archive::sidecar_path(&path), b"").unwrap();
        assert!(a.lookup(url).unwrap().is_none());
        // A sidecar that cannot be read at all is a fault, not a miss.
        std::fs::remove_file(Archive::sidecar_path(&path)).unwrap();
        std::fs::create_dir(Archive::sidecar_path(&path)).unwrap();
        assert!(a.lookup(url).is_err());
        std::fs::remove_dir(Archive::sidecar_path(&path)).unwrap();
        // A sidecar of an older nemlake, with fields this one does not
        // keep, still reads.
        std::fs::write(
            Archive::sidecar_path(&path),
            br#"{"url":"https://h/d/f.zip","size":4,"sha256":"x","fetched_at":"t","accept_ranges":true,"request":{"by":"x"}}"#,
        )
        .unwrap();
        assert_eq!(Archive::read_sidecar(&path).unwrap().unwrap().size, 4);
    }

    #[test]
    fn odd_paths_and_unreadable_places() {
        let dir = tempfile::tempdir().unwrap();
        let a = Archive::new(dir.path());
        assert_eq!(a.path_for("https://"), dir.path().join("_"));
        // A sidecar that is a folder cannot be read.
        let path = dir.path().join("f.zip");
        std::fs::create_dir_all(Archive::sidecar_path(&path)).unwrap();
        let e = Archive::read_sidecar(&path).unwrap_err();
        assert_eq!(e.diagnostic().code, "cache.io");
        // A file is not a folder of entries; nor is a folder a file to hash.
        std::fs::write(dir.path().join("plain"), b"x").unwrap();
        assert!(a.entries_under(Some(&dir.path().join("plain"))).is_empty());
        assert!(sha256_file(dir.path()).is_err());
    }
}
