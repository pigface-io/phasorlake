//! Where downloads live on disk.
//!
//! Two places with different promises:
//!
//! - The **archive** keeps every downloaded file exactly as the server sent
//!   it, at a path that mirrors its URL
//!   (`{archive}/www.nemweb.com.au/Data_Archive/.../PUBLIC_DVD_X_202401010000.zip`),
//!   beside a JSON sidecar that records when, how and from where it was
//!   requested and what its bytes hash to. Nothing in nemlake deletes an
//!   archived file: it is the untransformed record everything else can be
//!   derived from again.
//! - The **cache** holds what can be thrown away: directory listings with a
//!   time to live, and spooled inner zips of an ingest in progress.
//!
//! A member read out of a remote zip is archived under the zip's own path
//! with `.members` added, so `…/2012/MMSDM_2012_01.zip` and the files taken
//! from inside it sit side by side.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::FetchError;

/// What is known about an archived download: what the server said, what the
/// bytes hash to, and the request that fetched it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// The URL that identifies the file; for a member of a remote zip, the
    /// zip's URL with the member name as its fragment.
    pub url: String,
    /// ETag.
    #[serde(default)]
    pub etag: Option<String>,
    /// Last-Modified.
    #[serde(default)]
    pub last_modified: Option<String>,
    /// Bytes.
    pub size: u64,
    /// sha256 hex of the bytes.
    pub sha256: String,
    /// When the download completed, RFC 3339 UTC.
    pub fetched_at: String,
    /// Whether the server advertised byte ranges.
    #[serde(default)]
    pub accept_ranges: bool,
    /// How the file was requested. Absent for listings and for files
    /// archived before this was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Requested>,
    /// The file this one replaced at the same URL, when its bytes differed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Superseded>,
}

impl Sidecar {
    /// A sidecar with the facts every download has and no request record.
    pub fn bare(url: &str, size: u64, sha256: String) -> Self {
        Self {
            url: url.to_owned(),
            etag: None,
            last_modified: None,
            size,
            sha256,
            fetched_at: nemlake_core::clock::now_utc_rfc3339(),
            accept_ranges: false,
            request: None,
            supersedes: None,
        }
    }
}

/// When, how and from where a file was requested.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Requested {
    /// The nemlake command that asked, e.g.
    /// `fetch nem DISPATCHPRICE 2012-01..2012-01`.
    pub by: String,
    /// The program and version that made the request, e.g. `nemlake/0.1.0`.
    pub tool: String,
    /// The `User-Agent` sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// The source that listed the file, e.g. `nem-mmsdm`.
    pub source: String,
    /// The table the source said the file carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// The period the source said the file covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// When the request that delivered the file was sent, RFC 3339 UTC.
    pub started_at: String,
    /// The URL the request went to: the file itself, or the zip it is a
    /// member of.
    pub request_url: String,
    /// The `Range` header, when only part of the URL was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    /// The status the server answered with.
    pub http_status: u16,
    /// Requests sent until one delivered the file.
    pub attempts: u32,
    /// The byte the last request resumed from, when an earlier one broke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<u64>,
    /// Set when the file is one member of a remote zip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<MemberOf>,
    /// The size the listing showed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_size: Option<u64>,
    /// The modification time the listing showed, as written there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified: Option<String>,
}

/// Where in a remote zip an archived member came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberOf {
    /// The member's path inside the zip.
    pub name: String,
    /// Offset of its local file header in the zip.
    pub header_offset: u64,
    /// Bytes it occupies in the zip.
    pub compressed_size: u64,
    /// Zip compression method: 0 stored, 8 deflate. The archived file is the
    /// member as it was before the zip compressed it.
    pub method: u16,
    /// The CRC-32 the zip's directory records, which the bytes were checked
    /// against.
    pub crc32: String,
}

/// An earlier download of the same URL whose bytes were different.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Superseded {
    /// Its sha256.
    pub sha256: String,
    /// When it had been fetched.
    pub fetched_at: String,
    /// The file name it is kept under, beside the new file.
    pub kept_as: String,
}

/// A complete archived download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBlob {
    /// The bytes.
    pub path: PathBuf,
    /// What is known about them.
    pub sidecar: Sidecar,
}

/// The archive of downloads and the cache of listings.
#[derive(Debug, Clone)]
pub struct DiskCache {
    root: PathBuf,
    archive: PathBuf,
}

/// Hex of sha256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// `%XX` escapes turned back into the bytes they stand for.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let hex = |b: u8| (b as char).to_digit(16);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One path segment of a URL as a file or directory name: unescaped, with
/// the characters no file system takes, and the names that would climb out
/// of the archive, replaced.
fn segment(s: &str) -> String {
    let cleaned: String = percent_decode(s)
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    match cleaned.as_str() {
        "" | "." | ".." => "_".to_owned(),
        _ => cleaned,
    }
}

impl DiskCache {
    /// Listings under `root`, downloads under `root/archive`: everything in
    /// one directory, for tests and tools.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let archive = root.join("archive");
        Self { root, archive }
    }

    /// Listings under `cache`, downloads under `archive`: the layout of a
    /// nemlake home, where the two are siblings.
    pub fn at(cache: impl Into<PathBuf>, archive: impl Into<PathBuf>) -> Self {
        Self {
            root: cache.into(),
            archive: archive.into(),
        }
    }

    /// The directory of what can be thrown away: listings and spooled files.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory downloads are kept in.
    pub fn archive_root(&self) -> &Path {
        &self.archive
    }

    /// Where a URL's bytes are kept: the URL itself as a path under the
    /// archive, host first. A fragment names a member of a remote zip and
    /// goes under the zip's path with `.members` added. A query string,
    /// which no archive URL has, is folded into the file name as a hash so
    /// two queries of one path stay apart.
    pub fn path_for(&self, url: &str) -> PathBuf {
        let (rest, member) = match url.split_once('#') {
            Some((r, m)) => (r, Some(m)),
            None => (url, None),
        };
        let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
        let (location, query) = match rest.split_once('?') {
            Some((l, q)) => (l, Some(q)),
            None => (rest, None),
        };
        let mut path = self.archive.clone();
        let mut last = String::new();
        for s in location.split('/').filter(|s| !s.is_empty()) {
            if !last.is_empty() {
                path.push(&last);
            }
            last = segment(s);
        }
        if last.is_empty() {
            "_".clone_into(&mut last);
        }
        if let Some(q) = query {
            last = format!("{last}@{}", &sha256_hex(q.as_bytes())[..16]);
        }
        match member {
            None => path.push(last),
            Some(m) => {
                path.push(format!("{last}.members"));
                for s in m.split('/').filter(|s| !s.is_empty()) {
                    path.push(segment(s));
                }
            }
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
        if !sc.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&sc).map_err(|source| FetchError::CacheIo {
            path: sc.display().to_string(),
            source,
        })?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| FetchError::SidecarCorrupt {
                path: sc.display().to_string(),
                message: e.to_string(),
            })
    }

    /// A complete archived download for a URL, if both file and sidecar
    /// exist and the sidecar names the same URL and size.
    pub fn lookup(&self, url: &str) -> Result<Option<CachedBlob>, FetchError> {
        let path = self.path_for(url);
        if !path.is_file() {
            return Ok(None);
        }
        let Some(sidecar) = Self::read_sidecar(&path)? else {
            return Ok(None);
        };
        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .map_err(|source| FetchError::CacheIo {
                path: path.display().to_string(),
                source,
            })?;
        if sidecar.url != url || sidecar.size != size {
            return Ok(None);
        }
        Ok(Some(CachedBlob { path, sidecar }))
    }

    /// Hashes an archived file and compares it with its sidecar.
    pub fn verify(&self, blob: &CachedBlob) -> Result<(), FetchError> {
        let bytes = std::fs::read(&blob.path).map_err(|source| FetchError::CacheIo {
            path: blob.path.display().to_string(),
            source,
        })?;
        let observed = sha256_hex(&bytes);
        if observed != blob.sidecar.sha256 {
            return Err(FetchError::Sha256Mismatch {
                path: blob.path.display().to_string(),
                expected: blob.sidecar.sha256.clone(),
                observed,
            });
        }
        Ok(())
    }

    /// Writes a sidecar.
    pub fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<(), FetchError> {
        let sc = Self::sidecar_path(path);
        let text = serde_json::to_string_pretty(sidecar).expect("a Sidecar always serialises");
        std::fs::write(&sc, text).map_err(|source| FetchError::CacheIo {
            path: sc.display().to_string(),
            source,
        })
    }

    /// Makes room at `path` for a new download of the same URL. When a file
    /// is there with other bytes than `new_sha256`, it and its sidecar are
    /// kept beside it under a name that carries the old hash, and what to
    /// record about it is returned. The same bytes, or a file with no
    /// sidecar, are simply replaced.
    pub fn set_aside(path: &Path, new_sha256: &str) -> Result<Option<Superseded>, FetchError> {
        if !path.is_file() {
            return Ok(None);
        }
        let Ok(Some(old)) = Self::read_sidecar(path) else {
            return Ok(None);
        };
        if old.sha256 == new_sha256 {
            return Ok(None);
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kept_as = format!(
            "{name}.{}.superseded",
            &old.sha256[..old.sha256.len().min(12)]
        );
        let kept = path.with_file_name(&kept_as);
        let io = |p: &Path, source| FetchError::CacheIo {
            path: p.display().to_string(),
            source,
        };
        std::fs::rename(path, &kept).map_err(|e| io(&kept, e))?;
        let (from, to) = (Self::sidecar_path(path), Self::sidecar_path(&kept));
        std::fs::rename(&from, &to).map_err(|e| io(&to, e))?;
        Ok(Some(Superseded {
            sha256: old.sha256,
            fetched_at: old.fetched_at,
            kept_as,
        }))
    }

    /// Where a listing page is cached, with its own sidecar.
    pub fn listing_path(&self, url: &str) -> PathBuf {
        self.root
            .join("listings")
            .join(format!("{}.html", &sha256_hex(url.as_bytes())[..16]))
    }

    /// A cached listing no older than `max_age_secs`.
    pub fn lookup_listing(
        &self,
        url: &str,
        max_age_secs: i64,
    ) -> Result<Option<Vec<u8>>, FetchError> {
        let path = self.listing_path(url);
        if !path.is_file() {
            return Ok(None);
        }
        let Some(sidecar) = Self::read_sidecar(&path)? else {
            return Ok(None);
        };
        let fetched = parse_rfc3339_secs(&sidecar.fetched_at).unwrap_or(0);
        if sidecar.url != url || nemlake_core::clock::unix_now() - fetched > max_age_secs {
            return Ok(None);
        }
        std::fs::read(&path)
            .map(Some)
            .map_err(|source| FetchError::CacheIo {
                path: path.display().to_string(),
                source,
            })
    }

    /// Stores a listing page.
    pub fn store_listing(&self, url: &str, body: &[u8]) -> Result<(), FetchError> {
        let path = self.listing_path(url);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|source| FetchError::CacheIo {
                path: p.display().to_string(),
                source,
            })?;
        }
        std::fs::write(&path, body).map_err(|source| FetchError::CacheIo {
            path: path.display().to_string(),
            source,
        })?;
        Self::write_sidecar(
            &path,
            &Sidecar::bare(url, body.len() as u64, sha256_hex(body)),
        )
    }

    /// Every archived download, in path order.
    pub fn entries(&self) -> Result<Vec<CachedBlob>, FetchError> {
        let mut out = Vec::new();
        let mut stack = vec![self.archive.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for p in rd.filter_map(Result::ok).map(|e| e.path()) {
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "json") {
                    let data = p.with_extension("");
                    if data.is_file()
                        && let Ok(text) = std::fs::read_to_string(&p)
                        && let Ok(sidecar) = serde_json::from_str::<Sidecar>(&text)
                    {
                        out.push(CachedBlob {
                            path: data,
                            sidecar,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

/// A file that serves no purpose any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    /// The file.
    pub path: PathBuf,
    /// Its size.
    pub bytes: u64,
    /// Why it is a leftover, for the report.
    pub why: &'static str,
}

/// How old a spooled inner zip or an unfinished write must be before it is
/// taken for abandoned; a running ingest owns younger ones.
const BUSY_SECS: i64 = 3_600;
/// No listing or zip index is trusted for longer than this.
const LISTING_MAX_SECS: i64 = 90 * 86_400;

impl DiskCache {
    /// Files that are neither an archived download nor in use: partial
    /// downloads nobody resumed within `part_max_age_secs`, spooled inner
    /// zips of ingests that are long over, sidecars whose file is gone, and
    /// listings past any use. A complete download is never a leftover, and
    /// neither is a file in the archive that merely lacks a sidecar: it may
    /// be the only copy of something.
    pub fn leftovers(&self, part_max_age_secs: i64) -> Vec<Leftover> {
        let now = nemlake_core::clock::unix_now();
        let age = |p: &Path| -> i64 {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(i64::MAX, |d| now - d.as_secs() as i64)
        };
        let mut out = Vec::new();
        let mut push = |path: PathBuf, why: &'static str| {
            let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
            out.push(Leftover { path, bytes, why });
        };
        let mut stack = vec![
            (self.root.join("tmp"), "tmp"),
            (self.root.join("listings"), "listings"),
            (self.archive.clone(), "archive"),
        ];
        while let Some((dir, kind)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for p in rd.filter_map(Result::ok).map(|e| e.path()) {
                if p.is_dir() {
                    stack.push((p, kind));
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                match kind {
                    "tmp" => {
                        if age(&p) > BUSY_SECS {
                            push(p, "spooled file of a finished ingest");
                        }
                    }
                    "listings" => {
                        if age(&p) > LISTING_MAX_SECS {
                            push(p, "listing too old to be used");
                        }
                    }
                    _ if name.ends_with(".part") || name.ends_with(".part.json") => {
                        if age(&p) > part_max_age_secs {
                            push(p, "partial download nobody resumed");
                        }
                    }
                    _ if name.ends_with(".json")
                        && !p.with_extension("").is_file()
                        && age(&p) > BUSY_SECS =>
                    {
                        push(p, "sidecar without its download");
                    }
                    _ => {}
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

/// Seconds since the epoch of a `YYYY-MM-DDTHH:MM:SSZ` string.
pub fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mms = format!("{} {time}", date.replace('-', "/"));
    nemlake_core::nemtime::parse_local_seconds(mms.as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `bytes` to `path` and backdates it by `age_secs`.
    fn aged(path: &Path, bytes: &[u8], age_secs: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn the_archive_mirrors_the_url() {
        let c = DiskCache::at("/home/cache", "/home/archive");
        assert_eq!(c.root(), Path::new("/home/cache"));
        assert_eq!(c.archive_root(), Path::new("/home/archive"));
        assert_eq!(
            c.path_for("https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_202401010000.zip"),
            Path::new("/home/archive/www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_202401010000.zip")
        );
        // The # of the newer file names is escaped in a URL and real on disk.
        assert_eq!(
            c.path_for("https://h/DATA/PUBLIC_ARCHIVE%23REGION%23FILE01%23202608010000.zip"),
            Path::new("/home/archive/h/DATA/PUBLIC_ARCHIVE#REGION#FILE01#202608010000.zip")
        );
        // A member of a remote zip sits under the zip's own path.
        assert_eq!(
            c.path_for(
                "https://h/2012/MMSDM_2012_01.zip#01/DATA/PUBLIC_DVD_REGION_201201010000.zip"
            ),
            Path::new(
                "/home/archive/h/2012/MMSDM_2012_01.zip.members/01/DATA/PUBLIC_DVD_REGION_201201010000.zip"
            )
        );
        // Two URLs never share a path, and no URL climbs out of the archive.
        assert_ne!(
            c.path_for("https://h/b/a.zip"),
            c.path_for("https://h/a.zip")
        );
        assert_ne!(
            c.path_for("https://h/a.zip?x=1"),
            c.path_for("https://h/a.zip?x=2")
        );
        assert_ne!(
            c.path_for("https://h/a.zip?x=1"),
            c.path_for("https://h/a.zip")
        );
        assert_eq!(
            c.path_for("https://h/../../etc/passwd"),
            Path::new("/home/archive/h/_/_/etc/passwd")
        );
        assert_eq!(
            c.path_for("https://h:8080/a%2Fb/c%3Ad.zip"),
            Path::new("/home/archive/h_8080/a_b/c_d.zip")
        );
        assert_eq!(c.path_for("https://h/"), Path::new("/home/archive/h"));
        assert_eq!(c.path_for(""), Path::new("/home/archive/_"));
        assert_eq!(percent_decode("a%20b%2"), "a b%2");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(segment("a b"), "a b");
    }

    #[test]
    fn lookup_needs_file_sidecar_url_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path());
        assert_eq!(c.archive_root(), dir.path().join("archive"));
        let url = "https://h/a.zip";
        assert_eq!(c.lookup(url).unwrap(), None);
        let p = c.path_for(url);
        assert!(p.starts_with(dir.path().join("archive")));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"hello").unwrap();
        assert_eq!(c.lookup(url).unwrap(), None, "no sidecar");
        let sc = Sidecar {
            fetched_at: "2024-01-01T00:00:00Z".into(),
            ..Sidecar::bare(url, 5, sha256_hex(b"hello"))
        };
        DiskCache::write_sidecar(&p, &sc).unwrap();
        let blob = c.lookup(url).unwrap().unwrap();
        assert_eq!(blob.sidecar, sc);
        c.verify(&blob).unwrap();
        std::fs::write(&p, b"hellO").unwrap();
        assert!(matches!(
            c.verify(&blob),
            Err(FetchError::Sha256Mismatch { .. })
        ));
        std::fs::write(&p, b"hello world").unwrap();
        assert_eq!(c.lookup(url).unwrap(), None, "size differs");
        std::fs::write(DiskCache::sidecar_path(&p), b"{not json").unwrap();
        assert!(matches!(
            c.lookup(url),
            Err(FetchError::SidecarCorrupt { .. })
        ));
        assert_eq!(
            DiskCache::part_path(Path::new("/x/a.zip")),
            PathBuf::from("/x/a.zip.part")
        );
        assert_eq!(hex(&[0, 255]), "00ff");
    }

    #[test]
    fn a_sidecar_records_the_request_and_reads_back() {
        let sc = Sidecar {
            etag: Some("\"e\"".into()),
            request: Some(Requested {
                by: "fetch nem REGION 2012-01..2012-01".into(),
                tool: "nemlake/0.1.0".into(),
                user_agent: Some("nemlake/0.1.0".into()),
                source: "nem-mmsdm".into(),
                table: Some("REGION".into()),
                period: Some("2012-01".into()),
                started_at: "2026-10-03T10:00:00Z".into(),
                request_url: "https://h/2012/MMSDM_2012_01.zip".into(),
                range: Some("bytes=100-299".into()),
                http_status: 206,
                attempts: 2,
                resumed_from: None,
                member: Some(MemberOf {
                    name: "01/DATA/PUBLIC_DVD_REGION_201201010000.zip".into(),
                    header_offset: 100,
                    compressed_size: 150,
                    method: 8,
                    crc32: "0a1b2c3d".into(),
                }),
                listed_size: Some(1_912_843_433),
                listed_modified: Some("2/21/2012 3:10 PM".into()),
            }),
            supersedes: Some(Superseded {
                sha256: "abc".into(),
                fetched_at: "2025-01-01T00:00:00Z".into(),
                kept_as: "x.zip.abc.superseded".into(),
            }),
            ..Sidecar::bare(
                "https://h/2012/MMSDM_2012_01.zip#01/DATA/x.zip",
                3,
                "s".into(),
            )
        };
        let text = serde_json::to_string_pretty(&sc).unwrap();
        for key in [
            "\"by\"",
            "\"started_at\"",
            "\"request_url\"",
            "\"range\"",
            "\"member\"",
            "\"supersedes\"",
        ] {
            assert!(text.contains(key), "{key} in {text}");
        }
        assert_eq!(serde_json::from_str::<Sidecar>(&text).unwrap(), sc);
        // A sidecar written before requests were recorded still reads.
        let old = r#"{"url":"u","size":1,"sha256":"s","fetched_at":"2024-01-01T00:00:00Z"}"#;
        let read: Sidecar = serde_json::from_str(old).unwrap();
        assert_eq!(read.request, None);
        assert_eq!(read.supersedes, None);
        assert!(!serde_json::to_string(&read).unwrap().contains("request"));
    }

    #[test]
    fn a_changed_file_is_set_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path());
        let url = "https://h/a.zip";
        let p = c.path_for(url);
        // Nothing there, or a file with no sidecar: nothing to set aside.
        assert_eq!(DiskCache::set_aside(&p, "new").unwrap(), None);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"old bytes").unwrap();
        assert_eq!(DiskCache::set_aside(&p, "new").unwrap(), None);
        let old = Sidecar {
            fetched_at: "2024-01-01T00:00:00Z".into(),
            ..Sidecar::bare(url, 9, sha256_hex(b"old bytes"))
        };
        DiskCache::write_sidecar(&p, &old).unwrap();
        // The same bytes again: nothing to keep.
        assert_eq!(DiskCache::set_aside(&p, &old.sha256).unwrap(), None);
        assert!(p.is_file());
        // Other bytes: the old file and its sidecar move aside under the old hash.
        let s = DiskCache::set_aside(&p, "another hash").unwrap().unwrap();
        assert_eq!(s.sha256, old.sha256);
        assert_eq!(s.fetched_at, "2024-01-01T00:00:00Z");
        assert_eq!(s.kept_as, format!("a.zip.{}.superseded", &old.sha256[..12]));
        assert!(!p.exists());
        let kept = p.with_file_name(&s.kept_as);
        assert_eq!(std::fs::read(&kept).unwrap(), b"old bytes");
        assert_eq!(DiskCache::read_sidecar(&kept).unwrap(), Some(old));
        // The file that was set aside is still an entry of the archive.
        assert_eq!(c.entries().unwrap().len(), 1);
    }

    #[test]
    fn leftovers_are_never_archived_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::at(dir.path().join("cache"), dir.path().join("archive"));
        let day = 86_400;
        let src = dir.path().join("archive/h/DATA");
        // A complete download, old: never a leftover.
        aged(&src.join("good.zip"), b"zip", 400 * day);
        aged(&src.join("good.zip.json"), b"{}", 400 * day);
        // A file with no sidecar may be the only copy of something: kept.
        aged(&src.join("bare.zip"), b"1234567", 400 * day);
        // Partial downloads: one abandoned, one still young enough to resume.
        aged(&src.join("old.zip.part"), b"12345", 10 * day);
        aged(&src.join("old.zip.part.json"), b"{}", 10 * day);
        aged(&src.join("new.zip.part"), b"1", 2 * day);
        aged(&src.join("new.zip.part.json"), b"{}", 2 * day);
        // A sidecar whose download is gone, and one a fetch may be writing now.
        aged(&src.join("lost.zip.json"), b"{}", 2 * day);
        aged(&src.join("busy.zip.json"), b"{}", 60);
        // Spooled inner zips: an old one, and one an ingest may still read.
        aged(&dir.path().join("cache/tmp/spool-1.zip"), b"123", 2 * day);
        aged(&dir.path().join("cache/tmp/spool-2.zip"), b"123", 60);
        // Listings: one past any time to live, one current.
        aged(&dir.path().join("cache/listings/old"), b"<pre>", 100 * day);
        aged(
            &dir.path().join("cache/listings/old.json"),
            b"{}",
            100 * day,
        );
        aged(&dir.path().join("cache/listings/new"), b"<pre>", day);

        let names = |max_age: i64| -> Vec<(String, &'static str)> {
            c.leftovers(max_age)
                .into_iter()
                .map(|l| {
                    (
                        l.path
                            .strip_prefix(dir.path())
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                        l.why,
                    )
                })
                .collect()
        };
        let week = names(7 * 86_400);
        let paths: Vec<&str> = week.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            [
                "archive/h/DATA/lost.zip.json",
                "archive/h/DATA/old.zip.part",
                "archive/h/DATA/old.zip.part.json",
                "cache/listings/old",
                "cache/listings/old.json",
                "cache/tmp/spool-1.zip",
            ]
        );
        assert_eq!(week[0].1, "sidecar without its download");
        assert_eq!(week[1].1, "partial download nobody resumed");
        assert_eq!(week[3].1, "listing too old to be used");
        assert_eq!(week[5].1, "spooled file of a finished ingest");
        // A shorter grace takes the younger partial download too.
        let hour = names(3_600);
        assert!(hour.iter().any(|(p, _)| p == "archive/h/DATA/new.zip.part"));
        assert_eq!(hour.len(), week.len() + 2);
        assert_eq!(c.leftovers(7 * 86_400)[1].bytes, 5);
        assert!(
            DiskCache::new(dir.path().join("nowhere"))
                .leftovers(0)
                .is_empty()
        );
    }

    #[test]
    fn listings_and_entries() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path());
        assert_eq!(c.lookup_listing("https://h/", 10).unwrap(), None);
        c.store_listing("https://h/", b"<html>").unwrap();
        assert_eq!(
            c.lookup_listing("https://h/", 10).unwrap(),
            Some(b"<html>".to_vec())
        );
        assert_eq!(c.lookup_listing("https://h/", -1).unwrap(), None, "expired");
        assert_eq!(c.lookup_listing("https://other/", 10).unwrap(), None);
        // A download and a listing: entries() lists only downloads.
        let p = c.path_for("https://h/f.zip");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        DiskCache::write_sidecar(&p, &Sidecar::bare("https://h/f.zip", 1, sha256_hex(b"x")))
            .unwrap();
        let es = c.entries().unwrap();
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].path, p);
        assert_eq!(
            parse_rfc3339_secs("2024-01-01T00:00:00Z"),
            Some(1_704_067_200)
        );
        assert_eq!(parse_rfc3339_secs("junk"), None);
        let bad = c.listing_path("https://bad/");
        std::fs::write(&bad, b"x").unwrap();
        std::fs::write(DiskCache::sidecar_path(&bad), b"nope").unwrap();
        assert!(matches!(
            c.lookup_listing("https://bad/", 10),
            Err(FetchError::SidecarCorrupt { .. })
        ));
    }
}
