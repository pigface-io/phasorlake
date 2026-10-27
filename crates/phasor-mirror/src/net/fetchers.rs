//! One [`Fetcher`] per host, each with its own pace and its own refusal
//! breaker, behind the fetcher's interface: a request goes to the fetcher
//! of the host that serves its URL, so a refusal by one host leaves the
//! others reading, and each host is asked no faster than it allows.

use std::sync::Arc;

use super::archive::Archive;
use super::error::FetchError;
use super::event::Event;
use super::fetcher::{Fetcher, LocalFile, RangeRead};
use super::source::RemoteFile;
use super::transport::Span;
use crate::hosts::Offsets;
use crate::stamp::AEST;

/// A host's fetcher: its base URL, its clock and the fetcher itself.
pub struct HostFetcher {
    /// The host, `https://www.nemweb.com.au`, no trailing slash; empty
    /// for a fetcher that serves every URL.
    pub base_url: String,
    /// Seconds east of UTC of the times the host writes.
    pub offset_secs: i32,
    /// Its fetcher.
    pub fetcher: Fetcher,
}

impl HostFetcher {
    /// Whether `url` is on this host.
    fn serves(&self, url: &str) -> bool {
        self.base_url.is_empty()
            || url
                .strip_prefix(self.base_url.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

/// The fetchers of every host, the first the default for a URL no host
/// serves. They share one archive.
pub struct Fetchers {
    hosts: Vec<HostFetcher>,
}

impl Fetchers {
    /// Fetchers for `hosts`, at least one.
    ///
    /// # Panics
    ///
    /// With no host: there is no fetcher to default to.
    pub fn new(hosts: Vec<HostFetcher>) -> Self {
        assert!(!hosts.is_empty(), "at least one host");
        Self { hosts }
    }

    /// One fetcher for every URL, in AEST: nemweb alone, as before hosts.
    pub fn one(fetcher: Fetcher) -> Self {
        Self::new(vec![HostFetcher {
            base_url: String::new(),
            offset_secs: AEST,
            fetcher,
        }])
    }

    /// The hosts, in order.
    pub fn hosts(&self) -> &[HostFetcher] {
        &self.hosts
    }

    /// The host that serves `url`, else the first.
    pub fn for_url(&self, url: &str) -> &HostFetcher {
        self.hosts
            .iter()
            .find(|h| h.serves(url))
            .unwrap_or(&self.hosts[0])
    }

    /// Seconds east of UTC of the times the host of `url` writes.
    pub fn offset_of(&self, url: &str) -> i32 {
        self.for_url(url).offset_secs
    }

    /// The same, as a value a thread may carry.
    pub fn offsets(&self) -> Offsets {
        Offsets::from_bases(
            self.hosts
                .iter()
                .map(|h| (h.base_url.clone(), h.offset_secs))
                .collect(),
        )
    }

    /// The archive every fetcher writes to.
    pub fn archive(&self) -> &Archive {
        self.hosts[0].fetcher.archive()
    }

    /// Requests that succeeded over every host.
    pub fn succeeded(&self) -> u64 {
        self.hosts.iter().map(|h| h.fetcher.succeeded()).sum()
    }

    /// Whether any host has refused this client during this run.
    pub fn is_blocked(&self) -> bool {
        self.hosts.iter().any(|h| h.fetcher.is_blocked())
    }

    /// Sends every fetcher's progress events to `tx`.
    #[must_use]
    pub fn with_events(mut self, tx: &tokio::sync::mpsc::Sender<Event>) -> Self {
        self.hosts = self
            .hosts
            .into_iter()
            .map(|h| HostFetcher {
                fetcher: h.fetcher.with_events(tx.clone()),
                ..h
            })
            .collect();
        self
    }

    /// The fetchers with each host that refused replaced by what `build`
    /// makes of it, the others kept with their pace and their count.
    #[must_use]
    pub fn renewed(mut self, mut build: impl FnMut(&HostFetcher) -> Fetcher) -> Self {
        self.hosts = self
            .hosts
            .into_iter()
            .map(|h| {
                if h.fetcher.is_blocked() {
                    let fetcher = build(&h);
                    HostFetcher { fetcher, ..h }
                } else {
                    h
                }
            })
            .collect();
        self
    }

    /// GETs a page into memory, through the host's fetcher.
    pub async fn fetch_listing(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        self.for_url(url).fetcher.fetch_listing(url).await
    }

    /// Bytes `range` of a file, through the host's fetcher.
    pub async fn fetch_range(&self, url: &str, range: Span) -> Result<RangeRead, FetchError> {
        self.for_url(url).fetcher.fetch_range(url, range).await
    }

    /// A file into the archive, through the host's fetcher.
    pub async fn fetch(&self, remote: &RemoteFile) -> Result<LocalFile, FetchError> {
        self.for_url(remote.url.as_str())
            .fetcher
            .fetch(remote)
            .await
    }
}

/// A fetcher per host over one transport and one archive, each paced as
/// its host asks.
pub fn per_host(
    hosts: &[crate::hosts::Host],
    archive: &Archive,
    mut make: impl FnMut(
        &crate::hosts::Host,
    ) -> (
        Arc<dyn super::transport::Transport>,
        super::fetcher::FetcherConfig,
    ),
) -> Fetchers {
    Fetchers::new(
        hosts
            .iter()
            .map(|h| {
                let (transport, config) = make(h);
                HostFetcher {
                    base_url: h.base_url.clone(),
                    offset_secs: h.offset_secs,
                    fetcher: Fetcher::new(transport, archive.clone(), config),
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;

    use super::*;
    use crate::net::fetcher::FetcherConfig;
    use crate::net::transport::{Canned, StaticTransport};

    fn fast() -> FetcherConfig {
        FetcherConfig {
            attempts: 1,
            backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
            min_interval: Duration::ZERO,
        }
    }

    fn host(base: &str, offset: i32, t: &Arc<StaticTransport>, archive: &Archive) -> HostFetcher {
        HostFetcher {
            base_url: base.to_owned(),
            offset_secs: offset,
            fetcher: Fetcher::new(Arc::new(t.clone()), archive.clone(), fast()),
        }
    }

    #[tokio::test]
    async fn a_request_goes_to_the_host_that_serves_its_url() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let nem = Arc::new(StaticTransport::default());
        nem.add("https://n/a/", Canned::ok(b"<pre></pre>"));
        let wa = Arc::new(StaticTransport::default());
        wa.add("https://w/b/", Canned::ok(b"<pre>wa</pre>"));
        let mut refusal = Canned::status(403);
        refusal.body = Bytes::from_static(b"no");
        wa.add("https://w/no.zip", refusal);
        let f = Fetchers::new(vec![
            host("https://n", 36_000, &nem, &archive),
            host("https://w", 28_800, &wa, &archive),
        ]);
        assert_eq!(f.hosts().len(), 2);
        assert_eq!(f.offset_of("https://w/b/"), 28_800);
        assert_eq!(f.offset_of("https://n/a/"), 36_000);
        assert_eq!(
            f.offset_of("https://elsewhere/"),
            36_000,
            "the first is the default"
        );
        assert_eq!(f.offsets().offset_of("https://w/x"), 28_800);
        assert_eq!(f.archive().root(), dir.path());
        assert_eq!(
            f.fetch_listing("https://w/b/").await.unwrap(),
            b"<pre>wa</pre>"
        );
        assert_eq!(
            f.fetch_listing("https://n/a/").await.unwrap(),
            b"<pre></pre>"
        );
        assert_eq!(f.succeeded(), 2);
        assert_eq!((nem.request_count(), wa.request_count()), (1, 1));
        // A refusal by one host blocks that host alone.
        let remote = RemoteFile {
            url: url::Url::parse("https://w/no.zip").unwrap(),
            size: None,
            modified: None,
            modified_at: None,
        };
        assert!(matches!(
            f.fetch(&remote).await.unwrap_err(),
            FetchError::Blocked { .. }
        ));
        assert!(f.is_blocked());
        assert!(!f.hosts()[0].fetcher.is_blocked());
        assert!(f.hosts()[1].fetcher.is_blocked());
        assert_eq!(
            f.fetch_listing("https://n/a/").await.unwrap(),
            b"<pre></pre>"
        );
        assert!(
            f.fetch_range("https://w/b/", Span::Bytes { start: 0, end: 4 })
                .await
                .is_err()
        );
        // Renewed: only the refused host gets a new fetcher.
        let mut built = Vec::new();
        let f = f.renewed(|h| {
            built.push(h.base_url.clone());
            Fetcher::new(Arc::new(wa.clone()), archive.clone(), fast())
        });
        assert_eq!(built, ["https://w"]);
        assert!(!f.is_blocked());
        assert_eq!(
            f.succeeded(),
            2,
            "the kept host keeps its count, the renewed one starts over"
        );
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let f = f.with_events(&tx);
        assert_eq!(
            f.fetch_listing("https://w/b/").await.unwrap(),
            b"<pre>wa</pre>"
        );
        assert_eq!(f.succeeded(), 3);
        drop(rx);
    }

    #[tokio::test]
    async fn one_fetcher_serves_every_url_in_aest() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://any/x/", Canned::ok(b"<pre></pre>"));
        let f = Fetchers::one(Fetcher::new(Arc::new(t), Archive::new(dir.path()), fast()));
        assert_eq!(f.offset_of("https://any/x/"), AEST);
        assert_eq!(
            f.fetch_listing("https://any/x/").await.unwrap(),
            b"<pre></pre>"
        );
        let hosts = vec![
            crate::hosts::Host::nemweb("https://n", 7, &[], &[]).unwrap(),
            crate::hosts::Host {
                base_url: "https://w".to_owned(),
                offset_secs: 28_800,
                request_interval: Duration::from_millis(9),
                roots: vec![],
                watch_fast: vec![],
                watch_current: vec![],
            },
        ];
        let mut intervals = Vec::new();
        let f = per_host(&hosts, &Archive::new(dir.path()), |h| {
            intervals.push(h.request_interval);
            (
                Arc::new(crate::net::OfflineTransport),
                FetcherConfig {
                    min_interval: h.request_interval,
                    ..fast()
                },
            )
        });
        assert_eq!(
            intervals,
            [Duration::from_millis(7), Duration::from_millis(9)]
        );
        assert_eq!(f.for_url("https://w/z").base_url, "https://w");
        assert_eq!(f.for_url("https://q/").base_url, "https://n");
    }

    #[test]
    #[should_panic(expected = "at least one host")]
    fn no_host_is_refused() {
        let _ = Fetchers::new(vec![]);
    }
}
