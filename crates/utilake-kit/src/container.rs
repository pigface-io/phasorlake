//! Walking a zip, including zips inside zips, and handing each entry a
//! decoder accepts to a callback as a streaming reader.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write as _};
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
    /// No entry a decoder accepts, at any depth.
    #[error("The archive {path} has no {wanted} entry at any depth.")]
    NoCsv {
        /// The file.
        path: String,
        /// What was looked for, e.g. `.CSV`.
        wanted: String,
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
            Self::NoCsv { path, .. } => {
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

/// Which entries a walk hands to `on_entry`: those whose name `accepts`,
/// named for the message when there is none.
#[derive(Clone, Copy)]
pub struct Accepts<'a> {
    /// Whether an entry name (nested names joined with `!`) is wanted.
    pub accepts: &'a dyn Fn(&str) -> bool,
    /// What is wanted, for the message when nothing is: `.CSV`.
    pub wanted: &'a str,
}

/// Every `.CSV` entry.
pub const CSV: Accepts<'static> = Accepts {
    accepts: &|name| has_extension(name, ".csv"),
    wanted: ".CSV",
};

/// Whether `name` ends in `ext`, in any case.
pub fn has_extension(name: &str, ext: &str) -> bool {
    name.len() >= ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// Walks every entry of the zip at `path` that `accepts` wants, nested
/// zips included, in archive order. `on_entry` may return an error to stop
/// the walk.
///
/// A nested zip is walked in memory up to 64 MiB, else from a temporary
/// file in `tmp_dir`, removed once read: the zip format needs a seekable
/// central directory. Every other entry (a control file, a README) goes to
/// `on_other` with its reader.
pub fn walk_entries<E>(
    path: &Path,
    tmp_dir: &Path,
    accepts: Accepts<'_>,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    walk(
        path,
        tmp_dir,
        NESTED_IN_MEMORY,
        0,
        "",
        accepts,
        on_entry,
        on_other,
    )
}

/// [`walk_entries`] over every `.CSV` entry.
pub fn walk_csv_entries<E>(
    path: &Path,
    tmp_dir: &Path,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    walk_entries(path, tmp_dir, CSV, on_entry, on_other)
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

#[allow(clippy::too_many_arguments)]
fn walk<E>(
    path: &Path,
    tmp_dir: &Path,
    in_memory: u64,
    depth: u8,
    prefix: &str,
    accepts: Accepts<'_>,
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
        archive, &p, tmp_dir, in_memory, depth, prefix, accepts, on_entry, on_other,
    )
}

/// Where to look for what a walk wants, and what to do with it.
struct Walk<'w, 'a, E> {
    tmp_dir: &'w Path,
    in_memory: u64,
    accepts: Accepts<'a>,
    on_entry: &'w mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &'w mut dyn FnMut(Entry<'_>),
}

/// Every entry of `archive`, in archive order, except that an entry whose
/// local header disagrees with the central directory (see [`Misplaced`])
/// comes after the others.
#[allow(clippy::too_many_arguments)]
fn walk_zip<R: Read + Seek, E>(
    archive: zip::ZipArchive<R>,
    p: &str,
    tmp_dir: &Path,
    in_memory: u64,
    depth: u8,
    prefix: &str,
    accepts: Accepts<'_>,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<usize, WalkError<E>> {
    let mut w = Walk {
        tmp_dir,
        in_memory,
        accepts,
        on_entry,
        on_other,
    };
    let (mut archive, misplaced) = find_misplaced(archive, p)?;
    let mut found = 0usize;
    for i in 0..archive.len() {
        if misplaced.iter().any(|m| m.index == i) {
            continue;
        }
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
        // Entry names are ASCII in AEMO's archives; any other byte is kept
        // as the replacement character, never an error.
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let size = entry.size();
        found += visit(&mut w, p, depth, prefix, &name, size, &mut entry)?;
    }
    if !misplaced.is_empty() {
        let mut r = archive.into_inner();
        for m in &misplaced {
            let full = joined(prefix, &m.name);
            r.seek(SeekFrom::Start(m.start)).map_err(|e| {
                WalkError::Archive(ArchiveError::Entry {
                    path: p.to_owned(),
                    entry: full.clone(),
                    message: e.to_string(),
                })
            })?;
            let mut reader = Verified::new(Read::take(&mut r, m.compressed), m);
            found += visit(&mut w, p, depth, prefix, &m.name, m.size, &mut reader)?;
        }
    }
    if depth == 0 && found == 0 {
        return Err(WalkError::Archive(ArchiveError::NoCsv {
            path: p.to_owned(),
            wanted: w.accepts.wanted.to_owned(),
        }));
    }
    Ok(found)
}

/// `name` under `prefix`, nested names joined with `!`.
fn joined(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}!{name}")
    }
}

/// One entry named `name` (in its own zip) read from `reader`: handed to
/// `on_entry` when wanted, walked when a zip, else handed to `on_other`.
/// Returns how many wanted entries it held.
fn visit<E>(
    w: &mut Walk<'_, '_, E>,
    p: &str,
    depth: u8,
    prefix: &str,
    name: &str,
    size: u64,
    entry: &mut dyn Read,
) -> Result<usize, WalkError<E>> {
    let full = joined(prefix, name);
    if (w.accepts.accepts)(&full) {
        let mut reader = Counting {
            inner: entry,
            path: p,
            entry: &full,
            failed: None,
        };
        let result = (w.on_entry)(Entry {
            name: full.clone(),
            size: Some(size),
            reader: &mut reader,
        });
        if let Some(e) = reader.failed.take() {
            return Err(WalkError::Archive(e));
        }
        result.map_err(WalkError::Callback)?;
        return Ok(1);
    }
    if !has_extension(name, ".zip") {
        (w.on_other)(Entry {
            name: full,
            size: Some(size),
            reader: entry,
        });
        return Ok(0);
    }
    if depth + 1 > MAX_DEPTH {
        return Err(WalkError::Archive(ArchiveError::TooDeep {
            path: p.to_owned(),
            entry: full,
            depth: depth + 1,
        }));
    }
    let inner_label = format!("{p}!{name}");
    let bad_inner = |message: String| {
        WalkError::Archive(ArchiveError::Entry {
            path: p.to_owned(),
            entry: full.clone(),
            message,
        })
    };
    if size <= w.in_memory {
        // The zip format needs to seek to its central directory; a small
        // inner zip is walked in memory, never on disk.
        let mut bytes = Vec::with_capacity(size as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| bad_inner(e.to_string()))?;
        let inner = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| bad_inner(e.to_string()))?;
        return walk_zip(
            inner,
            &inner_label,
            w.tmp_dir,
            w.in_memory,
            depth + 1,
            &full,
            w.accepts,
            w.on_entry,
            w.on_other,
        );
    }
    let spool_err = |tmp: &Path, source: std::io::Error| {
        WalkError::Archive(ArchiveError::Spool {
            path: p.to_owned(),
            entry: full.clone(),
            tmp: tmp.display().to_string(),
            source,
        })
    };
    std::fs::create_dir_all(w.tmp_dir).map_err(|e| spool_err(w.tmp_dir, e))?;
    // Unique, and removed when dropped, whatever happens.
    let tmp = tempfile::NamedTempFile::new_in(w.tmp_dir).map_err(|e| spool_err(w.tmp_dir, e))?;
    {
        let mut out = std::io::BufWriter::with_capacity(SPOOL_BUFFER, tmp.as_file());
        std::io::copy(entry, &mut out)
            .and_then(|_| out.flush())
            .map_err(|e| spool_err(tmp.path(), e))?;
    }
    let file = tmp.reopen().map_err(|e| spool_err(tmp.path(), e))?;
    let inner = zip::ZipArchive::new(BufReader::with_capacity(ZIP_READ_BUFFER, file))
        .map_err(|e| bad_inner(e.to_string()))?;
    walk_zip(
        inner,
        &inner_label,
        w.tmp_dir,
        w.in_memory,
        depth + 1,
        &full,
        w.accepts,
        w.on_entry,
        w.on_other,
    )
}

/// An entry whose local header disagrees with its central directory
/// record: the header carries the PK signature but not the entry's name
/// where the name belongs, so the offset of its data computed from the
/// header is wrong. AEMO's archive has such entries (MMSDM 2013-10,
/// `PUBLIC_DVD_PREDISPATCHCONSTRAINT2_201310010000.zip`: 8-byte sizes
/// written over the name and extra-field lengths). Its data is the
/// `compressed` bytes that end where the next record begins; read from
/// there, it must inflate to the central record's size and CRC-32, or the
/// entry is refused as before.
#[derive(Debug)]
struct Misplaced {
    index: usize,
    name: String,
    start: u64,
    compressed: u64,
    size: u64,
    crc32: u32,
    deflated: bool,
}

/// The central directory record of one entry.
struct Central {
    header_start: u64,
    compressed: u64,
    size: u64,
    crc32: u32,
    method: zip::CompressionMethod,
    name_raw: Vec<u8>,
    name: String,
    dir: bool,
}

/// Size of a local file header before its name.
const LOCAL_HEADER: u64 = 30;

/// `archive` again, with every entry of it that is [`Misplaced`]. An entry
/// whose local header cannot be read, or lacks the signature, is left to
/// the zip reader, which reports it as before.
fn find_misplaced<R: Read + Seek, E>(
    mut archive: zip::ZipArchive<R>,
    p: &str,
) -> Result<(zip::ZipArchive<R>, Vec<Misplaced>), WalkError<E>> {
    let mut central = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let Ok(f) = archive.by_index_raw(i) else {
            // Reported by the walk itself.
            return Ok((archive, Vec::new()));
        };
        central.push(Central {
            header_start: f.header_start(),
            compressed: f.compressed_size(),
            size: f.size(),
            crc32: f.crc32(),
            method: f.compression(),
            name_raw: f.name_raw().to_vec(),
            name: String::from_utf8_lossy(f.name_raw()).into_owned(),
            dir: f.is_dir(),
        });
    }
    let directory = archive.central_directory_start();
    let mut r = archive.into_inner();
    let mut out = Vec::new();
    for (index, c) in central.iter().enumerate() {
        if c.dir || local_header_agrees(&mut r, c) {
            continue;
        }
        let deflated = match c.method {
            zip::CompressionMethod::Deflated => true,
            zip::CompressionMethod::Stored => false,
            _ => continue,
        };
        let end = central
            .iter()
            .map(|o| o.header_start)
            .filter(|&h| h > c.header_start)
            .min()
            .unwrap_or(directory);
        let Some(start) = end.checked_sub(c.compressed) else {
            continue;
        };
        if start < c.header_start + LOCAL_HEADER {
            continue;
        }
        out.push(Misplaced {
            index,
            name: c.name.clone(),
            start,
            compressed: c.compressed,
            size: c.size,
            crc32: c.crc32,
            deflated,
        });
    }
    let archive = zip::ZipArchive::new(r).map_err(|e| {
        WalkError::Archive(ArchiveError::Open {
            path: p.to_owned(),
            message: e.to_string(),
        })
    })?;
    Ok((archive, out))
}

/// Whether the local header of `c` names the entry as its central record
/// does. A header that cannot be read or has no signature counts as
/// agreeing: the zip reader reports it.
fn local_header_agrees<R: Read + Seek>(r: &mut R, c: &Central) -> bool {
    let mut head = [0u8; LOCAL_HEADER as usize];
    if r.seek(SeekFrom::Start(c.header_start)).is_err() || r.read_exact(&mut head).is_err() {
        return true;
    }
    if head[..4] != *b"PK\x03\x04" {
        return true;
    }
    let name_len = usize::from(u16::from_le_bytes([head[26], head[27]]));
    if name_len != c.name_raw.len() {
        return false;
    }
    let mut name = vec![0u8; name_len];
    if r.read_exact(&mut name).is_err() {
        return true;
    }
    name == c.name_raw
}

/// The bytes of a [`Misplaced`] entry: inflated (or stored), counted and
/// hashed, and refused at the end unless they are the size and CRC-32 its
/// central record gives.
struct Verified<'m, R: Read> {
    inner: Raw<R>,
    m: &'m Misplaced,
    hasher: crc32fast::Hasher,
    read: u64,
}

enum Raw<R: Read> {
    Deflated(flate2::read::DeflateDecoder<R>),
    Stored(R),
}

impl<'m, R: Read> Verified<'m, R> {
    fn new(raw: R, m: &'m Misplaced) -> Self {
        let inner = if m.deflated {
            Raw::Deflated(flate2::read::DeflateDecoder::new(raw))
        } else {
            Raw::Stored(raw)
        };
        Self {
            inner,
            m,
            hasher: crc32fast::Hasher::new(),
            read: 0,
        }
    }
}

impl<R: Read> Read for Verified<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = match &mut self.inner {
            Raw::Deflated(d) => d.read(buf)?,
            Raw::Stored(s) => s.read(buf)?,
        };
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        if n == 0 || self.read > self.m.size {
            let crc = self.hasher.clone().finalize();
            if self.read != self.m.size || crc != self.m.crc32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "its local header disagrees with the central directory, and its data read from where the next record begins is {} bytes with CRC-32 {crc:08x}, not {} bytes with {:08x}",
                        self.read, self.m.size, self.m.crc32
                    ),
                ));
            }
        }
        Ok(n)
    }
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
            CSV,
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
            utilake_core::Diagnose::diagnostic(&ArchiveError::NoCsv {
                path: "p".into(),
                wanted: ".CSV".into()
            })
            .code,
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

    /// Bytes that do not compress, so a deflate stream is long.
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_u32;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    }

    /// A zip of `entries`, the local header of entry `broken` damaged the
    /// way AEMO's are: its name-length field holds a size's high bytes.
    fn misplaced_zip(entries: &[(&str, &[u8], bool)], broken: usize) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            for (name, bytes, stored) in entries {
                let method = if *stored {
                    zip::CompressionMethod::Stored
                } else {
                    zip::CompressionMethod::Deflated
                };
                w.start_file(
                    *name,
                    zip::write::SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        let mut bytes = cursor.into_inner();
        let at = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone()))
            .unwrap()
            .by_index_raw(broken)
            .unwrap()
            .header_start() as usize;
        bytes[at + 26..at + 28].copy_from_slice(&0x4BAF_u16.to_le_bytes());
        bytes
    }

    #[test]
    fn an_entry_whose_local_header_disagrees_is_read_from_the_central_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let big = noise(200_000);
        let region = fixture("REGION_v1_head.csv");
        // The plain zip reader refuses it.
        let bytes = misplaced_zip(&[("A.CSV", &region, false), ("B.CSV", &big, false)], 1);
        let mut plain = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let mut sink = Vec::new();
        assert!(plain.by_index(1).unwrap().read_to_end(&mut sink).is_err());
        // The walk reads it, after the others.
        let z = dir.path().join("m.zip");
        std::fs::write(&z, &bytes).unwrap();
        let (got, _) = collect(&z, &tmp).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("A.CSV".to_owned(), region.clone()));
        assert_eq!(got[1], ("B.CSV".to_owned(), big.clone()));
        // The last entry (data ends at the central directory), and a
        // stored one with a text entry and a directory beside it.
        let stored = misplaced_zip(
            &[
                ("C.CSV", &region, true),
                ("d/", b"", true),
                ("n.txt", b"x", false),
            ],
            0,
        );
        let z = dir.path().join("s.zip");
        std::fs::write(&z, &stored).unwrap();
        let (got, others) = collect(&z, &tmp).unwrap();
        assert_eq!(got, [("C.CSV".to_owned(), region.clone())]);
        assert_eq!(others, ["n.txt"]);
        // Nested in a whole-month zip, in memory and spooled.
        let outer = dir.path().join("outer.zip");
        make_zip(&outer, &[("in/X.zip", &bytes), ("README.txt", b"r")]);
        for in_memory in [NESTED_IN_MEMORY, 0] {
            let (got, others) = collect_with(&outer, &tmp, in_memory).unwrap();
            let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(names, ["in/X.zip!A.CSV", "in/X.zip!B.CSV"]);
            assert_eq!(got[1].1, big);
            assert_eq!(others, ["README.txt"]);
        }
        // A misplaced zip entry is walked too.
        let holder = misplaced_zip(&[("in/X.zip", &bytes, false)], 0);
        let z = dir.path().join("h.zip");
        std::fs::write(&z, &holder).unwrap();
        let (got, _) = collect(&z, &tmp).unwrap();
        assert_eq!(got[1], ("in/X.zip!B.CSV".to_owned(), big.clone()));
    }

    #[test]
    fn a_misplaced_entry_that_does_not_match_its_crc_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let big = noise(50_000);
        // Stored, one byte changed: right size, wrong CRC-32.
        let mut bytes = misplaced_zip(&[("A.CSV", &big, true)], 0);
        let at = 30 + "A.CSV".len() + 100;
        bytes[at] ^= 0xFF;
        let z = dir.path().join("c.zip");
        std::fs::write(&z, &bytes).unwrap();
        let e = collect(&z, &tmp).unwrap_err();
        let WalkError::Archive(ArchiveError::Entry { message, .. }) = e else {
            panic!("{e}");
        };
        assert!(message.contains("local header disagrees"), "{message}");
        // A name of the right length but other bytes is misplaced too; a
        // central size larger than the bytes there overruns into the next
        // record and is refused, and one with no room is left to the reader.
        let mut renamed = misplaced_zip(&[("A.CSV", &big, true), ("B.CSV", b"b", true)], 0);
        renamed[26..28].copy_from_slice(&5_u16.to_le_bytes());
        renamed[30] = b'Z';
        let z = dir.path().join("r.zip");
        std::fs::write(&z, &renamed).unwrap();
        let (got, _) = collect(&z, &tmp).unwrap();
        assert_eq!(got[0], ("B.CSV".to_owned(), b"b".to_vec()));
        assert_eq!(got[1], ("A.CSV".to_owned(), big.clone()));
        let header = Central {
            header_start: 0,
            compressed: 0,
            size: 0,
            crc32: 0,
            method: zip::CompressionMethod::Stored,
            name_raw: b"A.CSV".to_vec(),
            name: "A.CSV".into(),
            dir: false,
        };
        assert!(local_header_agrees(
            &mut std::io::Cursor::new(b"PK"),
            &header
        ));
        assert!(local_header_agrees(
            &mut std::io::Cursor::new(vec![0u8; 40]),
            &header
        ));
        let mut short = b"PK\x03\x04".to_vec();
        short.resize(30, 0);
        short[26] = 5;
        assert!(local_header_agrees(
            &mut std::io::Cursor::new(short),
            &header
        ));
        let m = Misplaced {
            index: 0,
            name: "A.CSV".into(),
            start: 0,
            compressed: 4,
            size: 2,
            crc32: 0,
            deflated: false,
        };
        let mut v = Verified::new(&b"abcd"[..], &m);
        let mut out = Vec::new();
        assert!(v.read_to_end(&mut out).is_err(), "longer than its size");
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
