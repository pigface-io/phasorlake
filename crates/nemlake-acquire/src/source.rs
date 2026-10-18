//! What a request asks for and what a source answers with.

use async_trait::async_trait;
use nemlake_core::YearMonth;
use nemlake_core::timetext::Stamp;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::FetchError;
use crate::fetcher::Fetcher;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Table names (data-model names), upper-case. Empty means every table
    /// the source has for the period, whether a schema declares it or not.
    pub tables: Vec<String>,
    /// Table names to leave out, upper-case.
    pub exclude: Vec<String>,
    /// First month.
    pub from: YearMonth,
    /// Last month, inclusive.
    pub to: YearMonth,
}

/// A file a source wants fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFile {
    /// Where.
    pub url: Url,
    /// Source id, e.g. `nem-mmsdm`.
    pub source: &'static str,
    /// The table it carries, when the source knows (archive files do).
    pub table: Option<String>,
    /// The period it covers.
    pub period: Option<YearMonth>,
    /// Size the listing reported.
    pub size: Option<u64>,
    /// Modified time the listing (or the WEM manifest) reported, as text.
    pub modified: Option<String>,
    /// `modified` as an instant: `+10:00` for what nemweb lists, UTC for
    /// a WEM manifest. `None` when the text is not a time.
    pub modified_at: Option<Stamp>,
}

impl RemoteFile {
    /// The last path segment. `%23` is read as `#`: the newer archive
    /// names are `PUBLIC_ARCHIVE#TABLE#FILE01#...` and a URL has to escape
    /// the `#`.
    pub fn file_name(&self) -> String {
        self.url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .filter(|s| !s.is_empty())
            .unwrap_or("download")
            .replace("%23", "#")
    }

    /// The file name without its extension, lower-case, with every
    /// character other than letters, digits, `_`, `-` and `.` turned into
    /// `_`: the stem that names the Parquet files written from it, safe in
    /// a path, a glob and a URL.
    pub fn stem(&self) -> String {
        let name = self.file_name();
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
        stem.chars()
            .map(|c| {
                let c = c.to_ascii_lowercase();
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }
}

/// Where files live for one market and publication.
#[async_trait]
pub trait Source: Send + Sync {
    /// The files a query needs, in a stable order.
    async fn list(&self, query: &Query, fetcher: &Fetcher) -> Result<Vec<RemoteFile>, FetchError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let r = RemoteFile {
            url: Url::parse("https://h/a/PUBLIC_DVD_REGION_202401010000.zip").unwrap(),
            source: "nem-mmsdm",
            table: None,
            period: None,
            size: None,
            modified: None,
            modified_at: None,
        };
        assert_eq!(r.file_name(), "PUBLIC_DVD_REGION_202401010000.zip");
        assert_eq!(r.stem(), "public_dvd_region_202401010000");
        let r2 = RemoteFile {
            url: Url::parse("https://h/noext").unwrap(),
            ..r
        };
        assert_eq!(r2.stem(), "noext");
    }
}
