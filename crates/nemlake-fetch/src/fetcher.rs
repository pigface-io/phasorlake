//! Downloading with cache, retries, politeness and verification.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use nemlake_core::clock::now_utc_rfc3339;
use nemlake_core::event::Event;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::cache::{DiskCache, Sidecar, hex};
use crate::error::FetchError;
use crate::source::RemoteFile;
use crate::transport::{HttpRequest, Transport};

/// A file on disk, ready to ingest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// What was asked for.
    pub remote: RemoteFile,
    /// Where the bytes are.
    pub path: PathBuf,
    /// sha256 hex.
    pub sha256: String,
    /// Bytes.
    pub size: u64,
    /// Whether it came from the cache without a request.
    pub cached: bool,
    /// ETag.
    pub etag: Option<String>,
    /// Last-Modified.
    pub last_modified: Option<String>,
}

/// Knobs.
#[derive(Debug, Clone)]
pub struct FetcherConfig {
    /// Attempts per download.
    pub attempts: u32,
    /// First backoff.
    pub backoff: Duration,
    /// Longest backoff.
    pub max_backoff: Duration,
    /// Minimum gap between requests to the same host.
    pub min_interval: Duration,
    /// Re-download even when cached.
    pub refresh: bool,
    /// Hash cached files before trusting them.
    pub verify_cache: bool,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        Self {
            attempts: 5,
            backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            min_interval: Duration::from_millis(100),
            refresh: false,
            verify_cache: false,
        }
    }
}

/// Fetches remote files into the cache.
pub struct Fetcher {
    transport: Arc<dyn Transport>,
    cache: DiskCache,
    config: FetcherConfig,
    last_request: Mutex<Option<Instant>>,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
}

impl Fetcher {
    /// A fetcher over a transport and cache.
    pub fn new(transport: Arc<dyn Transport>, cache: DiskCache, config: FetcherConfig) -> Self {
        Self {
            transport,
            cache,
            config,
            last_request: Mutex::new(None),
            events: None,
        }
    }

    /// Sends progress events to `tx`.
    pub fn with_events(mut self, tx: tokio::sync::mpsc::Sender<Event>) -> Self {
        self.events = Some(tx);
        self
    }

    /// The cache.
    pub fn cache(&self) -> &DiskCache {
        &self.cache
    }

    /// The config.
    pub fn config(&self) -> &FetcherConfig {
        &self.config
    }

    /// Whether downloads are forbidden.
    pub fn is_offline(&self) -> bool {
        self.transport.is_offline()
    }

    async fn pace(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(t) = *last {
            let since = t.elapsed();
            if since < self.config.min_interval {
                tokio::time::sleep(self.config.min_interval - since).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// Fetches a listing page, using the listing cache when it is younger
    /// than `max_age_secs`.
    pub async fn fetch_listing(&self, url: &str, max_age_secs: i64) -> Result<Vec<u8>, FetchError> {
        if !self.config.refresh
            && let Some(body) = self.cache.lookup_listing(url, max_age_secs)?
        {
            return Ok(body);
        }
        if self.transport.is_offline() {
            return Err(FetchError::Offline {
                url: url.to_owned(),
            });
        }
        let body = self
            .get_with_retries(url, |resp| async move { read_all(url, resp).await })
            .await?;
        self.cache.store_listing(url, &body)?;
        Ok(body)
    }

    /// Fetches a remote file into the cache, or returns the cached copy.
    pub async fn fetch(&self, remote: &RemoteFile) -> Result<LocalFile, FetchError> {
        let url = remote.url.as_str();
        let file_name = remote.file_name();
        if !self.config.refresh
            && let Some(blob) = self.cache.lookup(remote.source, url, &file_name)?
        {
            let ok = if self.config.verify_cache {
                self.cache.verify(&blob).is_ok()
            } else {
                true
            };
            if ok {
                return Ok(LocalFile {
                    remote: remote.clone(),
                    path: blob.path,
                    sha256: blob.sidecar.sha256,
                    size: blob.sidecar.size,
                    cached: true,
                    etag: blob.sidecar.etag,
                    last_modified: blob.sidecar.last_modified,
                });
            }
        }
        if self.transport.is_offline() {
            return Err(FetchError::Offline {
                url: url.to_owned(),
            });
        }
        let path = self.cache.path_for(remote.source, url, &file_name);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|source| FetchError::CacheIo {
                path: p.display().to_string(),
                source,
            })?;
        }
        let part = DiskCache::part_path(&path);
        let events = self.events.clone();
        let (sha256, size, etag, last_modified, accept_ranges) = self
            .get_with_retries(url, |resp| {
                let part = part.clone();
                let events = events.clone();
                async move { stream_to_file(url, resp, &part, events).await }
            })
            .await?;
        std::fs::rename(&part, &path).map_err(|source| FetchError::CacheIo {
            path: path.display().to_string(),
            source,
        })?;
        let sidecar = Sidecar {
            url: url.to_owned(),
            etag: etag.clone(),
            last_modified: last_modified.clone(),
            size,
            sha256: sha256.clone(),
            fetched_at: now_utc_rfc3339(),
            accept_ranges,
        };
        DiskCache::write_sidecar(&path, &sidecar)?;
        Ok(LocalFile {
            remote: remote.clone(),
            path,
            sha256,
            size,
            cached: false,
            etag,
            last_modified,
        })
    }

    /// GETs `url` with retries and backoff, handing a 200 response to `consume`.
    async fn get_with_retries<T, F, Fut>(&self, url: &str, consume: F) -> Result<T, FetchError>
    where
        F: Fn(crate::transport::HttpResponse) -> Fut,
        Fut: std::future::Future<Output = Result<T, FetchError>>,
    {
        let mut last: Option<FetchError> = None;
        for attempt in 0..self.config.attempts.max(1) {
            if attempt > 0 {
                let exp = self
                    .config
                    .backoff
                    .saturating_mul(1u32 << (attempt - 1).min(16));
                let jitter = Duration::from_millis(u64::from(attempt) * 37 % 250);
                tokio::time::sleep(exp.min(self.config.max_backoff) + jitter).await;
            }
            self.pace().await;
            let req = HttpRequest::get(url);
            let result = match self.transport.get(req).await {
                Err(e) => Err(e),
                Ok(resp) if resp.status == 200 => consume(resp).await,
                Ok(resp) => {
                    let head = read_head(resp.body, 256).await;
                    Err(FetchError::HttpStatus {
                        url: url.to_owned(),
                        status: resp.status,
                        body_head: head,
                    })
                }
            };
            match result {
                Ok(v) => return Ok(v),
                Err(e) if e.is_retryable() => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(FetchError::RetriesExhausted {
            url: url.to_owned(),
            attempts: self.config.attempts.max(1),
            last: Box::new(last.unwrap_or(FetchError::Connect {
                url: url.to_owned(),
                message: "no attempt was made".into(),
            })),
        })
    }
}

async fn read_head(
    mut body: futures::stream::BoxStream<'static, Result<Vec<u8>, String>>,
    limit: usize,
) -> Vec<u8> {
    let mut out = Vec::new();
    while out.len() < limit {
        match body.next().await {
            Some(Ok(chunk)) => out.extend_from_slice(&chunk),
            _ => break,
        }
    }
    out.truncate(limit);
    out
}

async fn read_all(url: &str, resp: crate::transport::HttpResponse) -> Result<Vec<u8>, FetchError> {
    let mut body = resp.body;
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|message| FetchError::Body {
            url: url.to_owned(),
            received: out.len() as u64,
            message,
        })?;
        out.extend_from_slice(&chunk);
    }
    if let Some(expected) = resp.content_length
        && expected != out.len() as u64
    {
        return Err(FetchError::ContentLength {
            url: url.to_owned(),
            expected,
            received: out.len() as u64,
        });
    }
    Ok(out)
}

type Streamed = (String, u64, Option<String>, Option<String>, bool);

async fn stream_to_file(
    url: &str,
    resp: crate::transport::HttpResponse,
    part: &Path,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
) -> Result<Streamed, FetchError> {
    if let Some(tx) = &events {
        let _ = tx.try_send(Event::DownloadStarted {
            url: url.to_owned(),
            size: resp.content_length,
        });
    }
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(|source| FetchError::CacheIo {
            path: part.display().to_string(),
            source,
        })?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut body = resp.body;
    let mut last_event = 0u64;
    while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(message) => {
                let _ = tokio::fs::remove_file(part).await;
                return Err(FetchError::Body {
                    url: url.to_owned(),
                    received,
                    message,
                });
            }
        };
        hasher.update(&chunk);
        received += chunk.len() as u64;
        file.write_all(&chunk)
            .await
            .map_err(|source| FetchError::CacheIo {
                path: part.display().to_string(),
                source,
            })?;
        if received - last_event >= 4 << 20
            && let Some(tx) = &events
        {
            let _ = tx.try_send(Event::DownloadProgress {
                url: url.to_owned(),
                bytes: received,
            });
            last_event = received;
        }
    }
    file.flush().await.map_err(|source| FetchError::CacheIo {
        path: part.display().to_string(),
        source,
    })?;
    file.sync_all()
        .await
        .map_err(|source| FetchError::CacheIo {
            path: part.display().to_string(),
            source,
        })?;
    if let Some(expected) = resp.content_length
        && expected != received
    {
        let _ = tokio::fs::remove_file(part).await;
        return Err(FetchError::ContentLength {
            url: url.to_owned(),
            expected,
            received,
        });
    }
    Ok((
        hex(&hasher.finalize()),
        received,
        resp.etag,
        resp.last_modified,
        resp.accept_ranges,
    ))
}

#[cfg(test)]
mod tests {
    use nemlake_core::YearMonth;
    use url::Url;

    use super::*;
    use crate::source::FileKind;
    use crate::transport::{Canned, OfflineTransport, StaticTransport};

    fn remote(url: &str) -> RemoteFile {
        RemoteFile {
            url: Url::parse(url).unwrap(),
            source: "nem-mmsdm",
            table: Some("REGION".into()),
            period: Some(YearMonth::new(2024, 1).unwrap()),
            kind: FileKind::MmsZip,
            size: None,
            modified: None,
        }
    }

    fn fast() -> FetcherConfig {
        FetcherConfig {
            attempts: 3,
            backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            min_interval: Duration::ZERO,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn downloads_caches_and_skips_second_time() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/a.zip", Canned::ok(b"zip bytes"));
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let f =
            Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), fast()).with_events(tx);
        assert!(!f.is_offline());
        let local = f.fetch(&remote("https://h/a.zip")).await.unwrap();
        assert!(!local.cached);
        assert_eq!(local.size, 9);
        assert_eq!(local.sha256, crate::cache::sha256_hex(b"zip bytes"));
        assert_eq!(std::fs::read(&local.path).unwrap(), b"zip bytes");
        assert_eq!(local.etag.as_deref(), Some("\"etag\""));
        assert!(!DiskCache::part_path(&local.path).exists());
        let again = f.fetch(&remote("https://h/a.zip")).await.unwrap();
        assert!(again.cached);
        assert_eq!(again.path, local.path);
        assert_eq!(t.request_count(), 1, "the cache answered the second fetch");
        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the fetcher reports a download start");
        assert!(matches!(got, Some(Event::DownloadStarted { .. })));
        assert!(
            rx.try_recv().is_err(),
            "the fetcher reports starts and progress; the pipeline reports completion"
        );
        // Refresh forces a download; verify_cache catches a corrupted cache.
        let mut cfg = fast();
        cfg.refresh = true;
        let f2 = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), cfg);
        assert!(f2.config().refresh);
        assert!(!f2.fetch(&remote("https://h/a.zip")).await.unwrap().cached);
        assert_eq!(t.request_count(), 2);
        std::fs::write(&local.path, b"corrupted").unwrap();
        let mut cfg = fast();
        cfg.verify_cache = true;
        let f3 = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), cfg);
        assert!(
            !f3.fetch(&remote("https://h/a.zip")).await.unwrap().cached,
            "mismatch forced a download"
        );
        assert_eq!(f3.cache().root(), dir.path());
    }

    #[tokio::test]
    async fn retries_then_gives_up_and_404_is_final() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/flaky.zip", Canned::status(503))
            .add("https://h/flaky.zip", Canned::ok(b"ok"));
        let f = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), fast());
        let local = f.fetch(&remote("https://h/flaky.zip")).await.unwrap();
        assert_eq!(local.size, 2);
        assert_eq!(t.request_count(), 2);
        t.add("https://h/down.zip", Canned::status(503));
        let e = f.fetch(&remote("https://h/down.zip")).await.unwrap_err();
        assert!(
            matches!(e, FetchError::RetriesExhausted { attempts: 3, .. }),
            "{e}"
        );
        assert_eq!(t.request_count(), 5);
        t.add("https://h/missing.zip", Canned::status(404));
        let e = f.fetch(&remote("https://h/missing.zip")).await.unwrap_err();
        assert!(
            matches!(e, FetchError::HttpStatus { status: 404, .. }),
            "{e}"
        );
        assert_eq!(t.request_count(), 6, "no retry on 404");
        let mut c = Canned::ok(b"x");
        c.connect_error = Some("refused".into());
        t.add("https://h/refused.zip", c);
        assert!(matches!(
            f.fetch(&remote("https://h/refused.zip")).await.unwrap_err(),
            FetchError::RetriesExhausted { .. }
        ));
    }

    #[tokio::test]
    async fn truncated_and_broken_bodies_are_retried_and_leave_no_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut short = Canned::ok(b"abcdef");
        short.claimed_length = Some(10);
        let mut broken = Canned::ok(b"abcdef");
        broken.fail_after = Some(2);
        t.add("https://h/t.zip", short)
            .add("https://h/t.zip", broken)
            .add("https://h/t.zip", Canned::ok(b"abcdef"));
        let f = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), fast());
        let local = f.fetch(&remote("https://h/t.zip")).await.unwrap();
        assert_eq!(local.size, 6);
        assert_eq!(t.request_count(), 3);
        assert!(!DiskCache::part_path(&local.path).exists());
        // Only truncation, every time.
        let mut short = Canned::ok(b"abc");
        short.claimed_length = Some(10);
        t.add("https://h/short.zip", short);
        let e = f.fetch(&remote("https://h/short.zip")).await.unwrap_err();
        match e {
            FetchError::RetriesExhausted { last, .. } => assert!(matches!(
                *last,
                FetchError::ContentLength {
                    expected: 10,
                    received: 3,
                    ..
                }
            )),
            other => panic!("{other}"),
        }
    }

    #[tokio::test]
    async fn listings_cache_and_offline() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/dir/", Canned::ok(b"<html>listing</html>"));
        let f = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), fast());
        assert_eq!(
            f.fetch_listing("https://h/dir/", 3600).await.unwrap(),
            b"<html>listing</html>"
        );
        assert_eq!(
            f.fetch_listing("https://h/dir/", 3600).await.unwrap(),
            b"<html>listing</html>"
        );
        assert_eq!(t.request_count(), 1);
        let mut short = Canned::ok(b"abc");
        short.claimed_length = Some(10);
        t.add("https://h/bad/", short);
        assert!(matches!(
            f.fetch_listing("https://h/bad/", 3600).await.unwrap_err(),
            FetchError::RetriesExhausted { .. }
        ));
        let off = Fetcher::new(
            Arc::new(OfflineTransport),
            DiskCache::new(dir.path()),
            fast(),
        );
        assert!(off.is_offline());
        assert_eq!(
            off.fetch_listing("https://h/dir/", 3600).await.unwrap(),
            b"<html>listing</html>",
            "cached listing works offline"
        );
        assert!(matches!(
            off.fetch_listing("https://h/other/", 3600).await,
            Err(FetchError::Offline { .. })
        ));
        assert!(matches!(
            off.fetch(&remote("https://h/a.zip")).await,
            Err(FetchError::Offline { .. })
        ));
        // Pacing: two requests respect the minimum interval.
        let mut cfg = fast();
        cfg.min_interval = Duration::from_millis(30);
        cfg.refresh = true;
        let paced = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), cfg);
        let start = Instant::now();
        paced.fetch_listing("https://h/dir/", 0).await.unwrap();
        paced.fetch_listing("https://h/dir/", 0).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(30));
    }
}
