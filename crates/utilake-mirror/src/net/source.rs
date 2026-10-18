//! A file on nemweb to download.

use serde::{Deserialize, Serialize};
use url::Url;

use crate::stamp::Stamp;

/// A remote file: where it is and what its listing said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFile {
    /// The absolute URL.
    pub url: Url,
    /// Size in bytes, as listed.
    pub size: Option<u64>,
    /// Date and time, as listed.
    pub modified: Option<String>,
    /// The listed date and time as an instant (AEST).
    #[serde(default)]
    pub modified_at: Option<Stamp>,
}
