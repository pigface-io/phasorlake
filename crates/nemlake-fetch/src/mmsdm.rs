//! The monthly NEM archive on nemweb.
//!
//! ```text
//! {base}/Data_Archive/Wholesale_Electricity/MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}/MMSDM_Historical_Data_SQLLoader/DATA/
//!     PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip     one per table; big tables split: BIDPEROFFER1, BIDPEROFFER2
//! ```
//!
//! Directories are listed rather than URLs guessed, because split tables
//! and renamed files only show up in the listing. From 2025 AEMO also
//! publishes one zip per month beside the directory; the directory is used.

use async_trait::async_trait;
use nemlake_core::YearMonth;
use url::Url;

use crate::error::FetchError;
use crate::fetcher::Fetcher;
use crate::listing;
use crate::source::{FileKind, Query, RemoteFile, Source};

/// The source id.
pub const SOURCE_ID: &str = "nem-mmsdm";

/// Listings of months at least this old are cached for a week; the current
/// and previous month for an hour.
const OLD_LISTING_TTL: i64 = 7 * 86_400;
const RECENT_LISTING_TTL: i64 = 3_600;

/// The monthly archive.
#[derive(Debug, Clone)]
pub struct MmsdmArchive {
    base: Url,
}

impl MmsdmArchive {
    /// Over `base`, normally `https://www.nemweb.com.au`.
    pub fn new(base: &str) -> Result<Self, FetchError> {
        let base = Url::parse(base).map_err(|e| FetchError::Parse {
            url: base.to_owned(),
            what: "base URL",
            message: e.to_string(),
        })?;
        Ok(Self { base })
    }

    /// The month directory, e.g. `.../MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/`.
    pub fn month_root(&self, ym: YearMonth) -> Result<Url, FetchError> {
        if ym.year() < 2024 {
            // Earlier years are published as one zip per month (2009..2010) or
            // in layouts not yet sampled; a per-era resolver arrives in M1.
            return Err(FetchError::EraUnsupported {
                period: ym.to_string(),
            });
        }
        let path = format!(
            "/Data_Archive/Wholesale_Electricity/MMSDM/{}/MMSDM_{}_{:02}/MMSDM_Historical_Data_SQLLoader/",
            ym.year(),
            ym.year(),
            ym.month()
        );
        self.base.join(&path).map_err(|e| FetchError::Parse {
            url: path,
            what: "month URL",
            message: e.to_string(),
        })
    }

    /// The directories to list for a month.
    pub fn data_dirs(&self, ym: YearMonth) -> Result<Vec<Url>, FetchError> {
        let root = self.month_root(ym)?;
        Ok(["DATA/", "P5MIN_ALL_DATA/", "PREDISP_ALL_DATA/"]
            .iter()
            .filter_map(|d| root.join(d).ok())
            .collect())
    }

    /// `PUBLIC_DVD_{TABLE}{split?}_{YYYYMM}010000.zip` → `(TABLE, split)`; the
    /// `_ALL` suffix of the big-table directories is dropped.
    pub fn parse_name(name: &str) -> Option<(String, Option<u8>)> {
        let stem = name
            .strip_suffix(".zip")
            .or_else(|| name.strip_suffix(".ZIP"))?;
        let rest = stem.strip_prefix("PUBLIC_DVD_")?;
        let (table, suffix) = rest.rsplit_once('_')?;
        if suffix.len() != 12 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let table = table.strip_suffix("_ALL").unwrap_or(table);
        let digits: String = table
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() || digits.len() == table.len() {
            Some((table.to_owned(), None))
        } else {
            let base = &table[..table.len() - digits.len()];
            Some((
                base.to_owned(),
                digits.chars().rev().collect::<String>().parse().ok(),
            ))
        }
    }

    fn listing_ttl(ym: YearMonth) -> i64 {
        let now = nemlake_core::clock::unix_now();
        let (y, m) = nemlake_core::nemtime::local_year_month(now * 1_000_000, 0);
        let current = YearMonth::new(y, m as u8).expect("civil month is 1..=12");
        if current.ordinal() - ym.ordinal() >= 2 {
            OLD_LISTING_TTL
        } else {
            RECENT_LISTING_TTL
        }
    }
}

/// Whether a listed table name matches a requested one: equal, or the
/// requested name plus a split digit (`BIDPEROFFER1` for `BIDPEROFFER`).
fn matches_table(listed_table: &str, wanted: &str) -> bool {
    listed_table.eq_ignore_ascii_case(wanted)
}

#[async_trait]
impl Source for MmsdmArchive {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    async fn list(&self, query: &Query, fetcher: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        let wanted: Vec<String> = query
            .tables
            .iter()
            .map(|t| t.to_ascii_uppercase())
            .collect();
        for ym in query.from.range_inclusive(query.to) {
            let dirs = self.data_dirs(ym)?;
            let ttl = Self::listing_ttl(ym);
            let mut seen_tables: Vec<String> = Vec::new();
            let mut all_names: Vec<String> = Vec::new();
            let mut first_url = String::new();
            for (i, dir) in dirs.iter().enumerate() {
                let body = match fetcher.fetch_listing(dir.as_str(), ttl).await {
                    Ok(b) => b,
                    // The big-table directories do not exist for every month.
                    Err(FetchError::HttpStatus { status: 404, .. }) if i > 0 => continue,
                    Err(e) => return Err(e),
                };
                if i == 0 {
                    first_url = dir.to_string();
                }
                let entries = listing::parse(&body);
                if entries.is_empty() && i == 0 {
                    return Err(FetchError::ListingParse {
                        url: dir.to_string(),
                        sample: body[..body.len().min(256)].to_vec(),
                    });
                }
                for e in entries {
                    let Some((table, _split)) = Self::parse_name(&e.name) else {
                        continue;
                    };
                    all_names.push(table.clone());
                    if !wanted.is_empty() && !wanted.iter().any(|w| matches_table(&table, w)) {
                        continue;
                    }
                    let url = dir
                        .join(&e.href)
                        .or_else(|_| self.base.join(&e.href))
                        .map_err(|er| FetchError::Parse {
                            url: e.href.clone(),
                            what: "listing href",
                            message: er.to_string(),
                        })?;
                    if !seen_tables.contains(&table) {
                        seen_tables.push(table.clone());
                    }
                    out.push(RemoteFile {
                        url,
                        source: SOURCE_ID,
                        table: Some(table),
                        period: Some(ym),
                        kind: FileKind::MmsZip,
                        size: e.size,
                        modified: e.modified,
                    });
                }
            }
            for w in &wanted {
                if !seen_tables.iter().any(|t| matches_table(t, w)) {
                    // Nearest by longest shared prefix, then by name.
                    let mut nearest: Vec<&String> = all_names.iter().collect();
                    nearest.sort();
                    nearest.dedup();
                    nearest.sort_by_key(|n| {
                        std::cmp::Reverse(
                            n.bytes().zip(w.bytes()).take_while(|(a, b)| a == b).count(),
                        )
                    });
                    let nearest: Vec<&str> = nearest.iter().take(5).map(|s| s.as_str()).collect();
                    return Err(FetchError::NoMatch {
                        url: first_url.clone(),
                        entries: all_names.len(),
                        table: w.clone(),
                        period: ym.to_string(),
                        nearest: if nearest.is_empty() {
                            "none".into()
                        } else {
                            nearest.join(", ")
                        },
                    });
                }
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
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::cache::DiskCache;
    use crate::fetcher::FetcherConfig;
    use crate::transport::{Canned, StaticTransport};

    #[test]
    fn urls_and_names() {
        let a = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
        let root = a.month_root(YearMonth::new(2024, 1).unwrap()).unwrap();
        assert_eq!(
            root.as_str(),
            "https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/"
        );
        assert_eq!(
            a.data_dirs(YearMonth::new(2024, 1).unwrap()).unwrap().len(),
            3
        );
        assert!(matches!(
            a.month_root(YearMonth::new(2010, 1).unwrap()),
            Err(FetchError::EraUnsupported { .. })
        ));
        assert!(MmsdmArchive::new("not a url").is_err());
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip"),
            Some(("DISPATCH_UNIT_SCADA".into(), None))
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_BIDPEROFFER1_202401010000.zip"),
            Some(("BIDPEROFFER".into(), Some(1)))
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_P5MIN_CONSTRAINTSOLUTION4_ALL_202401010000.zip"),
            Some(("P5MIN_CONSTRAINTSOLUTION".into(), Some(4)))
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_P5MIN_UNITSOLUTION_ALL_202401010000.zip"),
            Some(("P5MIN_UNITSOLUTION".into(), None))
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_MARKETNOTICEDATA_202401.DMP"),
            None
        );
        assert_eq!(MmsdmArchive::parse_name("PUBLIC_DVD_X_2024.zip"), None);
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_123_202401010000.zip"),
            Some(("123".into(), None))
        );
        assert!(matches_table("REGION", "region"));
        assert!(!matches_table("REGIONAPC", "REGION"));
        assert!(
            MmsdmArchive::listing_ttl(YearMonth::new(2020, 1).unwrap())
                > MmsdmArchive::listing_ttl(YearMonth::new(2999, 1).unwrap())
        );
    }

    #[tokio::test]
    async fn lists_from_fixture_listing() {
        let dir = tempfile::tempdir().unwrap();
        let html = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/listings/mmsdm_2024_01_data.html"
        ))
        .unwrap();
        let t = Arc::new(StaticTransport::default());
        let a = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
        let ym = YearMonth::new(2024, 1).unwrap();
        let dirs = a.data_dirs(ym).unwrap();
        t.add(dirs[0].as_str(), Canned::ok(&html));
        t.add(dirs[1].as_str(), Canned::status(404));
        t.add(dirs[2].as_str(), Canned::ok(br#"<pre> 2/13/2024 8:27 AM 12 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/PREDISP_ALL_DATA/PUBLIC_DVD_PREDISPATCHPRICE_ALL_202401010000.zip">PUBLIC_DVD_PREDISPATCHPRICE_ALL_202401010000.zip</A><br></pre>"#));
        let cfg = FetcherConfig {
            attempts: 1,
            min_interval: Duration::ZERO,
            ..Default::default()
        };
        let f = Fetcher::new(Arc::new(t.clone()), DiskCache::new(dir.path()), cfg);
        assert_eq!(a.id(), "nem-mmsdm");
        let q = Query {
            tables: vec!["dispatch_unit_scada".into(), "BIDPEROFFER".into()],
            from: ym,
            to: ym,
        };
        let files = a.list(&q, &f).await.unwrap();
        let names: Vec<String> = files.iter().map(RemoteFile::file_name).collect();
        assert_eq!(
            names,
            [
                "PUBLIC_DVD_BIDPEROFFER1_202401010000.zip",
                "PUBLIC_DVD_BIDPEROFFER2_202401010000.zip",
                "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip"
            ]
        );
        assert_eq!(files[2].size, Some(22_576_818));
        assert_eq!(files[2].table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
        assert_eq!(files[2].period, Some(ym));
        assert!(
            files[2]
                .url
                .as_str()
                .starts_with("https://www.nemweb.com.au/Data_Archive/")
        );
        // Every table: the 147 zips of the fixture listing plus the one in PREDISP_ALL_DATA.
        let all = a
            .list(
                &Query {
                    tables: vec![],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        assert_eq!(all.len(), 148);
        assert!(
            all.iter()
                .any(|r| r.table.as_deref() == Some("PREDISPATCHPRICE"))
        );
        // Unknown table names the nearest.
        let e = a
            .list(
                &Query {
                    tables: vec!["DISPATCH_UNIT_SCADAX".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap_err();
        match e {
            FetchError::NoMatch {
                table,
                nearest,
                entries,
                ..
            } => {
                assert_eq!(table, "DISPATCH_UNIT_SCADAX");
                assert!(nearest.contains("DISPATCH_UNIT_SCADA"), "{nearest}");
                assert!(entries > 140);
            }
            other => panic!("{other}"),
        }
        // A month with no listing fails on the first directory; a 404 there is not skipped.
        let ym2 = YearMonth::new(2024, 2).unwrap();
        let e = a
            .list(
                &Query {
                    tables: vec![],
                    from: ym2,
                    to: ym2,
                },
                &f,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(e, FetchError::HttpStatus { status: 404, .. }),
            "{e}"
        );
        // An unparseable first listing.
        let ym3 = YearMonth::new(2024, 3).unwrap();
        t.add(
            a.data_dirs(ym3).unwrap()[0].as_str(),
            Canned::ok(b"<html>no links</html>"),
        );
        assert!(matches!(
            a.list(
                &Query {
                    tables: vec![],
                    from: ym3,
                    to: ym3
                },
                &f
            )
            .await,
            Err(FetchError::ListingParse { .. })
        ));
        // Era.
        let old = YearMonth::new(2012, 1).unwrap();
        assert!(matches!(
            a.list(
                &Query {
                    tables: vec![],
                    from: old,
                    to: old
                },
                &f
            )
            .await,
            Err(FetchError::EraUnsupported { .. })
        ));
    }
}
