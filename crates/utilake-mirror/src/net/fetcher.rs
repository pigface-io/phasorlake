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

use crate::net::Event;
use crate::stamp::Stamp;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use sha2::Digest;
use tokio::sync::Mutex;

use crate::net::archive::{
    Address, Archive, Content, Held, Provenance, Served, Sha256, Via, lookup_at,
};
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
    /// Whether the archive held these bytes already: with no request, or,
    /// for a file nemweb listed anew, with the same bytes as before.
    pub cached: bool,
    /// ETag.
    pub etag: Option<String>,
    /// Last-Modified.
    pub last_modified: Option<String>,
    /// When the bytes downloaded were not kept: how they are the same as
    /// the version archived already that `path` is.
    pub same_as: Option<Same>,
}

/// A download the same as a version archived already, not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Same {
    /// A zip that holds the same files (names, CRC-32 and sizes) packed
    /// again, rather than the same bytes.
    pub repacked: bool,
    /// sha256 hex of the bytes downloaded.
    pub sha256: String,
    /// Their size.
    pub size: u64,
    /// The server's `Last-Modified` for them.
    pub last_modified: Option<String>,
}

impl LocalFile {
    /// An archived file, as found.
    fn cached(remote: &RemoteFile, found: Held) -> Self {
        let p = &found.provenance;
        Self {
            remote: remote.clone(),
            sha256: p.content.sha256.hex(),
            size: p.content.size,
            cached: true,
            etag: p.etag().map(str::to_owned),
            last_modified: p.modified().map(Stamp::http_date),
            same_as: None,
            path: found.path,
        }
    }
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

    /// The minimum gap between two of its requests.
    pub fn min_interval(&self) -> Duration {
        self.config.min_interval
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

    /// Bytes `range` of a file, by a ranged request, into memory, with what
    /// the server said of the whole file; not archived. Paced, retried and
    /// refused like every request.
    pub async fn fetch_range(&self, url: &str, range: Span) -> Result<RangeRead, FetchError> {
        self.ensure_online(url)?;
        self.get_with_retries(url, Some(range), |resp| async move {
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

    /// The archived copy of a remote file, or the file downloaded into the
    /// archive whole, with a sidecar that says where it came from.
    ///
    /// nemweb rewrites some files in place under the same name. When the
    /// archived copy is not the version the listing gives (another size or
    /// listed time, `Sidecar::is_listed_as`), that version is downloaded
    /// beside it (`Archive::version_path`) and the first is kept as it
    /// is. A download that is the same as a version archived already, by
    /// its bytes or, for a zip, by what it holds, is not kept: that
    /// version is the file, as `cached`, with `same_as` saying how.
    pub async fn fetch(&self, remote: &RemoteFile) -> Result<LocalFile, FetchError> {
        let url = remote.url.as_str();
        let base = self.archive.path_for(url);
        // The file at the path, from this URL and as listed: the common
        // case, answered without reading the folder.
        if let Some(first) = lookup_at(&base, url)?
            && (remote.modified_at.is_none()
                || first
                    .provenance
                    .listed_as(url, remote.size, remote.modified_at))
        {
            return Ok(LocalFile::cached(remote, first));
        }
        // Every file at the path, from any URL: both listings of a report
        // folder are one folder, so another URL's file may be there.
        let held = Archive::held_at(&base)?;
        let mine: Vec<&Held> = held.iter().filter(|a| a.provenance.names(url)).collect();
        if let Some(found) = mine
            .iter()
            .find(|a| a.provenance.listed_as(url, remote.size, remote.modified_at))
            .or(mine.first().filter(|_| remote.modified_at.is_none()))
        {
            return Ok(LocalFile::cached(remote, (*found).clone()));
        }
        let path = match held.is_empty() {
            true => base.clone(),
            false => {
                let at = remote.modified_at.unwrap_or_else(Stamp::now);
                let p = Archive::version_path(&base, at);
                match held.iter().any(|a| a.path == p) || p.exists() {
                    true => Archive::version_path(&base, Stamp::now()),
                    false => p,
                }
            }
        };
        self.ensure_online(url)?;
        let p = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(p).map_err(|source| FetchError::CacheIo {
            path: p.display().to_string(),
            source,
        })?;
        let part = Archive::part_path(&path);
        let streamed = match self.resume(url, &part).await? {
            Some(s) => s,
            None => {
                self.get_with_retries(url, None, |resp| {
                    stream_to_file(url, resp, &part, 0, self.events.clone())
                })
                .await?
            }
        };
        let _ = std::fs::remove_file(part_meta(&part));
        if !held.is_empty()
            && let Some((found, repacked)) =
                self.held_as(url, &held, &part, &streamed.sha256).await?
        {
            // Another URL's bytes, the same: one file, two addresses.
            let held = match found.provenance.names(url) {
                true => found,
                false => {
                    let mut found = found;
                    found.provenance.add(Address {
                        url: url.to_owned(),
                        seen: Stamp::now(),
                        gone: None,
                        via: Via::Same {
                            listed: Some(listed_of(remote)),
                        },
                    });
                    Archive::write(&found.path, &found.provenance)?;
                    found
                }
            };
            let same = Same {
                repacked,
                sha256: streamed.sha256.hex(),
                size: streamed.size,
                last_modified: streamed.last_modified,
            };
            let _ = std::fs::remove_file(&part);
            return Ok(LocalFile {
                same_as: Some(same),
                ..LocalFile::cached(remote, held)
            });
        }
        std::fs::rename(&part, &path).map_err(|source| FetchError::CacheIo {
            path: path.display().to_string(),
            source,
        })?;
        let modified = streamed
            .last_modified
            .as_deref()
            .and_then(Stamp::from_http_date);
        let provenance = Provenance::new(
            Content {
                size: streamed.size,
                sha256: streamed.sha256,
                crc32: Some(streamed.crc32),
            },
            format!("utilake-mirror/{}", env!("CARGO_PKG_VERSION")),
            Address {
                url: url.to_owned(),
                seen: Stamp::now(),
                gone: None,
                via: Via::Fetched {
                    listed: Some(listed_of(remote)),
                    served: Some(Served {
                        etag: streamed.etag.clone(),
                        modified,
                    }),
                },
            },
        );
        Archive::write(&path, &provenance)?;
        Ok(LocalFile {
            remote: remote.clone(),
            path,
            sha256: streamed.sha256.hex(),
            size: streamed.size,
            cached: false,
            etag: streamed.etag,
            last_modified: streamed.last_modified,
            same_as: None,
        })
    }

    /// The file held at the download's path that the download at `part`
    /// is the same as: by its bytes, from any URL, or, for a zip of the
    /// same URL, by what it holds.
    async fn held_as(
        &self,
        url: &str,
        held: &[Held],
        part: &Path,
        sha256: &Sha256,
    ) -> Result<Option<(Held, bool)>, FetchError> {
        if let Some(v) = held.iter().find(|v| v.provenance.content.sha256 == *sha256) {
            return Ok(Some((v.clone(), false)));
        }
        let Some(holds) = crate::zipdir::contents(part).await else {
            return Ok(None);
        };
        for v in held.iter().filter(|v| v.provenance.names(url)) {
            if crate::zipdir::contents(&v.path).await.as_ref() == Some(&holds) {
                return Ok(Some((v.clone(), true)));
            }
        }
        Ok(None)
    }

    /// The rest of a download a run before left in `part`: asked for from
    /// where it stops, appended when the server answers with that range of
    /// the same file (its size, ETag and Last-Modified as when the part was
    /// begun). None when there is no part, or it cannot be continued: then
    /// the file is downloaded from the start.
    async fn resume(&self, url: &str, part: &Path) -> Result<Option<Streamed>, FetchError> {
        let from = std::fs::metadata(part).map_or(0, |m| m.len());
        let begun: Option<PartMeta> = std::fs::read(part_meta(part))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let Some(begun) = begun.filter(|_| from > 0) else {
            return Ok(None);
        };
        // Whole already, cut off just before it was renamed: no request.
        if begun.size == Some(from) {
            let h = crate::net::archive::content_of(part)?;
            return Ok(Some(Streamed {
                sha256: h.sha256,
                crc32: h.crc32.unwrap_or_default(),
                size: h.size,
                etag: begun.etag,
                last_modified: begun.last_modified,
            }));
        }
        let span = Span {
            start: from,
            end: Span::TO_END,
        };
        let attempt = self
            .get_with_retries(url, Some(span), |resp| {
                let same = resp.total_size == begun.size
                    && resp.etag == begun.etag
                    && resp.last_modified == begun.last_modified;
                async move {
                    if !same {
                        return Ok(None);
                    }
                    stream_to_file(url, resp, part, from, self.events.clone())
                        .await
                        .map(Some)
                }
            })
            .await;
        match attempt {
            Ok(done) => Ok(done),
            // The whole file instead of the rest, or a range past its end.
            Err(
                FetchError::RangeUnsupported { .. } | FetchError::HttpStatus { status: 416, .. },
            ) => Ok(None),
            Err(e) => Err(e),
        }
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

/// The listing line a remote file was listed with.
fn listed_of(remote: &RemoteFile) -> utilake_provenance::Listed {
    utilake_provenance::Listed {
        size: remote.size,
        at: remote.modified_at,
    }
}

/// A completed download: its bytes' hash and size, and what the server said.
struct Streamed {
    sha256: Sha256,
    crc32: u32,
    size: u64,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// What a download said of the file when it began: kept beside its part,
/// so a later run continues it only if the server still has that file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PartMeta {
    size: Option<u64>,
    etag: Option<String>,
    /// The `Last-Modified` header as sent; what a later run compares.
    last_modified: Option<String>,
    /// The same as an instant (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified_at: Option<Stamp>,
}

/// Where a part's [`PartMeta`] is kept.
fn part_meta(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_owned();
    s.push(".resume");
    PathBuf::from(s)
}

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
    crate::net::archive::to_disk(&file)?;
    Ok((Sha256(hasher.finalize().into()), crc.finalize(), size))
}

async fn stream_to_file(
    url: &str,
    resp: HttpResponse,
    part: &Path,
    from: u64,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
) -> Result<Streamed, FetchError> {
    // The whole file's size: what is kept plus what comes.
    let total = resp.content_length.map(|n| n + from);
    if from == 0 {
        let meta = PartMeta {
            size: total,
            etag: resp.etag.clone(),
            last_modified: resp.last_modified.clone(),
            last_modified_at: resp
                .last_modified
                .as_deref()
                .and_then(Stamp::from_http_date),
        };
        let text = serde_json::to_vec(&meta).expect("a PartMeta serialises");
        let _ = std::fs::write(part_meta(part), text);
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
    // A part cut off is kept, its bytes good so far, for a later run to
    // continue; one of the wrong length is not.
    if let Some(message) = broken {
        return Err(FetchError::Body {
            url: url.to_owned(),
            received,
            message,
        });
    }
    let (sha256, crc32, size) = match written {
        Ok(w) => w,
        Err(source) => {
            return Err(FetchError::CacheIo {
                path: part.display().to_string(),
                source,
            });
        }
    };
    if let Some(expected) = total
        && expected != size
    {
        let _ = std::fs::remove_file(part);
        let _ = std::fs::remove_file(part_meta(part));
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
    use url::Url;

    use super::*;
    use crate::net::archive::sha256_hex;
    use crate::net::transport::{Canned, OfflineTransport, StaticTransport};

    fn remote(url: &str) -> RemoteFile {
        RemoteFile {
            url: Url::parse(url).unwrap(),
            size: None,
            modified: None,
            modified_at: None,
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
        let sc = Archive::read(&local.path).unwrap().unwrap();
        assert_eq!(sc.url(), url);
        assert_eq!(sc.content.sha256.hex(), local.sha256);
        assert_eq!(
            sc.tool,
            format!("utilake-mirror/{}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(sc.content.crc32, Some(crc32fast::hash(&body)));
        assert_eq!(
            sc.modified().map(Stamp::http_date).as_deref(),
            Some("Tue, 13 Feb 2024 08:27:00 GMT")
        );
        assert_eq!(sc.modified().unwrap().to_string(), "2024-02-13T08:27:00Z");
        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(Archive::record_path(&local.path)).unwrap(),
        )
        .unwrap();
        crate::stamp::check::assert_aware(&written);
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
    async fn a_later_version_is_kept_beside_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://h/Reports/CURRENT/A/s.csv";
        let t = Arc::new(StaticTransport::default());
        t.add(url, Canned::ok(b"one"))
            .add(url, Canned::ok(b"two!"))
            .add(url, Canned::ok(b"two!"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let listed = |size, at: &str| RemoteFile {
            size: Some(size),
            modified: Some(at.to_owned()),
            modified_at: Stamp::from_listing(at, crate::stamp::AEST),
            ..remote(url)
        };
        let first = f
            .fetch(&listed(3, "Monday, October 12, 2026 10:45 AM"))
            .await
            .unwrap();
        let later = listed(4, "Tuesday, October 13, 2026 11:53 AM");
        let second = f.fetch(&later).await.unwrap();
        assert!(!second.cached);
        assert_eq!(
            second.path,
            dir.path()
                .join("h/Reports/CURRENT/A/s.csv@20261013T115300+1000")
        );
        assert_eq!(std::fs::read(&first.path).unwrap(), b"one");
        assert_eq!(std::fs::read(&second.path).unwrap(), b"two!");
        // Found again beside the first, with no request.
        let again = f.fetch(&later).await.unwrap();
        assert!(again.cached);
        assert_eq!(again.path, second.path);
        // A listing with no time cannot name a version: the first is the file.
        let untimed = RemoteFile {
            size: Some(9),
            ..remote(url)
        };
        assert_eq!(f.fetch(&untimed).await.unwrap().path, first.path);
        assert_eq!(t.request_count(), 2);
        // Listed again with the bytes of the second: not kept a third time.
        let third = f
            .fetch(&listed(4, "Wednesday, October 14, 2026 09:00 AM"))
            .await
            .unwrap();
        assert!(third.cached);
        assert_eq!(third.path, second.path);
        let same = third.same_as.unwrap();
        assert!(!same.repacked);
        assert_eq!((same.sha256, same.size), (sha256_hex(b"two!"), 4));
        assert!(same.last_modified.is_some());
        assert!(
            !dir.path()
                .join("h/Reports/CURRENT/A/s.csv@20261014T090000+1000")
                .exists()
        );

        // A zip packed again with the same files is not kept either.
        let zip = "https://h/Reports/CURRENT/A/z.zip";
        let (a, b) = (
            crate::testzip::stored("a.csv", b"x,y\n"),
            crate::testzip::deflated("a.csv", b"x,y\n"),
        );
        assert_ne!(a, b);
        let c = crate::testzip::stored("a.csv", b"new data\n");
        t.add(zip, Canned::ok(&a))
            .add(zip, Canned::ok(&b))
            .add(zip, Canned::ok(&c));
        let listed_zip = |at: &str| RemoteFile {
            modified_at: Stamp::from_listing(at, crate::stamp::AEST),
            ..remote(zip)
        };
        let z1 = f
            .fetch(&listed_zip("Monday, October 12, 2026 10:45 AM"))
            .await
            .unwrap();
        let z2 = f
            .fetch(&listed_zip("Tuesday, October 13, 2026 11:53 AM"))
            .await
            .unwrap();
        assert_eq!(z2.path, z1.path);
        let same = z2.same_as.unwrap();
        assert!(same.repacked);
        assert_eq!(same.sha256, sha256_hex(&b));
        // New files in it: a version.
        let z3 = f
            .fetch(&listed_zip("Wednesday, October 14, 2026 09:00 AM"))
            .await
            .unwrap();
        assert!(z3.same_as.is_none() && !z3.cached);
        assert_ne!(z3.path, z1.path);
        // Versions of a file in a folder that is not there: none.
        let none = crate::net::archive::versions(&dir.path().join("no/such/a.csv"), zip).unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn both_listings_of_a_report_folder_share_one_folder() {
        let dir = tempfile::tempdir().unwrap();
        let current = "https://www.nemweb.com.au/Reports/CURRENT/GSH/Benchmark_Price/B.zip";
        let archived = "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/Benchmark_Price/B.zip";
        let t = Arc::new(StaticTransport::default());
        t.add(current, Canned::ok(b"partial"))
            .add(archived, Canned::ok(b"final month"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let listed = |url: &str, at: &str| RemoteFile {
            modified: Some(at.to_owned()),
            modified_at: Stamp::from_listing(at, crate::stamp::AEST),
            ..remote(url)
        };
        let first = f
            .fetch(&listed(current, "Monday, October 12, 2026 10:45 AM"))
            .await
            .unwrap();
        let plain = dir
            .path()
            .join("www.nemweb.com.au/Reports/GSH/Benchmark_Price/B.zip");
        assert_eq!(first.path, plain);
        // The same name with other bytes is a version beside it, never an
        // overwrite.
        let second = f
            .fetch(&listed(archived, "Tuesday, October 13, 2026 11:53 AM"))
            .await
            .unwrap();
        assert!(!second.cached);
        assert_eq!(
            second.path,
            Archive::version_path(
                &plain,
                Stamp::from_listing("Tuesday, October 13, 2026 11:53 AM", crate::stamp::AEST)
                    .unwrap()
            )
        );
        assert_eq!(std::fs::read(&plain).unwrap(), b"partial");
        // Each is found by its own URL.
        assert_eq!(f.archive().lookup(current).unwrap().unwrap().path, plain);
        assert_eq!(
            f.archive().lookup(archived).unwrap().unwrap().path,
            second.path
        );
        assert_eq!(Archive::held_at(&plain).unwrap().len(), 2);
        let again = f
            .fetch(&listed(archived, "Tuesday, October 13, 2026 11:53 AM"))
            .await
            .unwrap();
        assert!(again.cached);
        assert_eq!(again.path, second.path);
        assert_eq!(t.request_count(), 2);

        // The same name with equal bytes: one file, both URLs on it.
        let a = "https://www.nemweb.com.au/Reports/CURRENT/GSH/X/s.csv";
        let b = "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/X/s.csv";
        t.add(a, Canned::ok(b"same")).add(b, Canned::ok(b"same"));
        let one = f
            .fetch(&listed(a, "Monday, October 12, 2026 10:45 AM"))
            .await
            .unwrap();
        let two = f
            .fetch(&listed(b, "Tuesday, October 13, 2026 11:53 AM"))
            .await
            .unwrap();
        assert!(two.cached && two.same_as.is_some());
        assert_eq!(two.path, one.path);
        let sc = Archive::read(&one.path).unwrap().unwrap();
        assert_eq!(sc.url(), a);
        assert_eq!(sc.addresses.len(), 2);
        assert_eq!(sc.addresses[1].url, b);
        assert!(matches!(sc.addresses[1].via, Via::Same { .. }));
        // The data file is untouched: its hash is the download's.
        assert_eq!(sc.content.sha256.hex(), sha256_hex(b"same"));
        assert_eq!(f.archive().lookup(b).unwrap().unwrap().path, one.path);
        let three = f
            .fetch(&listed(b, "Tuesday, October 13, 2026 11:53 AM"))
            .await
            .unwrap();
        assert!(three.cached && three.same_as.is_none());
        assert_eq!(t.request_count(), 4);

        // A listing with no time, of a URL whose path another URL holds,
        // is kept beside it under the time it was downloaded.
        let c = "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/X/t.csv";
        let d = "https://www.nemweb.com.au/Reports/CURRENT/GSH/X/t.csv";
        t.add(c, Canned::ok(b"c")).add(d, Canned::ok(b"dd"));
        let c1 = f.fetch(&remote(c)).await.unwrap();
        let d1 = f.fetch(&remote(d)).await.unwrap();
        assert_ne!(c1.path, d1.path);
        assert!(d1.path.to_string_lossy().contains("t.csv@"));
        // A time already taken by a version: kept under the download's.
        let e = "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/X/u.csv";
        let g = "https://www.nemweb.com.au/Reports/CURRENT/GSH/X/u.csv";
        t.add(e, Canned::ok(b"e")).add(g, Canned::ok(b"gg"));
        let at = "Monday, October 12, 2026 10:45 AM";
        let e1 = f.fetch(&remote(e)).await.unwrap();
        std::fs::write(
            Archive::version_path(
                &e1.path,
                Stamp::from_listing(at, crate::stamp::AEST).unwrap(),
            ),
            b"x",
        )
        .unwrap();
        let g1 = f.fetch(&listed(g, at)).await.unwrap();
        assert!(!g1.cached);
        assert_ne!(
            g1.path,
            Archive::version_path(
                &e1.path,
                Stamp::from_listing(at, crate::stamp::AEST).unwrap()
            )
        );
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
        let r = f.fetch_range("https://h/z.zip", span).await.unwrap();
        assert_eq!((r.bytes.as_slice(), r.total_size), (&b"6789"[..], Some(10)));
        assert_eq!(r.etag.as_deref(), Some("\"etag\""));
        assert!(r.last_modified.is_some());
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

    #[tokio::test]
    async fn a_cut_off_download_is_continued_where_it_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://h/r.zip";
        let body: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let t = Arc::new(StaticTransport::default());
        let mut cut = Canned::ok(&body);
        cut.fail_after = Some(20_000);
        t.add(url, cut);
        t.add(url, Canned::ok(&body));
        let mut once = fast();
        once.attempts = 1;
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), once);
        assert!(f.fetch(&remote(url)).await.is_err());
        let path = f.archive().path_for(url);
        let part = Archive::part_path(&path);
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 20_000);
        let begun: serde_json::Value =
            serde_json::from_slice(&std::fs::read(part_meta(&part)).unwrap()).unwrap();
        assert_eq!(begun["last_modified"], "Tue, 13 Feb 2024 08:27:00 GMT");
        assert_eq!(begun["last_modified_at"], "2024-02-13T08:27:00Z");
        crate::stamp::check::assert_aware(&begun);
        // The next run asks only for the rest, and the file is whole.
        let local = f.fetch(&remote(url)).await.unwrap();
        assert_eq!(std::fs::read(&local.path).unwrap(), body);
        assert_eq!(local.sha256, sha256_hex(&body));
        let sc = Archive::read(&local.path).unwrap().unwrap();
        assert_eq!(sc.content.crc32, Some(crc32fast::hash(&body)));
        assert!(!part.exists() && !part_meta(&part).exists());
        assert_eq!(t.request_count(), 2);
    }

    #[tokio::test]
    async fn a_part_that_cannot_be_continued_is_begun_again() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://h/s.zip";
        let body = b"0123456789".to_vec();
        let leave = |f: &Fetcher, bytes: &[u8], meta: Option<&str>| {
            let part = Archive::part_path(&f.archive().path_for(url));
            std::fs::create_dir_all(part.parent().unwrap()).unwrap();
            std::fs::write(&part, bytes).unwrap();
            match meta {
                Some(m) => std::fs::write(part_meta(&part), m).unwrap(),
                None => {
                    let _ = std::fs::remove_file(part_meta(&part));
                }
            }
        };
        let same =
            r#"{"size":10,"etag":"\"etag\"","last_modified":"Tue, 13 Feb 2024 08:27:00 GMT"}"#;
        let fresh_fetch = |t: &Arc<StaticTransport>| {
            Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast())
        };
        // The file changed since: begun again.
        let t = Arc::new(StaticTransport::default());
        t.add(url, Canned::ok(&body));
        let f = fresh_fetch(&t);
        leave(
            &f,
            b"XXXX",
            Some(r#"{"size":10,"etag":"\"other\"","last_modified":null}"#),
        );
        let local = f.fetch(&remote(url)).await.unwrap();
        assert_eq!(std::fs::read(&local.path).unwrap(), body);
        std::fs::remove_file(&local.path).unwrap();
        std::fs::remove_file(Archive::record_path(&local.path)).unwrap();
        // A server that sends the whole file for a range: begun again.
        let t = Arc::new(StaticTransport::default());
        let mut whole = Canned::ok(&body);
        whole.ranges = false;
        t.add(url, whole);
        let f = fresh_fetch(&t);
        leave(&f, b"0123", Some(same));
        let local = f.fetch(&remote(url)).await.unwrap();
        assert_eq!(std::fs::read(&local.path).unwrap(), body);
        std::fs::remove_file(&local.path).unwrap();
        std::fs::remove_file(Archive::record_path(&local.path)).unwrap();
        // A part already whole: finished without a request.
        let t = Arc::new(StaticTransport::default());
        let f = fresh_fetch(&t);
        leave(&f, &body, Some(same));
        let local = f.fetch(&remote(url)).await.unwrap();
        assert_eq!(
            (local.sha256.clone(), t.request_count()),
            (sha256_hex(&body), 0)
        );
        std::fs::remove_file(&local.path).unwrap();
        std::fs::remove_file(Archive::record_path(&local.path)).unwrap();
        // ...unless it cannot be read.
        leave(&f, &body, Some(same));
        let part = Archive::part_path(&f.archive().path_for(url));
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(matches!(
            f.fetch(&remote(url)).await,
            Err(FetchError::CacheIo { .. })
        ));
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::remove_file(&part).unwrap();
        // A range past the end (416), and a part with no record of how it
        // began: begun again.
        let t = Arc::new(StaticTransport::default());
        t.add(url, Canned::status(416));
        t.add(url, Canned::ok(&body));
        let f = fresh_fetch(&t);
        leave(&f, b"0123456789AB", Some(same));
        let local = f.fetch(&remote(url)).await.unwrap();
        assert_eq!(std::fs::read(&local.path).unwrap(), body);
        std::fs::remove_file(&local.path).unwrap();
        std::fs::remove_file(Archive::record_path(&local.path)).unwrap();
        leave(&f, b"0123", None);
        let local = f.fetch(&remote(url)).await.unwrap();
        assert_eq!(std::fs::read(&local.path).unwrap(), body);
        std::fs::remove_file(&local.path).unwrap();
        std::fs::remove_file(Archive::record_path(&local.path)).unwrap();
        // The resume itself refused: the error, not a fresh start.
        let t = Arc::new(StaticTransport::default());
        t.add(url, Canned::status(403));
        let f = fresh_fetch(&t);
        leave(&f, b"0123", Some(same));
        assert!(matches!(
            f.fetch(&remote(url)).await,
            Err(FetchError::Blocked { .. })
        ));
    }
}
