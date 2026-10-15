//! A CRC-32 for every downloaded file: files downloaded before downloads
//! recorded one are read once, their sha256 checked against their sidecar
//! on the way, and the CRC-32 recorded in the sidecar.

use serde::Serialize;

use crate::diag::{Diagnose as _, Diagnostic};
use crate::error::MirrorError;
use crate::net::{Archive, FetchError, hashes_of};

/// One file read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileHashed<'a> {
    /// The file's URL.
    pub url: &'a str,
    /// Whether it is still the bytes its sidecar records.
    pub intact: bool,
    /// Files read so far.
    pub done: usize,
    /// Files to read.
    pub total: usize,
}

/// What a pass did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HashSummary {
    /// Downloaded files.
    pub files: usize,
    /// Of those, read in this pass.
    pub read: usize,
    /// Of those read, CRC-32s recorded.
    pub recorded: usize,
    /// Files that are not the bytes their sidecar records, or could not be
    /// read; their sidecars are left as they are.
    pub failed: Vec<Diagnostic>,
}

/// Reads every downloaded file whose sidecar has no CRC-32 (with `all`,
/// every downloaded file), checks its sha256 and size against the sidecar,
/// and records its CRC-32 there. One file at a time: they share one disk.
pub fn hash(
    archive: &Archive,
    all: bool,
    mut on_file: impl FnMut(&FileHashed<'_>),
) -> Result<HashSummary, MirrorError> {
    let entries = archive.entries_under(None);
    let todo: Vec<_> = entries
        .iter()
        .filter(|a| all || a.sidecar.crc32.is_none())
        .collect();
    let mut summary = HashSummary {
        files: entries.len(),
        read: todo.len(),
        ..HashSummary::default()
    };
    for (i, a) in todo.iter().enumerate() {
        let intact = match hashes_of(&a.path) {
            Ok(h) if h.sha256 == a.sidecar.sha256 && h.size == a.sidecar.size => {
                if a.sidecar.crc32 != Some(h.crc32) {
                    let mut sidecar = a.sidecar.clone();
                    sidecar.crc32 = Some(h.crc32);
                    Archive::write_sidecar(&a.path, &sidecar)?;
                    summary.recorded += 1;
                }
                true
            }
            Ok(h) => {
                summary.failed.push(
                    FetchError::Sha256Mismatch {
                        path: a.path.display().to_string(),
                        expected: a.sidecar.sha256.clone(),
                        observed: h.sha256,
                    }
                    .diagnostic(),
                );
                false
            }
            Err(e) => {
                summary
                    .failed
                    .push(MirrorError::io(&a.path)(e).diagnostic());
                false
            }
        };
        on_file(&FileHashed {
            url: &a.sidecar.url,
            intact,
            done: i + 1,
            total: todo.len(),
        });
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Sidecar;
    use crate::net::archive::sha256_hex;

    fn archived(
        archive: &Archive,
        url: &str,
        bytes: &[u8],
        crc32: Option<u32>,
    ) -> std::path::PathBuf {
        let p = archive.path_for(url);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        Archive::write_sidecar(
            &p,
            &Sidecar {
                url: url.into(),
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
                crc32,
                fetched_at: "t".into(),
                etag: None,
                last_modified: None,
                listed_size: None,
                listed_modified: None,
                tool: "x".into(),
            },
        )
        .unwrap();
        p
    }

    #[test]
    fn records_the_missing_crcs_and_checks_every_file_read() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let a = archived(&archive, "https://h/a", b"aaa", None);
        archived(
            &archive,
            "https://h/b",
            b"bbb",
            Some(crc32fast::hash(b"bbb")),
        );
        let c = archived(&archive, "https://h/c", b"ccc", None);
        std::fs::write(&c, b"CCC").unwrap();
        let mut seen = Vec::new();
        let s = hash(&archive, false, |f| seen.push((f.url.to_owned(), f.intact))).unwrap();
        assert_eq!((s.files, s.read, s.recorded), (3, 2, 1));
        assert_eq!(s.failed.len(), 1);
        assert_eq!(s.failed[0].code, "cache.sha256_mismatch");
        assert_eq!(
            seen,
            [("https://h/a".into(), true), ("https://h/c".into(), false)]
        );
        let sc = Archive::read_sidecar(&a).unwrap().unwrap();
        assert_eq!(sc.crc32, Some(crc32fast::hash(b"aaa")));
        assert!(Archive::read_sidecar(&c).unwrap().unwrap().crc32.is_none());

        // Everything, again: what holds is not written twice.
        let s = hash(&archive, true, |_| {}).unwrap();
        assert_eq!((s.read, s.recorded, s.failed.len()), (3, 0, 1));
        // A file that cannot be read.
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&c, std::fs::Permissions::from_mode(0o000)).unwrap();
        let s = hash(&archive, false, |_| {}).unwrap();
        assert_eq!(s.failed[0].code, "mirror.io");
    }
}
