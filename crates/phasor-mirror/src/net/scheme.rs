//! One transport for every host: a request goes to the transport of its
//! URL's scheme, `ftp://` to the FTP one and the rest to HTTP, so the
//! fetchers, their pacing and their breakers need not know which.

use std::sync::Arc;

use async_trait::async_trait;

use super::FetchError;
use super::transport::{HttpResponse, Span, Transport};

/// HTTP and FTP transports behind one.
pub struct SchemeTransport {
    http: Arc<dyn Transport>,
    ftp: Arc<dyn Transport>,
}

impl SchemeTransport {
    /// `http` for http(s) URLs, `ftp` for ftp URLs.
    pub fn new(http: Arc<dyn Transport>, ftp: Arc<dyn Transport>) -> Self {
        Self { http, ftp }
    }

    fn of(&self, url: &str) -> &dyn Transport {
        if url.starts_with("ftp://") {
            self.ftp.as_ref()
        } else {
            self.http.as_ref()
        }
    }
}

#[async_trait]
impl Transport for SchemeTransport {
    async fn get(&self, url: &str) -> Result<HttpResponse, FetchError> {
        self.of(url).get(url).await
    }
    async fn get_range(&self, url: &str, span: Span) -> Result<HttpResponse, FetchError> {
        self.of(url).get_range(url, span).await
    }
    async fn head(&self, url: &str) -> Result<HttpResponse, FetchError> {
        self.of(url).head(url).await
    }
    fn is_offline(&self) -> bool {
        self.http.is_offline() && self.ftp.is_offline()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::transport::{Canned, OfflineTransport, StaticTransport};
    use futures::StreamExt as _;

    #[tokio::test]
    async fn each_scheme_goes_to_its_transport() {
        let http = Arc::new(StaticTransport::default());
        http.add("https://h/a", Canned::ok(b"web"));
        let ftp = Arc::new(StaticTransport::default());
        ftp.add("ftp://f/a", Canned::ok(b"file"));
        let t = SchemeTransport::new(Arc::new(http.clone()), Arc::new(ftp.clone()));
        let body = |r: HttpResponse| async { r.body.map(|c| c.unwrap().to_vec()).concat().await };
        assert_eq!(body(t.get("https://h/a").await.unwrap()).await, b"web");
        assert_eq!(body(t.get("ftp://f/a").await.unwrap()).await, b"file");
        assert_eq!(t.head("ftp://f/a").await.unwrap().content_length, Some(4));
        assert_eq!(
            t.get_range("https://h/a", Span::Last(1))
                .await
                .unwrap()
                .status,
            200
        );
        assert_eq!((http.request_count(), ftp.request_count()), (2, 2));
        assert!(!t.is_offline());
        let off = SchemeTransport::new(Arc::new(OfflineTransport), Arc::new(OfflineTransport));
        assert!(off.is_offline());
        let half = SchemeTransport::new(Arc::new(http), Arc::new(OfflineTransport));
        assert!(!half.is_offline());
    }
}
