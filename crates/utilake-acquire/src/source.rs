//! What a request asks for and what a source answers with.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;
use utilake_core::timetext::Stamp;
use utilake_core::{Period, TimeRange};
pub use utilake_zip::zipdir::Entry;

use crate::archive::Archive;
use crate::error::FetchError;
use crate::fetcher::{Fetcher, LocalFile};

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Table names (data-model names), upper-case. Empty means every table
    /// the source has for the period, whether a schema declares it or not.
    pub tables: Vec<String>,
    /// Table names to leave out, upper-case.
    pub exclude: Vec<String>,
    /// The periods asked for, inclusive.
    pub range: TimeRange,
}

/// Which entries of a zip a source wants fetched alone, out of the
/// directory it was shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picks {
    /// The zip whole: every entry, or more of them than ranged requests
    /// are worth.
    Whole,
    /// These entries, each as the file it becomes.
    Entries(Vec<Pick>),
}

/// One entry of a zip to fetch alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// Its index in the directory shown.
    pub index: usize,
    /// The file the entry becomes: its URL is the zip's with `!{entry}`
    /// appended, its table and period what the entry's name says.
    pub remote: RemoteFile,
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
    pub period: Option<Period>,
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
///
/// A source lists what a query needs from the network, finds what of that
/// is already in the archive, and reads what a file's name alone says.
/// The pipeline knows no source by name: every one it runs is a `dyn
/// Source`.
#[async_trait]
pub trait Source: Send + Sync {
    /// The source id, as the manifest records it with every file:
    /// `nem-mmsdm`, `wem-datafiles`. Never changed.
    fn id(&self) -> &'static str;

    /// The files a query needs, in a stable order.
    async fn list(&self, query: &Query, fetcher: &Fetcher) -> Result<Vec<RemoteFile>, FetchError>;

    /// The archived files of `query`, from their sidecars: nothing is
    /// hashed, nothing downloaded. A source whose archive is not read
    /// this way has none.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        let _ = (archive, query);
        Ok(Vec::new())
    }

    /// What this source knows about a file from its URL alone: the table
    /// and period its name says. `None` when the name is not one of its
    /// files.
    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        let _ = url;
        None
    }

    /// Whether `remote` is a zip that may hold more than a query needs (a
    /// whole-month archive, a day's zip of five-minute files), so its
    /// directory is worth reading by ranged requests before it is fetched.
    fn wants_ranged(&self, remote: &RemoteFile) -> bool {
        let _ = remote;
        false
    }

    /// Which entries of the zip `remote`, whose directory is `entries`, a
    /// query needs: the whole zip unless the source says otherwise.
    fn pick(&self, remote: &RemoteFile, entries: &[Entry], query: &Query) -> Picks {
        let _ = (remote, entries, query);
        Picks::Whole
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bare;

    #[async_trait]
    impl Source for Bare {
        fn id(&self) -> &'static str {
            "bare"
        }
        async fn list(&self, _q: &Query, _f: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn a_source_without_an_archive_or_names_answers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let q = Query {
            tables: Vec::new(),
            exclude: Vec::new(),
            range: TimeRange::months(
                utilake_core::YearMonth::new(2024, 1).unwrap(),
                utilake_core::YearMonth::new(2024, 1).unwrap(),
            ),
        };
        assert!(
            Bare.archived(&Archive::new(dir.path()), &q)
                .unwrap()
                .is_empty()
        );
        assert!(
            Bare.describe(&Url::parse("https://h/x.zip").unwrap())
                .is_none()
        );
        assert_eq!(Bare.id(), "bare");
        let f = Fetcher::new(
            std::sync::Arc::new(crate::transport::OfflineTransport),
            Archive::new(dir.path()),
            crate::fetcher::FetcherConfig::default(),
        );
        assert!(Bare.list(&q, &f).await.unwrap().is_empty());
        // No ranges, and the whole zip, unless a source says otherwise.
        let r = RemoteFile {
            url: Url::parse("https://h/MMSDM_2014_01.zip").unwrap(),
            source: "bare",
            table: None,
            period: None,
            size: None,
            modified: None,
            modified_at: None,
        };
        assert!(!Bare.wants_ranged(&r));
        assert_eq!(Bare.pick(&r, &[], &q), Picks::Whole);
        let p = Pick {
            index: 1,
            remote: r.clone(),
        };
        assert_eq!(Picks::Entries(vec![p.clone()]), Picks::Entries(vec![p]));
    }

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
        let odd = RemoteFile {
            url: Url::parse("https://h/a%20b(c).zip").unwrap(),
            ..r2
        };
        assert_eq!(odd.stem(), "a_20b_c_");
    }
}
