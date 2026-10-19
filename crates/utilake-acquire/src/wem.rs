//! WEM public data: one CSV per dataset per month, listed by a YAML manifest.
//!
//! ```yaml
//! - url: "https://data.wa.aemo.com.au/datafiles/facility-scada/facility-scada-2023-10.csv"
//!   year: 2023
//!   month: 10
//!   generatedAt: "Thu Nov 02 2023 11:35:00Z"
//! ```
//!
//! Only manifests are read today; downloading and parsing the CSVs needs
//! the plain-CSV decoder, which is not written yet.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;
use utilake_core::YearMonth;
use utilake_core::calendar::WEM_OFFSET_SECS;
use utilake_core::timetext::{self, Stamp};

use crate::error::FetchError;
use crate::fetcher::Fetcher;
use crate::source::{Query, RemoteFile, Source};

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
#[derive(Debug, Clone)]
pub struct WemDatafiles {
    base: Url,
    /// Dataset names to list, e.g. `facility-scada`.
    pub datasets: Vec<String>,
}

impl WemDatafiles {
    /// Over `base`, normally `https://data.wa.aemo.com.au`.
    pub fn new(base: &str, datasets: Vec<String>) -> Result<Self, FetchError> {
        let base = Url::parse(base).map_err(|e| FetchError::Parse {
            url: base.to_owned(),
            what: "base URL",
            message: e.to_string(),
        })?;
        Ok(Self { base, datasets })
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

#[async_trait]
impl Source for WemDatafiles {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    async fn list(&self, query: &Query, fetcher: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
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

    use super::*;
    use crate::archive::Archive;
    use crate::fetcher::FetcherConfig;
    use crate::transport::{Canned, StaticTransport};
    use utilake_core::TimeRange;

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
