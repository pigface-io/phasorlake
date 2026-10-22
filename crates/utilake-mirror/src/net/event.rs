//! What a download reports while it runs.

use serde::{Deserialize, Serialize};

/// One thing that happened during a download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
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
}
