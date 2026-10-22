//! Moving bytes over HTTP, behind a trait so tests and `--offline` need no
//! network.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::net::error::FetchError;

/// A response with a streaming body. Chunks are passed on as the transport
/// received them, never copied.
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// `Content-Length`.
    pub content_length: Option<u64>,
    /// The whole file's size, from `Content-Range` (`bytes a-b/size`), when
    /// a range was answered.
    pub total_size: Option<u64>,
    /// `ETag`.
    pub etag: Option<String>,
    /// `Last-Modified`.
    pub last_modified: Option<String>,
    /// The body.
    pub body: BoxStream<'static, Result<Bytes, String>>,
}

/// Bytes `start..end` of a file, for a ranged request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// The first byte.
    pub start: u64,
    /// One past the last byte; [`Span::TO_END`] for the rest of the file.
    pub end: u64,
}

impl Span {
    /// An `end` that asks for everything from `start` on.
    pub const TO_END: u64 = u64::MAX;
}

/// Moves bytes.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Performs a GET of `url`, of only `range` when given, and returns the
    /// response with its body streaming.
    async fn get(&self, url: &str, range: Option<Span>) -> Result<HttpResponse, FetchError>;
    /// Whether this transport never touches the network.
    fn is_offline(&self) -> bool {
        false
    }
}

/// The production transport.
pub struct ReqwestTransport {
    client: reqwest::Client,
    timeout_secs: u64,
}

impl ReqwestTransport {
    /// A client with the given user agent and timeouts.
    pub fn new(user_agent: &str, connect_timeout: Duration, read_timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(connect_timeout)
            .read_timeout(read_timeout)
            .build()
            .expect("a client with rustls and these settings builds on every supported platform");
        Self {
            client,
            timeout_secs: read_timeout.as_secs(),
        }
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn get(&self, url: &str, range: Option<Span>) -> Result<HttpResponse, FetchError> {
        let mut request = self.client.get(url);
        if let Some(r) = range {
            let last = if r.end == Span::TO_END {
                String::new()
            } else {
                (r.end - 1).to_string()
            };
            request = request.header("Range", format!("bytes={}-{last}", r.start));
        }
        let resp = request.send().await.map_err(|e| {
            if e.is_timeout() {
                FetchError::Timeout {
                    url: url.to_owned(),
                    secs: self.timeout_secs,
                }
            } else {
                FetchError::Connect {
                    url: url.to_owned(),
                    message: e.to_string(),
                }
            }
        })?;
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let owned = url.to_owned();
        let timeout = self.timeout_secs;
        Ok(HttpResponse {
            status: resp.status().as_u16(),
            content_length: resp.content_length(),
            total_size: header("content-range").and_then(|r| total_of(&r)),
            etag: header("etag"),
            last_modified: header("last-modified"),
            body: resp
                .bytes_stream()
                .map(move |chunk| {
                    chunk.map_err(|e| {
                        if e.is_timeout() {
                            format!("timed out after {timeout} seconds reading {owned}")
                        } else {
                            e.to_string()
                        }
                    })
                })
                .boxed(),
        })
    }
}

/// The whole size in a `Content-Range` value: `bytes 0-99/1234` is 1234.
fn total_of(range: &str) -> Option<u64> {
    range.rsplit('/').next()?.parse().ok()
}

/// A transport that refuses every request.
#[derive(Debug, Default, Clone, Copy)]
pub struct OfflineTransport;

#[async_trait]
impl Transport for OfflineTransport {
    async fn get(&self, url: &str, _: Option<Span>) -> Result<HttpResponse, FetchError> {
        Err(FetchError::Offline {
            url: url.to_owned(),
        })
    }
    fn is_offline(&self) -> bool {
        true
    }
}

/// A canned response for tests.
#[derive(Debug, Clone)]
pub struct Canned {
    /// Status.
    pub status: u16,
    /// Body.
    pub body: Bytes,
    /// Bytes per chunk of the body stream.
    pub chunk: usize,
    /// Content-Length to claim; `None` claims the body's length, `Some(0)` claims none.
    pub claimed_length: Option<u64>,
    /// ETag.
    pub etag: Option<String>,
    /// Fail the body stream after this many bytes.
    pub fail_after: Option<usize>,
    /// Fail at connect with this message.
    pub connect_error: Option<String>,
    /// Whether a ranged request gets its range (206) or, as from a server
    /// that ignores ranges, the whole body (200).
    pub ranges: bool,
}

impl Canned {
    /// A 200 with a body.
    pub fn ok(body: &[u8]) -> Self {
        Self {
            status: 200,
            body: Bytes::copy_from_slice(body),
            chunk: 1024,
            claimed_length: None,
            etag: Some("\"etag\"".into()),
            fail_after: None,
            connect_error: None,
            ranges: true,
        }
    }
    /// A status with a small HTML body.
    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: format!("<html>{status}</html>").into_bytes().into(),
            chunk: 1024,
            claimed_length: None,
            etag: None,
            fail_after: None,
            connect_error: None,
            ranges: true,
        }
    }
}

/// Serves canned responses by URL, in order per URL, and counts requests.
#[derive(Debug, Default)]
pub struct StaticTransport {
    responses: std::sync::Mutex<HashMap<String, Vec<Canned>>>,
    /// Requests seen, in order.
    pub requests: std::sync::Mutex<Vec<String>>,
}

impl StaticTransport {
    /// Queues a response for a URL; several queue in order, the last repeats.
    pub fn add(&self, url: &str, c: Canned) -> &Self {
        self.responses
            .lock()
            .expect("not poisoned")
            .entry(url.to_owned())
            .or_default()
            .push(c);
        self
    }

    /// Number of requests made.
    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("not poisoned").len()
    }
}

#[async_trait]
impl Transport for Arc<StaticTransport> {
    async fn get(&self, url: &str, range: Option<Span>) -> Result<HttpResponse, FetchError> {
        self.requests
            .lock()
            .expect("not poisoned")
            .push(url.to_owned());
        let canned = {
            let mut map = self.responses.lock().expect("not poisoned");
            let Some(queue) = map.get_mut(url) else {
                return Ok(HttpResponse {
                    status: 404,
                    content_length: Some(0),
                    total_size: None,
                    etag: None,
                    last_modified: None,
                    body: futures::stream::empty().boxed(),
                });
            };
            if queue.len() > 1 {
                queue.remove(0)
            } else {
                queue[0].clone()
            }
        };
        if let Some(m) = canned.connect_error {
            return Err(FetchError::Connect {
                url: url.to_owned(),
                message: m,
            });
        }
        let (status, body, total_size) = match range {
            Some(r) if canned.ranges && canned.status == 200 => {
                let len = canned.body.len();
                let end = usize::try_from(r.end).map_or(len, |e| e.min(len));
                let start = usize::try_from(r.start).map_or(end, |s| s.min(end));
                (206, canned.body.slice(start..end), Some(len as u64))
            }
            _ => (canned.status, canned.body, None),
        };
        let content_length = match canned.claimed_length {
            None => Some(body.len() as u64),
            Some(0) => None,
            Some(n) => Some(n),
        };
        let len = body.len();
        // Views of the one body, as a server's chunks would arrive.
        let chunks: Vec<Result<Bytes, String>> = match canned.fail_after {
            Some(n) => vec![Ok(body.slice(..n.min(len))), Err("connection reset".into())],
            None => {
                let step = canned.chunk.max(1);
                (0..len)
                    .step_by(step)
                    .map(|i| Ok(body.slice(i..(i + step).min(len))))
                    .collect()
            }
        };
        Ok(HttpResponse {
            status,
            content_length,
            total_size,
            etag: canned.etag,
            last_modified: Some("Tue, 13 Feb 2024 08:27:00 GMT".into()),
            body: futures::stream::iter(chunks).boxed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn offline_and_static() {
        let off = OfflineTransport;
        assert!(off.is_offline());
        let r = off.get("https://h/x", None).await;
        assert!(matches!(r, Err(FetchError::Offline { .. })));
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/a", Canned::ok(b"abc"))
            .add("https://h/a", Canned::status(503));
        assert!(!t.is_offline());
        let r = t.get("https://h/a", None).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.content_length, Some(3));
        let body: Vec<u8> = r.body.map(|c| c.unwrap().to_vec()).concat().await;
        assert_eq!(body, b"abc");
        let r = t.get("https://h/a", None).await.unwrap();
        assert_eq!(r.status, 503, "second queued response");
        let r = t.get("https://h/a", None).await.unwrap();
        assert_eq!(r.status, 503, "last repeats");
        let r = t.get("https://h/none", None).await.unwrap();
        assert_eq!(r.status, 404);
        assert_eq!(t.request_count(), 4);
        let mut c = Canned::ok(b"abcdef");
        c.fail_after = Some(2);
        c.claimed_length = Some(99);
        t.add("https://h/b", c);
        let r = t.get("https://h/b", None).await.unwrap();
        assert_eq!(r.content_length, Some(99));
        let chunks: Vec<_> = r.body.collect().await;
        assert!(chunks[1].is_err());
        let mut c = Canned::ok(b"x");
        c.claimed_length = Some(0);
        c.connect_error = Some("refused".into());
        t.add("https://h/c", c);
        assert!(matches!(
            t.get("https://h/c", None).await,
            Err(FetchError::Connect { .. })
        ));
        // Chunks are views of the one body, in order.
        let mut c = Canned::ok(b"0123456789");
        c.chunk = 4;
        t.add("https://h/d", c);
        let chunks: Vec<Bytes> = t
            .get("https://h/d", None)
            .await
            .unwrap()
            .body
            .map(Result::unwrap)
            .collect()
            .await;
        assert_eq!(chunks, [&b"0123"[..], b"4567", b"89"]);
    }

    #[tokio::test]
    async fn ranges_from_a_canned_body() {
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/r", Canned::ok(b"0123456789"));
        let body = |r: HttpResponse| async move {
            r.body
                .map(Result::unwrap)
                .collect::<Vec<Bytes>>()
                .await
                .concat()
        };
        let r = t
            .get("https://h/r", Some(Span { start: 2, end: 5 }))
            .await
            .unwrap();
        assert_eq!((r.status, r.content_length), (206, Some(3)));
        assert_eq!(body(r).await, b"234");
        let r = t
            .get("https://h/r", Some(Span { start: 20, end: 30 }))
            .await
            .unwrap();
        assert_eq!((r.status, r.content_length), (206, Some(0)));
        let mut whole = Canned::ok(b"0123456789");
        whole.ranges = false;
        t.add("https://h/w", whole);
        let r = t
            .get("https://h/w", Some(Span { start: 2, end: 5 }))
            .await
            .unwrap();
        assert_eq!((r.status, r.content_length), (200, Some(10)));
        assert!(
            OfflineTransport
                .get("https://h/r", Some(Span { start: 0, end: 1 }))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn the_real_transport_against_a_local_server() {
        use crate::net::testserver::{Reply, Server};
        let srv = Server::start();
        srv.route("/ok", Reply::Ok(b"hello".to_vec()));
        srv.route("/stall", Reply::Stall);
        srv.route(
            "/cut",
            Reply::Truncate {
                claimed: 10,
                sent: b"abc".to_vec(),
            },
        );
        srv.route(
            "/slow",
            Reply::StallBody {
                claimed: 10,
                sent: b"abc".to_vec(),
            },
        );
        let t = ReqwestTransport::new(
            "nemweb-mirror-test",
            Duration::from_secs(2),
            Duration::from_millis(300),
        );
        assert!(!t.is_offline());
        let r = t.get(&srv.url("/ok"), None).await.unwrap();
        assert_eq!((r.status, r.content_length), (200, Some(5)));
        assert_eq!(r.etag.as_deref(), Some("\"e\""));
        assert_eq!(
            r.last_modified.as_deref(),
            Some("Mon, 12 Oct 2026 00:32:30 GMT")
        );
        let body: Vec<Bytes> = r.body.map(Result::unwrap).collect().await;
        assert_eq!(body.concat(), b"hello");
        assert_eq!(t.get(&srv.url("/none"), None).await.unwrap().status, 404);
        assert!(matches!(
            t.get(&srv.url("/stall"), None).await,
            Err(FetchError::Timeout { secs: 0, .. })
        ));
        // The first failure of a body, as the fetcher stops at it.
        let first_error = |r: HttpResponse| async move {
            let mut errors = r.body.filter_map(|c| async move { c.err() }).boxed();
            errors.next().await.expect("the body fails")
        };
        let cut = first_error(t.get(&srv.url("/cut"), None).await.unwrap()).await;
        assert!(!cut.is_empty() && !cut.contains("timed out"), "{cut}");
        let slow = first_error(t.get(&srv.url("/slow"), None).await.unwrap()).await;
        assert!(slow.contains("timed out after 0 seconds reading"), "{slow}");
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", closed.local_addr().unwrap());
        drop(closed);
        assert!(matches!(
            t.get(&url, None).await,
            Err(FetchError::Connect { .. })
        ));
        srv.route("/whole", Reply::Whole(b"hello".to_vec()));
        let r = t
            .get(&srv.url("/ok"), Some(Span { start: 1, end: 4 }))
            .await
            .unwrap();
        assert_eq!((r.status, r.content_length), (206, Some(3)));
        let r = t
            .get(&srv.url("/whole"), Some(Span { start: 1, end: 4 }))
            .await
            .unwrap();
        assert_eq!((r.status, r.content_length), (200, Some(5)));
        assert_eq!(srv.hits().len(), 7);
    }
}
