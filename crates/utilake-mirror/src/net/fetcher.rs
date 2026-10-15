//! Downloading: paced, retried, stopped for good when refused, and kept in
//! the archive.
//!
//! A file is downloaded whole, once. Its bytes go from the transport to
//! disk without a copy: each chunk is handed as it arrived to a writer on a
//! blocking thread, which hashes it and writes it through a 1 MiB buffer,
//! so no hashing or file I/O runs on the async runtime.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::clock::now_utc_rfc3339;
use crate::net::Event;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::net::archive::{Archive, Sidecar, hex};
use crate::net::error::FetchError;
use crate::net::source::RemoteFile;
use crate::net::transport::{HttpResponse, Span, Transport};

/// A file on disk, ready to read.
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
    /// Whether it was in the archive already, with no request.
    pub cached: bool,
    /// ETag.
    pub etag: Option<String>,
    /// Last-Modified.
    pub last_modified: Option<String>,
}

/// Knobs.
#[derive(Debug, Clone)]
pub struct FetcherConfig {
    /// Attempts per request.
    pub attempts: u32,
    /// First backoff.
    pub backoff: Duration,
    /// Longest backoff.
    pub max_backoff: Duration,
    /// Minimum gap between two requests.
    pub min_interval: Duration,
}

/// Fetches remote files into the archive.
pub struct Fetcher {
    transport: Arc<dyn Transport>,
    archive: Archive,
    config: FetcherConfig,
    last_request: Mutex<Option<Instant>>,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
    /// Set once the server refuses this client (HTTP 403, or 429 through
    /// every retry): status, the URL refused and the start of its body.
    /// Every later request fails at once without touching the network.
    blocked: std::sync::OnceLock<(u16, String, Vec<u8>)>,
    /// Requests the server answered with 200 and whose body arrived whole.
    succeeded: std::sync::atomic::AtomicU64,
}

/// Chunks in flight between the download and the writer.
const SPOOL_CHUNKS: usize = 16;
/// Bytes the writer gathers before each write.
const WRITE_BUFFER: usize = 1 << 20;

impl Fetcher {
    /// A fetcher over a transport and an archive.
    pub fn new(transport: Arc<dyn Transport>, archive: Archive, config: FetcherConfig) -> Self {
        Self {
            transport,
            archive,
            config,
            last_request: Mutex::new(None),
            blocked: std::sync::OnceLock::new(),
            events: None,
            succeeded: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Requests that succeeded so far: a run that got somewhere.
    pub fn succeeded(&self) -> u64 {
        self.succeeded.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Sends progress events to `tx`.
    #[must_use]
    pub fn with_events(mut self, tx: tokio::sync::mpsc::Sender<Event>) -> Self {
        self.events = Some(tx);
        self
    }

    /// The archive.
    pub fn archive(&self) -> &Archive {
        &self.archive
    }

    /// Whether the server has refused this client during this run.
    pub fn is_blocked(&self) -> bool {
        self.blocked.get().is_some()
    }

    fn blocked_error(&self, url: &str) -> Option<FetchError> {
        self.blocked
            .get()
            .map(|(status, first_url, body_head)| FetchError::Blocked {
                url: url.to_owned(),
                status: *status,
                first_url: first_url.clone(),
                body_head: body_head.clone(),
            })
    }

    fn ensure_online(&self, url: &str) -> Result<(), FetchError> {
        if self.transport.is_offline() {
            return Err(FetchError::Offline {
                url: url.to_owned(),
            });
        }
        Ok(())
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

    /// GETs a page (a directory listing, a report page) into memory.
    pub async fn fetch_listing(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        self.ensure_online(url)?;
        self.get_with_retries(url, None, |resp| read_all(url, resp))
            .await
    }

    /// Bytes `range` of a file, by a ranged request, into memory; not
    /// archived. Paced, retried and refused like every request.
    pub async fn fetch_range(&self, url: &str, range: Span) -> Result<Vec<u8>, FetchError> {
        self.ensure_online(url)?;
        self.get_with_retries(url, Some(range), |resp| read_all(url, resp))
            .await
    }

    /// The archived copy of a remote file, or the file downloaded into the
    /// archive whole, with a sidecar that says where it came from.
    pub async fn fetch(&self, remote: &RemoteFile) -> Result<LocalFile, FetchError> {
        let url = remote.url.as_str();
        if let Some(found) = self.archive.lookup(url)? {
            return Ok(LocalFile {
                remote: remote.clone(),
                path: found.path,
                sha256: found.sidecar.sha256,
                size: found.sidecar.size,
                cached: true,
                etag: found.sidecar.etag,
                last_modified: found.sidecar.last_modified,
            });
        }
        self.ensure_online(url)?;
        let path = self.archive.path_for(url);
        let p = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(p).map_err(|source| FetchError::CacheIo {
            path: p.display().to_string(),
            source,
        })?;
        let part = Archive::part_path(&path);
        let streamed = self
            .get_with_retries(url, None, |resp| {
                stream_to_file(url, resp, &part, self.events.clone())
            })
            .await?;
        std::fs::rename(&part, &path).map_err(|source| FetchError::CacheIo {
            path: path.display().to_string(),
            source,
        })?;
        let sidecar = Sidecar {
            url: url.to_owned(),
            size: streamed.size,
            sha256: streamed.sha256,
            crc32: Some(streamed.crc32),
            fetched_at: now_utc_rfc3339(),
            etag: streamed.etag,
            last_modified: streamed.last_modified,
            tool: format!("nemweb-mirror/{}", env!("CARGO_PKG_VERSION")),
        };
        Archive::write_sidecar(&path, &sidecar)?;
        Ok(LocalFile {
            remote: remote.clone(),
            path,
            sha256: sidecar.sha256,
            size: sidecar.size,
            cached: false,
            etag: sidecar.etag,
            last_modified: sidecar.last_modified,
        })
    }

    /// GETs `url` (only `range` of it, when given) and hands the answer to
    /// `consume` (a 200, or a 206 for a range), pacing every attempt,
    /// retrying what is worth retrying with backoff, and tripping the
    /// breaker on a refusal.
    async fn get_with_retries<T, F, Fut>(
        &self,
        url: &str,
        range: Option<Span>,
        consume: F,
    ) -> Result<T, FetchError>
    where
        F: Fn(HttpResponse) -> Fut,
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
            // A server that has refused us is left alone for the rest of the run.
            if let Some(e) = self.blocked_error(url) {
                return Err(e);
            }
            self.pace().await;
            let answered = if range.is_some() { 206 } else { 200 };
            let result = match self.transport.get(url, range).await {
                Err(e) => Err(e),
                Ok(resp) if resp.status == answered => consume(resp).await,
                // The whole file instead of the range: not read.
                Ok(resp) if resp.status == 200 => Err(FetchError::RangeUnsupported {
                    url: url.to_owned(),
                }),
                Ok(resp) if resp.status == 403 => {
                    let head = read_head(resp.body, 256).await;
                    let _ = self.blocked.set((403, url.to_owned(), head));
                    return Err(self.blocked_error(url).expect("just set"));
                }
                Ok(resp) => Err(FetchError::HttpStatus {
                    url: url.to_owned(),
                    status: resp.status,
                    body_head: read_head(resp.body, 256).await,
                }),
            };
            match result {
                Ok(v) => {
                    self.succeeded
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(v);
                }
                Err(e) if e.is_retryable() => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        // Too Many Requests through every retry is the same refusal, stated politely.
        if let Some(FetchError::HttpStatus {
            status: 429,
            body_head,
            ..
        }) = &last
        {
            let _ = self.blocked.set((429, url.to_owned(), body_head.clone()));
            return Err(self.blocked_error(url).expect("just set"));
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

async fn read_head(mut body: BoxStream<'static, Result<Bytes, String>>, limit: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(limit);
    while out.len() < limit {
        match body.next().await {
            Some(Ok(chunk)) => {
                let take = chunk.len().min(limit - out.len());
                out.extend_from_slice(&chunk[..take]);
            }
            _ => break,
        }
    }
    out
}

async fn read_all(url: &str, resp: HttpResponse) -> Result<Vec<u8>, FetchError> {
    let mut body = resp.body;
    let expected = resp.content_length;
    let room = expected.map_or(64 << 10, |n| usize::try_from(n).unwrap_or(0));
    let mut out = Vec::with_capacity(room);
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|message| FetchError::Body {
            url: url.to_owned(),
            received: out.len() as u64,
            message,
        })?;
        out.extend_from_slice(&chunk);
    }
    if let Some(expected) = expected
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

/// A completed download: its bytes' hash and size, and what the server said.
struct Streamed {
    sha256: String,
    crc32: u32,
    size: u64,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Writes the chunks it receives to `part` through a 1 MiB buffer, taking
/// their sha256 and CRC-32 on the way, and makes the file durable; the
/// hashes and the size. Runs on a blocking thread.
fn spool(
    part: &Path,
    mut rx: tokio::sync::mpsc::Receiver<Bytes>,
) -> std::io::Result<(String, u32, u64)> {
    let file = std::fs::File::create(part)?;
    let mut out = std::io::BufWriter::with_capacity(WRITE_BUFFER, file);
    let mut hasher = Sha256::new();
    let mut crc = crc32fast::Hasher::new();
    let mut size = 0u64;
    while let Some(chunk) = rx.blocking_recv() {
        hasher.update(&chunk);
        crc.update(&chunk);
        out.write_all(&chunk)?;
        size += chunk.len() as u64;
    }
    let file = out
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    file.sync_all()?;
    Ok((hex(&hasher.finalize()), crc.finalize(), size))
}

async fn stream_to_file(
    url: &str,
    resp: HttpResponse,
    part: &Path,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
) -> Result<Streamed, FetchError> {
    let total = resp.content_length;
    if let Some(tx) = &events {
        let _ = tx.try_send(Event::DownloadStarted {
            url: url.to_owned(),
            size: total,
        });
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(SPOOL_CHUNKS);
    let owned = part.to_owned();
    let writer = tokio::task::spawn_blocking(move || spool(&owned, rx));
    let mut body = resp.body;
    let mut received: u64 = 0;
    let mut last_event = 0u64;
    let mut broken = None;
    while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(message) => {
                broken = Some(message);
                break;
            }
        };
        received += chunk.len() as u64;
        // The writer stopped: its error says why.
        if tx.send(chunk).await.is_err() {
            break;
        }
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
    drop(tx);
    let written = writer
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)));
    let fail = |e: FetchError| {
        let _ = std::fs::remove_file(part);
        Err(e)
    };
    if let Some(message) = broken {
        return fail(FetchError::Body {
            url: url.to_owned(),
            received,
            message,
        });
    }
    let (sha256, crc32, size) = match written {
        Ok(w) => w,
        Err(source) => {
            return fail(FetchError::CacheIo {
                path: part.display().to_string(),
                source,
            });
        }
    };
    if let Some(expected) = total
        && expected != size
    {
        return fail(FetchError::ContentLength {
            url: url.to_owned(),
            expected,
            received: size,
        });
    }
    Ok(Streamed {
        sha256,
        crc32,
        size,
        etag: resp.etag,
        last_modified: resp.last_modified,
    })
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use crate::net::archive::sha256_hex;
    use crate::net::transport::{Canned, OfflineTransport, StaticTransport};

    fn remote(url: &str) -> RemoteFile {
        RemoteFile {
            url: Url::parse(url).unwrap(),
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
        }
    }

    #[tokio::test]
    async fn downloads_once_into_the_archive_with_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://h/Data_Archive/DATA/a.zip";
        let t = Arc::new(StaticTransport::default());
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        t.add(url, Canned::ok(&body));
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast()).with_events(tx);
        let local = f.fetch(&remote(url)).await.unwrap();
        assert!(!local.cached);
        assert_eq!(local.size, body.len() as u64);
        assert_eq!(local.sha256, sha256_hex(&body));
        assert_eq!(local.path, dir.path().join("h/Data_Archive/DATA/a.zip"));
        assert_eq!(std::fs::read(&local.path).unwrap(), body);
        assert_eq!(local.etag.as_deref(), Some("\"etag\""));
        assert!(!Archive::part_path(&local.path).exists());
        let sc = Archive::read_sidecar(&local.path).unwrap().unwrap();
        assert_eq!(sc.url, url);
        assert_eq!(sc.sha256, local.sha256);
        assert_eq!(
            sc.tool,
            format!("nemweb-mirror/{}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(sc.crc32, Some(crc32fast::hash(&body)));
        assert_eq!(
            sc.last_modified.as_deref(),
            Some("Tue, 13 Feb 2024 08:27:00 GMT")
        );
        // Once archived, it is never asked for again.
        let again = f.fetch(&remote(url)).await.unwrap();
        assert!(again.cached);
        assert_eq!((again.path, again.sha256), (local.path, local.sha256));
        assert_eq!(t.request_count(), 1);
        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the fetcher reports a download start");
        assert!(matches!(got, Some(Event::DownloadStarted { .. })));
        assert_eq!(f.archive().root(), dir.path());
    }

    #[tokio::test]
    async fn retries_then_gives_up_and_404_is_final() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/flaky.zip", Canned::status(503))
            .add("https://h/flaky.zip", Canned::ok(b"ok"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
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
    async fn a_refusal_stops_every_later_request() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut refusal = Canned::status(403);
        refusal.body = Bytes::from_static(b"Website is unavailable");
        t.add("https://h/a.zip", refusal);
        t.add("https://h/b.zip", Canned::ok(b"fine"));
        t.add("https://h/listing/", Canned::ok(b"<pre></pre>"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        assert!(!f.is_blocked());
        // A 403 is not retried: one request, and the breaker trips.
        let e = f.fetch(&remote("https://h/a.zip")).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::Blocked { status: 403, first_url, .. } if first_url == "https://h/a.zip"),
            "{e}"
        );
        assert_eq!(t.request_count(), 1);
        assert!(f.is_blocked());
        let d = crate::diag::Diagnose::diagnostic(&e);
        assert_eq!(d.code, "net.blocked");
        assert!(d.sample.is_some());
        assert!(e.to_string().contains("request_interval_ms"), "{e}");
        // Everything after it fails at once and sends nothing.
        let e = f.fetch(&remote("https://h/b.zip")).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::Blocked { url, first_url, .. } if url == "https://h/b.zip" && first_url == "https://h/a.zip"),
            "{e}"
        );
        assert!(matches!(
            f.fetch_listing("https://h/listing/").await,
            Err(FetchError::Blocked { .. })
        ));
        assert_eq!(t.request_count(), 1, "no request after the refusal");

        // 429 is retried with backoff; when every attempt is refused the breaker trips too.
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/busy.zip", Canned::status(429));
        t.add("https://h/b.zip", Canned::ok(b"fine"));
        let f = Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(dir.path().join("two")),
            fast(),
        );
        let e = f.fetch(&remote("https://h/busy.zip")).await.unwrap_err();
        assert!(matches!(&e, FetchError::Blocked { status: 429, .. }), "{e}");
        assert_eq!(t.request_count(), 3);
        assert!(f.fetch(&remote("https://h/b.zip")).await.is_err());
        assert_eq!(t.request_count(), 3);
    }

    #[tokio::test]
    async fn truncated_and_broken_bodies_are_retried_and_leave_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut short = Canned::ok(b"abcdef");
        short.claimed_length = Some(10);
        let mut broken = Canned::ok(b"abcdef");
        broken.fail_after = Some(2);
        t.add("https://h/t.zip", short)
            .add("https://h/t.zip", broken)
            .add("https://h/t.zip", Canned::ok(b"abcdef"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let local = f.fetch(&remote("https://h/t.zip")).await.unwrap();
        assert_eq!(local.size, 6);
        assert_eq!(t.request_count(), 3);
        assert!(!Archive::part_path(&local.path).exists());
        // Only truncation, every time: no file, no part, no sidecar.
        let mut short = Canned::ok(b"abc");
        short.claimed_length = Some(10);
        t.add("https://h/short.zip", short);
        let e = f.fetch(&remote("https://h/short.zip")).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::RetriesExhausted { last, .. }
                if matches!(**last, FetchError::ContentLength { expected: 10, received: 3, .. })),
            "{e}"
        );
        let path = f.archive().path_for("https://h/short.zip");
        assert!(!path.exists() && !Archive::part_path(&path).exists());
        assert!(f.archive().lookup("https://h/short.zip").unwrap().is_none());
    }

    #[tokio::test]
    async fn listings_offline_and_pacing() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/dir/", Canned::ok(b"<html>listing</html>"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        assert_eq!(
            f.fetch_listing("https://h/dir/").await.unwrap(),
            b"<html>listing</html>"
        );
        let mut short = Canned::ok(b"abc");
        short.claimed_length = Some(10);
        t.add("https://h/bad/", short);
        assert!(matches!(
            f.fetch_listing("https://h/bad/").await.unwrap_err(),
            FetchError::RetriesExhausted { .. }
        ));
        // Offline: an archived file serves, nothing else does.
        t.add("https://h/a.zip", Canned::ok(b"zip"));
        f.fetch(&remote("https://h/a.zip")).await.unwrap();
        let off = Fetcher::new(Arc::new(OfflineTransport), Archive::new(dir.path()), fast());
        assert!(off.fetch(&remote("https://h/a.zip")).await.unwrap().cached);
        assert!(matches!(
            off.fetch_listing("https://h/dir/").await,
            Err(FetchError::Offline { .. })
        ));
        assert!(matches!(
            off.fetch(&remote("https://h/b.zip")).await,
            Err(FetchError::Offline { .. })
        ));
        // Pacing: two requests respect the minimum interval.
        let mut cfg = fast();
        cfg.min_interval = Duration::from_millis(30);
        let paced = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), cfg);
        let start = Instant::now();
        paced.fetch_listing("https://h/dir/").await.unwrap();
        paced.fetch_listing("https://h/dir/").await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(30));
    }

    #[tokio::test]
    async fn the_archive_failing_to_take_a_file_is_a_cache_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        // More chunks than the channel to the writer holds.
        let mut many = Canned::ok(&[1u8; 64 << 10]);
        many.chunk = 1 << 10;
        t.add("https://h/a/b.zip", many);
        // The archive's root is a file: no folder can be made under it.
        let root = dir.path().join("file");
        std::fs::write(&root, "").unwrap();
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(&root), fast());
        let e = f.fetch(&remote("https://h/a/b.zip")).await.unwrap_err();
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
        // The part file cannot be created: the writer stops, its error says
        // why, and the body stops.
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let path = f.archive().path_for("https://h/a/b.zip");
        std::fs::create_dir_all(Archive::part_path(&path)).unwrap();
        let e = f.fetch(&remote("https://h/a/b.zip")).await.unwrap_err();
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
        std::fs::remove_dir(Archive::part_path(&path)).unwrap();
        // The file's own path is a folder with something in it: no rename.
        std::fs::create_dir_all(path.join("x")).unwrap();
        let e = f.fetch(&remote("https://h/a/b.zip")).await.unwrap_err();
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
    }

    #[tokio::test]
    async fn a_listing_whose_body_breaks_and_progress_on_a_big_file() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut broken = Canned::ok(b"0123456789");
        broken.fail_after = Some(4);
        t.add("https://h/list/", broken);
        let big = vec![7u8; 9 << 20];
        let mut c = Canned::ok(&big);
        c.chunk = 1 << 20;
        t.add("https://h/big.zip", c);
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast()).with_events(tx);
        let e = f.fetch_listing("https://h/list/").await.unwrap_err();
        assert!(matches!(e, FetchError::RetriesExhausted { .. }), "{e}");
        f.fetch(&remote("https://h/big.zip")).await.unwrap();
        let mut progress = 0;
        while let Ok(ev) = rx.try_recv() {
            progress += usize::from(matches!(ev, Event::DownloadProgress { .. }));
        }
        assert_eq!(progress, 2, "one every 4 MiB");
    }

    #[tokio::test]
    async fn a_range_of_a_file_and_a_server_that_sends_it_all() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/z.zip", Canned::ok(b"0123456789"));
        let mut whole = Canned::ok(b"0123456789");
        whole.ranges = false;
        t.add("https://h/w.zip", whole);
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let span = Span { start: 6, end: 10 };
        assert_eq!(
            f.fetch_range("https://h/z.zip", span).await.unwrap(),
            b"6789"
        );
        assert_eq!(f.succeeded(), 1);
        let e = f.fetch_range("https://h/w.zip", span).await.unwrap_err();
        assert!(matches!(e, FetchError::RangeUnsupported { .. }), "{e}");
        assert_eq!(t.request_count(), 2, "not retried");
        let off = Fetcher::new(Arc::new(OfflineTransport), Archive::new(dir.path()), fast());
        assert!(matches!(
            off.fetch_range("https://h/z.zip", span).await,
            Err(FetchError::Offline { .. })
        ));
    }
}
