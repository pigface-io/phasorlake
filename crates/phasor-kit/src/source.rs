//! What a request asks for, what a source answers with, and the file a fetch hands back.

use std::path::PathBuf;

use async_trait::async_trait;
use phasor_core::timetext::Stamp;
use phasor_core::{Period, TimeRange};
use phasor_net::FetchError;
use phasor_net::archive::{Archive, Held};
pub use phasor_zip::zipdir::Entry;
use serde::{Deserialize, Serialize};
use url::Url;

/// How a source fetches a listing page into memory.
#[async_trait]
pub trait Listings: Send + Sync {
    /// GETs the page at `url`.
    async fn fetch_listing(&self, url: &str) -> Result<Vec<u8>, FetchError>;
}

/// A file on disk, ready to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// What was asked for.
    pub remote: RemoteFile,
    /// Where the bytes are.
    pub path: PathBuf,
    /// sha256 hex.
    pub sha256: String,
    /// Bytes.
    pub size: u64,
    /// Whether it was in the archive already, with no request.
    pub cached: bool,
    /// ETag.
    pub etag: Option<String>,
    /// The server's `Last-Modified`, as sent (an HTTP date).
    pub last_modified: Option<String>,
    /// `last_modified` as an instant (UTC).
    pub last_modified_at: Option<Stamp>,
    /// When the bytes were fetched, per their record; none for a file not from the archive.
    pub fetched_at: Option<Stamp>,
    /// The previous version of this file, when the source keeps versions: only rows
    /// new to this version are ingested.
    pub previous: Option<PathBuf>,
}

impl LocalFile {
    /// An archived file as the answer to `remote`; `cached` when no request was made.
    pub fn from_held(remote: &RemoteFile, found: Held, cached: bool) -> Self {
        let p = &found.provenance;
        Self {
            remote: remote.clone(),
            sha256: p.content.sha256.hex(),
            size: p.content.size,
            cached,
            etag: p.etag().map(str::to_owned),
            last_modified: p.modified().map(Stamp::http_date),
            last_modified_at: p.modified(),
            fetched_at: Some(p.fetched_at()),
            previous: None,
            path: found.path,
        }
    }
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Upper-case data-model table names. Empty means every table the source has.
    pub tables: Vec<String>,
    /// Table names to leave out, upper-case.
    pub exclude: Vec<String>,
    /// The periods asked for, inclusive.
    pub range: TimeRange,
    /// Instants asked for, `[from, to)` in microseconds since the epoch, when finer
    /// than a day. `None` means the whole of every period.
    pub instants: Option<(i64, i64)>,
}

/// How finely a source cuts its files: a month, or a day or less.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    /// One file covers a month.
    Month,
    /// One file covers a day, or a slice of one.
    Day,
}

/// Which entries of a zip a source wants fetched alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picks {
    /// The zip whole: every entry, or more than ranged requests are worth.
    Whole,
    /// These entries, each as the file it becomes.
    Entries(Vec<Pick>),
}

/// One entry of a zip to fetch alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// Its index in the directory shown.
    pub index: usize,
    /// The file the entry becomes: the zip's URL plus `!{entry}`, table and period from its name.
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
    /// `modified` as an instant: `+10:00` for nemweb, UTC for a WEM manifest; `None` if not a time.
    pub modified_at: Option<Stamp>,
}

impl RemoteFile {
    /// The last path segment, `%23` read as `#` (archive names like `PUBLIC_ARCHIVE#TABLE#...`).
    pub fn file_name(&self) -> String {
        self.url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .filter(|s| !s.is_empty())
            .unwrap_or("download")
            .replace("%23", "#")
    }

    /// The file name without extension, lower-case, other than `[a-z0-9_.-]` turned
    /// into `_`: the stem of its Parquet files, safe in a path, glob and URL.
    pub fn stem(&self) -> String {
        let name = self.file_name();
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
        // A version of a file (a stamp as the URL's fragment) keeps its own Parquet files.
        let tagged = self.url.fragment().map(|f| format!("{stem}-{f}"));
        tagged
            .as_deref()
            .unwrap_or(stem)
            .chars()
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

/// Where files live for one market and publication: listed from the network,
/// found in the archive, described by name alone.
#[async_trait]
pub trait Source: Send + Sync {
    /// The source id the manifest records with every file (`nem-mmsdm`). Never changed.
    fn id(&self) -> &'static str;

    /// The files a query needs, in a stable order.
    async fn list(
        &self,
        query: &Query,
        listings: &dyn Listings,
    ) -> Result<Vec<RemoteFile>, FetchError>;

    /// The archived files of `query`, from their sidecars alone. None by default.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        let _ = (archive, query);
        Ok(Vec::new())
    }

    /// The table and period a file's URL alone says; `None` when not one of its files.
    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        let _ = url;
        None
    }

    /// How finely the source cuts its files.
    fn granularity(&self) -> Granularity {
        Granularity::Month
    }

    /// Whether this source publishes `table`; every table by default.
    fn serves(&self, table: &str) -> bool {
        let _ = table;
        true
    }

    /// Whether `remote` is a zip worth reading by its directory through ranged
    /// requests because `query` needs only part of it.
    fn wants_ranged(&self, remote: &RemoteFile, query: &Query) -> bool {
        let _ = (remote, query);
        false
    }

    /// Which entries of the zip `remote` a query needs; the whole zip by default.
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
        async fn list(&self, _q: &Query, _l: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
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
                phasor_core::YearMonth::new(2024, 1).unwrap(),
                phasor_core::YearMonth::new(2024, 1).unwrap(),
            ),
            instants: None,
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
        struct NoPages;
        #[async_trait]
        impl Listings for NoPages {
            async fn fetch_listing(&self, url: &str) -> Result<Vec<u8>, FetchError> {
                Err(FetchError::Offline {
                    url: url.to_owned(),
                })
            }
        }
        assert!(Bare.list(&q, &NoPages).await.unwrap().is_empty());
        assert!(NoPages.fetch_listing("https://h/").await.is_err());
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
        assert!(!Bare.wants_ranged(&r, &q));
        assert_eq!(Bare.granularity(), Granularity::Month);
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
        // A version of a file, told apart by its fragment, has its own stem.
        let version = RemoteFile {
            url: Url::parse("https://h/GBB/GasBBActualFlowStorageLast31.CSV#20261021T191100+1000")
                .unwrap(),
            ..odd.clone()
        };
        assert_eq!(
            version.stem(),
            "gasbbactualflowstoragelast31-20261021t191100_1000"
        );
    }
}
