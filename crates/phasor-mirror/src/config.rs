//! The home, the mirror's records under it, and `{home}/config.toml`, whose
//! unknown keys are errors.
//!
//! ```text
//! {home}/config.toml               settings
//! {home}/archive/                  the bytes and their provenance records
//! {home}/state/                    the mirror's records ([`Mirror`])
//!   lock, crawl.json, peeks.ndjson, removals.ndjson, pruned.ndjson
//!   folders/{host}/{folder}/       listing.ndjson, pages/{LISTING}.html, events.ndjson
//!   cache/                         held.ndjson, status.json, catalog.ndjson, catalog.txt
//!   tmp/
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::MirrorError;
use crate::hosts::Host;
use crate::keep::Month;
use crate::net::Archive;
use crate::watch::Cadence;
pub use phasor_core::config::ConfigError;

/// Where the mirror keeps things: `--home` or `PHASOR_MIRROR_HOME`. There is
/// no default: an archive is too large and too long-lived to land where a
/// forgotten flag puts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// A home at an explicit path.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The explicit flag or variable; `None` when neither names a home.
    pub fn resolve(flag: Option<&Path>) -> Option<Self> {
        flag.map(Self::at)
    }

    /// The root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every downloaded file at a path that mirrors its URL, beside its sidecar.
    pub fn archive(&self) -> PathBuf {
        self.root.join("archive")
    }

    /// The mirror's own records: crawl, folders, peeks, removals, caches.
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    /// The config file.
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// The mirror's records, as a [`Mirror`] that keeps everything.
    pub fn records(&self) -> Mirror {
        Mirror::new(self.state())
    }
}

/// The mirror's own records, `{home}/state`; what is under `cache/` is safe to delete.
#[derive(Debug, Clone)]
pub struct Mirror {
    dir: PathBuf,
    keep_from: Option<Month>,
}

impl Mirror {
    /// The mirror at `dir`, keeping every month.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            keep_from: None,
        }
    }

    /// The same records, keeping nothing for a month before `from` ([`crate::keep`]).
    pub fn keeping(self, from: Option<Month>) -> Self {
        Self {
            keep_from: from,
            ..self
        }
    }

    /// The month before which nothing is kept, if any.
    pub fn keep_from(&self) -> Option<Month> {
        self.keep_from
    }

    /// What `prune` deleted, one line a folder or file.
    pub fn pruned_path(&self) -> PathBuf {
        self.dir.join("pruned.ndjson")
    }

    /// The mirror's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Written when a crawl has read every page: when it ended.
    pub fn done_path(&self) -> PathBuf {
        self.dir.join("crawl.json")
    }

    /// One folder per local folder of the archive: its listing, pages and events.
    pub fn folders(&self) -> Archive {
        Archive::new(self.dir.join("folders"))
    }

    /// The folder of the listing URL `folder`.
    pub fn folder_dir(&self, folder: &str) -> PathBuf {
        self.folders().path_for(folder)
    }

    /// What every listing of `folder`'s local folder showed when last read.
    pub fn listing_path(&self, folder: &str) -> PathBuf {
        self.folder_dir(folder).join("listing.ndjson")
    }

    /// Where the page of a folder URL is kept, named for its listing or `index`.
    pub fn page_path(&self, folder: &Url) -> PathBuf {
        let probe = format!("{}_", folder.as_str());
        let name = phasor_provenance::layout::listing(&probe).unwrap_or_else(|| "index".into());
        self.folder_dir(folder.as_str())
            .join("pages")
            .join(format!("{name}.html"))
    }

    /// What the mirror derives and can derive again: safe to delete.
    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join("cache")
    }

    /// Scratch space.
    pub fn tmp_dir(&self) -> PathBuf {
        self.dir.join("tmp")
    }
}

/// Settings a user may persist. Every field has a default; every host is
/// named (`[[hosts]]`, [`HostConfig`]), nemweb included: a home with no host
/// reads nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Downloads at a time over every host; a host may allow itself fewer.
    pub downloads: usize,
    /// Minimum gap between two requests to one host, in milliseconds, for a
    /// host that sets none of its own.
    pub request_interval_ms: u64,
    /// Appended to the `User-Agent` header, e.g. a contact address.
    pub user_agent_suffix: String,
    /// Threads that read zips for the map; 0 means half the cores.
    pub jobs: usize,
    /// Tries per request before it fails, for a host that sets none of its own.
    pub attempts: u32,
    /// Wait before the second try, in milliseconds; doubles each try, up to 60 times this.
    pub backoff_ms: u64,
    /// Seconds a request may wait for the next bytes before it fails.
    pub read_timeout_secs: u64,
    /// Seconds between the counts a run writes for `status`.
    pub progress_every_secs: u64,
    /// Seconds between a watch's reads of a folder no rule names.
    pub watch_secs: u64,
    /// The fewest seconds between two reads of a folder found changed.
    pub watch_least_secs: u64,
    /// Whether each host's `robots.txt` is read before the host is, a root
    /// it disallows refused and its crawl delay honoured. Off: the servers
    /// read publish their files for programs, and some carry a blanket
    /// `Disallow: /` meant for search engines.
    pub robots: bool,
    /// The hosts read, each with its own roots, clock, pace and schedule.
    pub hosts: Vec<HostConfig>,
    /// Keep nothing whose data is for a month before this (`"2020-01"`);
    /// every month when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_from: Option<Month>,
}

/// The keys an older config file had at the top level, now said per host.
const RETIRED_KEYS: [&str; 5] = [
    "base_url",
    "watch_fast",
    "watch_fast_secs",
    "watch_current",
    "watch_current_secs",
];

/// How often a watch reads the folders under one path of a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// The folder, as a path under the host's `base_url`; every folder below it too.
    pub under: String,
    /// Seconds between reads of those folders.
    pub every_secs: u64,
}

/// How a host lists a folder ([`crate::listers`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ListingKind {
    /// An IIS directory page (AEMO's hosts).
    #[default]
    Iis,
    /// An Azure Blob Storage container listing, a page of blobs and prefixes
    /// at a time (the Electricity Authority's `emidatasets` account); its
    /// times are HTTP dates, so `listing_offset_secs` is 0.
    AzureBlob,
    /// A THREDDS data server (NCI): a folder is `catalog.xml` under
    /// `/thredds/catalog/`, its files served under `/thredds/fileServer/`;
    /// roots are given as `fileServer` paths. Times are UTC.
    Thredds,
    /// A fixed list of files (`documents`), each asked for by HEAD: a folder
    /// is the files it holds. Times are HTTP dates, so `listing_offset_secs`
    /// is 0; `roots` may be left out, they are the files' folders.
    Documents,
    /// An FTP server (`base_url = "ftp://…"`), listed by `MLSD` (else the
    /// names, each asked for its time and size); times are UTC, so
    /// `listing_offset_secs` is 0. `only` names the files a folder's listing
    /// keeps, for folders of thousands (the Bureau's `anon/gen/fwo/`).
    Ftp,
}

/// One host: `[[hosts]]` in the config file. nemweb is one like any other
/// (`phasor-mirror init` writes it).
///
/// ```toml
/// [[hosts]]
/// id = "nemweb"
/// base_url = "https://www.nemweb.com.au"
/// listing_offset_secs = 36000
/// roots = ["Data_Archive/", "Reports/ARCHIVE/", "Reports/CURRENT/"]
/// request_interval_ms = 300
/// downloads = 2
/// [[hosts.schedule]]
/// under = "Reports/CURRENT/STTM/"
/// every_secs = 900
/// [[hosts.schedule]]
/// under = "Reports/CURRENT/"
/// every_secs = 3600
///
/// [[hosts]]
/// base_url = "https://emidatasets.blob.core.windows.net"
/// listing = "azure-blob"
/// listing_offset_secs = 0
/// roots = ["publicdata/Datasets/Wholesale/DispatchAndPricing/"]
/// every_secs = 3600
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// A short name for logs and locks; the site's host name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The site.
    pub base_url: String,
    /// How it lists a folder: an IIS page unless said otherwise.
    #[serde(default)]
    pub listing: ListingKind,
    /// Seconds east of UTC of the times its listings and zips carry.
    pub listing_offset_secs: i32,
    /// Its own minimum gap between requests, in milliseconds; the top-level one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_interval_ms: Option<u64>,
    /// Its own downloads at a time, under the top-level count; that count when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<usize>,
    /// Its own tries per request; the top-level count when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    /// At most this many of its files downloaded in one round of a watch;
    /// the rest wait for the next round, so a backlog here never holds
    /// another host's due folders back. Every file when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<usize>,
    /// The folders mirrored, as paths under `base_url`.
    #[serde(default)]
    pub roots: Vec<String>,
    /// For a host of documents (`listing = "documents"`): the files watched,
    /// as paths under `base_url`, each kept by version as it is rewritten.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<String>,
    /// Patterns of the file names a folder's listing keeps (`*` any run,
    /// `?` one character); a file matching none is neither recorded nor
    /// downloaded. Every file when absent. Folders are always kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    /// Seconds between a watch's reads of this host's folders that no rule
    /// names; the top-level `watch_secs` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_secs: Option<u64>,
    /// How often a watch reads the folders under each path; the longest
    /// matching path wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schedule: Vec<Rule>,
    /// Folders a watch reads every 15 minutes (an older spelling of a rule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch_fast: Vec<String>,
    /// Folders a watch reads every hour (an older spelling of a rule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch_current: Vec<String>,
}

impl HostConfig {
    /// nemweb as the mirror has always read it: the three roots in AEST, the
    /// gas folders every 15 minutes, the rest of `Reports/CURRENT/` hourly.
    pub fn nemweb() -> Self {
        let fast = ["VicGas", "STTM", "GBB", "ECGS", "GSH"]
            .iter()
            .map(|f| Rule {
                under: format!("Reports/CURRENT/{f}/"),
                every_secs: 15 * 60,
            });
        Self {
            id: Some("nemweb".to_owned()),
            base_url: "https://www.nemweb.com.au".to_owned(),
            listing: ListingKind::Iis,
            listing_offset_secs: crate::stamp::AEST,
            request_interval_ms: None,
            downloads: None,
            attempts: None,
            batch: None,
            roots: ["Data_Archive/", "Reports/ARCHIVE/", "Reports/CURRENT/"]
                .iter()
                .map(|r| (*r).to_owned())
                .collect(),
            documents: Vec::new(),
            only: Vec::new(),
            every_secs: None,
            schedule: fast
                .chain(std::iter::once(Rule {
                    under: "Reports/CURRENT/".to_owned(),
                    every_secs: 60 * 60,
                }))
                .collect(),
            watch_fast: Vec::new(),
            watch_current: Vec::new(),
        }
    }

    /// The host's name: `id`, else the site's host name (`www.nemweb.com.au`).
    pub fn name(&self) -> String {
        self.id.clone().unwrap_or_else(|| {
            Url::parse(&self.base_url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_else(|| self.base_url.clone())
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            downloads: 2,
            request_interval_ms: 600,
            user_agent_suffix: String::new(),
            jobs: 0,
            attempts: 5,
            backoff_ms: 500,
            read_timeout_secs: 120,
            progress_every_secs: 5,
            watch_secs: 24 * 60 * 60,
            watch_least_secs: 5 * 60,
            robots: false,
            hosts: Vec::new(),
            keep_from: None,
        }
    }
}

impl Config {
    /// Loads the file, or the defaults when it does not exist; the message
    /// of a key that moved says where it went.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        phasor_core::config::load_toml(path).map_err(|e| match e {
            ConfigError::Parse {
                path,
                message,
                line,
            } => ConfigError::Parse {
                path,
                message: retired_key_message(message),
                line,
            },
            e => e,
        })
    }

    /// The config in `text`; the message of a key that moved says where it went.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| retired_key_message(e.message().to_owned()))
    }

    /// The config as a file's text.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("a Config always serialises")
    }

    /// Every host read, in the config's order; a root that is not a folder URL
    /// refuses. No host is read that the config does not name.
    pub fn hosts(&self) -> Result<Vec<Host>, MirrorError> {
        self.hosts
            .iter()
            .map(|h| Host::from_config(h, self.request_interval_ms))
            .collect()
    }

    /// How often a watch reads each folder, over every host: each host's rules
    /// by URL prefix (the longest wins), its `every_secs` for the rest of its
    /// roots, and `watch_secs` for any folder no host names.
    pub fn cadence(&self) -> Result<Cadence, MirrorError> {
        let hosts = self.hosts()?;
        let mut rules: Vec<(String, Duration)> = Vec::new();
        for h in &hosts {
            rules.extend(h.rules.iter().cloned());
            if let Some(every) = h.every {
                rules.extend(h.roots.iter().map(|r| (r.as_str().to_owned(), every)));
            }
        }
        Ok(Cadence {
            rules,
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

/// TOML's `message` with, when it names a key that moved, where it went.
fn retired_key_message(message: String) -> String {
    match RETIRED_KEYS
        .iter()
        .find(|k| message.contains(&format!("`{k}`")))
    {
        Some(key) => format!(
            "{message}. The key `{key}` moved into a host: nemweb is a [[hosts]] entry like any other (`phasor-mirror init` writes it), and a watch's cadence is its [[hosts.schedule]] rules."
        ),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Diagnose as _;

    #[test]
    fn the_records_live_under_the_home() {
        let h = Home::at("/h");
        let m = h.records();
        assert_eq!(m.dir(), Path::new("/h/state"));
        assert_eq!(m.done_path(), Path::new("/h/state/crawl.json"));
        assert_eq!(m.cache_dir(), Path::new("/h/state/cache"));
        assert_eq!(m.tmp_dir(), Path::new("/h/state/tmp"));
        let page = |u: &str| m.page_path(&Url::parse(u).unwrap());
        assert_eq!(
            page("https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/"),
            Path::new(
                "/h/state/folders/www.nemweb.com.au/Reports/Dispatch_SCADA/pages/CURRENT.html"
            )
        );
        assert_eq!(
            page("https://www.nemweb.com.au/Reports/ARCHIVE/"),
            Path::new("/h/state/folders/www.nemweb.com.au/Reports/ARCHIVE/pages/ARCHIVE.html")
        );
        assert_eq!(
            page("https://www.nemweb.com.au/Data_Archive/"),
            Path::new("/h/state/folders/www.nemweb.com.au/Data_Archive/pages/index.html")
        );
        assert_eq!(
            m.listing_path("https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/"),
            Path::new("/h/state/folders/www.nemweb.com.au/Reports/Dispatch_SCADA/listing.ndjson")
        );
    }

    #[test]
    fn home_layout_and_config() {
        assert!(Home::resolve(None).is_none(), "no default home");
        let h = Home::resolve(Some(Path::new("/m"))).unwrap();
        assert_eq!(h.root(), Path::new("/m"));
        assert_eq!(h.archive(), Path::new("/m/archive"));
        assert_eq!(h.config_file(), Path::new("/m/config.toml"));

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
        // A key that moved into the hosts says so.
        std::fs::write(&p, "watch_fast = [\"Reports/CURRENT/STTM/\"]\n").unwrap();
        let d = Config::load(&p).unwrap_err().diagnostic();
        assert!(
            d.message.contains("`watch_fast` moved into a host"),
            "{}",
            d.message
        );
        // The defaults name no host, and a config round-trips through TOML.
        assert!(Config::default().hosts().unwrap().is_empty());
        assert!(
            !Config::default().robots,
            "robots.txt is read only when asked"
        );
        let c = Config {
            hosts: vec![HostConfig::nemweb()],
            ..Config::default()
        };
        let text = c.to_toml();
        assert_eq!(Config::parse(&text).unwrap(), c);
        assert!(text.contains("[[hosts.schedule]]"), "{text}");
    }

    #[test]
    fn hosts_are_the_configured_ones_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            r#"
request_interval_ms = 400
watch_secs = 100
watch_least_secs = 10

[[hosts]]
id = "nemweb"
base_url = "https://www.nemweb.com.au"
listing_offset_secs = 36000
roots = ["Reports/CURRENT/"]
attempts = 2
[[hosts.schedule]]
under = "Reports/CURRENT/STTM/"
every_secs = 900
[[hosts.schedule]]
under = "Reports/CURRENT/"
every_secs = 3600

[[hosts]]
base_url = "https://data.wa.aemo.com.au/"
listing_offset_secs = 28800
roots = ["public/public-data/datafiles/"]
downloads = 1
watch_current = ["public/market-data/wemde/"]

[[hosts]]
base_url = "https://emidatasets.blob.core.windows.net"
listing = "azure-blob"
listing_offset_secs = 0
roots = ["publicdata/Datasets/Wholesale/DispatchAndPricing/"]
every_secs = 1800
"#,
        )
        .unwrap();
        let c = Config::load(&p).unwrap();
        let hosts = c.hosts().unwrap();
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].id, "nemweb");
        assert_eq!(hosts[0].base_url, "https://www.nemweb.com.au");
        assert_eq!(hosts[0].attempts, Some(2));
        assert_eq!(hosts[1].id, "data.wa.aemo.com.au");
        assert_eq!(hosts[1].base_url, "https://data.wa.aemo.com.au");
        assert_eq!(hosts[1].offset_secs, 28_800);
        assert_eq!(
            hosts[1].listing,
            ListingKind::Iis,
            "an IIS page unless said"
        );
        assert_eq!(hosts[1].request_interval, Duration::from_millis(400));
        assert_eq!(hosts[1].downloads, Some(1));
        assert_eq!(hosts[2].listing, ListingKind::AzureBlob);
        assert_eq!(hosts[2].offset_secs, 0);
        let cadence = c.cadence().unwrap();
        assert_eq!(
            cadence.rules,
            [
                (
                    "https://www.nemweb.com.au/Reports/CURRENT/STTM/".to_owned(),
                    Duration::from_secs(900)
                ),
                (
                    "https://www.nemweb.com.au/Reports/CURRENT/".to_owned(),
                    Duration::from_secs(3600)
                ),
                (
                    "https://data.wa.aemo.com.au/public/market-data/wemde/".to_owned(),
                    Duration::from_secs(3600)
                ),
                (
                    "https://emidatasets.blob.core.windows.net/publicdata/Datasets/Wholesale/DispatchAndPricing/".to_owned(),
                    Duration::from_secs(1800)
                ),
            ]
        );
        assert_eq!(
            (cadence.every, cadence.least),
            (Duration::from_secs(100), Duration::from_secs(10))
        );
        // A listing kind that is not one is a config error.
        std::fs::write(
            &p,
            "[[hosts]]\nbase_url = \"https://h\"\nlisting = \"gopher\"\nlisting_offset_secs = 0\nroots = []\n",
        )
        .unwrap();
        assert!(Config::load(&p).is_err());
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
    }

    #[test]
    fn a_thredds_host_and_a_host_of_documents_read_from_the_config() {
        let c = Config::parse(
            r#"
[[hosts]]
id = "nci"
base_url = "https://thredds.nci.org.au"
listing = "thredds"
listing_offset_secs = 0
roots = ["thredds/fileServer/rv74/satellite-products/arc/der/himawari-ahi/solar/p1h/v1.1/"]
every_secs = 3600

[[hosts]]
base_url = "https://aemo.com.au"
listing = "documents"
listing_offset_secs = 0
documents = [
  "/-/media/files/gas/sttm/data/sttm-price-and-withdrawals.xlsx",
  "-/media/files/gas/sttm/data/other.xlsx",
  "-/media/files/electricity/nem/registration.xlsx",
]
every_secs = 86400
"#,
        )
        .unwrap();
        let hosts = c.hosts().unwrap();
        assert_eq!(hosts[0].listing, ListingKind::Thredds);
        assert_eq!(hosts[0].id, "nci");
        assert_eq!(hosts[1].listing, ListingKind::Documents);
        assert_eq!(hosts[1].id, "aemo.com.au");
        assert_eq!(hosts[1].documents.len(), 3);
        // An FTP host: its roots are ftp URLs, its `only` patterns kept.
        let bom = Config::parse(
            r#"
[[hosts]]
id = "bom"
base_url = "ftp://ftp.bom.gov.au"
listing = "ftp"
listing_offset_secs = 0
request_interval_ms = 3000
roots = ["anon/gen/fwo/"]
only = ["IDN11060*", "ID?60920.xml"]
every_secs = 600
"#,
        )
        .unwrap();
        let h = &bom.hosts().unwrap()[0];
        assert_eq!(h.listing, ListingKind::Ftp);
        assert_eq!(h.roots[0].as_str(), "ftp://ftp.bom.gov.au/anon/gen/fwo/");
        assert_eq!(h.only, ["IDN11060*", "ID?60920.xml"]);
        assert_eq!(Config::parse(&bom.to_toml()).unwrap(), bom);
        assert_eq!(
            hosts[1].documents[0].as_str(),
            "https://aemo.com.au/-/media/files/gas/sttm/data/sttm-price-and-withdrawals.xlsx"
        );
        // The documents' folders are the roots, each once.
        assert_eq!(
            hosts[1].roots.iter().map(Url::as_str).collect::<Vec<_>>(),
            [
                "https://aemo.com.au/-/media/files/gas/sttm/data/",
                "https://aemo.com.au/-/media/files/electricity/nem/"
            ]
        );
        // A documents host round-trips, and a document that is no URL refuses.
        assert_eq!(Config::parse(&c.to_toml()).unwrap(), c);
        let bad = Config {
            hosts: vec![HostConfig {
                base_url: "aemo".to_owned(),
                ..c.hosts[1].clone()
            }],
            ..Config::default()
        };
        assert!(matches!(bad.hosts().unwrap_err(), MirrorError::Root { .. }));
    }
}
