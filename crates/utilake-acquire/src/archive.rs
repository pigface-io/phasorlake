//! The archive: every downloaded file, kept as the server sent it.
//!
//! A file lives at a path that mirrors its URL
//! (`{archive}/www.nemweb.com.au/Data_Archive/.../PUBLIC_DVD_X_202401010000.zip`)
//! beside a small JSON sidecar that records where it came from, when, and
//! what its bytes hash to. A file is downloaded once; nothing in utilake
//! replaces or deletes an archived file, which is the untransformed record
//! the census and the lake are derived from. To fetch one again, delete it.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utilake_core::calendar::NEM_OFFSET_SECS;
use utilake_core::timetext::{self, Stamp};

use crate::error::FetchError;

/// Where an archived file came from and what its bytes are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// The URL the file was downloaded from.
    pub url: String,
    /// Bytes.
    pub size: u64,
    /// sha256 hex of the bytes.
    pub sha256: String,
    /// When the download completed, RFC 3339 UTC.
    pub fetched_at: String,
    /// The server's `ETag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The server's `Last-Modified`: when AEMO published the file, as the
    /// server wrote it (an HTTP date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// `last_modified` as an instant (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_at: Option<Stamp>,
    /// The modified time the directory listing showed, as the listing
    /// wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified: Option<String>,
    /// `listed_modified` as an instant (AEST, `+10:00`: nemweb lists in
    /// market time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified_at: Option<Stamp>,
    /// The program and version that downloaded it, e.g. `utilake/0.1.0`.
    #[serde(default)]
    pub tool: String,
    /// Where the bytes came from when they are one entry of a zip, fetched
    /// alone by byte span rather than with the zip; none for a file
    /// downloaded whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_from: Option<Extracted>,
}

/// One entry of a zip, fetched alone: what the zip's central directory
/// said of it and where in the zip it was, so the archived bytes trace
/// back to the server's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extracted {
    /// The zip's URL.
    pub url: String,
    /// The entry's name in the zip.
    pub entry: String,
    /// The zip's size.
    pub zip_size: u64,
    /// Where the entry's local header starts in the zip.
    pub offset: u64,
    /// The entry's compressed size.
    pub compressed: u64,
    /// Its compression method (0 stored, 8 deflated).
    pub method: u16,
    /// Its CRC-32, as the directory declared it and the bytes matched.
    pub crc32: u32,
}

impl Sidecar {
    /// The sidecar with its `_at` instants derived from the text fields
    /// beside them, where a record written before they were kept (by an
    /// older utilake or mirror) lacks them. The text fields stay as
    /// written; a text that is not a time has no instant. A `fetched_at`
    /// that is not a time with an offset is the error returned.
    pub fn derived(mut self, path: &Path) -> Result<Self, FetchError> {
        if !timetext::is_rfc3339(&self.fetched_at) {
            return Err(FetchError::SidecarCorrupt {
                path: path.display().to_string(),
                message: format!(
                    "fetched_at {:?} is not RFC 3339 with an offset",
                    self.fetched_at
                ),
            });
        }
        if self.last_modified_at.is_none() {
            self.last_modified_at = self
                .last_modified
                .as_deref()
                .and_then(|s| timetext::parse(s, NEM_OFFSET_SECS));
        }
        if self.listed_modified_at.is_none() {
            self.listed_modified_at = self
                .listed_modified
                .as_deref()
                .and_then(|s| timetext::parse(s, NEM_OFFSET_SECS));
        }
        Ok(self)
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
    sha256_read(&mut std::fs::File::open(path)?)
}

/// sha256 hex and length of everything `f` reads, in 1 MiB pieces.
pub fn sha256_read(f: &mut impl Read) -> std::io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
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
    Ok((hex(&hasher.finalize()), len))
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

    /// Where a URL's bytes are kept (`utilake_plain_decode::layout`): the
    /// URL as a path under the archive, host first, one folder per report
    /// folder (a `CURRENT` or `ARCHIVE` listing segment dropped). A query
    /// string, which no archive URL has, is folded into the file name as a
    /// hash so two queries of one path stay apart; a fragment is not part
    /// of the file.
    pub fn path_for(&self, url: &str) -> PathBuf {
        let mut path = self.root.clone();
        for s in utilake_plain_decode::layout::local_segments(url) {
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

    /// The in-progress path of a download.
    pub fn part_path(path: &Path) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(".part");
        PathBuf::from(s)
    }

    /// The sidecar at `path`'s side, when there is one.
    pub fn read_sidecar(path: &Path) -> Result<Option<Sidecar>, FetchError> {
        let sc = Self::sidecar_path(path);
        let text = match std::fs::read_to_string(&sc) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err(&sc)(e)),
        };
        let sidecar: Sidecar =
            serde_json::from_str(&text).map_err(|e| FetchError::SidecarCorrupt {
                path: sc.display().to_string(),
                message: e.to_string(),
            })?;
        sidecar.derived(&sc).map(Some)
    }

    /// The archived file of a URL, if it and its sidecar are there and
    /// the sidecar names the same URL and size.
    pub fn lookup(&self, url: &str) -> Result<Option<Archived>, FetchError> {
        let path = self.path_for(url);
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        let Some(sidecar) = Self::read_sidecar(&path)? else {
            return Ok(None);
        };
        if sidecar.url != url || sidecar.size != meta.len() {
            return Ok(None);
        }
        Ok(Some(Archived { path, sidecar }))
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

    /// Writes a sidecar.
    pub fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<(), FetchError> {
        let sc = Self::sidecar_path(path);
        let text = serde_json::to_string_pretty(sidecar).expect("a Sidecar always serialises");
        std::fs::write(&sc, text).map_err(io_err(&sc))
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
                // What macOS keeps beside a file on a disk of another
                // format (`._x`) is no file of the archive.
                if p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("._"))
                {
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
    use utilake_core::Diagnose as _;

    #[test]
    fn the_archive_mirrors_the_url() {
        let a = Archive::new("/home/archive");
        // One folder per report folder: a listing segment is dropped.
        assert_eq!(
            a.path_for("https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/X.zip"),
            a.path_for("https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/X.zip")
        );
        assert_eq!(
            a.path_for("https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/X.zip"),
            Path::new("/home/archive/www.nemweb.com.au/Reports/Dispatch_SCADA/X.zip")
        );
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
            fetched_at: "2026-10-05T00:00:00Z".into(),
            etag: None,
            last_modified: Some("Tue, 13 Feb 2024 08:27:00 GMT".into()),
            last_modified_at: timetext::parse_rfc3339("2024-02-13T08:27:00Z"),
            listed_modified: Some("2/13/2024  6:27 PM".into()),
            listed_modified_at: timetext::parse_rfc3339("2024-02-13T18:27:00+10:00"),
            tool: "utilake/test".into(),
            extracted_from: None,
        };
        Archive::write_sidecar(&path, &sidecar).unwrap();
        let found = a.lookup(url).unwrap().unwrap();
        assert_eq!(found.sidecar, sidecar);
        // The text fields are written as they came; every `_at` field and
        // `fetched_at` parse with their offset.
        let text = std::fs::read_to_string(Archive::sidecar_path(&path)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["last_modified"], "Tue, 13 Feb 2024 08:27:00 GMT");
        assert_eq!(json["listed_modified"], "2/13/2024  6:27 PM");
        for key in ["fetched_at", "last_modified_at", "listed_modified_at"] {
            let s = json[key].as_str().unwrap();
            assert!(timetext::is_rfc3339(s), "{key}: {s}");
        }
        Archive::verify(&found).unwrap();
        assert_eq!(a.entries_under(None).len(), 1);
        // A macOS AppleDouble copy of the file and of its sidecar is passed
        // over.
        let apple =
            path.with_file_name(format!("._{}", path.file_name().unwrap().to_string_lossy()));
        std::fs::copy(&path, &apple).unwrap();
        std::fs::copy(Archive::sidecar_path(&path), Archive::sidecar_path(&apple)).unwrap();
        assert_eq!(a.entries_under(None).len(), 1);
        std::fs::remove_file(Archive::sidecar_path(&apple)).unwrap();
        std::fs::remove_file(&apple).unwrap();
        assert_eq!(a.entries_under(Some(&dir.path().join("h/d"))).len(), 1);
        // Another size, or another URL, is not this download.
        std::fs::write(&path, b"abcd").unwrap();
        assert!(a.lookup(url).unwrap().is_none());
        let changed = Archived {
            path: path.clone(),
            sidecar: sidecar.clone(),
        };
        assert_eq!(
            Archive::verify(&changed).unwrap_err().diagnostic().code,
            "cache.sha256_mismatch"
        );
        std::fs::write(Archive::sidecar_path(&path), b"{").unwrap();
        assert!(Archive::read_sidecar(&path).is_err());
        // A sidecar of an older utilake, with fields this one does not
        // keep, still reads.
        std::fs::write(
            Archive::sidecar_path(&path),
            br#"{"url":"https://h/d/f.zip","size":4,"sha256":"x","fetched_at":"2026-10-03T10:00:05Z","accept_ranges":true,"request":{"by":"x"}}"#,
        )
        .unwrap();
        assert_eq!(Archive::read_sidecar(&path).unwrap().unwrap().size, 4);
    }

    #[test]
    fn older_records_gain_their_instants_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.zip");
        std::fs::write(&path, b"abc").unwrap();
        // The mirror's record before it kept the `_at` fields: the HTTP
        // date as the server sent it, and the listing's text in market
        // time with no offset. Both stay as written; the instants are
        // derived beside them.
        std::fs::write(
            Archive::sidecar_path(&path),
            br#"{"url":"https://h/d/f.zip","size":3,"sha256":"x","crc32":1,"fetched_at":"2026-10-18T00:03:51Z","last_modified":"Mon, 18 May 2026 07:27:50 GMT","listed_size":3,"listed_modified":"Monday, May 18, 2026 05:27 PM","tool":"nemweb-mirror/0.1.0"}"#,
        )
        .unwrap();
        let sc = Archive::read_sidecar(&path).unwrap().unwrap();
        assert_eq!(sc.fetched_at, "2026-10-18T00:03:51Z");
        assert_eq!(
            sc.last_modified.as_deref(),
            Some("Mon, 18 May 2026 07:27:50 GMT")
        );
        assert_eq!(
            sc.last_modified_at.map(|s| s.rfc3339()).as_deref(),
            Some("2026-05-18T07:27:50Z")
        );
        assert_eq!(
            sc.listed_modified.as_deref(),
            Some("Monday, May 18, 2026 05:27 PM")
        );
        assert_eq!(
            sc.listed_modified_at.map(|s| s.rfc3339()).as_deref(),
            Some("2026-05-18T17:27:00+10:00")
        );
        // The mirror's record with the `_at` fields reads the same, and
        // what it wrote there is kept over what the text would give.
        std::fs::write(
            Archive::sidecar_path(&path),
            br#"{"url":"https://h/d/f.zip","size":3,"sha256":"x","crc32":1,"fetched_at":"2026-10-18T00:03:51Z","last_modified":"Mon, 18 May 2026 07:27:50 GMT","last_modified_at":"2026-05-18T07:27:50Z","listed_size":3,"listed_modified":"Monday, May 18, 2026 05:27 PM","listed_modified_at":"2026-05-18T17:27:00+10:00","tool":"nemweb-mirror/0.1.0"}"#,
        )
        .unwrap();
        assert_eq!(Archive::read_sidecar(&path).unwrap().unwrap(), sc);
        // A text that is not a time has no instant; a zip entry's naive
        // time is market time; an `_at` without an offset is corrupt, and
        // so is a fetched_at that is not a time.
        std::fs::write(
            Archive::sidecar_path(&path),
            br#"{"url":"u","size":3,"sha256":"x","fetched_at":"2026-10-18T00:03:51Z","last_modified":"unknown","listed_modified":"2017-01-12 12:24:16"}"#,
        )
        .unwrap();
        let sc = Archive::read_sidecar(&path).unwrap().unwrap();
        assert_eq!(sc.last_modified.as_deref(), Some("unknown"));
        assert_eq!(sc.last_modified_at, None);
        assert_eq!(
            sc.listed_modified_at.map(|s| s.rfc3339()).as_deref(),
            Some("2017-01-12T12:24:16+10:00")
        );
        std::fs::write(
            Archive::sidecar_path(&path),
            br#"{"url":"u","size":3,"sha256":"x","fetched_at":"2026-10-18T00:03:51Z","last_modified_at":"2026-05-18T07:27:50"}"#,
        )
        .unwrap();
        let err = Archive::read_sidecar(&path).unwrap_err();
        assert_eq!(err.diagnostic().code, "cache.sidecar_corrupt");
        assert!(err.to_string().contains("offset"), "{err}");
        std::fs::write(
            Archive::sidecar_path(&path),
            br#"{"url":"u","size":3,"sha256":"x","fetched_at":"t"}"#,
        )
        .unwrap();
        let err = Archive::read_sidecar(&path).unwrap_err();
        assert_eq!(err.diagnostic().code, "cache.sidecar_corrupt");
        assert!(err.to_string().contains("fetched_at"), "{err}");
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    /// A reader interrupted once, then done.
    struct Flaky(usize, &'static [u8]);

    impl Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0 += 1;
            match self.0 {
                1 => Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
                2 => {
                    let n = self.1.len().min(buf.len());
                    buf[..n].copy_from_slice(&self.1[..n]);
                    self.1 = &self.1[n..];
                    Ok(n)
                }
                3 => Ok(0),
                _ => Err(std::io::Error::other("gone")),
            }
        }
    }

    #[test]
    fn hashing_rides_out_an_interruption_and_reports_a_failure() {
        let (sha, len) = sha256_read(&mut Flaky(0, b"abc")).unwrap();
        assert_eq!((sha, len), (sha256_hex(b"abc"), 3));
        let mut gone = Flaky(3, b"");
        assert_eq!(sha256_read(&mut gone).unwrap_err().to_string(), "gone");
    }

    #[test]
    fn a_url_with_no_path_and_a_sidecar_that_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let a = Archive::new(dir.path());
        assert_eq!(a.path_for("https://h"), dir.path().join("h"));
        assert_eq!(a.path_for(""), dir.path().join("_"));
        assert_eq!(
            a.path_for("https://h/A.zip!B.zip"),
            dir.path().join("h").join("A.zip!B.zip")
        );
        // A sidecar path that is a directory: an I/O error, not "none".
        let path = dir.path().join("x.zip");
        std::fs::create_dir_all(Archive::sidecar_path(&path)).unwrap();
        assert!(matches!(
            Archive::read_sidecar(&path),
            Err(FetchError::CacheIo { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_read_is_passed_over() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let a = Archive::new(dir.path());
        let shut = dir.path().join("shut");
        std::fs::create_dir_all(&shut).unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(a.entries_under(None).is_empty());
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn an_extracted_entry_records_where_in_the_zip_it_was() {
        let text = r#"{"url":"https://h/A.zip!B.zip","size":3,"sha256":"s","fetched_at":"2026-10-05T00:00:00Z",
            "extracted_from":{"url":"https://h/A.zip","entry":"B.zip","zip_size":100,"offset":30,"compressed":3,"method":0,"crc32":7}}"#;
        let s: Sidecar = serde_json::from_str(text).unwrap();
        let x = s.extracted_from.clone().unwrap();
        assert_eq!(
            (x.entry.as_str(), x.offset, x.method, x.crc32),
            ("B.zip", 30, 0, 7)
        );
        let back = serde_json::to_string(&s).unwrap();
        assert!(back.contains("\"extracted_from\""));
        let plain: Sidecar = serde_json::from_str(
            r#"{"url":"u","size":1,"sha256":"s","fetched_at":"2026-10-05T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(plain.extracted_from.is_none());
        assert!(
            !serde_json::to_string(&plain)
                .unwrap()
                .contains("extracted_from")
        );
    }
}
