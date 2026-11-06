//! One [`Fetcher`] per host, each with its own pace and refusal breaker, so
//! a refusal by one host leaves the others reading.

use std::sync::Arc;

use super::archive::Archive;
use super::error::FetchError;
use super::event::Event;
use super::fetcher::{Fetcher, LocalFile, RangeRead};
use super::source::RemoteFile;
use super::transport::Span;
use crate::config::ListingKind;
use crate::hosts::Offsets;
use crate::stamp::AEST;

/// A host's fetcher: its base URL, its clock, how it lists a folder and the
/// fetcher itself.
pub struct HostFetcher {
    /// The host's base URL, no trailing slash; empty for one that serves every URL.
    pub base_url: String,
    /// Seconds east of UTC of the times the host writes.
    pub offset_secs: i32,
    /// How the host lists a folder.
    pub listing: ListingKind,
    /// For a host of documents, the files it watches, as absolute URLs.
    pub documents: Vec<String>,
    /// Patterns of the file names its listings keep; every file when empty.
    pub only: Vec<String>,
    /// At most this many of its files in one batched download pass, when
    /// it has a cap; the rest wait for the next pass.
    pub batch: Option<usize>,
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

/// The fetchers of every host, the first the default for a URL no host serves.
pub struct Fetchers {
    hosts: Vec<HostFetcher>,
    /// Each host's own cap on files downloading at once, when it has one;
    /// one entry per host, in order.
    limits: Vec<Option<Arc<tokio::sync::Semaphore>>>,
}

impl Fetchers {
    /// Fetchers for `hosts`, at least one, none capped below the whole.
    ///
    /// # Panics
    ///
    /// With no host: there is no fetcher to default to.
    pub fn new(hosts: Vec<HostFetcher>) -> Self {
        assert!(!hosts.is_empty(), "at least one host");
        let limits = vec![None; hosts.len()];
        Self { hosts, limits }
    }

    /// The same fetchers, each host downloading at most its `downloads` files
    /// at once where it says (one entry per host, in order).
    #[must_use]
    pub fn with_limits(mut self, downloads: &[Option<usize>]) -> Self {
        self.limits = self
            .hosts
            .iter()
            .enumerate()
            .map(|(i, _)| {
                downloads
                    .get(i)
                    .copied()
                    .flatten()
                    .map(|n| Arc::new(tokio::sync::Semaphore::new(n.max(1))))
            })
            .collect();
        self
    }

    /// The index of the host that serves `url`, else 0.
    fn index_of(&self, url: &str) -> usize {
        self.hosts.iter().position(|h| h.serves(url)).unwrap_or(0)
    }

    /// One fetcher for every URL, in AEST, listing IIS pages.
    pub fn one(fetcher: Fetcher) -> Self {
        Self::new(vec![HostFetcher {
            base_url: String::new(),
            offset_secs: AEST,
            listing: ListingKind::Iis,
            documents: Vec::new(),
            only: Vec::new(),
            batch: None,
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

    /// How the host of `url` lists a folder.
    pub fn listing_of(&self, url: &str) -> ListingKind {
        self.for_url(url).listing
    }

    /// The listing of the folder `folder`, as its host's kind gives it
    /// ([`ListingKind::fetch_folder`]).
    pub async fn fetch_folder(&self, folder: &str) -> Result<Vec<u8>, FetchError> {
        let host = self.for_url(folder);
        host.listing.fetch_folder(host, folder).await
    }

    /// Every link of `folder`'s listing `bytes` as a record, read as its
    /// host's kind reads a page, in the host's clock; a file whose name
    /// matches none of the host's `only` patterns is left out.
    pub fn read_page(
        &self,
        folder: &url::Url,
        bytes: &[u8],
        roots: &[url::Url],
        crawled_at: Option<crate::stamp::Stamp>,
    ) -> Result<Vec<crate::crawl::Remote>, crate::error::MirrorError> {
        let host = self.for_url(folder.as_str());
        let mut links =
            host.listing
                .read_page(folder, bytes, roots, crawled_at, host.offset_secs)?;
        if !host.only.is_empty() {
            links.retain(|r| {
                r.kind != crate::crawl::RemoteKind::File || {
                    let name = r.url.rsplit('/').next().unwrap_or(&r.url);
                    host.only.iter().any(|p| crate::listers::glob(p, name))
                }
            });
        }
        Ok(links)
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
                batch: None,
                fetcher: h.fetcher.with_events(tx.clone()),
                ..h
            })
            .collect();
        self
    }

    /// The fetchers with each host that refused replaced by what `build` makes of it.
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

    /// A file into the archive, through the host's fetcher, within the host's
    /// own cap on files at once.
    pub async fn fetch(&self, remote: &RemoteFile) -> Result<LocalFile, FetchError> {
        let i = self.index_of(remote.url.as_str());
        let _permit = match &self.limits[i] {
            Some(s) => {
                stretch!(DOWNLOAD_PERMIT_PATH);
                Some(s.acquire().await.expect("a semaphore never closed"))
            }
            None => None,
        };
        self.hosts[i].fetcher.fetch(remote).await
    }
}

/// A fetcher per host over one transport and one archive, each host capped
/// at its own `downloads` where it has one.
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
    let limits: Vec<Option<usize>> = hosts.iter().map(|h| h.downloads).collect();
    Fetchers::new(
        hosts
            .iter()
            .map(|h| {
                let (transport, config) = make(h);
                HostFetcher {
                    base_url: h.base_url.clone(),
                    offset_secs: h.offset_secs,
                    listing: h.listing,
                    documents: h.documents.iter().map(ToString::to_string).collect(),
                    only: h.only.clone(),
                    batch: h.batch,
                    fetcher: Fetcher::new(transport, archive.clone(), config),
                }
            })
            .collect(),
    )
    .with_limits(&limits)
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
            listing: ListingKind::Iis,
            documents: Vec::new(),
            only: Vec::new(),
            batch: None,
            fetcher: Fetcher::new(Arc::new(t.clone()), archive.clone(), fast()),
        }
    }

    #[tokio::test]
    async fn a_blob_hosts_folder_is_its_pages_one_after_another() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let t = Arc::new(StaticTransport::default());
        let folder = "https://b/c/Datasets/X/";
        let page = |blob: &str, next: &str| {
            format!(
                "<EnumerationResults><Blobs><Blob><Name>Datasets/X/{blob}</Name><Properties><Last-Modified>Tue, 04 Nov 2026 18:05:00 GMT</Last-Modified><Content-Length>3</Content-Length></Properties></Blob></Blobs><NextMarker>{next}</NextMarker></EnumerationResults>"
            )
        };
        t.add(
            &phasor_net::bloblist::list_url(folder, None).unwrap(),
            Canned::ok(page("a.csv", "m1").as_bytes()),
        );
        t.add(
            &phasor_net::bloblist::list_url(folder, Some("m1")).unwrap(),
            Canned::ok(page("b.csv", "").as_bytes()),
        );
        t.add(
            &phasor_net::bloblist::list_url("https://b/c/Datasets/Bad/", None).unwrap(),
            Canned::ok(b"<html/>"),
        );
        let blob = HostFetcher {
            listing: ListingKind::AzureBlob,
            ..host("https://b", 0, &t, &archive)
        };
        let f = Fetchers::new(vec![host("https://n", AEST, &t, &archive), blob]);
        assert_eq!(f.listing_of(folder), ListingKind::AzureBlob);
        assert_eq!(f.listing_of("https://n/x/"), ListingKind::Iis);
        let both = f.fetch_folder(folder).await.unwrap();
        let pages = phasor_net::bloblist::parse_pages(&both).unwrap();
        let names: Vec<&str> = pages.blobs.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Datasets/X/a.csv", "Datasets/X/b.csv"]);
        assert_eq!(pages.next_marker, None);
        assert_eq!(t.request_count(), 2);
        assert!(matches!(
            f.fetch_folder("https://b/c/Datasets/Bad/")
                .await
                .unwrap_err(),
            FetchError::Parse {
                what: "blob listing",
                ..
            }
        ));
        assert!(matches!(
            f.fetch_folder("https://b").await.unwrap_err(),
            FetchError::Parse {
                what: "blob folder",
                ..
            }
        ));
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
            crate::hosts::Host::from_config(
                &crate::config::HostConfig {
                    base_url: "https://n".to_owned(),
                    request_interval_ms: Some(7),
                    ..crate::config::HostConfig::nemweb()
                },
                1,
            )
            .unwrap(),
            crate::hosts::Host {
                id: "w".to_owned(),
                base_url: "https://w".to_owned(),
                listing: ListingKind::AzureBlob,
                offset_secs: 28_800,
                request_interval: Duration::from_millis(9),
                downloads: Some(1),
                attempts: None,
                batch: None,
                roots: vec![],
                documents: vec![],
                only: vec![],
                rules: vec![],
                every: None,
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
        assert_eq!(f.listing_of("https://w/z"), ListingKind::AzureBlob);
        assert_eq!(f.listing_of("https://n/z"), ListingKind::Iis);
    }

    #[tokio::test]
    async fn a_capped_host_downloads_within_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let t = Arc::new(StaticTransport::default());
        t.add("https://w/f.csv", Canned::ok(b"abc"));
        t.add("https://n/g.csv", Canned::ok(b"de"));
        let f = Fetchers::new(vec![
            host("https://n", AEST, &t, &archive),
            host("https://w", 0, &t, &archive),
        ])
        .with_limits(&[None, Some(1)]);
        let remote = |url: &str, size: u64| RemoteFile {
            url: url::Url::parse(url).unwrap(),
            size: Some(size),
            modified: None,
            modified_at: None,
        };
        let capped = f.fetch(&remote("https://w/f.csv", 3)).await.unwrap();
        let free = f.fetch(&remote("https://n/g.csv", 2)).await.unwrap();
        assert_eq!((capped.size, free.size), (3, 2));
        assert_eq!(t.request_count(), 2);
    }

    #[test]
    fn a_hosts_only_patterns_keep_the_files_named_and_every_folder() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let t = Arc::new(StaticTransport::default());
        // An FTP folder of four entries: two files the patterns name, one
        // they do not, and a folder, which patterns never touch.
        use phasor_ftp::listing::Entry;
        let entry = |name: &str, dir: bool| Entry {
            name: name.to_owned(),
            dir,
            size: if dir { None } else { Some(12) },
            modified: None,
        };
        let page = Entry::page(&[
            entry("IDN11060.xml", false),
            entry("IDV10753.xml", false),
            entry("sub", true),
            entry("IDN60920.xml", false),
        ]);
        let mut h = host("ftp://h", 0, &t, &archive);
        h.listing = ListingKind::Ftp;
        h.only = vec!["IDN11060*".to_owned(), "ID?60920.xml".to_owned()];
        let f = Fetchers::new(vec![h]);
        let folder = url::Url::parse("ftp://h/fwo/").unwrap();
        let links = f
            .read_page(&folder, &page, std::slice::from_ref(&folder), None)
            .unwrap();
        let urls: Vec<&str> = links.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "ftp://h/fwo/IDN11060.xml",
                "ftp://h/fwo/sub/",
                "ftp://h/fwo/IDN60920.xml"
            ]
        );
        // Without patterns every file stays.
        let mut h = host("ftp://h", 0, &t, &archive);
        h.listing = ListingKind::Ftp;
        let f = Fetchers::new(vec![h]);
        assert_eq!(
            f.read_page(&folder, &page, std::slice::from_ref(&folder), None)
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    #[should_panic(expected = "at least one host")]
    fn no_host_is_refused() {
        let _ = Fetchers::new(vec![]);
    }
}
