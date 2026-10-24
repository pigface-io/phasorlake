//! The archive: every downloaded file, kept as the server sent it.
//!
//! A file lives at a path named for its URL
//! (`{archive}/www.nemweb.com.au/Data_Archive/.../PUBLIC_DVD_X_202401010000.zip`)
//! beside its provenance record, `{name}.provenance.json`: what its bytes
//! are and every place they were served ([`utilake_provenance`]). A file is
//! downloaded once; nothing in utilake replaces or deletes an archived
//! file, which is the untransformed record the census and the lake are
//! derived from. To fetch one again, delete it. The store is the family's
//! ([`Archive`]); this module adds the lake's hashing and errors.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256 as Hasher};
pub use utilake_provenance::archive::{Archive, Held, is_noise, is_record};
pub use utilake_provenance::{Address, Content, Listed, Provenance, Served, Sha256, Span, Via};

use crate::error::FetchError;

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

/// sha256 hex and length of the file at `path`, read in 1 MiB pieces so a
/// file of any size takes the same memory. Blocking: call it off the
/// async runtime.
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

/// Hashes an archived file and holds it to its record.
pub fn verify(held: &Held) -> Result<(), FetchError> {
    let (observed, _) = sha256_file(&held.path).map_err(|source| FetchError::CacheIo {
        path: held.path.display().to_string(),
        source,
    })?;
    let expected = held.provenance.content.sha256.hex();
    if observed != expected {
        return Err(FetchError::Sha256Mismatch {
            path: held.path.display().to_string(),
            expected,
            observed,
        });
    }
    Ok(())
}

/// Adds `address` to the record of the file at `path`, when its URL is
/// not an address of it already. Whether the record changed; a file with
/// no record changes nothing.
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

/// Every archived file under `dir` (the archive itself by default), with
/// its record, in path order. A folder that cannot be read, or a record
/// that does not read, is passed over.
pub fn entries_under(archive: &Archive, dir: Option<&Path>) -> Vec<Held> {
    let Ok(paths) = Archive::walk(dir.unwrap_or(archive.root())) else {
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

#[cfg(test)]
mod tests;
