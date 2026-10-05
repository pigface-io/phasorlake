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
    /// `Range: bytes={start}-{end}`; an open end reads to the end of the file.
    pub range: Option<ByteRange>,
    /// `If-Range`: the ETag or Last-Modified the range is valid for. A server
    /// whose file changed answers 200 with the whole file instead of 206.
    pub if_range: Option<String>,
}

/// The bytes a partial GET asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// Bytes `start..=end`.
    Span {
        /// First byte.
        start: u64,
        /// Last byte, inclusive.
        end: u64,
    },
    /// From `start` to the end of the file.
    From {
        /// First byte.
        start: u64,
    },
    /// The last `len` bytes of the file.
    Last {
        /// How many.
        len: u64,
    },
}

impl ByteRange {
    /// The `Range` header value.
    #[must_use]
    pub fn header_value(&self) -> String {
        match *self {
            Self::Span { start, end } => format!("bytes={start}-{end}"),
            Self::From { start } => format!("bytes={start}-"),
            Self::Last { len } => format!("bytes=-{len}"),
        }
    }

    /// The half-open slice of a body of `total` bytes this range selects,
    /// clamped to the body.
    #[must_use]
    pub fn slice_of(&self, total: u64) -> (u64, u64) {
        match *self {
            Self::Span { start, end } => {
                let s = start.min(total);
                (s, end.saturating_add(1).min(total).max(s))
            }
            Self::From { start } => (start.min(total), total),
            Self::Last { len } => (total.saturating_sub(len), total),
        }
    }
}

/// The total length from a `Content-Range: bytes a-b/total` header value.
#[must_use]
pub fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.trim().parse().ok()
}

impl HttpRequest {
    /// A plain GET of `url`.
    #[must_use]
    pub fn get(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            range: None,
            if_range: None,
        }
    }

    /// The same request, valid only while the file still has `validator`.
    #[must_use]
    pub fn with_if_range(mut self, validator: impl Into<String>) -> Self {
        self.if_range = Some(validator.into());
        self
    }

    /// The same request limited to `range`.
    #[must_use]
    pub fn with_range(mut self, range: ByteRange) -> Self {
        self.range = Some(range);
        self
    }
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
    /// The whole file's length, from `Content-Range` on a partial answer.
    pub total_length: Option<u64>,
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
        if let Some(range) = &req.range {
            r = r.header("Range", range.header_value());
        }
        if let Some(v) = &req.if_range {
            r = r.header("If-Range", v);
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
            total_length: header("content-range")
                .as_deref()
                .and_then(content_range_total),
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
    /// The range each of those requests asked for.
    pub ranges: std::sync::Mutex<Vec<Option<ByteRange>>>,
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
        self.ranges.lock().expect("not poisoned").push(req.range);
        let canned = {
            let mut map = self.responses.lock().expect("not poisoned");
            let Some(queue) = map.get_mut(&req.url) else {
                return Ok(HttpResponse {
                    status: 404,
                    content_length: Some(0),
                    etag: None,
                    last_modified: None,
                    accept_ranges: false,
                    total_length: None,
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
        // A range over a 200 answer is served as 206 with the slice, as IIS does.
        let mut canned = canned;
        let mut total_length = None;
        // If-Range names the version the range is for; another version
        // is served whole, as the RFC says.
        let same_version = req
            .if_range
            .as_ref()
            .is_none_or(|v| canned.etag.as_ref() == Some(v));
        // The length the server believes the file has: what it claims in
        // Content-Length, which a test may set apart from the bytes it sends.
        let believed = match canned.claimed_length {
            Some(n) if n > 0 => n,
            _ => canned.body.len() as u64,
        };
        let mut content_length = match canned.claimed_length {
            None => Some(canned.body.len() as u64),
            Some(0) => None,
            Some(n) => Some(n),
        };
        if let Some(range) = req.range
            && canned.status == 200
            && same_version
        {
            let (start, end) = range.slice_of(believed);
            if start >= believed && !matches!(range, ByteRange::Last { .. }) {
                // Nothing of the file lies in the range.
                return Ok(HttpResponse {
                    status: 416,
                    content_length: Some(0),
                    etag: canned.etag,
                    last_modified: None,
                    accept_ranges: true,
                    total_length: Some(believed),
                    body: futures::stream::empty().boxed(),
                });
            }
            let have = canned.body.len() as u64;
            canned.body = canned.body[start.min(have) as usize..end.min(have) as usize].to_vec();
            canned.status = 206;
            total_length = Some(believed);
            content_length = Some(end - start);
        }
        let len = canned.body.len();
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
            total_length,
            body: futures::stream::iter(chunks).boxed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ranges() {
        let span = ByteRange::Span { start: 2, end: 4 };
        assert_eq!(span.header_value(), "bytes=2-4");
        assert_eq!(ByteRange::From { start: 100 }.header_value(), "bytes=100-");
        assert_eq!(ByteRange::Last { len: 22 }.header_value(), "bytes=-22");
        assert_eq!(span.slice_of(10), (2, 5));
        assert_eq!(span.slice_of(3), (2, 3));
        assert_eq!(
            ByteRange::Span { start: 50, end: 60 }.slice_of(10),
            (10, 10)
        );
        assert_eq!(ByteRange::From { start: 7 }.slice_of(10), (7, 10));
        assert_eq!(ByteRange::Last { len: 3 }.slice_of(10), (7, 10));
        assert_eq!(ByteRange::Last { len: 30 }.slice_of(10), (0, 10));
        assert_eq!(content_range_total("bytes 0-9/1234"), Some(1234));
        assert_eq!(content_range_total("bytes 0-9/*"), None);
        assert_eq!(content_range_total("nonsense"), None);

        let req = HttpRequest::get("https://h/a").with_range(span);
        assert_eq!(req.url, "https://h/a");
        assert_eq!(req.range, Some(span));
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/a", Canned::ok(b"0123456789"));
        let body = |r: HttpResponse| async move { r.body.map(Result::unwrap).concat().await };
        let r = t.get(req).await.unwrap();
        assert_eq!(r.status, 206);
        assert_eq!(r.content_length, Some(3));
        assert_eq!(r.total_length, Some(10));
        assert_eq!(body(r).await, b"234");
        let get = |range| t.get(HttpRequest::get("https://h/a").with_range(range));
        assert_eq!(
            body(get(ByteRange::From { start: 7 }).await.unwrap()).await,
            b"789"
        );
        assert_eq!(
            body(get(ByteRange::Last { len: 2 }).await.unwrap()).await,
            b"89"
        );
        // A range that starts past the end cannot be satisfied.
        let r = get(ByteRange::Span { start: 50, end: 60 }).await.unwrap();
        assert_eq!(r.status, 416);
        assert_eq!(r.total_length, Some(10));
        assert!(body(r).await.is_empty());
        assert_eq!(
            *t.ranges.lock().unwrap().last().unwrap(),
            Some(ByteRange::Span { start: 50, end: 60 })
        );
        // A server that claims more than it sends keeps to its claim in a
        // partial answer: the total and the promised remainder.
        let mut liar = Canned::ok(b"abc");
        liar.claimed_length = Some(10);
        t.add("https://h/liar", liar);
        let r = t
            .get(HttpRequest::get("https://h/liar").with_range(ByteRange::From { start: 3 }))
            .await
            .unwrap();
        assert_eq!(
            (r.status, r.total_length, r.content_length),
            (206, Some(10), Some(7))
        );
        assert!(body(r).await.is_empty());
        // If-Range: the same version is served in part, another one whole.
        let mut v1 = Canned::ok(b"0123456789");
        v1.etag = Some("\"v1\"".into());
        t.add("https://h/e", v1);
        let resume = |tag: &str| {
            t.get(
                HttpRequest::get("https://h/e")
                    .with_range(ByteRange::From { start: 8 })
                    .with_if_range(tag),
            )
        };
        let r = resume("\"v1\"").await.unwrap();
        assert_eq!(r.status, 206);
        assert_eq!(body(r).await, b"89");
        let r = resume("\"v0\"").await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(body(r).await, b"0123456789");
        // A plain GET reports no total.
        assert_eq!(
            t.get(HttpRequest::get("https://h/a"))
                .await
                .unwrap()
                .total_length,
            None
        );
        // A range does not turn an error status into a partial answer.
        t.add("https://h/gone", Canned::status(404));
        let r = t
            .get(HttpRequest::get("https://h/gone").with_range(span))
            .await
            .unwrap();
        assert_eq!(r.status, 404);
    }

    #[tokio::test]
    async fn offline_and_static() {
        let off = OfflineTransport;
        assert!(off.is_offline());
        let r = off.get(HttpRequest::get("https://h/x")).await;
        assert!(matches!(r, Err(FetchError::Offline { .. })));
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/a", Canned::ok(b"abc"))
            .add("https://h/a", Canned::status(503));
        assert!(!t.is_offline());
        let req = HttpRequest::get;
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
