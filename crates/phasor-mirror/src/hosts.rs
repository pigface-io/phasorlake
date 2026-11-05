//! The hosts the mirror reads, each named by the config with its own roots,
//! clock, pace, pool and schedule. Every listed or packed time is read in its
//! host's clock, from [`Offsets`].

use std::time::Duration;

use url::Url;

use crate::config::{HostConfig, ListingKind};
use crate::crawl::root;
use crate::error::MirrorError;
use crate::stamp::AEST;

/// One host, resolved from the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// Its name, for logs and locks: `nemweb`, or the site's host name.
    pub id: String,
    /// `https://www.nemweb.com.au`, no trailing slash.
    pub base_url: String,
    /// How it lists a folder.
    pub listing: ListingKind,
    /// Seconds east of UTC of the host's listed and packed times.
    pub offset_secs: i32,
    /// Minimum gap between two requests to this host.
    pub request_interval: Duration,
    /// Its own downloads at a time, when it allows itself fewer than the whole.
    pub downloads: Option<usize>,
    /// Its own tries per request, when it has them.
    pub attempts: Option<u32>,
    /// The folders mirrored, under `base_url`.
    pub roots: Vec<Url>,
    /// For a host of documents, the files watched, under `base_url`.
    pub documents: Vec<Url>,
    /// Patterns of the file names its folders' listings keep; every file when empty.
    pub only: Vec<String>,
    /// How often a watch reads the folders under each absolute URL prefix.
    pub rules: Vec<(String, Duration)>,
    /// How often a watch reads this host's other folders, when it says.
    pub every: Option<Duration>,
}

impl Host {
    /// A host from its `[[hosts]]` table, paced at `request_interval_ms`
    /// unless it says otherwise. `watch_fast` and `watch_current` are rules
    /// of 15 minutes and an hour.
    pub fn from_config(c: &HostConfig, request_interval_ms: u64) -> Result<Self, MirrorError> {
        let base = c.base_url.trim_end_matches('/').to_owned();
        let documents = under(&base, &c.documents)
            .iter()
            .map(|d| Url::parse(d).map_err(|_| MirrorError::Root { url: d.clone() }))
            .collect::<Result<Vec<Url>, MirrorError>>()?;
        // A host of documents needs no roots: its folders are the documents'.
        let mut root_paths = c.roots.clone();
        if c.listing == ListingKind::Documents && root_paths.is_empty() {
            for d in &c.documents {
                let folder = d.trim_start_matches('/');
                let folder = folder.rsplit_once('/').map_or("", |(dir, _)| dir);
                let folder = format!("{folder}/");
                if !root_paths.contains(&folder) {
                    root_paths.push(folder);
                }
            }
        }
        let roots = under(&base, &root_paths)
            .iter()
            .map(|r| root(r))
            .collect::<Result<Vec<Url>, MirrorError>>()?;
        let mut rules: Vec<(String, Duration)> = c
            .schedule
            .iter()
            .map(|r| {
                (
                    under(&base, std::slice::from_ref(&r.under)).remove(0),
                    Duration::from_secs(r.every_secs),
                )
            })
            .collect();
        rules.extend(
            under(&base, &c.watch_fast)
                .into_iter()
                .map(|p| (p, Duration::from_secs(15 * 60))),
        );
        rules.extend(
            under(&base, &c.watch_current)
                .into_iter()
                .map(|p| (p, Duration::from_secs(60 * 60))),
        );
        Ok(Self {
            id: c.name(),
            roots,
            documents,
            only: c.only.clone(),
            rules,
            every: c.every_secs.map(Duration::from_secs),
            base_url: base,
            listing: c.listing,
            offset_secs: c.listing_offset_secs,
            request_interval: Duration::from_millis(
                c.request_interval_ms.unwrap_or(request_interval_ms),
            ),
            downloads: c.downloads,
            attempts: c.attempts,
        })
    }

    /// A host made up for a root no configured host serves: an IIS site in
    /// AEST, paced at `request_interval_ms`, named by the root's host, with
    /// the root alone.
    pub fn ad_hoc(root: &Url, request_interval_ms: u64) -> Self {
        // The scheme says how the host lists and in whose clock: an FTP
        // server in UTC, any web site as an IIS site in AEST.
        let (listing, offset_secs) = match root.scheme() {
            "ftp" => (ListingKind::Ftp, 0),
            _ => (ListingKind::Iis, AEST),
        };
        Self {
            id: root.host_str().unwrap_or("host").to_owned(),
            base_url: root.origin().ascii_serialization(),
            listing,
            offset_secs,
            request_interval: Duration::from_millis(request_interval_ms),
            downloads: None,
            attempts: None,
            roots: vec![root.clone()],
            documents: Vec::new(),
            only: Vec::new(),
            rules: Vec::new(),
            every: None,
        }
    }

    /// Whether `url` is on this host.
    pub fn serves(&self, url: &str) -> bool {
        url.strip_prefix(self.base_url.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

/// Paths under a base, as absolute URL text.
fn under(base: &str, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| format!("{base}/{}", p.trim_start_matches('/')))
        .collect()
}

/// Which clock each URL's times are in: the first host that serves it, else AEST.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offsets {
    hosts: Vec<(String, i32)>,
}

impl Offsets {
    /// The offsets of `hosts`, asked in order.
    pub fn of(hosts: &[Host]) -> Self {
        Self {
            hosts: hosts
                .iter()
                .map(|h| (h.base_url.clone(), h.offset_secs))
                .collect(),
        }
    }

    /// One offset for every URL.
    pub fn always(offset: i32) -> Self {
        Self::from_bases(vec![(String::new(), offset)])
    }

    /// The offsets of hosts by base URL, asked in order; an empty base serves every URL.
    pub fn from_bases(hosts: Vec<(String, i32)>) -> Self {
        Self { hosts }
    }

    /// Seconds east of UTC of the times the host of `url` writes.
    pub fn offset_of(&self, url: &str) -> i32 {
        self.hosts
            .iter()
            .find(|(base, _)| {
                base.is_empty()
                    || url
                        .strip_prefix(base.as_str())
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
            .map_or(AEST, |(_, offset)| *offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Rule;

    #[test]
    fn nemweb_and_a_further_host_resolve_from_the_config() {
        let nem = Host::from_config(&HostConfig::nemweb(), 1000).unwrap();
        assert_eq!(nem.id, "nemweb");
        assert_eq!(nem.base_url, "https://www.nemweb.com.au");
        assert_eq!(nem.offset_secs, AEST);
        assert_eq!(nem.request_interval, Duration::from_millis(1000));
        assert_eq!(nem.roots.len(), 3);
        assert_eq!(nem.rules.len(), 6);
        assert_eq!(
            nem.rules[0],
            (
                "https://www.nemweb.com.au/Reports/CURRENT/VicGas/".to_owned(),
                Duration::from_secs(900)
            )
        );
        assert_eq!(
            nem.rules[5],
            (
                "https://www.nemweb.com.au/Reports/CURRENT/".to_owned(),
                Duration::from_secs(3600)
            )
        );
        assert_eq!((nem.every, nem.downloads, nem.attempts), (None, None, None));
        assert!(nem.serves("https://www.nemweb.com.au/Reports/"));
        assert!(nem.serves("https://www.nemweb.com.au"));
        assert!(!nem.serves("https://www.nemweb.com.au.evil/x"));
        assert!(!nem.serves("https://data.wa.aemo.com.au/public/"));

        let wa = HostConfig {
            id: None,
            base_url: "https://data.wa.aemo.com.au".to_owned(),
            listing: ListingKind::Iis,
            listing_offset_secs: 8 * 3600,
            request_interval_ms: None,
            downloads: Some(1),
            attempts: Some(3),
            documents: Vec::new(),
            only: Vec::new(),
            roots: vec![
                "public/public-data/datafiles/".to_owned(),
                "/public/market-data/wemde/facilityScada/previous/".to_owned(),
            ],
            every_secs: Some(7200),
            schedule: vec![Rule {
                under: "public/market-data/wemde/".to_owned(),
                every_secs: 600,
            }],
            watch_fast: vec!["public/fast/".to_owned()],
            watch_current: vec!["public/market-data/".to_owned()],
        };
        let h = Host::from_config(&wa, 750).unwrap();
        assert_eq!(h.id, "data.wa.aemo.com.au", "named by its host name");
        assert_eq!(h.offset_secs, 28_800);
        assert_eq!(h.listing, ListingKind::Iis);
        assert_eq!(h.request_interval, Duration::from_millis(750));
        assert_eq!((h.downloads, h.attempts), (Some(1), Some(3)));
        assert_eq!(h.every, Some(Duration::from_secs(7200)));
        assert_eq!(
            h.rules,
            [
                (
                    "https://data.wa.aemo.com.au/public/market-data/wemde/".to_owned(),
                    Duration::from_secs(600)
                ),
                (
                    "https://data.wa.aemo.com.au/public/fast/".to_owned(),
                    Duration::from_secs(900)
                ),
                (
                    "https://data.wa.aemo.com.au/public/market-data/".to_owned(),
                    Duration::from_secs(3600)
                ),
            ]
        );
        let blobs = HostConfig {
            listing: ListingKind::AzureBlob,
            ..wa.clone()
        };
        assert_eq!(
            Host::from_config(&blobs, 750).unwrap().listing,
            ListingKind::AzureBlob
        );
        assert_eq!(
            h.roots.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "https://data.wa.aemo.com.au/public/public-data/datafiles/",
                "https://data.wa.aemo.com.au/public/market-data/wemde/facilityScada/previous/"
            ]
        );
        let own_pace = HostConfig {
            request_interval_ms: Some(2000),
            ..wa.clone()
        };
        assert_eq!(
            Host::from_config(&own_pace, 750).unwrap().request_interval,
            Duration::from_millis(2000)
        );
        // A root that is not a folder path refuses, naming it.
        let bad = HostConfig {
            roots: vec!["public/file.csv".to_owned()],
            ..wa
        };
        assert!(matches!(
            Host::from_config(&bad, 750).unwrap_err(),
            MirrorError::Root { url } if url == "https://data.wa.aemo.com.au/public/file.csv"
        ));
        let bad_base = HostConfig {
            base_url: "nemweb".to_owned(),
            ..HostConfig::nemweb()
        };
        assert!(Host::from_config(&bad_base, 1).is_err());
        // A host made up for a root: the root's origin, the root alone.
        let h = Host::ad_hoc(&Url::parse("https://h:8080/A/B/").unwrap(), 5);
        assert_eq!(h.base_url, "https://h:8080");
        assert_eq!(h.id, "h");
        assert_eq!(h.roots[0].as_str(), "https://h:8080/A/B/");
        assert_eq!(h.offset_secs, AEST);
        assert_eq!(h.request_interval, Duration::from_millis(5));
        assert!(h.serves("https://h:8080/A/B/c.csv"));
    }

    #[test]
    fn an_offset_is_the_first_host_that_serves_the_url_else_aest() {
        let hosts = vec![
            Host::from_config(&HostConfig::nemweb(), 1).unwrap(),
            Host {
                id: "wa".to_owned(),
                base_url: "https://data.wa.aemo.com.au".to_owned(),
                listing: ListingKind::Iis,
                offset_secs: 28_800,
                request_interval: Duration::from_secs(1),
                downloads: None,
                attempts: None,
                roots: vec![],
                documents: vec![],
                only: vec![],
                rules: vec![],
                every: None,
            },
        ];
        let o = Offsets::of(&hosts);
        assert_eq!(o.offset_of("https://www.nemweb.com.au/Reports/"), AEST);
        assert_eq!(
            o.offset_of("https://data.wa.aemo.com.au/public/x.zip"),
            28_800
        );
        assert_eq!(o.offset_of("https://data.wa.aemo.com.au"), 28_800);
        assert_eq!(o.offset_of("https://data.wa.aemo.com.auz/"), AEST);
        assert_eq!(o.offset_of("https://elsewhere/"), AEST);
        assert_eq!(Offsets::default().offset_of("https://h/"), AEST);
        assert_eq!(Offsets::always(3600).offset_of("https://h/"), 3600);
    }
}
