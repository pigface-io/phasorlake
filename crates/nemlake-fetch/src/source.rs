//! What a request asks for and what a source answers with.

use async_trait::async_trait;
use nemlake_core::YearMonth;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::FetchError;
use crate::fetcher::Fetcher;
use crate::remotezip::ZipMember;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Table names (data-model names), upper-case. Empty means every table.
    pub tables: Vec<String>,
    /// First month.
    pub from: YearMonth,
    /// Last month, inclusive.
    pub to: YearMonth,
}

/// What kind of file a remote is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// A zip of MMS CSVs.
    MmsZip,
    /// A plain CSV with a header row.
    PlainCsv,
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
    /// Kind.
    pub kind: FileKind,
    /// Size the listing reported.
    pub size: Option<u64>,
    /// Modified time the listing reported, as text.
    pub modified: Option<String>,
    /// Set when the file is one member of a larger remote zip: `url` is then
    /// the container's URL with the member name as its fragment, and only
    /// the member's bytes are downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<Box<ZipMember>>,
}

impl RemoteFile {
    /// The identity of a member of the zip at `container`: `container#member`.
    pub fn member_url(container: &Url, member_name: &str) -> Url {
        let mut url = container.clone();
        url.set_fragment(Some(member_name));
        url
    }

    /// The URL to request: `url` without the member fragment.
    pub fn request_url(&self) -> Url {
        let mut url = self.url.clone();
        if self.member.is_some() {
            url.set_fragment(None);
        }
        url
    }

    /// The last path segment, or the member's file name.
    pub fn file_name(&self) -> String {
        if let Some(m) = &self.member {
            return m.file_name().to_owned();
        }
        self.url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("download")
            .to_owned()
    }

    /// The file name without its extension, lower-case: the stem that names
    /// the Parquet files written from it.
    pub fn stem(&self) -> String {
        let name = self.file_name();
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
        stem.to_ascii_lowercase()
    }
}

/// Where files live for one market and publication.
#[async_trait]
pub trait Source: Send + Sync {
    /// Stable id, e.g. `nem-mmsdm`.
    fn id(&self) -> &'static str;
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
            kind: FileKind::MmsZip,
            size: None,
            modified: None,
            member: None,
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
