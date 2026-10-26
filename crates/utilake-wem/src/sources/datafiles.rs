//! WEM public data: one CSV per dataset per month, listed by a YAML manifest.
//!
//! ```yaml
//! - url: "https://data.wa.aemo.com.au/datafiles/facility-scada/facility-scada-2023-10.csv"
//!   year: 2023
//!   month: 10
//!   generatedAt: "Thu Nov 02 2023 11:35:00Z"
//! ```
//!
//! The manifests list what the host publishes; the archive is read by the
//! feed's folders (`public/public-data/datafiles/{dataset}/`), every
//! version the mirror kept of a rewritten month in order.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;
use utilake_acquire::FeedFolders;
use utilake_core::YearMonth;
use utilake_core::calendar::WEM_OFFSET_SECS;
use utilake_core::timetext::{self, Stamp};
use utilake_kit::TableOf;
use utilake_plain_decode::feeds::feed;

use utilake_acquire::{Archive, FetchError};
use utilake_kit::{Listings, LocalFile, Query, RemoteFile, Source};

/// The source id.
pub const SOURCE_ID: &str = "wem-datafiles";

/// One manifest entry.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    /// The CSV.
    pub url: String,
    /// Year.
    #[serde(deserialize_with = "int_or_str")]
    pub year: i32,
    /// Month. AEMO writes `09`, which YAML 1.2 reads as a string, so both
    /// forms are accepted.
    #[serde(deserialize_with = "int_or_str")]
    pub month: u8,
    /// When AEMO generated it, as the manifest wrote it: `Thu Nov 02 2023
    /// 11:35:00Z`.
    #[serde(default)]
    pub generated_at: Option<String>,
    /// `generated_at` as an instant in UTC, set by [`parse_manifest`]; the
    /// field already ends in `_at`, so its twin is `_utc`. A value AEMO
    /// wrote without an offset is read as WEM time first. `None` when the
    /// text is not a time.
    #[serde(default, skip)]
    pub generated_at_utc: Option<Stamp>,
}

fn int_or_str<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr + TryFrom<i64>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Str(String),
    }
    match Raw::deserialize(d)? {
        Raw::Int(i) => {
            T::try_from(i).map_err(|_| serde::de::Error::custom(format!("{i} is out of range")))
        }
        Raw::Str(s) => s
            .trim()
            .parse()
            .map_err(|_| serde::de::Error::custom(format!("{s:?} is not a number"))),
    }
}

/// Parses a manifest's YAML.
pub fn parse_manifest(url: &str, text: &str) -> Result<Vec<ManifestEntry>, FetchError> {
    let mut entries: Vec<ManifestEntry> =
        serde_yaml_ng::from_str(text).map_err(|e| FetchError::Parse {
            url: url.to_owned(),
            what: "WEM manifest YAML",
            message: e.to_string(),
        })?;
    for e in &mut entries {
        e.generated_at_utc = e
            .generated_at
            .as_deref()
            .and_then(|s| timetext::parse(s, WEM_OFFSET_SECS))
            .map(|s| Stamp::new(s.secs_utc, 0));
    }
    Ok(entries)
}

/// WEM datasets.
pub struct WemDatafiles {
    base: Url,
    /// Dataset names to list, e.g. `facility-scada`.
    pub datasets: Vec<String>,
    /// The feed's folders, which find the files in the archive.
    folders: FeedFolders,
}

impl WemDatafiles {
    /// Over `base`, normally `https://data.wa.aemo.com.au`; no table
    /// claims a file until [`Self::with_tables`].
    pub fn new(base: &str, datasets: Vec<String>) -> Result<Self, FetchError> {
        let url = Url::parse(base).map_err(|e| FetchError::Parse {
            url: base.to_owned(),
            what: "base URL",
            message: e.to_string(),
        })?;
        let feed = feed("datafiles").expect("the feeds table names the WEM datafiles");
        let folders = FeedFolders::new(SOURCE_ID, base, feed, Arc::new(|_| None))?;
        Ok(Self {
            base: url,
            datasets,
            folders,
        })
    }

    /// The same source, its files' tables named by `table_of`.
    pub fn with_tables(self, table_of: TableOf) -> Result<Self, FetchError> {
        let feed = feed("datafiles").expect("the feeds table names the WEM datafiles");
        Ok(Self {
            folders: FeedFolders::new(SOURCE_ID, self.base.as_str(), feed, table_of)?,
            ..self
        })
    }

    /// The manifest URL of a dataset.
    pub fn manifest_url(&self, dataset: &str) -> Result<Url, FetchError> {
        self.base
            .join(&format!("/public/public-data/manifests/{dataset}.yaml"))
            .map_err(|e| FetchError::Parse {
                url: dataset.to_owned(),
                what: "manifest URL",
                message: e.to_string(),
            })
    }
}

/// What a datafile's name says: `{dataset}-YYYY-MM.csv` (a month) or
/// `{dataset}-YYYY.csv` (a year, whose period no month names), as AEMO
/// names them. `None` for any other name.
pub fn describe_name(url: Url) -> Option<RemoteFile> {
    let name = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())?
        .to_owned();
    let stem = name.strip_suffix(".csv")?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (dataset, last) = stem.rsplit_once('-')?;
    let (dataset, period) = if last.len() == 4 && digits(last) {
        (dataset, None)
    } else {
        let (dataset, year) = dataset.rsplit_once('-')?;
        if year.len() != 4 || last.len() != 2 || !digits(year) || !digits(last) {
            return None;
        }
        let ym: YearMonth = format!("{year}-{last}").parse().ok()?;
        (dataset, Some(ym.into()))
    };
    if dataset.is_empty() {
        return None;
    }
    Some(RemoteFile {
        url,
        source: SOURCE_ID,
        table: Some(dataset.to_owned()),
        period,
        size: None,
        modified: None,
        modified_at: None,
    })
}

#[async_trait]
impl Source for WemDatafiles {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    /// A datafile by its name, with the table the registry gives its block
    /// when the URL lies under the feed's folders.
    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        let mut r = describe_name(url.clone())?;
        if let Some(f) = self.folders.describe(url) {
            r.table = f.table.or(r.table);
            r.period = r.period.or(f.period);
        }
        Some(r)
    }

    /// The archived datafiles, every version the mirror kept of a
    /// rewritten month in order, each naming the one before it.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        self.folders.archived(archive, query)
    }

    async fn list(
        &self,
        query: &Query,
        fetcher: &dyn Listings,
    ) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        for ds in &self.datasets {
            let murl = self.manifest_url(ds)?;
            let body = fetcher.fetch_listing(murl.as_str()).await?;
            let text = String::from_utf8_lossy(&body);
            for e in parse_manifest(murl.as_str(), &text)? {
                let Some(ym) = YearMonth::new(e.year, e.month) else {
                    continue;
                };
                if !query.range.contains(ym) {
                    continue;
                }
                let url = Url::parse(&e.url).map_err(|er| FetchError::Parse {
                    url: e.url.clone(),
                    what: "manifest entry URL",
                    message: er.to_string(),
                })?;
                out.push(RemoteFile {
                    url,
                    source: SOURCE_ID,
                    table: Some(ds.clone()),
                    period: Some(ym.into()),
                    size: None,
                    modified: e.generated_at,
                    modified_at: e.generated_at_utc,
                });
            }
        }
        out.sort_by(|a, b| (a.period, &a.table).cmp(&(b.period, &b.table)));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;
    use utilake_acquire::Fetcher;

    use super::*;
    use utilake_acquire::transport::{Canned, StaticTransport};
    use utilake_acquire::{Archive, FetcherConfig};
    use utilake_core::TimeRange;

    #[test]
    fn a_datafile_name_says_its_dataset_and_month() {
        let r = describe_name(
            Url::parse(
                "https://data.wa.aemo.com.au/datafiles/facility-scada/facility-scada-2023-10.csv",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(r.table.as_deref(), Some("facility-scada"));
        assert_eq!(r.period.unwrap().to_string(), "2023-10");
        assert_eq!(r.source, SOURCE_ID);
        for bad in [
            "https://h/facility-scada.csv",
            "https://h/facility-scada-2023-13.csv",
            "https://h/-2023-10.csv",
            "https://h/x-23-10.csv",
            "https://h/facility-scada-2023-10.zip",
            "https://h/",
        ] {
            assert!(describe_name(Url::parse(bad).unwrap()).is_none(), "{bad}");
        }
        // A year's file: its dataset, no month.
        let yearly = describe_name(
            Url::parse("https://h/public/public-data/datafiles/balancing-summary/balancing-summary-2022.csv").unwrap(),
        )
        .unwrap();
        assert_eq!(yearly.table.as_deref(), Some("balancing-summary"));
        assert_eq!(yearly.period, None);
        let w = WemDatafiles::new("https://h", Vec::new()).unwrap();
        assert!(
            w.describe(&Url::parse("https://h/facility-scada-2023-10.csv").unwrap())
                .is_some()
        );
        // Under the feed's folders, the registry's table replaces the
        // dataset once tables are named; the archive is read there too.
        let named = w
            .with_tables(Arc::new(|b| Some(format!("WEM_{}", b.name))))
            .unwrap();
        let r = named
            .describe(
                &Url::parse("https://h/public/public-data/datafiles/facility-scada/facility-scada-2023-10.csv")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(r.table.as_deref(), Some("WEM_FACILITY_SCADA"));
        assert_eq!(r.period.unwrap().to_string(), "2023-10");
        let dir = tempfile::tempdir().unwrap();
        let q = Query {
            tables: Vec::new(),
            exclude: Vec::new(),
            range: TimeRange::months("2023-10".parse().unwrap(), "2023-10".parse().unwrap()),
            instants: None,
        };
        assert!(
            named
                .archived(&Archive::new(dir.path()), &q)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_fixture_manifest() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/wem/facility-scada-head.yaml"
        ))
        .unwrap();
        let entries = parse_manifest("m", &text).unwrap();
        assert!(entries.len() >= 4);
        assert_eq!(entries[0].year, 2023);
        assert_eq!(entries[0].month, 10);
        assert!(entries[0].url.ends_with("facility-scada-2023-10.csv"));
        assert_eq!(
            entries[0].generated_at.as_deref(),
            Some("Thu Nov 02 2023 11:35:00Z"),
            "as AEMO wrote it"
        );
        assert_eq!(
            entries[0].generated_at_utc.map(|s| s.rfc3339()).as_deref(),
            Some("2023-11-02T11:35:00Z"),
            "and as an instant"
        );
        assert!(
            entries
                .iter()
                .all(|e| e.generated_at.is_none() == e.generated_at_utc.is_none())
        );
        let odd = parse_manifest(
            "m",
            "- {url: u, year: 2023, month: 1, generatedAt: whenever}\n- {url: v, year: 2023, month: 2, generatedAt: 2023-02-01 09:00:00}\n",
        )
        .unwrap();
        assert_eq!(odd[0].generated_at.as_deref(), Some("whenever"));
        assert_eq!(odd[0].generated_at_utc, None, "not a time: no instant");
        assert_eq!(
            odd[1].generated_at_utc.map(|s| s.rfc3339()).as_deref(),
            Some("2023-02-01T01:00:00Z"),
            "no offset: WEM time, given in UTC"
        );
        assert!(parse_manifest("m", "- nope: 1").is_err());
        assert!(WemDatafiles::new("::", vec![]).is_err());
    }

    #[tokio::test]
    async fn lists_in_range() {
        let dir = tempfile::tempdir().unwrap();
        let text = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/wem/facility-scada-head.yaml"
        ))
        .unwrap();
        let w = WemDatafiles::new("https://data.wa.aemo.com.au", vec!["facility-scada".into()])
            .unwrap();
        let murl = w.manifest_url("facility-scada").unwrap();
        assert_eq!(
            murl.as_str(),
            "https://data.wa.aemo.com.au/public/public-data/manifests/facility-scada.yaml"
        );
        let t = Arc::new(StaticTransport::default());
        t.add(murl.as_str(), Canned::ok(&text));
        let f = Fetcher::new(
            Arc::new(t),
            Archive::new(dir.path()),
            FetcherConfig {
                attempts: 1,
                min_interval: Duration::ZERO,
                ..Default::default()
            },
        );
        let q = Query {
            exclude: Vec::new(),
            tables: vec![],
            range: TimeRange::months(
                YearMonth::new(2023, 8).unwrap(),
                YearMonth::new(2023, 9).unwrap(),
            ),
            instants: None,
        };
        let files = w.list(&q, &f).await.unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0].period,
            Some(YearMonth::new(2023, 8).unwrap().into())
        );
        assert_eq!(files[0].table.as_deref(), Some("facility-scada"));
        assert!(files[1].url.as_str().ends_with("2023-09.csv"));
    }
}
