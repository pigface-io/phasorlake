//! The archive: every downloaded file, kept as the server sent it.
//!
//! A file lives at a path named for its URL
//! (`{archive}/www.nemweb.com.au/Data_Archive/.../PUBLIC_DVD_X_202401010000.zip`,
//! one folder per report folder) beside its provenance record,
//! `{name}.provenance.json`: what its bytes are and every place nemweb
//! served them ([`utilake_provenance`]). A file is downloaded once; nothing
//! in utilake-mirror replaces an archived file, which is the untransformed
//! record the census and the lake are derived from. Only its record grows.
//! The store itself is the family's ([`Archive`]); this module adds what
//! the mirror does with it.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use utilake_provenance::archive::{Archive, Held, is_noise, is_record};
pub use utilake_provenance::{Address, Content, Provenance, Served, Sha256, Via};

use crate::net::error::FetchError;
use crate::stamp::Stamp;

/// Hands a file's writes to the disk with `fsync`. On macOS
/// `File::sync_all` and `sync_data` also flush the drive's own cache
/// (`F_FULLFSYNC`), which on a USB disk costs most of a second per small
/// file. What that flush adds guards only against a power cut, after which
/// a file's bytes may not match the sha256 in its record.
pub fn to_disk(file: &std::fs::File) -> std::io::Result<()> {
    rustix::fs::fsync(file).map_err(std::io::Error::from)
}

/// The names each directory holds, read once per directory. On exFAT a
/// lookup of a name that is not there scans the whole directory, so one
/// read of a large folder answers every miss in it.
#[derive(Debug, Default)]
pub struct DirNames {
    dirs: HashMap<PathBuf, HashSet<OsString>>,
}

impl DirNames {
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

impl From<utilake_provenance::archive::Error> for FetchError {
    fn from(e: utilake_provenance::archive::Error) -> Self {
        use utilake_provenance::archive::Error;
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

/// The content of the file at `path`, from one read of it in 1 MiB
/// pieces. Blocking: call it off the async runtime.
pub fn content_of(path: &Path) -> Result<Content, FetchError> {
    Content::of_file(path).map_err(|source| FetchError::CacheIo {
        path: path.display().to_string(),
        source,
    })
}

/// Whether `path` names a macOS AppleDouble file (`._name`), which the
/// system writes beside a file on a volume without extended attributes.
/// It is not a download: every walk of the archive skips it.
pub fn is_apple_double(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("._"))
}

/// Every version of `url` archived: the files at `path` and each
/// `{name}@{at}` beside it whose record names `url`, in name order, so in
/// the order nemweb listed them.
pub fn versions(path: &Path, url: &str) -> Result<Vec<Held>, FetchError> {
    Ok(Archive::held_at(path)?
        .into_iter()
        .filter(|h| h.provenance.names(url))
        .collect())
}

/// The file at `path` itself (not a version beside it) with its record,
/// when it is archived from `url`.
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

/// Adds `address` to the record of the file at `path`, when its URL is
/// not an address of it already. Whether the record changed.
pub fn add_address(path: &Path, address: Address) -> Result<bool, FetchError> {
    let Some(mut p) = Archive::read(path)? else {
        return Ok(false);
    };
    if !p.add(address) {
        return Ok(false);
    }
    Archive::write(path, &p)?;
    Ok(true)
}

/// Notes, in every record under the folder `folder` (a URL ending in
/// `/`), that each address under it is gone. How many records changed.
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

/// Removes an archived file and its record, whichever name the record has
/// (`{name}.provenance.json`, or format 1's `{name}.json`).
pub fn remove(path: &Path) -> Result<(), FetchError> {
    let io = |p: &Path| {
        let p = p.display().to_string();
        move |source| FetchError::CacheIo { path: p, source }
    };
    let mut old = path.as_os_str().to_owned();
    old.push(utilake_provenance::archive::OLD_RECORD);
    for record in [Archive::record_path(path), PathBuf::from(old)] {
        match std::fs::remove_file(&record) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&record)(e)),
            _ => {}
        }
    }
    std::fs::remove_file(path).map_err(io(path))
}

/// Every archived file under `dir` (the archive itself by default), with
/// its record, in path order. A folder that cannot be read, or a record
/// that does not read, is passed over.
pub fn entries_under(archive: &Archive, dir: Option<&Path>) -> Vec<Held> {
    let dir = dir.unwrap_or(archive.root());
    let Ok(paths) = Archive::walk(dir) else {
        return Vec::new();
    };
    paths
        .into_iter()
        .filter_map(|path| {
            let provenance = Archive::read(&path).ok().flatten()?;
            Some(Held { path, provenance })
        })
        .collect()
}

/// A record of `bytes` fetched from `url`, as the tests write one: no
/// listing, no response, no CRC-32 yet.
#[cfg(test)]
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
