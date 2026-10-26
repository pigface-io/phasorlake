//! Requests: paced, retried, stopped for good when refused; and a body
//! streamed to disk.
//!
//! [`Requests`] is what every fetcher of the family sends through: at least
//! `min_interval` between two requests, retries with backoff for what is
//! worth retrying, and a breaker that stops every later request at once
//! once the server refuses this client. [`stream_to_file`] hands a body's
//! chunks, as they arrive, to a writer on a blocking thread, which hashes
//! them and writes them through a 1 MiB buffer, so no hashing or file I/O
//! runs on the async runtime.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use sha2::Digest;
use tokio::sync::Mutex;
use utilake_core::event::Event;
use utilake_provenance::stamp::Stamp;

use crate::archive::{Sha256, to_disk};
use crate::error::FetchError;
use crate::transport::{HttpResponse, Span, Transport};

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

impl Default for FetcherConfig {
    fn default() -> Self {
        Self {
            attempts: 5,
            backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            min_interval: Duration::from_millis(250),
        }
    }
}

/// Bytes of a file got by a ranged request, and what the server said of
/// the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeRead {
    /// The bytes asked for.
    pub bytes: Vec<u8>,
    /// The whole file's size, from `Content-Range`.
    pub total_size: Option<u64>,
    /// The file's `ETag`.
    pub etag: Option<String>,
    /// The file's `Last-Modified`.
    pub last_modified: Option<String>,
}

/// Every request to one server goes through here.
pub struct Requests {
    transport: Arc<dyn Transport>,
    config: FetcherConfig,
    last_request: Mutex<Option<Instant>>,
    /// Set once the server refuses this client (HTTP 403, or 429 through
    /// every retry): status, the URL refused and the start of its body.
    /// Every later request fails at once without touching the network.
    blocked: OnceLock<(u16, String, Vec<u8>)>,
    /// Requests the server answered as asked and whose body arrived whole.
    succeeded: AtomicU64,
}

impl Requests {
    /// Requests over `transport`, paced and retried by `config`.
    pub fn new(transport: Arc<dyn Transport>, config: FetcherConfig) -> Self {
        Self {
            transport,
            config,
            last_request: Mutex::new(None),
            blocked: OnceLock::new(),
            succeeded: AtomicU64::new(0),
        }
    }

    /// The knobs.
    pub fn config(&self) -> &FetcherConfig {
        &self.config
    }

    /// Requests that succeeded so far: a run that got somewhere.
    pub fn succeeded(&self) -> u64 {
        self.succeeded.load(Ordering::Relaxed)
    }

    /// Whether the server has refused this client during this run.
    pub fn is_blocked(&self) -> bool {
        self.blocked.get().is_some()
    }

    /// The refusal a request of `url` meets, once the server refused.
    pub fn blocked_error(&self, url: &str) -> Option<FetchError> {
        self.blocked
            .get()
            .map(|(status, first_url, body_head)| FetchError::Blocked {
                url: url.to_owned(),
                status: *status,
                first_url: first_url.clone(),
                body_head: body_head.clone(),
            })
    }

    /// Trips the breaker on a refusal of `url` (unless it is tripped
    /// already) and gives the refusal.
    fn refuse(&self, status: u16, url: &str, body_head: Vec<u8>) -> FetchError {
        let _ = self.blocked.set((status, url.to_owned(), body_head));
        let (status, first_url, body_head) = self.blocked.get().cloned().unwrap_or_default();
        FetchError::Blocked {
            url: url.to_owned(),
            status,
            first_url,
            body_head,
        }
    }

    /// Fails with [`FetchError::Offline`] when the transport reaches no
    /// network.
    pub fn ensure_online(&self, url: &str) -> Result<(), FetchError> {
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

    /// Bytes `span` of a file, by a ranged request, into memory, with what
    /// the server said of the whole file. A server that answers with the
    /// whole file instead is [`FetchError::RangeIgnored`].
    pub async fn fetch_range(&self, url: &str, span: Span) -> Result<RangeRead, FetchError> {
        self.ensure_online(url)?;
        self.get_with_retries(url, Some(span), |resp| async move {
            let (total_size, etag, last_modified) = (
                resp.total_size,
                resp.etag.clone(),
                resp.last_modified.clone(),
            );
            let bytes = read_all(url, resp).await?;
            Ok(RangeRead {
                bytes,
                total_size,
                etag,
                last_modified,
            })
        })
        .await
    }

    /// GETs `url` (only `span` of it, when given) and hands the answer to
    /// `consume` (a 200, or a 206 for a span), pacing every attempt,
    /// retrying what is worth retrying with backoff, and tripping the
    /// breaker on a refusal.
    pub async fn get_with_retries<T, F, Fut>(
        &self,
        url: &str,
        span: Option<Span>,
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
            let answered = if span.is_some() { 206 } else { 200 };
            let sent = match span {
                Some(s) => self.transport.get_range(url, s).await,
                None => self.transport.get(url).await,
            };
            let result = match sent {
                Err(e) => Err(e),
                Ok(resp) if resp.status == answered => consume(resp).await,
                // The whole file instead of the span: not read.
                Ok(resp) if resp.status == 200 => Err(FetchError::RangeIgnored {
                    url: url.to_owned(),
                }),
                Ok(resp) if resp.status == 403 => {
                    let head = read_head(resp.body, 256).await;
                    return Err(self.refuse(403, url, head));
                }
                Ok(resp) => Err(FetchError::HttpStatus {
                    url: url.to_owned(),
                    status: resp.status,
                    body_head: read_head(resp.body, 256).await,
                }),
            };
            match result {
                Ok(v) => {
                    self.succeeded.fetch_add(1, Ordering::Relaxed);
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
        }) = last
        {
            return Err(self.refuse(429, url, body_head));
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

/// The first `limit` bytes of a body, or fewer when it ends or fails.
pub async fn read_head(
    mut body: BoxStream<'static, Result<Bytes, String>>,
    limit: usize,
) -> Vec<u8> {
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

/// A whole body into memory, held to its `Content-Length`.
pub async fn read_all(url: &str, resp: HttpResponse) -> Result<Vec<u8>, FetchError> {
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

/// A completed download: its bytes' hashes and size, and what the server
/// said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Streamed {
    /// The sha256 of every byte of the file.
    pub sha256: Sha256,
    /// Their CRC-32.
    pub crc32: u32,
    /// The file's size.
    pub size: u64,
    /// The server's `ETag`.
    pub etag: Option<String>,
    /// The server's `Last-Modified`, as sent.
    pub last_modified: Option<String>,
}

/// What a download said of the file when it began: kept beside its part,
/// so a later run continues it only if the server still has that file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PartMeta {
    /// The whole file's size, when the server said.
    pub size: Option<u64>,
    /// Its `ETag`.
    pub etag: Option<String>,
    /// The `Last-Modified` header as sent; what a later run compares.
    pub last_modified: Option<String>,
    /// The same as an instant (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_at: Option<Stamp>,
}

/// Where a part's [`PartMeta`] is kept.
pub fn part_meta(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_owned();
    s.push(".resume");
    PathBuf::from(s)
}

/// Chunks in flight between the download and the writer.
const SPOOL_CHUNKS: usize = 16;
/// Bytes the writer gathers before each write.
const WRITE_BUFFER: usize = 1 << 20;

/// Writes the chunks it receives to `part` through a 1 MiB buffer, taking
/// their sha256 and CRC-32 on the way, and makes the file durable; the
/// hashes and the size. With `from`, the part's first `from` bytes are kept
/// (and hashed first) and the chunks follow them. Runs on a blocking thread.
fn spool(
    part: &Path,
    from: u64,
    mut rx: tokio::sync::mpsc::Receiver<Bytes>,
) -> std::io::Result<(Sha256, u32, u64)> {
    let mut hasher = sha2::Sha256::new();
    let mut crc = crc32fast::Hasher::new();
    let file = if from > 0 {
        use std::io::{Read as _, Seek as _};
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(part)?;
        f.set_len(from)?;
        let mut buf = vec![0u8; WRITE_BUFFER];
        let mut kept = (&mut f).take(from);
        loop {
            match kept.read(&mut buf)? {
                0 => break,
                n => {
                    hasher.update(&buf[..n]);
                    crc.update(&buf[..n]);
                }
            }
        }
        f.seek(std::io::SeekFrom::End(0))?;
        f
    } else {
        std::fs::File::create(part)?
    };
    let mut out = std::io::BufWriter::with_capacity(WRITE_BUFFER, file);
    let mut size = from;
    while let Some(chunk) = rx.blocking_recv() {
        hasher.update(&chunk);
        crc.update(&chunk);
        out.write_all(&chunk)?;
        size += chunk.len() as u64;
    }
    let file = out
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    to_disk(&file)?;
    Ok((Sha256(hasher.finalize().into()), crc.finalize(), size))
}

/// Streams `resp`'s body to `part`, after the part's first `from` bytes
/// when it continues one, reporting progress to `events`. With `resume`,
/// a download cut off keeps its part and what the server said of the file
/// when it began ([`PartMeta`]), for a later run to continue; without, a
/// part cut off is removed. A part of the wrong length is always removed.
pub async fn stream_to_file(
    url: &str,
    resp: HttpResponse,
    part: &Path,
    from: u64,
    resume: bool,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
) -> Result<Streamed, FetchError> {
    // The whole file's size: what is kept plus what comes.
    let total = resp.content_length.map(|n| n + from);
    if resume && from == 0 {
        let meta = PartMeta {
            size: total,
            etag: resp.etag.clone(),
            last_modified: resp.last_modified.clone(),
            last_modified_at: resp
                .last_modified
                .as_deref()
                .and_then(Stamp::from_http_date),
        };
        if let Ok(text) = serde_json::to_vec(&meta) {
            let _ = std::fs::write(part_meta(part), text);
        }
    }
    if let Some(tx) = &events {
        let _ = tx.try_send(Event::DownloadStarted {
            url: url.to_owned(),
            size: total,
        });
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(SPOOL_CHUNKS);
    let owned = part.to_owned();
    let writer = tokio::task::spawn_blocking(move || spool(&owned, from, rx));
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
    let remove = || {
        let _ = std::fs::remove_file(part);
        let _ = std::fs::remove_file(part_meta(part));
    };
    if let Some(message) = broken {
        if !resume {
            remove();
        }
        return Err(FetchError::Body {
            url: url.to_owned(),
            received,
            message,
        });
    }
    let (sha256, crc32, size) = match written {
        Ok(w) => w,
        Err(source) => {
            if !resume {
                remove();
            }
            return Err(FetchError::CacheIo {
                path: part.display().to_string(),
                source,
            });
        }
    };
    if let Some(expected) = total
        && expected != size
    {
        remove();
        return Err(FetchError::ContentLength {
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
    use super::*;

    #[tokio::test]
    async fn streaming_to_a_file_reports_progress_and_a_writer_that_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let body = |bytes: Bytes| HttpResponse {
            status: 200,
            content_length: Some(bytes.len() as u64),
            total_size: None,
            etag: None,
            last_modified: None,
            body: futures::stream::iter(
                (0..bytes.len())
                    .step_by(1 << 20)
                    .map(|i| Ok(bytes.slice(i..(i + (1 << 20)).min(bytes.len()))))
                    .collect::<Vec<_>>(),
            )
            .boxed(),
        };
        // Five mebibytes in one-mebibyte chunks: one progress event at four.
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let big = Bytes::from(vec![7u8; 5 << 20]);
        let part = dir.path().join("big.part");
        let s = stream_to_file("https://h/big", body(big), &part, 0, false, Some(tx))
            .await
            .unwrap();
        assert_eq!(s.size, 5 << 20);
        let mut progress = 0;
        while let Ok(ev) = rx.try_recv() {
            if let Event::DownloadProgress { bytes, .. } = ev {
                progress += 1;
                assert_eq!(bytes, 4 << 20);
            }
        }
        assert_eq!(progress, 1);
        // A part that cannot be created: the writer stops, the download
        // (more chunks than the channel holds) breaks off, nothing is left
        // behind.
        let nowhere = dir.path().join("missing").join("x.part");
        let e = stream_to_file(
            "https://h/x",
            body(Bytes::from(vec![1u8; 20 << 20])),
            &nowhere,
            0,
            false,
            None,
        )
        .await
        .expect_err("no folder");
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
        assert!(!nowhere.exists());
    }
}
