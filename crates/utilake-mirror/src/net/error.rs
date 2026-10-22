//! Errors of listing, downloading and caching.

use crate::diag::{Category, Diagnose, Diagnostic};

/// Why a fetch failed.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The connection could not be made.
    #[error("Connecting to {url} failed: {message}")]
    Connect {
        /// URL.
        url: String,
        /// Cause.
        message: String,
    },
    /// The request or body read timed out.
    #[error("The request to {url} timed out after {secs} seconds.")]
    Timeout {
        /// URL.
        url: String,
        /// Timeout.
        secs: u64,
    },
    /// The server answered with a failure status.
    #[error("GET {url} returned HTTP {status}.")]
    HttpStatus {
        /// URL.
        url: String,
        /// Status.
        status: u16,
        /// First bytes of the body.
        body_head: Vec<u8>,
    },
    /// Fewer or more bytes than Content-Length.
    #[error(
        "Download of {url} ended after {received} bytes but Content-Length promised {expected}."
    )]
    ContentLength {
        /// URL.
        url: String,
        /// Promised.
        expected: u64,
        /// Received.
        received: u64,
    },
    /// Every attempt failed.
    #[error("Download of {url} failed after {attempts} attempts; the last error was: {last}")]
    RetriesExhausted {
        /// URL.
        url: String,
        /// Attempts.
        attempts: u32,
        /// Last error.
        last: Box<FetchError>,
    },
    /// Offline and not cached.
    #[error("{url} is not in the cache and --offline forbids downloading it.")]
    Offline {
        /// URL.
        url: String,
    },
    /// The body stream failed mid-way.
    #[error("Reading the body of {url} failed after {received} bytes: {message}")]
    Body {
        /// URL.
        url: String,
        /// Bytes so far.
        received: u64,
        /// Cause.
        message: String,
    },
    /// A ranged request was answered with the whole file.
    #[error(
        "{url} was asked for a range of its bytes and answered with the whole file; the server does not serve ranges."
    )]
    RangeUnsupported {
        /// URL.
        url: String,
    },
    /// The server refuses this client.
    #[error(
        "The server refused {url} with HTTP {status}, first seen at {first_url}. AEMO's web server does this after too many requests in a short time, so nemweb-mirror sends it nothing more in this run. Wait a while before trying again; a larger `request_interval_ms` in the config file slows nemweb-mirror down."
    )]
    Blocked {
        /// The URL of this request.
        url: String,
        /// The status that tripped the block.
        status: u16,
        /// The request that was refused first.
        first_url: String,
        /// First bytes of that refusal's body.
        body_head: Vec<u8>,
    },
    /// A file of the archive could not be read or written.
    #[error("The archive path {path} could not be read or written.")]
    CacheIo {
        /// Path.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A sidecar is unreadable.
    #[error("The cache sidecar {path} is not valid JSON and will be rewritten: {message}")]
    SidecarCorrupt {
        /// Path.
        path: String,
        /// Cause.
        message: String,
    },
    /// The archived bytes do not match the sidecar.
    #[error(
        "The archived file {path} hashes to {observed} but its sidecar records {expected}; the file changed on disk after it was downloaded."
    )]
    Sha256Mismatch {
        /// Path.
        path: String,
        /// Recorded.
        expected: String,
        /// Computed.
        observed: String,
    },
}

impl FetchError {
    /// Whether another attempt could succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Connect { .. }
            | Self::Timeout { .. }
            | Self::ContentLength { .. }
            | Self::Body { .. } => true,
            Self::HttpStatus { status, .. } => *status == 429 || *status == 408 || *status >= 500,
            _ => false,
        }
    }
}

impl Diagnose for FetchError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Connect { url, .. } => {
                Diagnostic::error("net.connect", Category::Net, msg).with_url(url.clone())
            }
            Self::Timeout { url, .. } => {
                Diagnostic::error("net.timeout", Category::Net, msg).with_url(url.clone())
            }
            Self::HttpStatus {
                url,
                status,
                body_head,
            } => Diagnostic::error("net.http_status", Category::Net, msg)
                .with_url(url.clone())
                .expected_observed("200", status.to_string())
                .with_sample(body_head),
            Self::ContentLength {
                url,
                expected,
                received,
            } => Diagnostic::error("net.content_length_mismatch", Category::Net, msg)
                .with_url(url.clone())
                .expected_observed(expected.to_string(), received.to_string()),
            Self::RetriesExhausted { url, last, .. } => {
                let mut d = Diagnostic::error("net.retries_exhausted", Category::Net, msg)
                    .with_url(url.clone());
                d.cause.push(last.to_string());
                d
            }
            Self::Offline { url } => {
                Diagnostic::error("net.offline", Category::Net, msg).with_url(url.clone())
            }
            Self::Body { url, .. } => {
                Diagnostic::error("net.body", Category::Net, msg).with_url(url.clone())
            }
            Self::RangeUnsupported { url } => {
                Diagnostic::error("net.range_unsupported", Category::Net, msg).with_url(url.clone())
            }
            Self::Blocked {
                url,
                status,
                body_head,
                ..
            } => Diagnostic::error("net.blocked", Category::Net, msg)
                .with_url(url.clone())
                .expected_observed("200", status.to_string())
                .with_sample(body_head),
            Self::CacheIo { path, source } => Diagnostic::error("cache.io", Category::Cache, msg)
                .with_file(path.clone())
                .caused_by(source),
            Self::SidecarCorrupt { path, .. } => {
                Diagnostic::warning("cache.sidecar_corrupt", Category::Cache, msg)
                    .with_file(path.clone())
            }
            Self::Sha256Mismatch {
                path,
                expected,
                observed,
            } => Diagnostic::warning("cache.sha256_mismatch", Category::Cache, msg)
                .with_file(path.clone())
                .expected_observed(expected.clone(), observed.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_and_codes() {
        assert!(
            FetchError::Connect {
                url: "u".into(),
                message: "m".into()
            }
            .is_retryable()
        );
        assert!(
            FetchError::Timeout {
                url: "u".into(),
                secs: 1
            }
            .is_retryable()
        );
        assert!(
            FetchError::HttpStatus {
                url: "u".into(),
                status: 503,
                body_head: vec![]
            }
            .is_retryable()
        );
        assert!(
            FetchError::HttpStatus {
                url: "u".into(),
                status: 429,
                body_head: vec![]
            }
            .is_retryable()
        );
        assert!(
            !FetchError::HttpStatus {
                url: "u".into(),
                status: 404,
                body_head: vec![]
            }
            .is_retryable()
        );
        assert!(
            FetchError::ContentLength {
                url: "u".into(),
                expected: 1,
                received: 0
            }
            .is_retryable()
        );
        assert!(
            FetchError::Body {
                url: "u".into(),
                received: 0,
                message: "m".into()
            }
            .is_retryable()
        );
        assert!(!FetchError::Offline { url: "u".into() }.is_retryable());
        let d = FetchError::HttpStatus {
            url: "u".into(),
            status: 503,
            body_head: b"<html>".to_vec(),
        }
        .diagnostic();
        assert_eq!(d.code, "net.http_status");
        assert_eq!(d.observed.as_deref(), Some("503"));
        assert_eq!(d.sample.unwrap().text, "<html>");
        let d = FetchError::RetriesExhausted {
            url: "u".into(),
            attempts: 5,
            last: Box::new(FetchError::Timeout {
                url: "u".into(),
                secs: 1,
            }),
        }
        .diagnostic();
        assert_eq!(d.code, "net.retries_exhausted");
        assert_eq!(d.cause.len(), 1);
        for (e, code) in [
            (
                FetchError::Connect {
                    url: "u".into(),
                    message: "m".into(),
                },
                "net.connect",
            ),
            (
                FetchError::Timeout {
                    url: "u".into(),
                    secs: 1,
                },
                "net.timeout",
            ),
            (
                FetchError::ContentLength {
                    url: "u".into(),
                    expected: 1,
                    received: 0,
                },
                "net.content_length_mismatch",
            ),
            (FetchError::Offline { url: "u".into() }, "net.offline"),
            (
                FetchError::RangeUnsupported { url: "u".into() },
                "net.range_unsupported",
            ),
            (
                FetchError::Body {
                    url: "u".into(),
                    received: 0,
                    message: "m".into(),
                },
                "net.body",
            ),
            (
                FetchError::Blocked {
                    url: "u".into(),
                    status: 403,
                    first_url: "f".into(),
                    body_head: b"Website is unavailable".to_vec(),
                },
                "net.blocked",
            ),
            (
                FetchError::CacheIo {
                    path: "p".into(),
                    source: std::io::Error::other("x"),
                },
                "cache.io",
            ),
            (
                FetchError::SidecarCorrupt {
                    path: "p".into(),
                    message: "m".into(),
                },
                "cache.sidecar_corrupt",
            ),
            (
                FetchError::Sha256Mismatch {
                    path: "p".into(),
                    expected: "a".into(),
                    observed: "b".into(),
                },
                "cache.sha256_mismatch",
            ),
        ] {
            assert_eq!(e.diagnostic().code, code, "{e}");
        }
    }
}
