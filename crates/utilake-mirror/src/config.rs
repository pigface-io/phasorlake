//! The home, the mirror's records under it, and its config file,
//! `{home}/mirror/config.toml`. Unknown keys are errors, so a typo never
//! falls back to a default quietly.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::diag::{Category, Diagnose, Diagnostic};
use crate::error::MirrorError;
use crate::hosts::Host;
use crate::net::Archive;
use crate::watch::Cadence;

/// Where the mirror keeps things: the archive of downloads and its own
/// records. `--home` or `UTILAKE_MIRROR_HOME`, else `~/.utilake-mirror`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// A home at an explicit path.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The explicit flag, else `~/.utilake-mirror` under the user's home.
    pub fn resolve(flag: Option<&Path>, user_home: Option<&Path>) -> Option<Self> {
        match flag {
            Some(p) => Some(Self::at(p)),
            None => user_home.map(|h| Self::at(h.join(".utilake-mirror"))),
        }
    }

    /// The root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every downloaded file at a path that mirrors its URL, each beside its
    /// sidecar (`docs/archive.md`): what nemlake reads.
    pub fn archive(&self) -> PathBuf {
        self.root.join("archive")
    }

    /// The mirror's own records: crawl, peek, removed duplicates, map.
    pub fn mirror(&self) -> PathBuf {
        self.root.join("mirror")
    }

    /// The config file.
    pub fn config_file(&self) -> PathBuf {
        self.mirror().join("config.toml")
    }

    /// The mirror's records, as a [`Mirror`].
    pub fn records(&self) -> Mirror {
        Mirror::new(self.mirror())
    }
}

/// Where the mirror keeps its own files: `{home}/mirror`.
#[derive(Debug, Clone)]
pub struct Mirror {
    dir: PathBuf,
}

impl Mirror {
    /// The mirror at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The mirror's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The current crawl.
    pub fn crawl_dir(&self) -> PathBuf {
        self.dir.join("crawl")
    }

    /// The crawl before the current one.
    pub fn previous_crawl_dir(&self) -> PathBuf {
        self.dir.join("crawl.prev")
    }

    /// The listing pages of the current crawl, kept as served, at their
    /// URL's path.
    pub fn pages(&self) -> Archive {
        Archive::new(self.crawl_dir().join("listings"))
    }

    /// The record of every link of the current crawl.
    pub fn record_path(&self) -> PathBuf {
        self.crawl_dir().join("remote.ndjson")
    }

    /// Written when the current crawl has read every page.
    pub fn done_path(&self) -> PathBuf {
        self.crawl_dir().join("done")
    }

    /// Where the page of a folder URL is kept.
    pub fn page_path(&self, folder: &Url) -> PathBuf {
        let mut path = self.pages().url_path(folder.as_str());
        path.push("index.html");
        path
    }
}

/// Settings a user may persist. Every field has a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Downloads at a time.
    pub downloads: usize,
    /// Minimum gap between two requests, in milliseconds. AEMO's web
    /// server refuses clients that ask too fast.
    pub request_interval_ms: u64,
    /// The site the roots are on.
    pub base_url: String,
    /// Appended to the `User-Agent` header, e.g. a contact address.
    pub user_agent_suffix: String,
    /// Threads that read zips for the map; 0 means half the cores.
    pub jobs: usize,
    /// Tries per request before it fails.
    pub attempts: u32,
    /// Wait before the second try, in milliseconds; it doubles each try,
    /// up to 60 times this.
    pub backoff_ms: u64,
    /// Seconds a request may wait for the next bytes before it fails.
    pub read_timeout_secs: u64,
    /// Seconds between the counts a run writes for `status`.
    pub progress_every_secs: u64,
    /// Folders a watch reads most often, as paths under `base_url`: those
    /// whose files nemweb rewrites in place several times a day.
    pub watch_fast: Vec<String>,
    /// Seconds between a watch's reads of `watch_fast`.
    pub watch_fast_secs: u64,
    /// Folders a watch reads next most often, as paths under `base_url`.
    pub watch_current: Vec<String>,
    /// Seconds between a watch's reads of `watch_current`.
    pub watch_current_secs: u64,
    /// Seconds between a watch's reads of every other folder.
    pub watch_secs: u64,
    /// The fewest seconds between two reads of a folder found changed.
    pub watch_least_secs: u64,
    /// Further hosts, each with its own roots and clock, read after
    /// nemweb; none by default.
    pub hosts: Vec<HostConfig>,
}

/// A further host: `[[hosts]]` in the config file.
///
/// ```toml
/// [[hosts]]
/// base_url = "https://data.wa.aemo.com.au"
/// listing_offset_secs = 28800
/// roots = ["public/public-data/datafiles/", "public/public-data/manifests/"]
/// watch_current = ["public/public-data/datafiles/"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// The site.
    pub base_url: String,
    /// Seconds east of UTC of the times its listings and zips carry.
    pub listing_offset_secs: i32,
    /// Its own minimum gap between two requests, in milliseconds; the
    /// top-level `request_interval_ms` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_interval_ms: Option<u64>,
    /// The folders mirrored, as paths under `base_url`.
    pub roots: Vec<String>,
    /// Folders a watch reads most often, as paths under `base_url`.
    #[serde(default)]
    pub watch_fast: Vec<String>,
    /// Folders a watch reads next most often, as paths under `base_url`.
    #[serde(default)]
    pub watch_current: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            downloads: 2,
            request_interval_ms: 1000,
            base_url: "https://www.nemweb.com.au".to_owned(),
            user_agent_suffix: String::new(),
            jobs: 0,
            attempts: 5,
            backoff_ms: 500,
            read_timeout_secs: 120,
            progress_every_secs: 5,
            watch_fast: ["VicGas", "STTM", "GBB", "ECGS", "GSH"]
                .iter()
                .map(|f| format!("Reports/CURRENT/{f}/"))
                .collect(),
            watch_fast_secs: 15 * 60,
            watch_current: vec!["Reports/CURRENT/".to_owned()],
            watch_current_secs: 60 * 60,
            watch_secs: 24 * 60 * 60,
            watch_least_secs: 5 * 60,
            hosts: Vec::new(),
        }
    }
}

impl Config {
    /// Loads the file, or the defaults when it does not exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|e| ConfigError::Parse {
            path: path.display().to_string(),
            message: e.message().to_owned(),
        })
    }

    /// Every host read: nemweb, from the top-level keys, then each of
    /// `hosts`; a root that is not a folder URL refuses.
    pub fn hosts(&self) -> Result<Vec<Host>, MirrorError> {
        let mut hosts = vec![Host::nemweb(
            &self.base_url,
            self.request_interval_ms,
            &self.watch_fast,
            &self.watch_current,
        )?];
        for h in &self.hosts {
            hosts.push(Host::from_config(h, self.request_interval_ms)?);
        }
        Ok(hosts)
    }

    /// How often a watch reads each folder, over every host.
    pub fn cadence(&self) -> Result<Cadence, MirrorError> {
        let hosts = self.hosts()?;
        Ok(Cadence {
            fast: hosts.iter().flat_map(|h| h.watch_fast.clone()).collect(),
            fast_every: Duration::from_secs(self.watch_fast_secs),
            current: hosts.iter().flat_map(|h| h.watch_current.clone()).collect(),
            current_every: Duration::from_secs(self.watch_current_secs),
            every: Duration::from_secs(self.watch_secs),
            least: Duration::from_secs(self.watch_least_secs),
        })
    }

    /// The map's threads: `jobs`, or half the cores when it is 0.
    pub fn effective_jobs(&self) -> usize {
        if self.jobs > 0 {
            return self.jobs;
        }
        std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1))
    }
}

/// Why a config file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but could not be read.
    #[error("The config file {path} could not be read.")]
    Read {
        /// The file.
        path: String,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid.
    #[error("The config file {path} is not valid: {message}")]
    Parse {
        /// The file.
        path: String,
        /// TOML's message, which names the key.
        message: String,
    },
}

impl Diagnose for ConfigError {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Read { path, source } => {
                Diagnostic::error("config.read", Category::Config, self.to_string())
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Parse { path, .. } => {
                Diagnostic::error("config.parse", Category::Config, self.to_string())
                    .with_file(path.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_records_live_under_the_home() {
        let h = Home::at("/h");
        let m = h.records();
        assert_eq!(m.dir(), Path::new("/h/mirror"));
        assert_eq!(m.crawl_dir(), Path::new("/h/mirror/crawl"));
        assert_eq!(m.previous_crawl_dir(), Path::new("/h/mirror/crawl.prev"));
        assert_eq!(m.record_path(), Path::new("/h/mirror/crawl/remote.ndjson"));
        assert_eq!(m.done_path(), Path::new("/h/mirror/crawl/done"));
        assert!(
            m.page_path(&Url::parse("https://www.nemweb.com.au/Reports/ARCHIVE/").unwrap())
                .ends_with("listings/www.nemweb.com.au/Reports/ARCHIVE/index.html")
        );
        assert_eq!(m.pages().root(), Path::new("/h/mirror/crawl/listings"));
    }

    #[test]
    fn home_layout_and_config() {
        let h = Home::resolve(None, Some(Path::new("/u"))).unwrap();
        assert_eq!(h.root(), Path::new("/u/.utilake-mirror"));
        assert!(Home::resolve(None, None).is_none());
        let h = Home::resolve(Some(Path::new("/m")), None).unwrap();
        assert_eq!(h.archive(), Path::new("/m/archive"));
        assert_eq!(h.config_file(), Path::new("/m/mirror/config.toml"));

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        assert_eq!(Config::load(&p).unwrap(), Config::default());
        std::fs::write(&p, "request_interval_ms = 750\njobs = 3\n").unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(
            (c.request_interval_ms, c.downloads, c.effective_jobs()),
            (750, 2, 3)
        );
        assert!(Config::default().effective_jobs() >= 1);
        std::fs::write(&p, "requests = 1\n").unwrap();
        let d = Config::load(&p).unwrap_err().diagnostic();
        assert_eq!(d.code, "config.parse");
        assert!(d.message.contains("requests"), "{}", d.message);
        let d = Config::load(dir.path()).unwrap_err().diagnostic();
        assert_eq!(d.code, "config.read");
    }

    #[test]
    fn hosts_are_nemweb_then_the_configured_ones() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            r#"
request_interval_ms = 400

[[hosts]]
base_url = "https://data.wa.aemo.com.au/"
listing_offset_secs = 28800
roots = ["public/public-data/datafiles/"]
watch_current = ["public/market-data/wemde/"]
"#,
        )
        .unwrap();
        let c = Config::load(&p).unwrap();
        let hosts = c.hosts().unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].base_url, "https://www.nemweb.com.au");
        assert_eq!(hosts[1].base_url, "https://data.wa.aemo.com.au");
        assert_eq!(hosts[1].offset_secs, 28_800);
        assert_eq!(hosts[1].request_interval, Duration::from_millis(400));
        let cadence = c.cadence().unwrap();
        assert_eq!(cadence.fast.len(), 5);
        assert_eq!(
            cadence.current,
            [
                "https://www.nemweb.com.au/Reports/CURRENT/",
                "https://data.wa.aemo.com.au/public/market-data/wemde/"
            ]
        );
        // A host without its clock or roots is a config error, named.
        std::fs::write(&p, "[[hosts]]\nbase_url = \"https://h\"\n").unwrap();
        let d = Config::load(&p).unwrap_err().diagnostic();
        assert_eq!(d.code, "config.parse");
        assert!(d.message.contains("listing_offset_secs"), "{}", d.message);
        // A root that is not a folder fails when the hosts are resolved.
        std::fs::write(
            &p,
            "[[hosts]]\nbase_url = \"https://h\"\nlisting_offset_secs = 0\nroots = [\"a.csv\"]\n",
        )
        .unwrap();
        let c = Config::load(&p).unwrap();
        assert!(matches!(c.hosts().unwrap_err(), MirrorError::Root { .. }));
        assert!(c.cadence().is_err());
        assert_eq!(Config::default().hosts().unwrap().len(), 1);
        let bad_base = Config {
            base_url: "nemweb".to_owned(),
            ..Config::default()
        };
        assert!(matches!(
            bad_base.hosts().unwrap_err(),
            MirrorError::Root { .. }
        ));
    }

    #[test]
    fn a_watch_cadence_is_under_the_base_url() {
        let c = Config {
            base_url: "https://h/".to_owned(),
            watch_fast: vec!["/R/C/G/".to_owned()],
            ..Config::default()
        }
        .cadence()
        .unwrap();
        assert_eq!(c.fast, ["https://h/R/C/G/"]);
        assert_eq!(c.current, ["https://h/Reports/CURRENT/"]);
        assert_eq!(
            (c.fast_every, c.current_every, c.every, c.least),
            (
                Duration::from_secs(900),
                Duration::from_secs(3600),
                Duration::from_secs(86_400),
                Duration::from_secs(300)
            )
        );
    }
}
