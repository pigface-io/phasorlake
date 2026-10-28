//! Moving bytes over HTTP, behind a trait so tests and `--offline` need no network.

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(feature = "http")]
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::FetchError;

/// A byte span of a file, asked for with a `Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// Bytes `start..end`, `end` not included.
    Bytes {
        /// First byte.
        start: u64,
        /// One past the last.
        end: u64,
    },
    /// Every byte from `start` to the end of the file.
    From(u64),
    /// The last `n` bytes of the file, however long it is.
    Last(u64),
}

impl Span {
    /// The `Range` header's value.
    pub fn header(self) -> String {
        match self {
            Self::Bytes { start, end } => format!("bytes={start}-{}", end.saturating_sub(1)),
            Self::From(start) => format!("bytes={start}-"),
            Self::Last(n) => format!("bytes=-{n}"),
        }
    }
}

/// The whole file's size from a `Content-Range` header (`bytes 0-99/1234`); none for `*`.
pub fn total_of(content_range: &str) -> Option<u64> {
    content_range.rsplit_once('/')?.1.trim().parse().ok()
}

/// A response with a streaming body; chunks are passed on uncopied.
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// `Content-Length`.
    pub content_length: Option<u64>,
    /// The whole file's size, from `Content-Range` on a 206.
    pub total_size: Option<u64>,
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
    /// GETs `url`, the body streaming.
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError>;
    /// GETs one span of `url`: 206 when honoured, 200 with the whole file when not.
    /// By default a transport asks for the whole file.
    async fn get_range(&self, url: &str, span: Span) -> Result<HttpResponse, FetchError> {
        let _ = span;
        self.get(url).await
    }
    /// Whether this transport never touches the network.
    fn is_offline(&self) -> bool {
        false
    }
}

/// The production transport.
#[cfg(feature = "http")]
pub struct ReqwestTransport {
    client: reqwest::Client,
    timeout_secs: u64,
}

#[cfg(feature = "http")]
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

#[cfg(feature = "http")]
impl ReqwestTransport {
    async fn send(&self, url: &str, span: Option<Span>) -> Result<HttpResponse, FetchError> {
        let mut request = self.client.get(url);
        if let Some(span) = span {
            request = request.header(reqwest::header::RANGE, span.header());
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
            total_size: header("content-range").as_deref().and_then(total_of),
            etag: header("etag"),
            last_modified: header("last-modified"),
            body: resp
                .bytes_stream()
                .map(move |chunk| {
                    chunk.map_err(|e| body_error(e.is_timeout(), &e.to_string(), timeout, &owned))
                })
                .boxed(),
        })
    }
}

#[cfg(feature = "http")]
#[async_trait]
impl Transport for ReqwestTransport {
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError> {
        self.send(url, None).await
    }
    async fn get_range(&self, url: &str, span: Span) -> Result<HttpResponse, FetchError> {
        self.send(url, Some(span)).await
    }
}

/// What a failed body says: a stall names the timeout and URL, else the transport's words.
#[cfg(feature = "http")]
fn body_error(timed_out: bool, message: &str, timeout_secs: u64, url: &str) -> String {
    if timed_out {
        format!("timed out after {timeout_secs} seconds reading {url}")
    } else {
        message.to_owned()
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
    /// Whether ranged requests are honoured (206) or ignored (200 with the whole body).
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
            ranges: false,
        }
    }
    /// A 200 with a body, whose spans are served on a ranged request.
    pub fn ranged(body: &[u8]) -> Self {
        Self {
            ranges: true,
            ..Self::ok(body)
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
            ranges: false,
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

impl StaticTransport {
    fn serve(&self, url: &str, span: Option<Span>) -> Result<HttpResponse, FetchError> {
        self.requests
            .lock()
            .expect("not poisoned")
            .push(match span {
                Some(s) => format!("{url} {}", s.header()),
                None => url.to_owned(),
            });
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
        // A span of the body, as a server honouring the range serves it.
        let (status, body, total_size) = match span {
            Some(s) if canned.ranges && canned.status == 200 => {
                let total = canned.body.len() as u64;
                let (start, end) = match s {
                    Span::Bytes { start, end } => (start.min(total), end.min(total)),
                    Span::From(start) => (start.min(total), total),
                    Span::Last(n) => (total.saturating_sub(n), total),
                };
                let window = canned
                    .body
                    .slice(usize::try_from(start).unwrap_or(0)..usize::try_from(end).unwrap_or(0));
                (206, window, Some(total))
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

#[async_trait]
impl Transport for Arc<StaticTransport> {
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError> {
        self.serve(url, None)
    }
    async fn get_range(&self, url: &str, span: Span) -> Result<HttpResponse, FetchError> {
        self.serve(url, Some(span))
    }
}

#[cfg(all(test, feature = "http"))]
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
            "phasor-test",
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

#[cfg(test)]
mod ranged_tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_span_is_a_range_header_and_a_content_range_says_the_total() {
        assert_eq!(Span::Bytes { start: 0, end: 100 }.header(), "bytes=0-99");
        assert_eq!(Span::Bytes { start: 5, end: 5 }.header(), "bytes=5-4");
        assert_eq!(Span::Bytes { start: 0, end: 0 }.header(), "bytes=0-0");
        assert_eq!(Span::Last(65_557).header(), "bytes=-65557");
        assert_eq!(total_of("bytes 0-99/1234"), Some(1234));
        assert_eq!(total_of("bytes 0-99/*"), None);
        assert_eq!(total_of("bytes"), None);
    }

    async fn body_of(resp: HttpResponse) -> Vec<u8> {
        let mut out = Vec::new();
        let mut body = resp.body;
        while let Some(c) = body.next().await {
            out.extend_from_slice(&c.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn the_static_transport_serves_spans_of_a_ranged_body_and_ignores_them_otherwise() {
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/r.zip", Canned::ranged(b"0123456789"));
        t.add("https://h/w.zip", Canned::ok(b"0123456789"));
        let r = t
            .get_range("https://h/r.zip", Span::Bytes { start: 2, end: 5 })
            .await
            .unwrap();
        assert_eq!(
            (r.status, r.total_size, r.content_length),
            (206, Some(10), Some(3))
        );
        assert_eq!(body_of(r).await, b"234");
        let r = t.get_range("https://h/r.zip", Span::Last(4)).await.unwrap();
        assert_eq!(r.status, 206);
        assert_eq!(body_of(r).await, b"6789");
        // A span past the end is clipped; the last bytes of a short file
        // are all of it.
        let r = t
            .get_range("https://h/r.zip", Span::Bytes { start: 8, end: 50 })
            .await
            .unwrap();
        assert_eq!(body_of(r).await, b"89");
        let r = t
            .get_range("https://h/r.zip", Span::Last(50))
            .await
            .unwrap();
        assert_eq!(body_of(r).await, b"0123456789");
        // A server that ignores ranges answers 200 with everything.
        let r = t
            .get_range("https://h/w.zip", Span::Bytes { start: 2, end: 5 })
            .await
            .unwrap();
        assert_eq!((r.status, r.total_size), (200, None));
        assert_eq!(body_of(r).await, b"0123456789");
        // A plain GET of a ranged body is the whole body.
        let r = t.get("https://h/r.zip").await.unwrap();
        assert_eq!((r.status, r.total_size), (200, None));
        assert_eq!(body_of(r).await, b"0123456789");
        let seen = t.requests.lock().unwrap().clone();
        assert_eq!(seen[0], "https://h/r.zip bytes=2-4");
        assert_eq!(seen[5], "https://h/r.zip");
        // A body whose length is not claimed.
        let mut unclaimed = Canned::ranged(b"0123456789");
        unclaimed.claimed_length = Some(0);
        t.add("https://h/u.zip", unclaimed);
        let r = t.get_range("https://h/u.zip", Span::Last(2)).await.unwrap();
        assert_eq!((r.status, r.content_length), (206, None));
        // What is not queued is a 404 to a span too.
        let r = t.get_range("https://h/none", Span::Last(1)).await.unwrap();
        assert_eq!(r.status, 404);
    }

    /// A transport that only knows GET: a span is the whole file.
    struct Plain;

    #[async_trait]
    impl Transport for Plain {
        async fn get(&self, _url: &str) -> Result<HttpResponse, FetchError> {
            Ok(HttpResponse {
                status: 200,
                content_length: Some(3),
                total_size: None,
                etag: None,
                last_modified: None,
                body: futures::stream::iter([Ok(Bytes::from_static(b"abc"))]).boxed(),
            })
        }
    }

    #[tokio::test]
    async fn a_transport_without_ranges_answers_a_span_with_the_file_and_offline_refuses() {
        let r = Plain.get_range("https://h/a", Span::Last(1)).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(body_of(r).await, b"abc");
        assert!(!Plain.is_offline());
        assert!(matches!(
            OfflineTransport
                .get_range("https://h/a", Span::Last(1))
                .await,
            Err(FetchError::Offline { .. })
        ));
    }

    /// A local server answering each connection with the next reply; the request lines it saw.
    async fn serve(replies: Vec<Vec<u8>>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for reply in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                sock.write_all(&reply).await.unwrap();
                // Closed in order: the client reads everything first.
                let _ = sock.shutdown().await;
                let mut rest = [0u8; 256];
                while matches!(sock.read(&mut rest).await, Ok(n) if n > 0) {}
            }
            seen
        });
        (format!("http://{addr}"), handle)
    }

    fn http(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut r =
            format!("HTTP/1.1 {status}\r\n{headers}\r\nConnection: close\r\n\r\n").into_bytes();
        r.extend_from_slice(body);
        r
    }

    #[tokio::test]
    async fn reqwest_gets_a_file_and_a_span_and_reports_what_fails() {
        let (base, seen) = serve(vec![
            http(
                "200 OK",
                "Content-Length: 5\r\nETag: \"e\"\r\nLast-Modified: Tue, 13 Feb 2024 08:27:00 GMT",
                b"hello",
            ),
            http(
                "206 Partial Content",
                "Content-Length: 2\r\nContent-Range: bytes 1-2/5",
                b"el",
            ),
            // A body shorter than its length, then the connection closes.
            http("200 OK", "Content-Length: 50", b"short"),
        ])
        .await;
        let t = ReqwestTransport::new(
            "phasor-test",
            Duration::from_secs(2),
            Duration::from_millis(150),
        )
        .unwrap();
        let r = t.get(&format!("{base}/a.zip")).await.unwrap();
        assert_eq!(
            (r.status, r.content_length, r.total_size),
            (200, Some(5), None)
        );
        assert_eq!(r.etag.as_deref(), Some("\"e\""));
        assert_eq!(
            r.last_modified.as_deref(),
            Some("Tue, 13 Feb 2024 08:27:00 GMT")
        );
        assert_eq!(body_of(r).await, b"hello");
        let r = t
            .get_range(&format!("{base}/a.zip"), Span::Bytes { start: 1, end: 3 })
            .await
            .unwrap();
        assert_eq!((r.status, r.total_size), (206, Some(5)));
        assert_eq!(body_of(r).await, b"el");
        // The short body ends in an error from the stream, not a timeout.
        let r = t.get(&format!("{base}/short")).await.unwrap();
        let mut body = r.body;
        let mut got = Vec::new();
        let mut err = None;
        while let Some(c) = body.next().await {
            match c {
                Ok(b) => got.extend_from_slice(&b),
                Err(e) => err = Some(e),
            }
        }
        assert_eq!(got, b"short");
        let err = err.expect("the closed connection is an error");
        assert!(!err.contains("timed out"), "{err}");
        // A stalled body is told apart from a broken one by its words.
        assert_eq!(
            body_error(true, "x", 7, "https://h/a"),
            "timed out after 7 seconds reading https://h/a"
        );
        assert_eq!(body_error(false, "reset", 7, "https://h/a"), "reset");
        let requests = seen.await.unwrap();
        assert!(requests[1].contains("range: bytes=1-2"), "{}", requests[1]);
        assert!(requests[0].contains("user-agent: phasor-test"));
        // Nothing listens here: connecting fails.
        let e = t
            .get("http://127.0.0.1:9/x")
            .await
            .map(|_| ())
            .expect_err("nothing listens on port 9");
        assert!(matches!(e, FetchError::Connect { .. }), "{e}");
        // A server that never answers: the request times out.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;
            drop(sock);
        });
        let e = t
            .get(&format!("http://{addr}/slow"))
            .await
            .map(|_| ())
            .expect_err("the server never answers");
        assert!(matches!(e, FetchError::Timeout { secs: 0, .. }), "{e}");
        hold.await.unwrap();
        // A client that cannot be built.
        assert!(ReqwestTransport::new("\n", Duration::ZERO, Duration::ZERO).is_err());
    }
}
