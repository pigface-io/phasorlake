//! Walking a zip, including zips inside zips, and handing each CSV entry to
//! a callback as a streaming reader.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use nemlake_core::{Category, Diagnose, Diagnostic};

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
/// Nested zips are spooled to `tmp_dir` because the zip format needs a
/// seekable central directory, and are deleted afterwards. Every other
/// entry (a control file, a README) goes to `on_other` with its reader.
pub fn walk_csv_entries<E>(
    path: &Path,
    tmp_dir: &Path,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    walk(path, tmp_dir, 0, "", on_entry, on_other)
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

fn walk<E>(
    path: &Path,
    tmp_dir: &Path,
    depth: u8,
    prefix: &str,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    let p = path.display().to_string();
    let file = File::open(path).map_err(|e| {
        WalkError::Archive(ArchiveError::Open {
            path: p.clone(),
            message: e.to_string(),
        })
    })?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| {
        WalkError::Archive(ArchiveError::Open {
            path: p.clone(),
            message: e.to_string(),
        })
    })?;
    let mut found = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| {
            WalkError::Archive(ArchiveError::Entry {
                path: p.clone(),
                entry: i.to_string(),
                message: e.to_string(),
            })
        })?;
        let name = entry.name().to_owned();
        let full = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}!{name}")
        };
        if entry.is_dir() {
            continue;
        }
        let upper = name.to_ascii_uppercase();
        if upper.ends_with(".CSV") {
            let size = Some(entry.size());
            let mut reader = Counting {
                inner: &mut entry,
                path: &p,
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
        } else if upper.ends_with(".ZIP") {
            if depth + 1 > MAX_DEPTH {
                return Err(WalkError::Archive(ArchiveError::TooDeep {
                    path: p,
                    entry: full,
                    depth: depth + 1,
                }));
            }
            std::fs::create_dir_all(tmp_dir).map_err(|source| {
                WalkError::Archive(ArchiveError::Spool {
                    path: p.clone(),
                    entry: full.clone(),
                    tmp: tmp_dir.display().to_string(),
                    source,
                })
            })?;
            let tmp: PathBuf = tmp_dir.join(format!(
                "{}-{}-{}.zip",
                std::process::id(),
                i,
                name.replace(['/', '\\'], "_")
            ));
            {
                let mut out = File::create(&tmp).map_err(|source| {
                    WalkError::Archive(ArchiveError::Spool {
                        path: p.clone(),
                        entry: full.clone(),
                        tmp: tmp.display().to_string(),
                        source,
                    })
                })?;
                std::io::copy(&mut entry, &mut out).map_err(|source| {
                    WalkError::Archive(ArchiveError::Spool {
                        path: p.clone(),
                        entry: full.clone(),
                        tmp: tmp.display().to_string(),
                        source,
                    })
                })?;
            }
            let inner = walk(&tmp, tmp_dir, depth + 1, &full, on_entry, on_other);
            let _ = std::fs::remove_file(&tmp);
            found += inner?;
        } else {
            on_other(Entry {
                name: full,
                size: Some(entry.size()),
                reader: &mut entry,
            });
        }
    }
    if depth == 0 && found == 0 {
        return Err(WalkError::Archive(ArchiveError::NoCsv { path: p }));
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
        let mut got = Vec::new();
        let mut others = Vec::new();
        walk_csv_entries(
            path,
            tmp,
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
        let (got, others) = collect(&outer, &dir.path().join("tmp")).unwrap();
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["A.CSV", "nested.zip!INNER.CSV", "B.csv"]);
        assert_eq!(got[1].1, inter);
        assert_eq!(others, ["nested.zip!notes.txt", "readme.md"]);
        assert!(
            std::fs::read_dir(dir.path().join("tmp"))
                .unwrap()
                .next()
                .is_none(),
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
            nemlake_core::Diagnose::diagnostic(&ArchiveError::NoCsv { path: "p".into() }).code,
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
        let d = nemlake_core::Diagnose::diagnostic(&ArchiveError::TooDeep {
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
        let e = collect(&l1, &tmp_file).unwrap_err();
        assert!(
            matches!(e, WalkError::Archive(ArchiveError::Spool { .. })),
            "{e}"
        );
        assert_eq!(
            nemlake_core::Diagnose::diagnostic(&ArchiveError::Entry {
                path: "p".into(),
                entry: "e".into(),
                message: "m".into()
            })
            .code,
            "zip.bad_entry"
        );
        assert_eq!(
            nemlake_core::Diagnose::diagnostic(&ArchiveError::Open {
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
            nemlake_core::Diagnose::diagnostic(&e).code,
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
