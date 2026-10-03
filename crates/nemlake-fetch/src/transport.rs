//! Moving bytes over HTTP, behind a trait so tests and `--offline` need no
//! network.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::FetchError;

/// A GET.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// URL.
    pub url: String,
    /// `If-None-Match`.
    pub if_none_match: Option<String>,
    /// `If-Modified-Since`.
    pub if_modified_since: Option<String>,
}

/// A response with a streaming body.
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// `Content-Length`.
    pub content_length: Option<u64>,
    /// `ETag`.
    pub etag: Option<String>,
    /// `Last-Modified`.
    pub last_modified: Option<String>,
    /// `Accept-Ranges: bytes`.
    pub accept_ranges: bool,
    /// The body.
    pub body: BoxStream<'static, Result<Vec<u8>, String>>,
}

/// Moves bytes.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Performs a GET and returns the response with its body streaming.
    async fn get(&self, req: HttpRequest) -> Result<HttpResponse, FetchError>;
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
    async fn get(&self, req: HttpRequest) -> Result<HttpResponse, FetchError> {
        let mut r = self.client.get(&req.url);
        if let Some(e) = &req.if_none_match {
            r = r.header("If-None-Match", e);
        }
        if let Some(m) = &req.if_modified_since {
            r = r.header("If-Modified-Since", m);
        }
        let resp = r.send().await.map_err(|e| {
            if e.is_timeout() {
                FetchError::Timeout {
                    url: req.url.clone(),
                    secs: self.timeout_secs,
                }
            } else {
                FetchError::Connect {
                    url: req.url.clone(),
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
        let url = req.url.clone();
        let timeout = self.timeout_secs;
        Ok(HttpResponse {
            status: resp.status().as_u16(),
            content_length: resp.content_length(),
            etag: header("etag"),
            last_modified: header("last-modified"),
            accept_ranges: header("accept-ranges").is_some_and(|v| v.eq_ignore_ascii_case("bytes")),
            body: resp
                .bytes_stream()
                .map(move |chunk| {
                    chunk.map(|b| b.to_vec()).map_err(|e| {
                        if e.is_timeout() {
                            format!("timed out after {timeout} seconds reading {url}")
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
    async fn get(&self, req: HttpRequest) -> Result<HttpResponse, FetchError> {
        Err(FetchError::Offline { url: req.url })
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
    pub body: Vec<u8>,
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
            body: body.to_vec(),
            claimed_length: None,
            etag: Some("\"etag\"".into()),
            fail_after: None,
            connect_error: None,
        }
    }
    /// A status with an empty body.
    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: format!("<html>{status}</html>").into_bytes(),
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
    async fn get(&self, req: HttpRequest) -> Result<HttpResponse, FetchError> {
        self.requests
            .lock()
            .expect("not poisoned")
            .push(req.url.clone());
        let canned = {
            let mut map = self.responses.lock().expect("not poisoned");
            let Some(queue) = map.get_mut(&req.url) else {
                return Ok(HttpResponse {
                    status: 404,
                    content_length: Some(0),
                    etag: None,
                    last_modified: None,
                    accept_ranges: false,
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
                url: req.url,
                message: m,
            });
        }
        let len = canned.body.len();
        let content_length = match canned.claimed_length {
            None => Some(len as u64),
            Some(0) => None,
            Some(n) => Some(n),
        };
        let chunks: Vec<Result<Vec<u8>, String>> = match canned.fail_after {
            Some(n) => vec![
                Ok(canned.body[..n.min(len)].to_vec()),
                Err("connection reset".into()),
            ],
            None => canned.body.chunks(1024).map(|c| Ok(c.to_vec())).collect(),
        };
        Ok(HttpResponse {
            status: canned.status,
            content_length,
            etag: canned.etag,
            last_modified: Some("Tue, 13 Feb 2024 08:27:00 GMT".into()),
            accept_ranges: true,
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
        let r = off
            .get(HttpRequest {
                url: "https://h/x".into(),
                if_none_match: None,
                if_modified_since: None,
            })
            .await;
        assert!(matches!(r, Err(FetchError::Offline { .. })));
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/a", Canned::ok(b"abc"))
            .add("https://h/a", Canned::status(503));
        assert!(!t.is_offline());
        let req = |u: &str| HttpRequest {
            url: u.into(),
            if_none_match: None,
            if_modified_since: None,
        };
        let r = t.get(req("https://h/a")).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.content_length, Some(3));
        let body: Vec<u8> = r.body.map(Result::unwrap).concat().await;
        assert_eq!(body, b"abc");
        let r = t.get(req("https://h/a")).await.unwrap();
        assert_eq!(r.status, 503, "second queued response");
        let r = t.get(req("https://h/a")).await.unwrap();
        assert_eq!(r.status, 503, "last repeats");
        let r = t.get(req("https://h/none")).await.unwrap();
        assert_eq!(r.status, 404);
        assert_eq!(t.request_count(), 4);
        let mut c = Canned::ok(b"abcdef");
        c.fail_after = Some(2);
        c.claimed_length = Some(99);
        t.add("https://h/b", c);
        let r = t.get(req("https://h/b")).await.unwrap();
        assert_eq!(r.content_length, Some(99));
        let chunks: Vec<_> = r.body.collect().await;
        assert!(chunks[1].is_err());
        let mut c = Canned::ok(b"x");
        c.connect_error = Some("refused".into());
        t.add("https://h/c", c);
        assert!(matches!(
            t.get(req("https://h/c")).await,
            Err(FetchError::Connect { .. })
        ));
        let real = ReqwestTransport::new(
            "nemlake-test",
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(!real.is_offline());
        let r = real.get(req("http://127.0.0.1:9/never")).await;
        assert!(matches!(
            r,
            Err(FetchError::Connect { .. } | FetchError::Timeout { .. })
        ));
    }
}
