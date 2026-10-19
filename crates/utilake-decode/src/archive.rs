//! Walking a zip, including zips inside zips, and handing each CSV entry to
//! a callback as a streaming reader.

use std::fs::File;
use std::io::{BufReader, Read, Seek, Write as _};
use std::path::Path;

use utilake_core::{Category, Diagnose, Diagnostic};

/// Zips nested deeper than this are refused.
pub const MAX_DEPTH: u8 = 2;

/// Why an archive could not be walked.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The file could not be opened as a zip.
    #[error("The archive {path} could not be opened: {message}")]
    Open {
        /// The file.
        path: String,
        /// Cause.
        message: String,
    },
    /// An entry could not be read.
    #[error("Entry {entry} of {path} could not be read: {message}")]
    Entry {
        /// The file.
        path: String,
        /// The entry.
        entry: String,
        /// Cause.
        message: String,
    },
    /// Nested too deep.
    #[error("Entry {entry} of {path} is a zip nested {depth} deep; the limit is {MAX_DEPTH}.")]
    TooDeep {
        /// The file.
        path: String,
        /// The entry.
        entry: String,
        /// Depth reached.
        depth: u8,
    },
    /// No CSV at any depth.
    #[error("The archive {path} has no .CSV entry at any depth.")]
    NoCsv {
        /// The file.
        path: String,
    },
    /// Spooling a nested zip to disk failed.
    #[error("Spooling nested entry {entry} of {path} to {tmp} failed.")]
    Spool {
        /// The file.
        path: String,
        /// The entry.
        entry: String,
        /// Temp path.
        tmp: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

impl Diagnose for ArchiveError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Open { path, .. } => {
                Diagnostic::error("zip.open_failed", Category::Zip, msg).with_file(path.clone())
            }
            Self::Entry { path, entry, .. } => {
                Diagnostic::error("zip.bad_entry", Category::Zip, msg)
                    .with_file(path.clone())
                    .with_entry(entry.clone())
            }
            Self::TooDeep { path, entry, .. } => {
                Diagnostic::error("zip.nesting_too_deep", Category::Zip, msg)
                    .with_file(path.clone())
                    .with_entry(entry.clone())
            }
            Self::NoCsv { path } => {
                Diagnostic::error("zip.no_csv", Category::Zip, msg).with_file(path.clone())
            }
            Self::Spool {
                path,
                entry,
                source,
                ..
            } => Diagnostic::error("zip.spool_failed", Category::Zip, msg)
                .with_file(path.clone())
                .with_entry(entry.clone())
                .caused_by(source),
        }
    }
}

/// What the walker found at one entry.
pub struct Entry<'a> {
    /// Entry path, nested ones joined with `!` (`inner.zip!FILE.CSV`).
    pub name: String,
    /// Uncompressed size when the zip records it.
    pub size: Option<u64>,
    /// The streaming reader of the entry's bytes.
    pub reader: &'a mut dyn Read,
}

/// Walks every `.CSV` entry of the zip at `path`, nested zips included, in
/// archive order. `on_entry` may return an error to stop the walk.
///
/// A nested zip is walked in memory up to 64 MiB, else from a temporary
/// file in `tmp_dir`, removed once read: the zip format needs a seekable
/// central directory. Every other
/// entry (a control file, a README) goes to `on_other` with its reader.
pub fn walk_csv_entries<E>(
    path: &Path,
    tmp_dir: &Path,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    walk(path, tmp_dir, NESTED_IN_MEMORY, 0, "", on_entry, on_other)
}

/// Either the archive failed or the callback did.
#[derive(Debug, thiserror::Error)]
pub enum WalkError<E> {
    /// The archive.
    #[error(transparent)]
    Archive(ArchiveError),
    /// The callback.
    #[error("the entry callback failed")]
    Callback(E),
}

/// Read buffer of a zip on disk. The zip reader asks for its compressed
/// bytes 8 KiB at a time; this lets each system call bring many of them.
const ZIP_READ_BUFFER: usize = 64 << 10;
/// A nested zip up to this size is read into memory and walked there;
/// a larger one goes to a temporary file.
const NESTED_IN_MEMORY: u64 = 64 << 20;
/// Write buffer of a nested zip spooled to disk.
const SPOOL_BUFFER: usize = 1 << 20;

fn walk<E>(
    path: &Path,
    tmp_dir: &Path,
    in_memory: u64,
    depth: u8,
    prefix: &str,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    let p = path.display().to_string();
    let open = |message: String| {
        WalkError::Archive(ArchiveError::Open {
            path: p.clone(),
            message,
        })
    };
    let file = File::open(path).map_err(|e| open(e.to_string()))?;
    let archive = zip::ZipArchive::new(BufReader::with_capacity(ZIP_READ_BUFFER, file))
        .map_err(|e| open(e.to_string()))?;
    walk_zip(
        archive, &p, tmp_dir, in_memory, depth, prefix, on_entry, on_other,
    )
}

#[allow(clippy::too_many_arguments)]
fn walk_zip<R: Read + Seek, E>(
    mut archive: zip::ZipArchive<R>,
    p: &str,
    tmp_dir: &Path,
    in_memory: u64,
    depth: u8,
    prefix: &str,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    let mut found = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| {
            WalkError::Archive(ArchiveError::Entry {
                path: p.to_owned(),
                entry: i.to_string(),
                message: e.to_string(),
            })
        })?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name();
        let full = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}!{name}")
        };
        let is = |ext: &str| {
            name.len() >= ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
        };
        if is(".csv") {
            let size = Some(entry.size());
            let mut reader = Counting {
                inner: &mut entry,
                path: p,
                entry: &full,
                failed: None,
            };
            let result = on_entry(Entry {
                name: full.clone(),
                size,
                reader: &mut reader,
            });
            if let Some(e) = reader.failed.take() {
                return Err(WalkError::Archive(e));
            }
            result.map_err(WalkError::Callback)?;
            found += 1;
        } else if is(".zip") {
            if depth + 1 > MAX_DEPTH {
                return Err(WalkError::Archive(ArchiveError::TooDeep {
                    path: p.to_owned(),
                    entry: full,
                    depth: depth + 1,
                }));
            }
            let size = entry.size();
            let inner_label = format!("{p}!{name}");
            let bad_inner = |message: String| {
                WalkError::Archive(ArchiveError::Entry {
                    path: p.to_owned(),
                    entry: full.clone(),
                    message,
                })
            };
            if size <= in_memory {
                // The zip format needs to seek to its central directory;
                // a small inner zip is walked in memory, never on disk.
                let mut bytes = Vec::with_capacity(size as usize);
                entry
                    .read_to_end(&mut bytes)
                    .map_err(|e| bad_inner(e.to_string()))?;
                drop(entry);
                let inner = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                    .map_err(|e| bad_inner(e.to_string()))?;
                found += walk_zip(
                    inner,
                    &inner_label,
                    tmp_dir,
                    in_memory,
                    depth + 1,
                    &full,
                    on_entry,
                    on_other,
                )?;
            } else {
                let spool_err = |tmp: &Path, source: std::io::Error| {
                    WalkError::Archive(ArchiveError::Spool {
                        path: p.to_owned(),
                        entry: full.clone(),
                        tmp: tmp.display().to_string(),
                        source,
                    })
                };
                std::fs::create_dir_all(tmp_dir).map_err(|e| spool_err(tmp_dir, e))?;
                // Unique, and removed when dropped, whatever happens.
                let tmp =
                    tempfile::NamedTempFile::new_in(tmp_dir).map_err(|e| spool_err(tmp_dir, e))?;
                {
                    let mut out = std::io::BufWriter::with_capacity(SPOOL_BUFFER, tmp.as_file());
                    std::io::copy(&mut entry, &mut out)
                        .and_then(|_| out.flush())
                        .map_err(|e| spool_err(tmp.path(), e))?;
                }
                drop(entry);
                let file = tmp.reopen().map_err(|e| spool_err(tmp.path(), e))?;
                let inner = zip::ZipArchive::new(BufReader::with_capacity(ZIP_READ_BUFFER, file))
                    .map_err(|e| bad_inner(e.to_string()))?;
                found += walk_zip(
                    inner,
                    &inner_label,
                    tmp_dir,
                    in_memory,
                    depth + 1,
                    &full,
                    on_entry,
                    on_other,
                )?;
            }
        } else {
            on_other(Entry {
                name: full,
                size: Some(entry.size()),
                reader: &mut entry,
            });
        }
    }
    if depth == 0 && found == 0 {
        return Err(WalkError::Archive(ArchiveError::NoCsv {
            path: p.to_owned(),
        }));
    }
    Ok(found)
}

/// Wraps the entry reader so an inflate error is reported as the archive's,
/// with the entry name, rather than as a plain I/O error of the scanner.
struct Counting<'a> {
    inner: &'a mut dyn Read,
    path: &'a str,
    entry: &'a str,
    failed: Option<ArchiveError>,
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.read(buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                self.failed = Some(ArchiveError::Entry {
                    path: self.path.to_owned(),
                    entry: self.entry.to_owned(),
                    message: e.to_string(),
                });
                Err(e)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    /// Builds a zip in `dir` with the given (name, bytes) entries.
    pub(crate) fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        for (name, bytes) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/csv/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    type Collected = (Vec<(String, Vec<u8>)>, Vec<String>);

    fn collect(path: &Path, tmp: &Path) -> Result<Collected, WalkError<std::io::Error>> {
        collect_with(path, tmp, NESTED_IN_MEMORY)
    }

    /// As [`collect`], with nested zips above `in_memory` bytes spooled.
    fn collect_with(
        path: &Path,
        tmp: &Path,
        in_memory: u64,
    ) -> Result<Collected, WalkError<std::io::Error>> {
        let mut got = Vec::new();
        let mut others = Vec::new();
        walk(
            path,
            tmp,
            in_memory,
            0,
            "",
            &mut |e| {
                let mut buf = Vec::new();
                e.reader.read_to_end(&mut buf)?;
                got.push((e.name, buf));
                Ok(())
            },
            &mut |o| others.push(o.name),
        )?;
        Ok((got, others))
    }

    #[test]
    fn walks_flat_nested_and_multi_entry_zips() {
        let dir = tempfile::tempdir().unwrap();
        let region = fixture("REGION_v1_head.csv");
        let inter = fixture("INTERCONNECTOR_v1_head.csv");
        let inner = dir.path().join("inner.zip");
        make_zip(&inner, &[("INNER.CSV", &inter), ("notes.txt", b"x")]);
        let inner_bytes = std::fs::read(&inner).unwrap();
        let outer = dir.path().join("outer.zip");
        make_zip(
            &outer,
            &[
                ("A.CSV", &region),
                ("dir/", b""),
                ("nested.zip", &inner_bytes),
                ("B.csv", &region),
                ("readme.md", b"hi"),
            ],
        );
        let tmp = dir.path().join("tmp");
        let (got, others) = collect(&outer, &tmp).unwrap();
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["A.CSV", "nested.zip!INNER.CSV", "B.csv"]);
        assert_eq!(got[1].1, inter);
        assert_eq!(others, ["nested.zip!notes.txt", "readme.md"]);
        assert!(!tmp.exists(), "a small nested zip is read in memory");
        // A big one goes through a temporary file, removed once read.
        let spooled = collect_with(&outer, &tmp, 0).unwrap();
        assert_eq!(spooled.0, got);
        assert!(
            std::fs::read_dir(&tmp).unwrap().next().is_none(),
            "spooled zip removed"
        );
    }

    #[test]
    fn errors() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let e = collect(&dir.path().join("none.zip"), &tmp).unwrap_err();
        assert!(
            matches!(e, WalkError::Archive(ArchiveError::Open { .. })),
            "{e}"
        );
        let bad = dir.path().join("bad.zip");
        std::fs::write(&bad, b"not a zip").unwrap();
        assert!(matches!(
            collect(&bad, &tmp).unwrap_err(),
            WalkError::Archive(ArchiveError::Open { .. })
        ));
        let empty = dir.path().join("empty.zip");
        make_zip(&empty, &[("a.txt", b"x")]);
        let e = collect(&empty, &tmp).unwrap_err();
        assert!(
            matches!(e, WalkError::Archive(ArchiveError::NoCsv { .. })),
            "{e}"
        );
        assert_eq!(
            utilake_core::Diagnose::diagnostic(&ArchiveError::NoCsv { path: "p".into() }).code,
            "zip.no_csv"
        );
        // Depth: zip in zip in zip.
        let l2 = dir.path().join("l2.zip");
        make_zip(&l2, &[("X.CSV", b"C,x\n")]);
        let l1 = dir.path().join("l1.zip");
        make_zip(&l1, &[("l2.zip", &std::fs::read(&l2).unwrap())]);
        let l0 = dir.path().join("l0.zip");
        make_zip(&l0, &[("l1.zip", &std::fs::read(&l1).unwrap())]);
        let (got, _) = collect(&l1, &tmp).unwrap();
        assert_eq!(got[0].0, "l2.zip!X.CSV");
        let (got, _) = collect(&l0, &tmp).unwrap();
        assert_eq!(
            got[0].0, "l1.zip!l2.zip!X.CSV",
            "two levels of nesting are allowed"
        );
        let l_minus = dir.path().join("lm.zip");
        make_zip(&l_minus, &[("l0.zip", &std::fs::read(&l0).unwrap())]);
        let e = collect(&l_minus, &tmp).unwrap_err();
        assert!(
            matches!(
                e,
                WalkError::Archive(ArchiveError::TooDeep { depth: 3, .. })
            ),
            "{e}"
        );
        let d = utilake_core::Diagnose::diagnostic(&ArchiveError::TooDeep {
            path: "p".into(),
            entry: "e".into(),
            depth: 3,
        });
        assert_eq!(d.code, "zip.nesting_too_deep");
        assert_eq!(d.entry.as_deref(), Some("e"));
        // Callback error propagates.
        let one = dir.path().join("one.zip");
        make_zip(&one, &[("A.CSV", b"C,x\n")]);
        let r: Result<usize, WalkError<&str>> =
            walk_csv_entries(&one, &tmp, &mut |_| Err("stop"), &mut |_| {});
        assert!(matches!(r, Err(WalkError::Callback("stop"))));
        // Spool failure: tmp dir is a file.
        let tmp_file = dir.path().join("tmpfile");
        std::fs::write(&tmp_file, b"").unwrap();
        let e = collect_with(&l1, &tmp_file, 0).unwrap_err();
        assert!(
            matches!(e, WalkError::Archive(ArchiveError::Spool { .. })),
            "{e}"
        );
        // A nested entry that is not a zip.
        let fake = dir.path().join("fake.zip");
        make_zip(&fake, &[("inner.zip", b"not a zip")]);
        assert!(matches!(
            collect(&fake, &tmp).unwrap_err(),
            WalkError::Archive(ArchiveError::Entry { .. })
        ));
        assert!(matches!(
            collect_with(&fake, &tmp, 0).unwrap_err(),
            WalkError::Archive(ArchiveError::Entry { .. })
        ));
        assert_eq!(
            utilake_core::Diagnose::diagnostic(&ArchiveError::Entry {
                path: "p".into(),
                entry: "e".into(),
                message: "m".into()
            })
            .code,
            "zip.bad_entry"
        );
        assert_eq!(
            utilake_core::Diagnose::diagnostic(&ArchiveError::Open {
                path: "p".into(),
                message: "m".into()
            })
            .code,
            "zip.open_failed"
        );
        let e = ArchiveError::Spool {
            path: "p".into(),
            entry: "e".into(),
            tmp: "t".into(),
            source: std::io::Error::other("x"),
        };
        assert_eq!(
            utilake_core::Diagnose::diagnostic(&e).code,
            "zip.spool_failed"
        );
    }

    #[test]
    fn corrupt_entry_is_reported_as_the_archives() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("c.zip");
        // Incompressible data so the deflate stream is long enough to damage.
        let mut state = 0x9E37_79B9_u32;
        let noise: Vec<u8> = (0..100_000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        make_zip(&z, &[("A.CSV", &noise)]);
        // Flip bytes in the middle of the deflate stream.
        let mut bytes = std::fs::read(&z).unwrap();
        for b in &mut bytes[5_000..5_100] {
            *b ^= 0xFF;
        }
        std::fs::write(&z, &bytes).unwrap();
        let e = collect(&z, &dir.path().join("tmp")).unwrap_err();
        assert!(
            matches!(e, WalkError::Archive(ArchiveError::Entry { .. })),
            "{e}"
        );
    }
}
