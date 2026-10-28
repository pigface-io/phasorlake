//! The archive: every downloaded file kept as the server sent it, at a path named
//! for its URL beside its `{name}.provenance.json` record. An archived file is never
//! replaced; only its record grows. This module adds hashing, checks and record edits.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256 as Hasher};

pub use phasor_provenance::archive::{Archive, Held, is_noise, is_record};
pub use phasor_provenance::{Address, Content, Listed, Provenance, Served, Sha256, Span, Via};

use crate::error::FetchError;
use phasor_provenance::stamp::Stamp;

/// Syncs a file's writes with `fsync`, not macOS `F_FULLFSYNC`, which costs most of a
/// second per small file on a USB disk and guards only against a power cut.
pub fn to_disk(file: &std::fs::File) -> std::io::Result<()> {
    rustix::fs::fsync(file).map_err(std::io::Error::from)
}

/// Syncs a batch of files written unsynced, then each folder holding one, so their names last too.
pub fn batch_to_disk<'p>(
    paths: impl IntoIterator<Item = &'p Path>,
) -> Result<(), (PathBuf, std::io::Error)> {
    let sync = |p: &Path| match std::fs::File::open(p) {
        Ok(f) => to_disk(&f).map_err(|e| (p.to_path_buf(), e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err((p.to_path_buf(), e)),
    };
    let mut folders = std::collections::BTreeSet::new();
    for p in paths {
        sync(p)?;
        folders.extend(p.parent());
    }
    folders.into_iter().try_for_each(sync)
}

/// The names each directory holds, read once per directory.
#[derive(Debug, Default)]
pub struct DirNames {
    dirs: HashMap<PathBuf, HashSet<OsString>>,
}

impl DirNames {
    /// Whether `path`'s directory holds its name; an unreadable directory holds nothing.
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

impl From<phasor_provenance::archive::Error> for FetchError {
    fn from(e: phasor_provenance::archive::Error) -> Self {
        use phasor_provenance::archive::Error;
        match e {
            Error::Io { path, source } => Self::CacheIo { path, source },
            Error::Record { path, source } => Self::SidecarCorrupt {
                path,
                message: source.to_string(),
            },
        }
    }
}

/// sha256 hex of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::of(bytes).hex()
}

/// sha256 hex and length of the file at `path`, read in 1 MiB pieces. Blocking.
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    sha256_read(&mut std::fs::File::open(path)?)
}

/// sha256 hex and length of everything `f` reads, in 1 MiB pieces.
pub fn sha256_read(f: &mut impl Read) -> std::io::Result<(String, u64)> {
    let mut hasher = Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut len = 0u64;
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    Ok((Sha256(hasher.finalize().into()).hex(), len))
}

/// The content of the file at `path`, from one read. Blocking.
pub fn content_of(path: &Path) -> Result<Content, FetchError> {
    Content::of_file(path).map_err(|source| FetchError::CacheIo {
        path: path.display().to_string(),
        source,
    })
}

/// Whether `path` names a macOS AppleDouble file (`._name`), which every walk skips.
pub fn is_apple_double(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("._"))
}

/// Every version of `url` archived (the file at `path` and each `{name}@{at}`), in name order.
pub fn versions(path: &Path, url: &str) -> Result<Vec<Held>, FetchError> {
    Ok(Archive::held_at(path)?
        .into_iter()
        .filter(|h| h.provenance.names(url))
        .collect())
}

/// The file at `path` itself, not a version beside it, when archived from `url`.
pub fn lookup_at(path: &Path, url: &str) -> Result<Option<Held>, FetchError> {
    Ok(Archive::held_at(path)?
        .into_iter()
        .next()
        .filter(|h| h.path == path && h.provenance.names(url)))
}

/// Hashes an archived file and holds it to its record.
pub fn verify(held: &Held) -> Result<(), FetchError> {
    let observed = content_of(&held.path)?;
    if observed.sha256 != held.provenance.content.sha256 {
        return Err(FetchError::Sha256Mismatch {
            path: held.path.display().to_string(),
            expected: held.provenance.content.sha256.hex(),
            observed: observed.sha256.hex(),
        });
    }
    Ok(())
}

/// Adds `address` to the record at `path` unless present; whether it changed.
/// A record written not `durable` is left for the caller to sync.
pub fn add_address(path: &Path, address: Address, durable: bool) -> Result<bool, FetchError> {
    let Some(mut p) = Archive::read(path)? else {
        return Ok(false);
    };
    if !p.add(address) {
        return Ok(false);
    }
    if durable {
        Archive::write(path, &p)?;
    } else {
        Archive::write_unsynced(path, &p)?;
    }
    Ok(true)
}

/// Marks every address under `folder` (a URL ending in `/`) gone; how many records changed.
pub fn folder_gone(archive: &Archive, folder: &str, at: Stamp) -> Result<usize, FetchError> {
    let mut n = 0;
    for h in entries_under(archive, Some(&archive.path_for(folder))) {
        let gone: Vec<&str> = h
            .provenance
            .addresses
            .iter()
            .map(|a| a.url.as_str())
            .filter(|u| u.starts_with(folder))
            .collect();
        if Archive::gone(&h.path, gone, at)? {
            n += 1;
        }
    }
    Ok(n)
}

/// Removes an archived file and its record (`{name}.provenance.json`, or format 1's `{name}.json`).
pub fn remove(path: &Path) -> Result<(), FetchError> {
    let io = |p: &Path| {
        let p = p.display().to_string();
        move |source| FetchError::CacheIo { path: p, source }
    };
    let mut old = path.as_os_str().to_owned();
    old.push(phasor_provenance::archive::OLD_RECORD);
    for record in [Archive::record_path(path), PathBuf::from(old)] {
        match std::fs::remove_file(&record) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&record)(e)),
            _ => {}
        }
    }
    std::fs::remove_file(path).map_err(io(path))
}

/// Every archived file under `dir` (default the archive) with its record, in path order;
/// unreadable folders and records are passed over.
pub fn entries_under(archive: &Archive, dir: Option<&Path>) -> Vec<Held> {
    let dir = dir.unwrap_or(archive.root());
    let Ok(paths) = Archive::walk_records(dir) else {
        return Vec::new();
    };
    paths
        .into_iter()
        .filter_map(|(path, record)| {
            let provenance = Archive::read_record(&record).ok()?;
            Some(Held { path, provenance })
        })
        .collect()
}

/// A test record of `bytes` fetched from `url`: no listing, no response, no CRC-32.
#[cfg(any(test, feature = "test-support"))]
pub fn test_record(url: &str, bytes: &[u8]) -> Provenance {
    Provenance::new(
        Content {
            crc32: None,
            ..Content::of(bytes)
        },
        "test",
        Address {
            url: url.into(),
            seen: Stamp::from_unix(1_800_000_000),
            gone: None,
            via: Via::Fetched {
                listed: None,
                served: None,
            },
        },
    )
}

#[cfg(test)]
mod tests;
