//! Events the pipeline emits while it runs. The CLI turns them into progress
//! bars or NDJSON; tests assert on them; libraries never print.

use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::report::SourceOutcome;

/// One thing that happened during a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The source listed this many files for the request.
    Listed {
        /// Files planned.
        files: usize,
    },
    /// A download began.
    DownloadStarted {
        /// The URL.
        url: String,
        /// Content-Length when the server sent one.
        size: Option<u64>,
    },
    /// Bytes arrived.
    DownloadProgress {
        /// The URL.
        url: String,
        /// Bytes received so far.
        bytes: u64,
    },
    /// A download finished (or was served from cache).
    DownloadDone {
        /// The URL.
        url: String,
        /// Whether it came from the cache without a request.
        cached: bool,
        /// Bytes on disk.
        bytes: u64,
    },
    /// Parsing of one source began.
    IngestStarted {
        /// URL or path.
        source: String,
    },
    /// An I row was resolved against the registry.
    BlockResolved {
        /// URL or path plus entry.
        source: String,
        /// The table.
        table: String,
        /// The version.
        version: u16,
        /// `exact`, `aliased`, `drift`, `inferred`.
        status: String,
    },
    /// A source finished.
    SourceDone(Box<SourceOutcome>),
    /// A diagnostic was raised.
    Diagnostic(Box<Diagnostic>),
    /// The run finished.
    Summary(Box<Summary>),
}

/// The end-of-run totals.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Files the source listed.
    pub listed: usize,
    /// Files downloaded this run.
    pub downloaded: usize,
    /// Files served from cache.
    pub cached: usize,
    /// Sources ingested.
    pub ingested: usize,
    /// Sources skipped as already ingested.
    pub skipped: usize,
    /// Sources that failed.
    pub failed: usize,
    /// Rows written.
    pub rows: u64,
    /// Rows quarantined.
    pub bad_rows: u64,
    /// CSV bytes scanned.
    pub bytes_in: u64,
    /// Parquet bytes written.
    pub bytes_out: u64,
    /// Wall time in milliseconds.
    pub elapsed_ms: u64,
    /// Errors raised.
    pub errors: u64,
    /// Warnings raised.
    pub warnings: u64,
}

impl Summary {
    /// CSV megabytes scanned per second of wall time.
    pub fn mb_per_s(&self) -> f64 {
        if self.elapsed_ms == 0 {
            return 0.0;
        }
        (self.bytes_in as f64 / 1_000_000.0) / (self.elapsed_ms as f64 / 1000.0)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn serialises_with_tag() {
        let e = Event::Listed { files: 3 };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"event":"listed","files":3}"#);
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back, e);
        let sum = Summary { bytes_in: 2_000_000, elapsed_ms: 1000, ..Default::default() };
        assert!((sum.mb_per_s() - 2.0).abs() < 1e-9);
        assert_eq!(Summary::default().mb_per_s(), 0.0);
        let s = serde_json::to_string(&Event::Summary(Box::new(sum))).unwrap();
        assert!(s.starts_with(r#"{"event":"summary""#));
    }
}
