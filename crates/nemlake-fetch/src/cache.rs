//! The on-disk download cache: `{root}/{source}/{hash}-{filename}` plus a
//! JSON sidecar with what the server said and what the bytes hash to.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::FetchError;

/// What is known about a cached download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// The URL.
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
    /// When it was fetched, RFC 3339.
    pub fetched_at: String,
    /// Whether the server advertised byte ranges.
    #[serde(default)]
    pub accept_ranges: bool,
}

/// A complete cached download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBlob {
    /// The bytes.
    pub path: PathBuf,
    /// What is known about them.
    pub sidecar: Sidecar,
}

/// The cache directory.
#[derive(Debug, Clone)]
pub struct DiskCache {
    root: PathBuf,
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

impl DiskCache {
    /// A cache rooted at `root` (the `cache/` directory under the home).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a URL's bytes live: a hash of the URL keeps distinct URLs with
    /// the same file name apart, the file name keeps the directory browsable.
    pub fn path_for(&self, source: &str, url: &str, file_name: &str) -> PathBuf {
        let h = sha256_hex(url.as_bytes());
        self.root
            .join(source)
            .join(format!("{}-{}", &h[..16], sanitize(file_name)))
    }

    /// The sidecar of a cached path.
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

    /// A complete cached download for a URL, if both file and sidecar exist
    /// and the sidecar names the same URL and size.
    pub fn lookup(
        &self,
        source: &str,
        url: &str,
        file_name: &str,
    ) -> Result<Option<CachedBlob>, FetchError> {
        let path = self.path_for(source, url, file_name);
        let sc = Self::sidecar_path(&path);
        if !path.is_file() || !sc.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&sc).map_err(|source| FetchError::CacheIo {
            path: sc.display().to_string(),
            source,
        })?;
        let sidecar: Sidecar =
            serde_json::from_str(&text).map_err(|e| FetchError::SidecarCorrupt {
                path: sc.display().to_string(),
                message: e.to_string(),
            })?;
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

    /// Hashes a cached file and compares it with its sidecar.
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
        let sc = Self::sidecar_path(&path);
        if !path.is_file() || !sc.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&sc).map_err(|source| FetchError::CacheIo {
            path: sc.display().to_string(),
            source,
        })?;
        let sidecar: Sidecar =
            serde_json::from_str(&text).map_err(|e| FetchError::SidecarCorrupt {
                path: sc.display().to_string(),
                message: e.to_string(),
            })?;
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
            &Sidecar {
                url: url.to_owned(),
                etag: None,
                last_modified: None,
                size: body.len() as u64,
                sha256: sha256_hex(body),
                fetched_at: nemlake_core::clock::now_utc_rfc3339(),
                accept_ranges: false,
            },
        )
    }

    /// Every cached download under the root, as (path, sidecar).
    pub fn entries(&self) -> Result<Vec<CachedBlob>, FetchError> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n != "listings" && n != "tmp") {
                        stack.push(p);
                    }
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

    /// Removes a cached download and its sidecar.
    pub fn remove(&self, blob: &CachedBlob) -> Result<(), FetchError> {
        for p in [blob.path.clone(), Self::sidecar_path(&blob.path)] {
            if p.exists() {
                std::fs::remove_file(&p).map_err(|source| FetchError::CacheIo {
                    path: p.display().to_string(),
                    source,
                })?;
            }
        }
        Ok(())
    }
}

/// A file in the cache that serves no purpose any more.
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
    /// Files that no complete download accounts for: partial downloads
    /// nobody resumed within `part_max_age_secs`, spooled inner zips of
    /// ingests that are long over, downloads without a sidecar, sidecars
    /// without a download, and listings past any use.
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
        let mut stack = vec![(self.root.clone(), "")];
        while let Some((dir, kind)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for p in rd.filter_map(Result::ok).map(|e| e.path()) {
                if p.is_dir() {
                    let kind = match (kind, p.file_name().and_then(|n| n.to_str())) {
                        ("", Some("listings")) => "listings",
                        ("", Some("tmp")) => "tmp",
                        (k, _) => k,
                    };
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
                    _ if name.ends_with(".json") => {
                        if !p.with_extension("").is_file() && age(&p) > BUSY_SECS {
                            push(p, "sidecar without its download");
                        }
                    }
                    _ => {
                        if !Self::sidecar_path(&p).is_file() && age(&p) > BUSY_SECS {
                            push(p, "download without its sidecar");
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
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
    fn leftovers_are_what_no_complete_download_accounts_for() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path());
        let day = 86_400;
        let src = dir.path().join("nem-mmsdm");
        // A complete download, old: never a leftover.
        aged(&src.join("aa-good.zip"), b"zip", 400 * day);
        aged(&src.join("aa-good.zip.json"), b"{}", 400 * day);
        // Partial downloads: one abandoned, one still young enough to resume.
        aged(&src.join("bb-old.zip.part"), b"12345", 10 * day);
        aged(&src.join("bb-old.zip.part.json"), b"{}", 10 * day);
        aged(&src.join("cc-new.zip.part"), b"1", 2 * day);
        aged(&src.join("cc-new.zip.part.json"), b"{}", 2 * day);
        // A download whose sidecar is gone, and a sidecar whose download is gone.
        aged(&src.join("dd-bare.zip"), b"1234567", 2 * day);
        aged(&src.join("ee-lost.zip.json"), b"{}", 2 * day);
        // The same two shapes, minutes old: a fetch may be between its two writes.
        aged(&src.join("ff-busy.zip"), b"1", 60);
        aged(&src.join("gg-busy.zip.json"), b"{}", 60);
        // Spooled inner zips: an old one, and one an ingest may still read.
        aged(&dir.path().join("tmp/spool-1.zip"), b"123", 2 * day);
        aged(&dir.path().join("tmp/spool-2.zip"), b"123", 60);
        // Listings: one past any time-to-live, one current.
        aged(&dir.path().join("listings/old"), b"<pre>", 100 * day);
        aged(&dir.path().join("listings/old.json"), b"{}", 100 * day);
        aged(&dir.path().join("listings/new"), b"<pre>", day);

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
        let week = names(7 * i64::from(day as u32));
        let paths: Vec<&str> = week.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            [
                "listings/old",
                "listings/old.json",
                "nem-mmsdm/bb-old.zip.part",
                "nem-mmsdm/bb-old.zip.part.json",
                "nem-mmsdm/dd-bare.zip",
                "nem-mmsdm/ee-lost.zip.json",
                "tmp/spool-1.zip",
            ]
        );
        assert_eq!(week[2].1, "partial download nobody resumed");
        assert_eq!(week[4].1, "download without its sidecar");
        assert_eq!(week[5].1, "sidecar without its download");
        assert_eq!(week[6].1, "spooled file of a finished ingest");
        assert_eq!(week[0].1, "listing too old to be used");
        // A shorter grace takes the younger partial download too.
        let hour = names(3_600);
        assert!(hour.iter().any(|(p, _)| p == "nem-mmsdm/cc-new.zip.part"));
        assert!(
            hour.iter()
                .any(|(p, _)| p == "nem-mmsdm/cc-new.zip.part.json")
        );
        assert_eq!(hour.len(), week.len() + 2);
        // Sizes are reported, and an empty cache has nothing.
        let sized = c.leftovers(7 * i64::from(day as u32));
        assert_eq!(sized[2].bytes, 5);
        assert_eq!(sized[4].bytes, 7);
        assert!(
            DiskCache::new(dir.path().join("nowhere"))
                .leftovers(0)
                .is_empty()
        );
    }

    #[test]
    fn paths_and_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path());
        assert_eq!(c.root(), dir.path());
        let p = c.path_for("nem-mmsdm", "https://h/a.zip", "a b.zip");
        assert!(p.starts_with(dir.path().join("nem-mmsdm")));
        assert!(
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("-a_b.zip")
        );
        assert_ne!(
            c.path_for("nem-mmsdm", "https://h/b/a.zip", "a.zip"),
            c.path_for("nem-mmsdm", "https://h/a.zip", "a.zip")
        );
        assert_eq!(
            c.lookup("nem-mmsdm", "https://h/a.zip", "a.zip").unwrap(),
            None
        );
        let p = c.path_for("nem-mmsdm", "https://h/a.zip", "a.zip");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"hello").unwrap();
        assert_eq!(
            c.lookup("nem-mmsdm", "https://h/a.zip", "a.zip").unwrap(),
            None,
            "no sidecar"
        );
        let sc = Sidecar {
            url: "https://h/a.zip".into(),
            etag: None,
            last_modified: None,
            size: 5,
            sha256: sha256_hex(b"hello"),
            fetched_at: "2024-01-01T00:00:00Z".into(),
            accept_ranges: false,
        };
        DiskCache::write_sidecar(&p, &sc).unwrap();
        let blob = c
            .lookup("nem-mmsdm", "https://h/a.zip", "a.zip")
            .unwrap()
            .unwrap();
        assert_eq!(blob.sidecar, sc);
        c.verify(&blob).unwrap();
        std::fs::write(&p, b"hellO").unwrap();
        assert!(matches!(
            c.verify(&blob),
            Err(FetchError::Sha256Mismatch { .. })
        ));
        std::fs::write(&p, b"hello world").unwrap();
        assert_eq!(
            c.lookup("nem-mmsdm", "https://h/a.zip", "a.zip").unwrap(),
            None,
            "size differs"
        );
        std::fs::write(DiskCache::sidecar_path(&p), b"{not json").unwrap();
        assert!(matches!(
            c.lookup("nem-mmsdm", "https://h/a.zip", "a.zip"),
            Err(FetchError::SidecarCorrupt { .. })
        ));
        assert_eq!(
            DiskCache::part_path(Path::new("/x/a.zip")),
            PathBuf::from("/x/a.zip.part")
        );
        assert_eq!(hex(&[0, 255]), "00ff");
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
        let p = c.path_for("s", "https://h/f.zip", "f.zip");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        DiskCache::write_sidecar(
            &p,
            &Sidecar {
                url: "https://h/f.zip".into(),
                etag: None,
                last_modified: None,
                size: 1,
                sha256: sha256_hex(b"x"),
                fetched_at: "2024-01-01T00:00:00Z".into(),
                accept_ranges: true,
            },
        )
        .unwrap();
        let es = c.entries().unwrap();
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].path, p);
        c.remove(&es[0]).unwrap();
        assert!(c.entries().unwrap().is_empty());
        c.remove(&CachedBlob {
            path: dir.path().join("gone"),
            sidecar: es[0].sidecar.clone(),
        })
        .unwrap();
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
