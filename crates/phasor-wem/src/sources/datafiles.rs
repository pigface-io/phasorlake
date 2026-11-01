//! WEM public data: one CSV per dataset per period, listed by a YAML manifest.
//!
//! ```yaml
//! - url: "https://data.wa.aemo.com.au/datafiles/facility-scada/facility-scada-2023-10.csv"
//!   year: 2023
//!   month: 10
//!   generatedAt: "Thu Nov 02 2023 11:35:00Z"
//! ```
//!
//! A dataset's files are a month's (`facility-scada-2023-10.csv`), a year's
//! (`MarketClearingPrices-2026.csv`: `year` alone, the file written again
//! whole as the year grows) or the dataset's one standing file
//! (`facilities.csv`: neither). A month's file is of its month; a year's or a
//! standing file is of the month it was generated in, its release, and its
//! rows say their own times. AEMO writes each again at the same URL, so a
//! file is asked for by its generation: the `generatedAt` is the URL's
//! fragment, and a generation not yet archived is downloaded and kept beside
//! the earlier ones, each with its own Parquet files. The manifest's URLs sit
//! under `/datafiles/`, a second address of the
//! folders the feeds table names (`public/public-data/datafiles`); a file is
//! fetched from the folder the census reads, so the two agree on every file.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use phasor_acquire::FeedFolders;
use phasor_core::calendar::{WEM_OFFSET_SECS, local_year_month};
use phasor_core::timetext::{self, Stamp};
use phasor_core::{Period, TimeRange, YearMonth};
use phasor_kit::{Serves, TableOf};
use phasor_plain_decode::feeds::feed;
use phasor_plain_decode::names::version_of;
use serde::Deserialize;
use url::Url;

use phasor_acquire::{Archive, FetchError};
use phasor_kit::{Listings, LocalFile, Query, RemoteFile, Source};

/// The source id.
pub const SOURCE_ID: &str = "wem-datafiles";

/// The dataset folders whose files a table is read from: for `None`, every
/// dataset the kit knows of; for a table, the folders the census saw its
/// blocks in.
pub type DatasetsOf = Arc<dyn Fn(Option<&str>) -> Vec<String> + Send + Sync>;

/// What a datafile covers, by its manifest entry or its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Covers {
    /// One month.
    Month(YearMonth),
    /// One year, the file written again whole as the year grows.
    Year(i32),
    /// The dataset's one standing file: the table as of its generation.
    Standing,
}

/// One manifest entry.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    /// The CSV.
    pub url: String,
    /// Year; none for a dataset's standing file.
    #[serde(default, deserialize_with = "opt_int_or_str")]
    pub year: Option<i32>,
    /// Month; none for a year's file or a standing file. AEMO writes `09`, which
    /// YAML 1.2 reads as a string, so both forms are accepted.
    #[serde(default, deserialize_with = "opt_int_or_str")]
    pub month: Option<u8>,
    /// When AEMO generated it, as written: `Thu Nov 02 2023 11:35:00Z`.
    #[serde(default)]
    pub generated_at: Option<String>,
    /// `generated_at` in UTC, set by [`parse_manifest`]; text without an offset is read
    /// in WEM time. `None` when not a time.
    #[serde(default, skip)]
    pub generated_at_utc: Option<Stamp>,
}

impl ManifestEntry {
    /// What the entry's file covers; `None` for a month without a year.
    pub fn covers(&self) -> Option<Covers> {
        match (self.year, self.month) {
            (Some(y), Some(m)) => YearMonth::new(y, m).map(Covers::Month),
            (Some(y), None) => Some(Covers::Year(y)),
            (None, None) => Some(Covers::Standing),
            (None, Some(_)) => None,
        }
    }

    /// The month the file was generated in, in WEM time.
    pub fn generated_month(&self) -> Option<YearMonth> {
        self.generated_at_utc.and_then(wem_month)
    }
}

fn opt_int_or_str<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
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
    match Option::<Raw>::deserialize(d)? {
        None => Ok(None),
        Some(Raw::Int(i)) => T::try_from(i)
            .map(Some)
            .map_err(|_| serde::de::Error::custom(format!("{i} is out of range"))),
        Some(Raw::Str(s)) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| serde::de::Error::custom(format!("{s:?} is not a number"))),
    }
}

/// Parses a manifest's YAML.
pub fn parse_manifest(url: &str, text: &str) -> Result<Vec<ManifestEntry>, FetchError> {
    let mut entries: Vec<ManifestEntry> =
        serde_yaml_ng::from_str(text).map_err(FetchError::parse(url, "WEM manifest YAML"))?;
    for e in &mut entries {
        e.generated_at_utc = e
            .generated_at
            .as_deref()
            .and_then(|s| timetext::parse(s, WEM_OFFSET_SECS))
            .map(|s| Stamp::new(s.secs_utc, 0));
    }
    Ok(entries)
}

/// The month of an instant in WEM time.
fn wem_month(at: Stamp) -> Option<YearMonth> {
    let (year, month) =
        local_year_month(at.secs_utc.saturating_mul(1_000_000_000), WEM_OFFSET_SECS);
    YearMonth::new(year, u8::try_from(month).ok()?)
}

/// A generation as a URL fragment: its instant, compact (`20261101T002218Z`).
fn generation_tag(at: Stamp) -> String {
    at.rfc3339().replace([':', '-'], "")
}

/// Whether a file covering `covers` (a standing file generated in
/// `generated`) is asked for by `range`: a month's file by its month, a
/// year's when any month of the year is, and a standing file from the month
/// it was generated in on, since it stands until AEMO writes it again.
fn in_range(covers: Covers, generated: Option<YearMonth>, range: &TimeRange) -> bool {
    match covers {
        Covers::Month(m) => range.contains(m),
        Covers::Year(y) => range.first_month().year() <= y && y <= range.last_month().year(),
        Covers::Standing => generated.is_none_or(|m| m <= range.last_month()),
    }
}

/// WEM datasets.
pub struct WemDatafiles {
    base: Url,
    /// Dataset names to list, e.g. `facility-scada`, beside those of the tables asked for.
    pub datasets: Vec<String>,
    /// The feed's folders, which name a file's table and find the files in the archive.
    folders: FeedFolders,
    /// The dataset folders of each table.
    datasets_of: DatasetsOf,
    /// Whether a table is one this source publishes.
    serves: Serves,
}

impl WemDatafiles {
    /// Over `base` (normally `https://data.wa.aemo.com.au`); no table claims a file until [`Self::with_tables`].
    pub fn new(base: &str, datasets: Vec<String>) -> Result<Self, FetchError> {
        let url = Url::parse(base).map_err(FetchError::parse(base, "base URL"))?;
        let feed = feed("datafiles").expect("the feeds table names the WEM datafiles");
        let folders = FeedFolders::new(SOURCE_ID, base, feed, Arc::new(|_| None))?;
        Ok(Self {
            base: url,
            datasets,
            folders,
            datasets_of: Arc::new(|_| Vec::new()),
            serves: Arc::new(|_| true),
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

    /// The same source, listing the datasets `datasets_of` names for the tables asked for.
    #[must_use]
    pub fn with_datasets_of(mut self, datasets_of: DatasetsOf) -> Self {
        self.datasets_of = datasets_of;
        self
    }

    /// The same source, serving only the tables `serves` says are the feed's.
    #[must_use]
    pub fn with_tables_served(mut self, serves: Serves) -> Self {
        self.serves = serves;
        self
    }

    /// The manifest URL of a dataset.
    pub fn manifest_url(&self, dataset: &str) -> Result<Url, FetchError> {
        self.base
            .join(&format!("/public/public-data/manifests/{dataset}.yaml"))
            .map_err(FetchError::parse(dataset, "manifest URL"))
    }

    /// The datasets a query lists: the configured ones, and those of the tables it
    /// names (every dataset the tables are known in, when it names none).
    fn datasets_for(&self, query: &Query) -> Vec<String> {
        let mut out: BTreeSet<String> = self.datasets.iter().cloned().collect();
        if query.tables.is_empty() {
            out.extend((self.datasets_of)(None));
        } else {
            for t in &query.tables {
                out.extend((self.datasets_of)(Some(t)));
            }
        }
        out.into_iter().collect()
    }

    /// A manifest entry's file at the address the feeds table names,
    /// `{base}/public/public-data/datafiles/{dataset}/{file}`, its generation the fragment.
    fn address(&self, dataset: &str, e: &ManifestEntry) -> Result<Url, FetchError> {
        let parse = || FetchError::parse(&e.url, "manifest entry URL");
        let given = Url::parse(&e.url).map_err(parse())?;
        let name = given
            .path_segments()
            .and_then(|mut s| s.next_back())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| parse()(url::ParseError::RelativeUrlWithoutBase))?;
        let root = self.folders.feed().roots[0].trim_matches('/');
        let mut url = self
            .base
            .join(&format!("/{root}/{dataset}/{name}"))
            .map_err(parse())?;
        if let Some(at) = e.generated_at_utc {
            url.set_fragment(Some(&generation_tag(at)));
        }
        Ok(url)
    }

    /// Whether a query asks for `table`.
    fn wanted(table: &str, query: &Query) -> bool {
        if query.exclude.iter().any(|x| x.eq_ignore_ascii_case(table)) {
            return false;
        }
        query.tables.is_empty() || query.tables.iter().any(|t| t.eq_ignore_ascii_case(table))
    }
}

/// What a datafile's name says: `{dataset}-YYYY-MM.csv` covers a month,
/// `{dataset}-YYYY.csv` a year, and any other `{dataset}.csv` is the
/// dataset's standing file; a mirror version's `@stamp` is not part of the
/// name. `None` for a name that is not a `.csv`, has no dataset, or names a
/// month that is not one.
pub fn name_covers(url: &Url) -> Option<(String, Covers)> {
    let name = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())?;
    let (name, _) = version_of(name);
    let stem = name.strip_suffix(".csv")?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (dataset, covers) = match stem.rsplit_once('-') {
        Some((dataset, year)) if year.len() == 4 && digits(year) => {
            (dataset, Covers::Year(year.parse().ok()?))
        }
        Some((head, month)) if month.len() == 2 && digits(month) => match head.rsplit_once('-') {
            Some((dataset, year)) if year.len() == 4 && digits(year) => {
                let ym = YearMonth::new(year.parse().ok()?, month.parse().ok()?)?;
                (dataset, Covers::Month(ym))
            }
            _ => (stem, Covers::Standing),
        },
        _ => (stem, Covers::Standing),
    };
    (!dataset.is_empty()).then(|| (dataset.to_owned(), covers))
}

/// A datafile by its name alone ([`name_covers`]): its dataset as the table, a
/// month's file with its month.
pub fn describe_name(url: Url) -> Option<RemoteFile> {
    let (dataset, covers) = name_covers(&url)?;
    Some(RemoteFile {
        url,
        source: SOURCE_ID,
        table: Some(dataset),
        period: match covers {
            Covers::Month(m) => Some(Period::Month(m)),
            Covers::Year(_) | Covers::Standing => None,
        },
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

    fn serves(&self, table: &str) -> bool {
        (self.serves)(table)
    }

    /// A datafile by its name, with its table when the URL lies under the feed's
    /// folders; a standing file (no period in its name) only there, since any
    /// `.csv` could be named so.
    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        let (_, covers) = name_covers(url)?;
        let under = self.folders.describe(url);
        if covers == Covers::Standing && under.is_none() {
            return None;
        }
        let mut r = describe_name(url.clone())?;
        if let Some(f) = under {
            r.table = f.table.or(r.table);
            r.period = r.period.or(f.period);
        }
        Some(r)
    }

    /// Every archived version of each datafile, in order, each naming the one before it.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        self.folders.archived(archive, query)
    }

    async fn list(
        &self,
        query: &Query,
        fetcher: &dyn Listings,
    ) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        for ds in self.datasets_for(query) {
            let murl = self.manifest_url(&ds)?;
            let body = fetcher.fetch_listing(murl.as_str()).await?;
            let text = String::from_utf8_lossy(&body);
            for e in parse_manifest(murl.as_str(), &text)? {
                let Some(covers) = e.covers() else {
                    continue;
                };
                let generated = e.generated_month();
                if !in_range(covers, generated, &query.range) {
                    continue;
                }
                let url = self.address(&ds, &e)?;
                let table = self
                    .folders
                    .describe(&url)
                    .and_then(|f| f.table)
                    .unwrap_or_else(|| ds.clone());
                if !Self::wanted(&table, query) {
                    continue;
                }
                let period = match covers {
                    Covers::Month(m) => Some(m),
                    Covers::Year(_) | Covers::Standing => generated,
                };
                out.push(RemoteFile {
                    url,
                    source: SOURCE_ID,
                    table: Some(table),
                    period: period.map(Period::Month),
                    size: None,
                    modified: e.generated_at,
                    modified_at: e.generated_at_utc,
                });
            }
        }
        out.sort_by(|a, b| {
            (a.period, &a.table, a.url.as_str()).cmp(&(b.period, &b.table, b.url.as_str()))
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use phasor_acquire::Fetcher;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use phasor_acquire::transport::{Canned, StaticTransport};
    use phasor_acquire::{Archive, FetcherConfig};
    use phasor_core::TimeRange;

    const ROOT: &str = "https://data.wa.aemo.com.au/public/public-data/datafiles";

    fn months(a: &str, b: &str) -> TimeRange {
        TimeRange::months(a.parse().unwrap(), b.parse().unwrap())
    }

    fn query(tables: &[&str], range: TimeRange) -> Query {
        Query {
            tables: tables.iter().map(|t| (*t).to_owned()).collect(),
            exclude: Vec::new(),
            range,
            instants: None,
        }
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn a_datafile_name_says_its_dataset_and_what_it_covers() {
        let covers = |u: &str| name_covers(&url(u));
        assert_eq!(
            covers("https://h/datafiles/facility-scada/facility-scada-2023-10.csv"),
            Some((
                "facility-scada".into(),
                Covers::Month(YearMonth::new(2023, 10).unwrap())
            ))
        );
        assert_eq!(
            covers(&format!(
                "{ROOT}/balancing-summary/balancing-summary-2022.csv"
            )),
            Some(("balancing-summary".into(), Covers::Year(2022)))
        );
        assert_eq!(
            covers(&format!("{ROOT}/facilities/facilities.csv")),
            Some(("facilities".into(), Covers::Standing))
        );
        // A tail that is not a month: the whole stem is the dataset.
        assert_eq!(
            covers("https://h/x-23-10.csv"),
            Some(("x-23-10".into(), Covers::Standing))
        );
        // A mirror's version of a file: the name before the `@`.
        assert_eq!(
            covers("https://h/stem-summary/stem-summary-2026.csv@20261021T083000+0800"),
            Some(("stem-summary".into(), Covers::Year(2026)))
        );
        for bad in [
            "https://h/facility-scada-2023-13.csv",
            "https://h/-2023-10.csv",
            "https://h/-2023.csv",
            "https://h/.csv",
            "https://h/facility-scada-2023-10.zip",
            "https://h/",
        ] {
            assert!(covers(bad).is_none(), "{bad}");
        }
        let r = describe_name(url(
            "https://data.wa.aemo.com.au/datafiles/facility-scada/facility-scada-2023-10.csv",
        ))
        .unwrap();
        assert_eq!(r.table.as_deref(), Some("facility-scada"));
        assert_eq!(r.period.unwrap().to_string(), "2023-10");
        assert_eq!(r.source, SOURCE_ID);
        let yearly = describe_name(url(&format!(
            "{ROOT}/balancing-summary/balancing-summary-2022.csv"
        )))
        .unwrap();
        assert_eq!(yearly.table.as_deref(), Some("balancing-summary"));
        assert_eq!(yearly.period, None);
        assert!(describe_name(url("https://h/.csv")).is_none());
    }

    #[test]
    fn a_source_claims_dated_files_anywhere_and_standing_files_under_its_folders() {
        let w = WemDatafiles::new("https://h", Vec::new()).unwrap();
        assert_eq!(w.id(), SOURCE_ID);
        assert!(w.serves("ANYTHING"));
        assert!(
            w.describe(&url("https://h/facility-scada-2023-10.csv"))
                .is_some()
        );
        // A standing file is a datafile only in a dataset folder: any `.csv` could be named so.
        assert!(w.describe(&url("https://h/facilities.csv")).is_none());
        let standing = w
            .describe(&url(&format!("{ROOT}/facilities/facilities.csv")))
            .unwrap();
        assert_eq!(standing.table.as_deref(), Some("facilities"));
        assert_eq!(standing.period, None, "nothing in the URL says when");
        // Under the feed's folders, the registry's table replaces the
        // dataset once tables are named; the archive is read there too.
        let named = w
            .with_tables(Arc::new(|b| Some(format!("WEM_{}", b.name))))
            .unwrap()
            .with_tables_served(Arc::new(|t| t.eq_ignore_ascii_case("WEM_FACILITIES")));
        assert!(named.serves("wem_facilities") && !named.serves("WEM_FACILITY_SCADA"));
        let r = named
            .describe(&url(&format!(
                "{ROOT}/facility-scada/facility-scada-2023-10.csv"
            )))
            .unwrap();
        assert_eq!(r.table.as_deref(), Some("WEM_FACILITY_SCADA"));
        assert_eq!(r.period.unwrap().to_string(), "2023-10");
        let standing = named
            .describe(&url(&format!("{ROOT}/facilities/facilities.csv")))
            .unwrap();
        assert_eq!(standing.table.as_deref(), Some("WEM_FACILITIES"));
        // A mirror's version of a year's file: the month it was listed in.
        let version = named
            .describe(&url(&format!(
                "{ROOT}/market-clearing-prices-csv/MarketClearingPrices-2026.csv@20261021T083000+0800"
            )))
            .unwrap();
        assert_eq!(
            version.table.as_deref(),
            Some("WEM_MARKET_CLEARING_PRICES_CSV")
        );
        assert_eq!(version.period.unwrap().to_string(), "2026-10");
        let dir = tempfile::tempdir().unwrap();
        assert!(
            named
                .archived(
                    &Archive::new(dir.path()),
                    &query(&[], months("2023-10", "2023-10"))
                )
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
        assert_eq!(entries[0].year, Some(2023));
        assert_eq!(entries[0].month, Some(10));
        assert_eq!(
            entries[0].covers(),
            Some(Covers::Month(YearMonth::new(2023, 10).unwrap()))
        );
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
        assert_eq!(
            entries[0].generated_month().unwrap().to_string(),
            "2023-11",
            "in WEM time"
        );
        assert!(
            entries
                .iter()
                .all(|e| e.generated_at.is_none() == e.generated_at_utc.is_none())
        );
        let odd = parse_manifest(
            "m",
            "- {url: u, year: 2023, month: 1, generatedAt: whenever}\n\
             - {url: v, year: 2023, month: 2, generatedAt: 2023-02-01 09:00:00}\n\
             - {url: w, year: '2026', generatedAt: 2026-06-26 08:30:12}\n\
             - {url: x, generatedAt: 2023-09-28 06:30:00}\n\
             - {url: y, month: 3}\n\
             - {url: z, year: 2023, month: 13}\n\
             - {url: n, year: null, month: ~}\n",
        )
        .unwrap();
        assert_eq!(odd[0].generated_at.as_deref(), Some("whenever"));
        assert_eq!(odd[0].generated_at_utc, None, "not a time: no instant");
        assert_eq!(odd[0].generated_month(), None);
        assert_eq!(
            odd[1].generated_at_utc.map(|s| s.rfc3339()).as_deref(),
            Some("2023-02-01T01:00:00Z"),
            "no offset: WEM time, given in UTC"
        );
        assert_eq!(odd[2].covers(), Some(Covers::Year(2026)), "a year's file");
        assert_eq!(odd[3].covers(), Some(Covers::Standing), "a standing file");
        assert_eq!(odd[4].covers(), None, "a month without a year");
        assert_eq!(odd[5].covers(), None, "a month that is not one");
        assert_eq!(odd[6].covers(), Some(Covers::Standing), "null is none");
        assert!(parse_manifest("m", "- nope: 1").is_err());
        assert!(parse_manifest("m", "- {url: u, year: x}").is_err());
        assert!(parse_manifest("m", "- {url: u, year: 2023, month: 300}").is_err());
        assert!(WemDatafiles::new("::", vec![]).is_err());
    }

    #[test]
    fn what_a_range_asks_for() {
        let range = months("2026-08", "2026-10");
        let ym = |s: &str| s.parse::<YearMonth>().unwrap();
        assert!(in_range(Covers::Month(ym("2026-09")), None, &range));
        assert!(!in_range(Covers::Month(ym("2026-11")), None, &range));
        assert!(in_range(Covers::Year(2026), None, &range));
        assert!(!in_range(Covers::Year(2025), None, &range));
        let turn = months("2025-12", "2026-01");
        assert!(in_range(Covers::Year(2026), None, &turn));
        assert!(in_range(Covers::Year(2025), None, &turn));
        // A standing file stands from its generation on.
        assert!(in_range(Covers::Standing, Some(ym("2023-09")), &range));
        assert!(in_range(Covers::Standing, Some(ym("2026-10")), &range));
        assert!(!in_range(Covers::Standing, Some(ym("2026-11")), &range));
        assert!(in_range(Covers::Standing, None, &range));
        let at = Stamp::new(1_762_000_000, 0);
        assert_eq!(generation_tag(at), at.rfc3339().replace([':', '-'], ""));
        assert!(!generation_tag(at).contains(['-', ':']));
    }

    fn manifest(dataset: &str, entries: &str) -> (String, Vec<u8>) {
        (
            format!("https://data.wa.aemo.com.au/public/public-data/manifests/{dataset}.yaml"),
            format!(
                "# {dataset} manifest\n# using base URL: https://data.wa.aemo.com.au/datafiles\n\n{entries}"
            )
            .into_bytes(),
        )
    }

    fn fetcher(t: &Arc<StaticTransport>, dir: &std::path::Path) -> Fetcher {
        Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(dir),
            FetcherConfig {
                attempts: 1,
                min_interval: Duration::ZERO,
                ..Default::default()
            },
        )
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
        let f = fetcher(&t, dir.path());
        let files = w
            .list(&query(&[], months("2023-08", "2023-09")), &f)
            .await
            .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0].period,
            Some(YearMonth::new(2023, 8).unwrap().into())
        );
        assert_eq!(files[0].table.as_deref(), Some("facility-scada"));
        // Fetched from the folder the census reads, by its generation.
        assert_eq!(
            files[1].url.as_str(),
            format!(
                "{ROOT}/facility-scada/facility-scada-2023-09.csv#{}",
                generation_tag(files[1].modified_at.unwrap())
            )
        );
        assert_eq!(
            files[1].modified.as_deref(),
            Some("Mon Oct 02 2023 11:35:07Z")
        );
        // A table the query leaves out.
        let mut q = query(&[], months("2023-08", "2023-09"));
        q.exclude.push("FACILITY-SCADA".into());
        assert!(w.list(&q, &f).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn lists_the_datasets_of_the_tables_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let prices = "- url: \"https://data.wa.aemo.com.au/datafiles/market-clearing-prices-csv/MarketClearingPrices-2026.csv\"\n  year: 2026\n  generatedAt: \"Fri Jun 26 2026 08:30:12Z\"\n\n\
                      - url: \"https://data.wa.aemo.com.au/datafiles/market-clearing-prices-csv/MarketClearingPrices-2025.csv\"\n  year: 2025\n  generatedAt: \"Fri Jun 26 2026 08:30:12Z\"\n";
        let facilities = "- url: \"https://data.wa.aemo.com.au/datafiles/facilities/facilities.csv\"\n  generatedAt: \"Thu Sep 28 2023 06:30:00Z\"\n";
        let stem = "- url: \"https://data.wa.aemo.com.au/datafiles/stem-bids-and-offers/stem-bids-and-offers-2026-10.csv\"\n  year: 2026\n  month: 10\n  generatedAt: \"Sat Oct 31 2026 11:30:54Z\"\n\n\
                    - url: \"https://data.wa.aemo.com.au/datafiles/stem-bids-and-offers/stem-bids-and-offers-2026-07.csv\"\n  year: 2026\n  month: 07\n\n\
                    - url: \"https://data.wa.aemo.com.au/datafiles/stem-bids-and-offers/odd.csv\"\n  month: 7\n";
        let scada = "- url: \"https://data.wa.aemo.com.au/datafiles/facility-scada/facility-scada-2026-09.csv\"\n  year: 2026\n  month: 9\n";
        for (dataset, entries) in [
            ("market-clearing-prices-csv", prices),
            ("facilities", facilities),
            ("stem-bids-and-offers", stem),
            ("facility-scada", scada),
        ] {
            let (u, body) = manifest(dataset, entries);
            t.add(&u, Canned::ok(&body));
        }
        let f = fetcher(&t, dir.path());
        let table_of: TableOf = Arc::new(|b| match b.name.as_str() {
            "MARKET_CLEARING_PRICES_CSV" => Some("WEM_MARKET_CLEARING_PRICES".to_owned()),
            "FACILITIES" => Some("WEM_FACILITIES".to_owned()),
            "STEM_BIDS_AND_OFFERS" => Some("WEM_STEM_BIDS_AND_OFFERS".to_owned()),
            _ => None,
        });
        let every = || {
            vec![
                "market-clearing-prices-csv".to_owned(),
                "facilities".to_owned(),
                "stem-bids-and-offers".to_owned(),
            ]
        };
        let datasets_of: DatasetsOf = Arc::new(move |t| match t {
            None => every(),
            Some(t) if t.eq_ignore_ascii_case("WEM_MARKET_CLEARING_PRICES") => {
                vec!["market-clearing-prices-csv".into()]
            }
            Some(t) if t.eq_ignore_ascii_case("WEM_FACILITIES") => vec!["facilities".into()],
            Some(t) if t.eq_ignore_ascii_case("WEM_STEM_BIDS_AND_OFFERS") => {
                vec!["stem-bids-and-offers".into()]
            }
            Some(_) => Vec::new(),
        });
        let w = WemDatafiles::new("https://data.wa.aemo.com.au", vec!["facility-scada".into()])
            .unwrap()
            .with_tables(table_of)
            .unwrap()
            .with_datasets_of(datasets_of);
        // One table: its dataset's manifest, the year's file whole.
        let files = w
            .list(
                &query(
                    &["wem_market_clearing_prices"],
                    months("2026-08", "2026-10"),
                ),
                &f,
            )
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].url.as_str(),
            format!(
                "{ROOT}/market-clearing-prices-csv/MarketClearingPrices-2026.csv#20260626T083012Z"
            )
        );
        assert_eq!(
            files[0].table.as_deref(),
            Some("WEM_MARKET_CLEARING_PRICES")
        );
        assert_eq!(
            files[0].period.unwrap().to_string(),
            "2026-06",
            "a year's file: the month it was generated in; its rows say their own"
        );
        assert_eq!(
            files[0].modified_at.unwrap().rfc3339(),
            "2026-06-26T08:30:12Z"
        );
        assert_eq!(
            t.request_count(),
            2,
            "its manifest and the configured dataset's"
        );
        // A standing file generated before the range stands through it, with the month it
        // was generated in as its snapshot; one generated after the range does not.
        let files = w
            .list(
                &query(&["WEM_FACILITIES"], months("2026-08", "2026-10")),
                &f,
            )
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].url.as_str(),
            format!("{ROOT}/facilities/facilities.csv#20230928T063000Z")
        );
        assert_eq!(files[0].period.unwrap().to_string(), "2023-09");
        assert!(
            w.list(
                &query(&["WEM_FACILITIES"], months("2023-01", "2023-08")),
                &f
            )
            .await
            .unwrap()
            .is_empty()
        );
        // No table: every dataset known, by month, table and URL; an entry with a month and
        // no year is passed over, and one without a generation has no fragment.
        let files = w
            .list(&query(&[], months("2026-07", "2026-10")), &f)
            .await
            .unwrap();
        let urls: Vec<&str> = files.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                format!("{ROOT}/facilities/facilities.csv#20230928T063000Z"),
                format!(
                    "{ROOT}/market-clearing-prices-csv/MarketClearingPrices-2026.csv#20260626T083012Z"
                ),
                format!("{ROOT}/stem-bids-and-offers/stem-bids-and-offers-2026-07.csv"),
                format!("{ROOT}/facility-scada/facility-scada-2026-09.csv"),
                format!(
                    "{ROOT}/stem-bids-and-offers/stem-bids-and-offers-2026-10.csv#20261031T113054Z"
                ),
            ]
        );
        assert_eq!(
            files[3].table.as_deref(),
            Some("facility-scada"),
            "no table claims it: its dataset"
        );
        // A table whose dataset has no entry in range lists nothing.
        assert!(
            w.list(
                &query(&["WEM_STEM_BIDS_AND_OFFERS"], months("2025-01", "2025-02")),
                &f
            )
            .await
            .unwrap()
            .is_empty()
        );
        // A manifest whose entry URL is not one, or has no file name, is refused.
        for bad in [
            "- url: \"not a url\"\n",
            "- url: \"https://data.wa.aemo.com.au\"\n",
        ] {
            let t2 = Arc::new(StaticTransport::default());
            let (u, body) = manifest("facilities", bad);
            t2.add(&u, Canned::ok(&body));
            let r = w
                .list(
                    &query(&["WEM_FACILITIES"], months("2026-08", "2026-10")),
                    &fetcher(&t2, dir.path()),
                )
                .await;
            assert!(
                matches!(
                    r,
                    Err(FetchError::Parse {
                        what: "manifest entry URL",
                        ..
                    })
                ),
                "{bad}"
            );
        }
    }
}
