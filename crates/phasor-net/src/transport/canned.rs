//! Canned responses served by URL, for tests here and in the crates that
//! enable `test-support`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;

use super::{HttpResponse, Span, Transport};
use crate::error::FetchError;

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
