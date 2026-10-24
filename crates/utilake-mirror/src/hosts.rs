//! The hosts the mirror reads: nemweb, and any other site the config
//! names, each with its own roots, clock and pace.
//!
//! AEMO publishes the NEM on `www.nemweb.com.au`, whose listings and zips
//! carry AEST times, and the WEM on `data.wa.aemo.com.au`, whose carry
//! AWST (UTC+8). A record's `_at` instant is right only when the time is
//! read in its host's clock, so every place a listed or packed time is read
//! asks [`Offsets`] for the URL's offset.

use std::time::Duration;

use url::Url;

use crate::crawl::{default_roots, root};
use crate::error::MirrorError;
use crate::stamp::AEST;

/// One host, resolved from the config: its base URL without a trailing
/// slash, its roots as absolute folder URLs, its clock and its pace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// `https://www.nemweb.com.au`, no trailing slash.
    pub base_url: String,
    /// Seconds east of UTC of the host's listed and packed times.
    pub offset_secs: i32,
    /// Minimum gap between two requests to this host.
    pub request_interval: Duration,
    /// The folders mirrored, under `base_url`.
    pub roots: Vec<Url>,
    /// Folders a watch reads most often, as absolute URL prefixes.
    pub watch_fast: Vec<String>,
    /// Folders a watch reads next most often, as absolute URL prefixes.
    pub watch_current: Vec<String>,
}

impl Host {
    /// nemweb, from the config's top-level keys: the long-term archive and
    /// the report folders, in AEST.
    pub fn nemweb(
        base_url: &str,
        request_interval_ms: u64,
        watch_fast: &[String],
        watch_current: &[String],
    ) -> Result<Self, MirrorError> {
        let base = base_url.trim_end_matches('/').to_owned();
        Ok(Self {
            roots: default_roots(&base)?,
            watch_fast: under(&base, watch_fast),
            watch_current: under(&base, watch_current),
            base_url: base,
            offset_secs: AEST,
            request_interval: Duration::from_millis(request_interval_ms),
        })
    }

    /// A further host from its `[[hosts]]` table, its roots given as paths
    /// under its base.
    pub fn from_config(
        c: &crate::config::HostConfig,
        request_interval_ms: u64,
    ) -> Result<Self, MirrorError> {
        let base = c.base_url.trim_end_matches('/').to_owned();
        let roots = under(&base, &c.roots)
            .iter()
            .map(|r| root(r))
            .collect::<Result<Vec<Url>, MirrorError>>()?;
        Ok(Self {
            roots,
            watch_fast: under(&base, &c.watch_fast),
            watch_current: under(&base, &c.watch_current),
            base_url: base,
            offset_secs: c.listing_offset_secs,
            request_interval: Duration::from_millis(
                c.request_interval_ms.unwrap_or(request_interval_ms),
            ),
        })
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

/// Which clock each host's times are in, by URL: the first host that
/// serves the URL, else AEST, nemweb's.
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

    /// The offsets of hosts given as base URL and offset, asked in order;
    /// an empty base serves every URL.
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

    #[test]
    fn nemweb_and_a_further_host_resolve_from_the_config() {
        let nem = Host::nemweb(
            "https://www.nemweb.com.au/",
            1000,
            &["Reports/CURRENT/GBB/".to_owned()],
            &["/Reports/CURRENT/".to_owned()],
        )
        .unwrap();
        assert_eq!(nem.base_url, "https://www.nemweb.com.au");
        assert_eq!(nem.offset_secs, AEST);
        assert_eq!(nem.request_interval, Duration::from_millis(1000));
        assert_eq!(nem.roots.len(), 3);
        assert_eq!(
            nem.watch_fast,
            ["https://www.nemweb.com.au/Reports/CURRENT/GBB/"]
        );
        assert_eq!(
            nem.watch_current,
            ["https://www.nemweb.com.au/Reports/CURRENT/"]
        );
        assert!(nem.serves("https://www.nemweb.com.au/Reports/"));
        assert!(nem.serves("https://www.nemweb.com.au"));
        assert!(!nem.serves("https://www.nemweb.com.au.evil/x"));
        assert!(!nem.serves("https://data.wa.aemo.com.au/public/"));

        let wa = crate::config::HostConfig {
            base_url: "https://data.wa.aemo.com.au".to_owned(),
            listing_offset_secs: 8 * 3600,
            request_interval_ms: None,
            roots: vec![
                "public/public-data/datafiles/".to_owned(),
                "/public/market-data/wemde/facilityScada/previous/".to_owned(),
            ],
            watch_fast: vec![],
            watch_current: vec!["public/market-data/wemde/".to_owned()],
        };
        let h = Host::from_config(&wa, 750).unwrap();
        assert_eq!(h.offset_secs, 28_800);
        assert_eq!(h.request_interval, Duration::from_millis(750));
        assert_eq!(
            h.roots.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "https://data.wa.aemo.com.au/public/public-data/datafiles/",
                "https://data.wa.aemo.com.au/public/market-data/wemde/facilityScada/previous/"
            ]
        );
        assert_eq!(
            h.watch_current,
            ["https://data.wa.aemo.com.au/public/market-data/wemde/"]
        );
        let own_pace = crate::config::HostConfig {
            request_interval_ms: Some(2000),
            ..wa.clone()
        };
        assert_eq!(
            Host::from_config(&own_pace, 750).unwrap().request_interval,
            Duration::from_millis(2000)
        );
        // A root that is not a folder path refuses, naming it.
        let bad = crate::config::HostConfig {
            roots: vec!["public/file.csv".to_owned()],
            ..wa
        };
        assert!(matches!(
            Host::from_config(&bad, 750).unwrap_err(),
            MirrorError::Root { url } if url == "https://data.wa.aemo.com.au/public/file.csv"
        ));
        assert!(Host::nemweb("not a url", 1, &[], &[]).is_err());
    }

    #[test]
    fn an_offset_is_the_first_host_that_serves_the_url_else_aest() {
        let hosts = vec![
            Host::nemweb("https://www.nemweb.com.au", 1, &[], &[]).unwrap(),
            Host {
                base_url: "https://data.wa.aemo.com.au".to_owned(),
                offset_secs: 28_800,
                request_interval: Duration::from_secs(1),
                roots: vec![],
                watch_fast: vec![],
                watch_current: vec![],
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
