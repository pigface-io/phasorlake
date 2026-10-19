//! Moving bytes over HTTP, behind a trait so tests and `--offline` need no
//! network.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::FetchError;

/// A response with a streaming body. Chunks are passed on as the transport
/// received them, never copied.
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// `Content-Length`.
    pub content_length: Option<u64>,
    /// `ETag`.
    pub etag: Option<String>,
    /// `Last-Modified`.
    pub last_modified: Option<String>,
    /// The body.
    pub body: BoxStream<'static, Result<Bytes, String>>,
}

/// Moves bytes.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Performs a GET of `url` and returns the response with its body
    /// streaming.
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError>;
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
    pub fn new(
        user_agent: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(connect_timeout)
            .read_timeout(read_timeout)
            .build()
            .map_err(|e| FetchError::Connect {
                url: String::new(),
                message: e.to_string(),
            })?;
        Ok(Self {
            client,
            timeout_secs: read_timeout.as_secs(),
        })
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError> {
        let resp = self.client.get(url).send().await.map_err(|e| {
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

/// A transport that refuses every request.
#[derive(Debug, Default, Clone, Copy)]
pub struct OfflineTransport;

#[async_trait]
impl Transport for OfflineTransport {
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError> {
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
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError> {
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
        let content_length = match canned.claimed_length {
            None => Some(canned.body.len() as u64),
            Some(0) => None,
            Some(n) => Some(n),
        };
        let body = canned.body;
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
            status: canned.status,
            content_length,
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
        let r = off.get("https://h/x").await;
        assert!(matches!(r, Err(FetchError::Offline { .. })));
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/a", Canned::ok(b"abc"))
            .add("https://h/a", Canned::status(503));
        assert!(!t.is_offline());
        let r = t.get("https://h/a").await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.content_length, Some(3));
        let body: Vec<u8> = r.body.map(|c| c.unwrap().to_vec()).concat().await;
        assert_eq!(body, b"abc");
        let r = t.get("https://h/a").await.unwrap();
        assert_eq!(r.status, 503, "second queued response");
        let r = t.get("https://h/a").await.unwrap();
        assert_eq!(r.status, 503, "last repeats");
        let r = t.get("https://h/none").await.unwrap();
        assert_eq!(r.status, 404);
        assert_eq!(t.request_count(), 4);
        let mut c = Canned::ok(b"abcdef");
        c.fail_after = Some(2);
        c.claimed_length = Some(99);
        t.add("https://h/b", c);
        let r = t.get("https://h/b").await.unwrap();
        assert_eq!(r.content_length, Some(99));
        let chunks: Vec<_> = r.body.collect().await;
        assert!(chunks[1].is_err());
        let mut c = Canned::ok(b"x");
        c.claimed_length = Some(0);
        c.connect_error = Some("refused".into());
        t.add("https://h/c", c);
        assert!(matches!(
            t.get("https://h/c").await,
            Err(FetchError::Connect { .. })
        ));
        // Chunks are views of the one body, in order.
        let mut c = Canned::ok(b"0123456789");
        c.chunk = 4;
        t.add("https://h/d", c);
        let chunks: Vec<Bytes> = t
            .get("https://h/d")
            .await
            .unwrap()
            .body
            .map(Result::unwrap)
            .collect()
            .await;
        assert_eq!(chunks, [&b"0123"[..], b"4567", b"89"]);
        let real = ReqwestTransport::new(
            "utilake-test",
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(!real.is_offline());
        let r = real.get("http://127.0.0.1:9/never").await;
        assert!(matches!(
            r,
            Err(FetchError::Connect { .. } | FetchError::Timeout { .. })
        ));
    }
}
