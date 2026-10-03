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
use crate::remotezip::{self, Inflater, Tail, ZipMember};
use crate::source::RemoteFile;
use crate::transport::{ByteRange, HttpRequest, Transport};

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
            min_interval: Duration::from_millis(250),
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
    /// Set once the server refuses this client (HTTP 403, or 429 through
    /// every retry): status, the URL refused and the start of its body.
    /// Every later request fails at once without touching the network.
    blocked: std::sync::OnceLock<(u16, String, Vec<u8>)>,
}

impl Fetcher {
    /// A fetcher over a transport and cache.
    pub fn new(transport: Arc<dyn Transport>, cache: DiskCache, config: FetcherConfig) -> Self {
        Self {
            transport,
            cache,
            config,
            last_request: Mutex::new(None),
            blocked: std::sync::OnceLock::new(),
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

    /// GETs part of `url` into memory, paced and retried like every other
    /// request.
    pub async fn fetch_range(&self, url: &str, range: ByteRange) -> Result<Vec<u8>, FetchError> {
        let req = HttpRequest::get(url).with_range(range);
        self.get_with_retries(&req, |resp| async move { read_all(url, resp).await })
            .await
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
            .get_with_retries(&HttpRequest::get(url), |resp| async move {
                read_all(url, resp).await
            })
            .await?;
        self.cache.store_listing(url, &body)?;
        Ok(body)
    }

    /// Reads the index of a remote zip with ranged requests: the names,
    /// sizes and offsets of its members. The result is cached like a listing.
    pub async fn list_zip_members(
        &self,
        url: &str,
        max_age_secs: i64,
    ) -> Result<Vec<ZipMember>, FetchError> {
        let key = format!("{url}#central-directory");
        if !self.config.refresh
            && let Some(body) = self.cache.lookup_listing(&key, max_age_secs)?
            && let Ok(members) = serde_json::from_slice::<Vec<ZipMember>>(&body)
        {
            return Ok(members);
        }
        if self.transport.is_offline() {
            return Err(FetchError::Offline {
                url: url.to_owned(),
            });
        }
        let bad = |message: String| FetchError::RemoteZip {
            url: url.to_owned(),
            message,
        };
        let tail_req = HttpRequest::get(url).with_range(ByteRange::Last {
            len: remotezip::TAIL_BYTES,
        });
        let (tail, total) = self
            .get_with_retries(&tail_req, |resp| async move {
                let total = resp.total_length;
                Ok((read_all(url, resp).await?, total))
            })
            .await?;
        let total = total.ok_or_else(|| {
            bad("the server did not say how long the file is (no Content-Range)".into())
        })?;
        let tail_start = total.saturating_sub(tail.len() as u64);
        let location = match remotezip::parse_tail(&tail, tail_start).map_err(bad)? {
            Tail::Located(l) => l,
            Tail::NeedEocd64 { offset } => {
                let rec = self.get_span(url, offset, offset + 55).await?;
                remotezip::parse_eocd64(&rec).map_err(bad)?
            }
        };
        if location.offset.saturating_add(location.size) > total {
            return Err(bad(format!(
                "its directory is recorded at bytes {}..{} but the file has {total}",
                location.offset,
                location.offset.saturating_add(location.size)
            )));
        }
        let directory = if location.size == 0 {
            Vec::new()
        } else if location.offset >= tail_start {
            let from = (location.offset - tail_start) as usize;
            tail[from..from + location.size as usize].to_vec()
        } else {
            self.get_span(url, location.offset, location.offset + location.size - 1)
                .await?
        };
        let members = remotezip::parse_directory(&directory).map_err(bad)?;
        if members.len() as u64 != location.entries {
            return Err(bad(format!(
                "its end record counts {} members but the directory lists {}",
                location.entries,
                members.len()
            )));
        }
        let json = serde_json::to_vec(&members).expect("members serialise");
        self.cache.store_listing(&key, &json)?;
        Ok(members)
    }

    /// GETs bytes `start..=end` of `url` into memory.
    async fn get_span(&self, url: &str, start: u64, end: u64) -> Result<Vec<u8>, FetchError> {
        self.fetch_range(url, ByteRange::Span { start, end }).await
    }

    /// Fetches a remote file into the cache, or returns the cached copy.
    /// A file that is a member of a remote zip is extracted with a ranged
    /// request for its bytes alone.
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
        let (sha256, size, etag, last_modified, accept_ranges) = match &remote.member {
            None => {
                self.get_with_retries(&HttpRequest::get(url), |resp| {
                    let part = part.clone();
                    let events = events.clone();
                    async move { stream_to_file(url, resp, &part, events).await }
                })
                .await?
            }
            Some(member) => {
                let container = remote.request_url();
                let container = container.as_str();
                let unsupported = |reason: String| FetchError::MemberUnsupported {
                    url: container.to_owned(),
                    member: member.name.clone(),
                    reason,
                };
                if member.flags & 1 != 0 {
                    return Err(unsupported("it is encrypted".into()));
                }
                if member.method != 0 && member.method != 8 {
                    return Err(unsupported(format!(
                        "it uses zip compression method {}, and only stored (0) and deflate (8) are read",
                        member.method
                    )));
                }
                let (start, end) = member.request_span(None);
                let req = HttpRequest::get(container).with_range(ByteRange::Span { start, end });
                self.get_with_retries(&req, |resp| {
                    let part = part.clone();
                    let events = events.clone();
                    async move {
                        stream_member_to_file(container, url, resp, &part, member, events).await
                    }
                })
                .await?
            }
        };
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

    /// Sends `req` with retries and backoff, handing the response to
    /// `consume` when it is a 200, or a 206 for a ranged request.
    async fn get_with_retries<T, F, Fut>(
        &self,
        req: &HttpRequest,
        consume: F,
    ) -> Result<T, FetchError>
    where
        F: Fn(crate::transport::HttpResponse) -> Fut,
        Fut: std::future::Future<Output = Result<T, FetchError>>,
    {
        let url = req.url.as_str();
        let want = if req.range.is_some() { 206 } else { 200 };
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
            let result = match self.transport.get(req.clone()).await {
                Err(e) => Err(e),
                Ok(resp) if resp.status == want => consume(resp).await,
                Ok(resp) if resp.status == 403 => {
                    let head = read_head(resp.body, 256).await;
                    let _ = self.blocked.set((403, url.to_owned(), head));
                    return Err(self.blocked_error(url).expect("just set"));
                }
                // A server without range support answers 200 with the whole
                // file; reading that would defeat the point.
                Ok(resp) if want == 206 && resp.status == 200 => Err(FetchError::RangeIgnored {
                    url: url.to_owned(),
                    status: resp.status,
                }),
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

/// Streams the ranged response for one zip member into `part`: skips the
/// local file header, inflates the member's data, and checks the result
/// against the size and CRC-32 the zip's directory recorded.
async fn stream_member_to_file(
    container: &str,
    identity: &str,
    resp: crate::transport::HttpResponse,
    part: &Path,
    member: &ZipMember,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
) -> Result<Streamed, FetchError> {
    if let Some(tx) = &events {
        let _ = tx.try_send(Event::DownloadStarted {
            url: identity.to_owned(),
            size: Some(member.compressed_size),
        });
    }
    let cache_io = |source| FetchError::CacheIo {
        path: part.display().to_string(),
        source,
    };
    let mut file = tokio::fs::File::create(part).await.map_err(cache_io)?;
    let result = async {
        let mut sha = Sha256::new();
        let mut crc = crc32fast::Hasher::new();
        let mut inflater = Inflater::new();
        let mut out: Vec<u8> = Vec::new();
        let mut header: Vec<u8> = Vec::new();
        let mut in_data = false;
        let mut remaining = member.compressed_size;
        let mut received: u64 = 0;
        let mut written: u64 = 0;
        let mut last_event = 0u64;
        let mut body = resp.body;
        while remaining > 0 {
            let Some(chunk) = body.next().await else {
                break;
            };
            let chunk = chunk.map_err(|message| FetchError::Body {
                url: container.to_owned(),
                received,
                message,
            })?;
            received += chunk.len() as u64;
            // The local header comes first; its length is only known once
            // its 30 fixed bytes have arrived.
            let data: Vec<u8> = if in_data {
                chunk
            } else {
                header.extend_from_slice(&chunk);
                let offset = remotezip::local_data_offset(&header).map_err(|message| {
                    FetchError::RemoteZip {
                        url: container.to_owned(),
                        message: format!("member {}: {message}", member.name),
                    }
                })?;
                match offset {
                    Some(o) if header.len() >= o => {
                        in_data = true;
                        header.split_off(o)
                    }
                    _ => continue,
                }
            };
            let take = (data.len() as u64).min(remaining) as usize;
            remaining -= take as u64;
            out.clear();
            if member.method == 0 {
                out.extend_from_slice(&data[..take]);
            } else {
                inflater.feed(&data[..take], &mut out).map_err(|message| {
                    FetchError::RemoteZip {
                        url: container.to_owned(),
                        message: format!("member {} does not inflate: {message}", member.name),
                    }
                })?;
            }
            sha.update(&out);
            crc.update(&out);
            written += out.len() as u64;
            file.write_all(&out).await.map_err(cache_io)?;
            if received - last_event >= 4 << 20
                && let Some(tx) = &events
            {
                let _ = tx.try_send(Event::DownloadProgress {
                    url: identity.to_owned(),
                    bytes: received,
                });
                last_event = received;
            }
        }
        if remaining > 0 {
            return Err(FetchError::ContentLength {
                url: container.to_owned(),
                expected: member.compressed_size,
                received: member.compressed_size - remaining,
            });
        }
        file.flush().await.map_err(cache_io)?;
        file.sync_all().await.map_err(cache_io)?;
        let mismatch = |what, expected: String, observed: String| FetchError::MemberMismatch {
            url: container.to_owned(),
            member: member.name.clone(),
            what,
            expected,
            observed,
        };
        if written != member.uncompressed_size {
            return Err(mismatch(
                "size",
                member.uncompressed_size.to_string(),
                written.to_string(),
            ));
        }
        let crc = crc.finalize();
        if crc != member.crc32 {
            return Err(mismatch(
                "crc32",
                format!("{:08x}", member.crc32),
                format!("{crc:08x}"),
            ));
        }
        Ok((hex(&sha.finalize()), written))
    }
    .await;
    match result {
        Ok((sha256, size)) => Ok((sha256, size, resp.etag, resp.last_modified, true)),
        Err(e) => {
            let _ = tokio::fs::remove_file(part).await;
            Err(e)
        }
    }
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
            member: None,
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
    async fn a_refusal_stops_every_later_request() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut refusal = Canned::status(403);
        refusal.body = b"Website is unavailable".to_vec();
        t.add("https://h/a.zip", refusal);
        t.add("https://h/b.zip", Canned::ok(b"fine"));
        t.add("https://h/listing/", Canned::ok(b"<pre></pre>"));
        let f = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), fast());
        assert!(!f.is_blocked());
        // A 403 is not retried: one request, and the breaker trips.
        let e = f.fetch(&remote("https://h/a.zip")).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::Blocked { status: 403, first_url, .. } if first_url == "https://h/a.zip"),
            "{e}"
        );
        assert_eq!(t.request_count(), 1);
        assert!(f.is_blocked());
        let d = nemlake_core::Diagnose::diagnostic(&e);
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
            f.fetch_listing("https://h/listing/", 60).await,
            Err(FetchError::Blocked { .. })
        ));
        assert!(matches!(
            f.fetch_range("https://h/b.zip", ByteRange::Last { len: 2 })
                .await,
            Err(FetchError::Blocked { .. })
        ));
        assert_eq!(t.request_count(), 1, "no request after the refusal");

        // 429 is retried with backoff; when every attempt is refused the breaker trips too.
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/busy.zip", Canned::status(429));
        t.add("https://h/b.zip", Canned::ok(b"fine"));
        let f = Fetcher::new(
            Arc::new(t.clone()),
            DiskCache::new(dir.path().join("two")),
            fast(),
        );
        let e = f.fetch(&remote("https://h/busy.zip")).await.unwrap_err();
        assert!(matches!(&e, FetchError::Blocked { status: 429, .. }), "{e}");
        assert_eq!(t.request_count(), 3);
        assert!(f.fetch(&remote("https://h/b.zip")).await.is_err());
        assert_eq!(t.request_count(), 3);

        // A ranged read works through the same path when nothing is wrong.
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/b.zip", Canned::ok(b"0123456789"));
        let f = Fetcher::new(
            Arc::new(t.clone()),
            DiskCache::new(dir.path().join("three")),
            fast(),
        );
        assert_eq!(
            f.fetch_range("https://h/b.zip", ByteRange::Span { start: 2, end: 4 })
                .await
                .unwrap(),
            b"234"
        );
        assert!(!f.is_blocked());
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
