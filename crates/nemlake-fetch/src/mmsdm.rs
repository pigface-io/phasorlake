//! The monthly NEM archive on nemweb.
//!
//! ```text
//! {base}/Data_Archive/Wholesale_Electricity/MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}/MMSDM_Historical_Data_SQLLoader/DATA/
//!     PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip     one per table; big tables split: BIDPEROFFER1, BIDPEROFFER2
//! ```
//!
//! Directories are listed rather than URLs guessed, because split tables
//! and renamed files only show up in the listing.
//!
//! Months from 2015 on have that directory. July 2009 to December 2014 are
//! published only as one zip per month of 1.4 to 2.1 GB:
//!
//! ```text
//! {base}/Data_Archive/Wholesale_Electricity/MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}.zip
//!     {MM}/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip
//! ```
//!
//! For those the zip's index is read with ranged requests and each
//! per-table zip becomes a [`RemoteFile`] with a `member`, so fetching one
//! table downloads that member only. Where a month has both a directory and
//! a whole-month zip, the directory is used.

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

/// The first year whose months are published as directories of per-table
/// zips. Earlier months exist only as one whole-month zip.
const FIRST_DIRECTORY_YEAR: i32 = 2015;

/// The sub-directories of a month that hold per-table data zips.
const DATA_DIRS: [&str; 3] = ["DATA/", "P5MIN_ALL_DATA/", "PREDISP_ALL_DATA/"];

/// A whole-month zip never changes once published; its index is cached this long.
const ZIP_DIRECTORY_TTL: i64 = 90 * 86_400;

/// How the archive publishes one month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonthLayout {
    /// A directory of per-table zips (`MMSDM_2024_01/.../DATA/`).
    Directory,
    /// Only one zip of the whole month (`MMSDM_2012_01.zip`) holding the
    /// per-table zips; they are read out of it with ranged requests.
    WholeZip {
        /// The zip.
        url: Url,
        /// Its size, when the listing showed it.
        size: Option<u64>,
    },
}

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

    /// The year directory, e.g. `.../MMSDM/2012/`.
    pub fn year_root(&self, year: i32) -> Result<Url, FetchError> {
        let path = format!("/Data_Archive/Wholesale_Electricity/MMSDM/{year}/");
        self.base.join(&path).map_err(|e| FetchError::Parse {
            url: path,
            what: "year URL",
            message: e.to_string(),
        })
    }

    /// The month directory, e.g. `.../MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/`.
    /// It exists from 2015 on; earlier months are only published as one zip
    /// (see [`MonthLayout`]).
    pub fn month_root(&self, ym: YearMonth) -> Result<Url, FetchError> {
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

    /// Finds how a month is published by reading its year's listing.
    pub async fn month_layout(
        &self,
        ym: YearMonth,
        fetcher: &Fetcher,
    ) -> Result<MonthLayout, FetchError> {
        let year_url = self.year_root(ym.year())?;
        let missing = |available: String| FetchError::MonthMissing {
            period: ym.to_string(),
            url: year_url.to_string(),
            available,
        };
        let body = match fetcher
            .fetch_listing(year_url.as_str(), Self::listing_ttl(ym))
            .await
        {
            Ok(b) => b,
            Err(FetchError::HttpStatus { status: 404, .. }) => {
                return Err(missing("no such year".into()));
            }
            Err(e) => return Err(e),
        };
        let entries = listing::parse(&body);
        let dir_name = format!("MMSDM_{}_{:02}", ym.year(), ym.month());
        let zip_name = format!("{dir_name}.zip");
        if entries
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(&dir_name))
        {
            return Ok(MonthLayout::Directory);
        }
        if let Some(e) = entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(&zip_name))
        {
            let url = year_url.join(&e.href).map_err(|er| FetchError::Parse {
                url: e.href.clone(),
                what: "listing href",
                message: er.to_string(),
            })?;
            return Ok(MonthLayout::WholeZip { url, size: e.size });
        }
        let mut months: Vec<&str> = entries
            .iter()
            .filter(|e| e.name.starts_with("MMSDM_"))
            .map(|e| e.name.trim_end_matches(".zip"))
            .collect();
        months.sort_unstable();
        months.dedup();
        Err(missing(if months.is_empty() {
            "no months".into()
        } else {
            months.join(", ")
        }))
    }

    /// Whether a member of a whole-month zip is one of the per-table data zips.
    fn is_data_member(name: &str) -> bool {
        let upper = name.to_ascii_uppercase();
        upper.ends_with(".ZIP")
            && DATA_DIRS
                .iter()
                .any(|d| upper.contains(&format!("/MMSDM_HISTORICAL_DATA_SQLLOADER/{d}")))
    }

    /// Every per-table file of a month with the table it carries, from
    /// whichever layout the month is published in, plus the URL listed.
    async fn month_files(
        &self,
        ym: YearMonth,
        fetcher: &Fetcher,
    ) -> Result<(String, Vec<(String, RemoteFile)>), FetchError> {
        // From 2015 the month has a directory; try it first and fall back to
        // the year listing when it is not there.
        if ym.year() >= FIRST_DIRECTORY_YEAR {
            match self.directory_files(ym, fetcher).await {
                Err(FetchError::HttpStatus { status: 404, .. }) => {}
                other => return other,
            }
        }
        match self.month_layout(ym, fetcher).await? {
            MonthLayout::Directory => self.directory_files(ym, fetcher).await,
            MonthLayout::WholeZip { url, .. } => {
                let members = fetcher
                    .list_zip_members(url.as_str(), ZIP_DIRECTORY_TTL)
                    .await?;
                let mut out = Vec::new();
                for m in members {
                    if !Self::is_data_member(&m.name) {
                        continue;
                    }
                    let Some((table, _split)) = Self::parse_name(m.file_name()) else {
                        continue;
                    };
                    out.push((
                        table.clone(),
                        RemoteFile {
                            url: RemoteFile::member_url(&url, &m.name),
                            source: SOURCE_ID,
                            table: Some(table),
                            period: Some(ym),
                            kind: FileKind::MmsZip,
                            size: Some(m.uncompressed_size),
                            modified: None,
                            member: Some(Box::new(m)),
                        },
                    ));
                }
                Ok((url.to_string(), out))
            }
        }
    }

    /// The per-table files of a month published as a directory.
    async fn directory_files(
        &self,
        ym: YearMonth,
        fetcher: &Fetcher,
    ) -> Result<(String, Vec<(String, RemoteFile)>), FetchError> {
        let dirs = self.data_dirs(ym)?;
        let ttl = Self::listing_ttl(ym);
        let mut out = Vec::new();
        for (i, dir) in dirs.iter().enumerate() {
            let body = match fetcher.fetch_listing(dir.as_str(), ttl).await {
                Ok(b) => b,
                // The big-table directories do not exist for every month.
                Err(FetchError::HttpStatus { status: 404, .. }) if i > 0 => continue,
                Err(e) => return Err(e),
            };
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
                let url = dir
                    .join(&e.href)
                    .or_else(|_| self.base.join(&e.href))
                    .map_err(|er| FetchError::Parse {
                        url: e.href.clone(),
                        what: "listing href",
                        message: er.to_string(),
                    })?;
                out.push((
                    table.clone(),
                    RemoteFile {
                        url,
                        source: SOURCE_ID,
                        table: Some(table),
                        period: Some(ym),
                        kind: FileKind::MmsZip,
                        size: e.size,
                        modified: e.modified,
                        member: None,
                    },
                ));
            }
        }
        Ok((dirs[0].to_string(), out))
    }

    /// The directories to list for a month published as a directory.
    pub fn data_dirs(&self, ym: YearMonth) -> Result<Vec<Url>, FetchError> {
        let root = self.month_root(ym)?;
        Ok(DATA_DIRS.iter().filter_map(|d| root.join(d).ok()).collect())
    }

    /// The table a data zip carries and which of its files it is, for
    /// either naming scheme (see [`nemlake_core::archive_name`]):
    /// `PUBLIC_DVD_BIDPEROFFER1_202401010000.zip` and
    /// `PUBLIC_ARCHIVE#BIDPEROFFER#FILE01#202608010000.zip` are both
    /// `("BIDPEROFFER", Some(1))`.
    pub fn parse_name(name: &str) -> Option<(String, Option<u8>)> {
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".zip") {
            return None;
        }
        let parsed = nemlake_core::archive_name::parse(name)?;
        if parsed.file.is_some() {
            return Some((parsed.table, parsed.file));
        }
        let (base, split) = parsed.split_table();
        Some((base.to_owned(), split))
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
            let (listed_url, files) = self.month_files(ym, fetcher).await?;
            let mut seen_tables: Vec<&str> = Vec::new();
            for (table, remote) in &files {
                if !wanted.is_empty() && !wanted.iter().any(|w| matches_table(table, w)) {
                    continue;
                }
                if !seen_tables.contains(&table.as_str()) {
                    seen_tables.push(table);
                }
                out.push(remote.clone());
            }
            for w in &wanted {
                if !seen_tables.iter().any(|t| matches_table(t, w)) {
                    // Nearest by longest shared prefix, then by name.
                    let mut nearest: Vec<&String> = files.iter().map(|(t, _)| t).collect();
                    nearest.sort();
                    nearest.dedup();
                    nearest.sort_by_key(|n| {
                        std::cmp::Reverse(
                            n.bytes().zip(w.bytes()).take_while(|(a, b)| a == b).count(),
                        )
                    });
                    let nearest: Vec<&str> = nearest.iter().take(5).map(|s| s.as_str()).collect();
                    return Err(FetchError::NoMatch {
                        url: listed_url.clone(),
                        entries: files.len(),
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
        assert_eq!(
            a.year_root(2012).unwrap().as_str(),
            "https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2012/"
        );
        assert!(MmsdmArchive::is_data_member(
            "01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_201201010000.zip"
        ));
        assert!(MmsdmArchive::is_data_member(
            "01/MMSDM_Historical_Data_SQLLoader/PREDISP_ALL_DATA/PUBLIC_DVD_X_201201010000.zip"
        ));
        assert!(!MmsdmArchive::is_data_member(
            "01/MMSDM_Historical_Data_SQLLoader/CTL/PUBLIC_DVD_REGION_201201.ctl"
        ));
        assert!(!MmsdmArchive::is_data_member(
            "01/MMSDM_Historical_Data_SQLLoader/DATA/Readme.htm"
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
        // A month with no directory is looked up in its year's listing, and
        // a year that lists nothing for it is a missing month.
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
        assert!(matches!(e, FetchError::MonthMissing { .. }), "{e}");
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
    }

    #[tokio::test]
    async fn lists_the_hash_naming_scheme() {
        // Recent months name files PUBLIC_ARCHIVE#TABLE#FILEnn#YYYYMM010000.zip;
        // the listing escapes the # in the link and shows it in the text.
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let a = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
        let ym = YearMonth::new(2026, 8).unwrap();
        let dirs = a.data_dirs(ym).unwrap();
        let link = |name: &str| {
            format!(
                r#" 9/9/2026  8:27 AM  1584114 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2026/MMSDM_2026_08/MMSDM_Historical_Data_SQLLoader/DATA/{}">{name}</A><br>"#,
                name.replace('#', "%23")
            )
        };
        let html = format!(
            "<pre>{}{}{}{}</pre>",
            link("PUBLIC_ARCHIVE#BIDOFFERPERIOD#FILE01#202608010000.zip"),
            link("PUBLIC_ARCHIVE#BIDOFFERPERIOD#FILE02#202608010000.zip"),
            link("PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202608010000.zip"),
            link("PUBLIC_RUN_SQLLDR_202608.bat")
        );
        t.add(dirs[0].as_str(), Canned::ok(html.as_bytes()));
        t.add(dirs[1].as_str(), Canned::status(404));
        t.add(dirs[2].as_str(), Canned::status(404));
        let f = quick(&t, dir.path());
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
        assert_eq!(all.len(), 3);
        let bids = a
            .list(
                &Query {
                    tables: vec!["BIDOFFERPERIOD".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        assert_eq!(bids.len(), 2, "both files of a split table");
        let price = a
            .list(
                &Query {
                    tables: vec!["dispatchprice".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        assert_eq!(price.len(), 1);
        let r = &price[0];
        assert_eq!(r.table.as_deref(), Some("DISPATCHPRICE"));
        assert!(
            r.url
                .path()
                .ends_with("PUBLIC_ARCHIVE%23DISPATCHPRICE%23FILE01%23202608010000.zip"),
            "{}",
            r.url
        );
        assert_eq!(r.url.fragment(), None, "the # stays escaped in the path");
        assert_eq!(
            r.file_name(),
            "PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202608010000.zip"
        );
        assert_eq!(r.stem(), "public_archive_dispatchprice_file01_202608010000");
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_ARCHIVE#BIDOFFERPERIOD#FILE02#202608010000.zip"),
            Some(("BIDOFFERPERIOD".into(), Some(2)))
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_ARCHIVE#X#FILE01#202608.ctl"),
            None
        );
        // The cache keeps the bytes under a name without the #.
        t.add(r.url.as_str(), Canned::ok(b"zip bytes"));
        let local = f.fetch(r).await.unwrap();
        let cached = local.path.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(
            cached.contains("DISPATCHPRICE") && !cached.contains('#'),
            "{cached}"
        );
        assert_eq!(local.remote.file_name(), r.file_name());
    }

    /// A whole-month zip as AEMO published them before 2015: per-table zips
    /// inside one zip, beside control files and documentation.
    fn month_zip() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        use crate::remotezip::tests::make_zip;
        let region_csv = b"C,NEMP.WORLD,DVD_REGION,AEMO,PUBLIC,2012/02/07,00:00:00\r\nI,PARTICIPANT_REGISTRATION,REGION,1,REGIONID,DESCRIPTION\r\nD,PARTICIPANT_REGISTRATION,REGION,1,NSW1,New South Wales\r\nC,\"END OF REPORT\",4\r\n";
        let region = make_zip(
            &[("PUBLIC_DVD_REGION_201201010000.CSV", region_csv)],
            &[],
            false,
        );
        let price = make_zip(
            &[("PUBLIC_DVD_PREDISPATCHPRICE_201201010000.CSV", b"C,x\r\n")],
            &[],
            false,
        );
        let month = make_zip(
            &[
                ("01/", b""),
                (
                    "01/MMSDM_Historical_Data_SQLLoader/CTL/PUBLIC_DVD_REGION_201201.ctl",
                    b"LOAD DATA",
                ),
                ("01/MMSDM_Historical_Data_SQLLoader/DATA/", b""),
                (
                    "01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_201201010000.zip",
                    &region,
                ),
                (
                    "01/MMSDM_Historical_Data_SQLLoader/DATA/Readme.htm",
                    b"<html>",
                ),
                (
                    "01/MMSDM_Historical_Data_SQLLoader/PREDISP_ALL_DATA/PUBLIC_DVD_PREDISPATCHPRICE_201201010000.zip",
                    &price,
                ),
            ],
            // AEMO deflates most members and stores a few; cover both.
            &[
                "01/MMSDM_Historical_Data_SQLLoader/PREDISP_ALL_DATA/PUBLIC_DVD_PREDISPATCHPRICE_201201010000.zip",
            ],
            false,
        );
        (month, region, price)
    }

    const YEAR_2012: &str =
        "https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2012/";

    fn year_listing() -> Vec<u8> {
        br#"<pre><A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/">[To Parent Directory]</A><br><br> 2/21/2012  3:10 PM   1912843433 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2012/MMSDM_2012_01.zip">MMSDM_2012_01.zip</A><br> 3/21/2012  3:10 PM   1784225268 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2012/MMSDM_2012_02.zip">MMSDM_2012_02.zip</A><br></pre>"#.to_vec()
    }

    fn quick(t: &Arc<StaticTransport>, dir: &std::path::Path) -> Fetcher {
        Fetcher::new(
            Arc::new(Arc::clone(t)),
            DiskCache::new(dir),
            FetcherConfig {
                attempts: 2,
                backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(1),
                min_interval: Duration::ZERO,
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn whole_month_zip_is_listed_and_one_member_fetched() {
        let dir = tempfile::tempdir().unwrap();
        let (month, region, price) = month_zip();
        let zip_url = format!("{YEAR_2012}MMSDM_2012_01.zip");
        let t = Arc::new(StaticTransport::default());
        t.add(YEAR_2012, Canned::ok(&year_listing()));
        t.add(&zip_url, Canned::ok(&month));
        let f = quick(&t, dir.path());
        let a = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
        let ym = YearMonth::new(2012, 1).unwrap();
        assert_eq!(
            a.month_layout(ym, &f).await.unwrap(),
            MonthLayout::WholeZip {
                url: Url::parse(&zip_url).unwrap(),
                size: Some(1_912_843_433)
            }
        );

        // Every data member is listed, each as a member of the month zip.
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
        assert_eq!(all.len(), 2);
        let tables: Vec<&str> = all.iter().filter_map(|r| r.table.as_deref()).collect();
        assert_eq!(tables, ["PREDISPATCHPRICE", "REGION"]);
        let region_remote = all
            .iter()
            .find(|r| r.table.as_deref() == Some("REGION"))
            .unwrap();
        assert_eq!(
            region_remote.url.as_str(),
            format!(
                "{zip_url}#01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_201201010000.zip"
            )
        );
        assert_eq!(region_remote.request_url().as_str(), zip_url);
        assert_eq!(
            region_remote.file_name(),
            "PUBLIC_DVD_REGION_201201010000.zip"
        );
        assert_eq!(region_remote.stem(), "public_dvd_region_201201010000");
        assert_eq!(region_remote.size, Some(region.len() as u64));
        assert_eq!(region_remote.period, Some(ym));

        // The index was read with ranged requests and is now cached.
        let before = t.request_count();
        let one = a
            .list(
                &Query {
                    tables: vec!["region".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(
            t.request_count(),
            before,
            "listing and index come from the cache"
        );

        // Fetching the member yields exactly the inner zip.
        let local = f.fetch(&one[0]).await.unwrap();
        assert!(!local.cached);
        assert_eq!(std::fs::read(&local.path).unwrap(), region);
        assert_eq!(local.size, region.len() as u64);
        assert_eq!(local.sha256, crate::cache::sha256_hex(&region));
        assert!(
            local
                .path
                .ends_with(local.path.file_name().unwrap().to_str().unwrap())
        );
        assert!(
            local
                .path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("PUBLIC_DVD_REGION_201201010000.zip")
        );
        let again = f.fetch(&one[0]).await.unwrap();
        assert!(again.cached);
        assert_eq!(again.sha256, local.sha256);
        // A stored member comes out the same way.
        let p = all
            .iter()
            .find(|r| r.table.as_deref() == Some("PREDISPATCHPRICE"))
            .unwrap();
        assert_eq!(p.member.as_ref().unwrap().method, 0);
        assert_eq!(
            std::fs::read(f.fetch(p).await.unwrap().path).unwrap(),
            price
        );

        // A table the month does not have names the nearest ones.
        let e = a
            .list(
                &Query {
                    tables: vec!["REGIONX".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&e, FetchError::NoMatch { nearest, .. } if nearest.contains("REGION")),
            "{e}"
        );
    }

    #[tokio::test]
    async fn missing_months_and_broken_month_zips() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add(YEAR_2012, Canned::ok(&year_listing()));
        let f = quick(&t, dir.path());
        let a = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
        let q = |y, m| {
            let ym = YearMonth::new(y, m).unwrap();
            Query {
                tables: vec![],
                from: ym,
                to: ym,
            }
        };
        // A month the year does not list.
        let e = a.list(&q(2012, 7), &f).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::MonthMissing { available, .. } if available == "MMSDM_2012_01, MMSDM_2012_02"),
            "{e}"
        );
        // A year that does not exist.
        let e = a.list(&q(2008, 1), &f).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::MonthMissing { available, .. } if available == "no such year"),
            "{e}"
        );
        // A year listing with nothing in it.
        t.add(
            "https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2011/",
            Canned::ok(b"<pre></pre>"),
        );
        let e = a.list(&q(2011, 1), &f).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::MonthMissing { available, .. } if available == "no months"),
            "{e}"
        );
        // A recent month with no directory yet falls back to the year listing.
        t.add(
            "https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2030/",
            Canned::ok(br#"<pre> 1/1/2030 1:00 AM &lt;dir&gt; <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2030/MMSDM_2030_01/">MMSDM_2030_01</A><br></pre>"#),
        );
        let e = a.list(&q(2030, 2), &f).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::MonthMissing { available, .. } if available == "MMSDM_2030_01"),
            "{e}"
        );
        assert_eq!(
            a.month_layout(YearMonth::new(2030, 1).unwrap(), &f)
                .await
                .unwrap(),
            MonthLayout::Directory
        );

        // The month zip is not a zip.
        let zip_url = format!("{YEAR_2012}MMSDM_2012_01.zip");
        t.add(&zip_url, Canned::ok(b"<html>Service Unavailable</html>"));
        let e = a.list(&q(2012, 1), &f).await.unwrap_err();
        assert!(matches!(&e, FetchError::RemoteZip { .. }), "{e}");
        assert_eq!(
            nemlake_core::Diagnose::diagnostic(&e).code,
            "zip.remote_index"
        );
    }

    #[tokio::test]
    async fn corrupt_or_unsupported_members_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (month, region, _) = month_zip();
        let zip_url = format!("{YEAR_2012}MMSDM_2012_01.zip");
        let t = Arc::new(StaticTransport::default());
        t.add(YEAR_2012, Canned::ok(&year_listing()));
        t.add(&zip_url, Canned::ok(&month));
        let f = quick(&t, dir.path());
        let a = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
        let ym = YearMonth::new(2012, 1).unwrap();
        let listed = a
            .list(
                &Query {
                    tables: vec!["REGION".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        let remote = &listed[0];
        let member = remote.member.clone().unwrap();

        // The directory's CRC disagrees with the bytes: refused after retries, nothing cached.
        let mut wrong_crc = remote.clone();
        wrong_crc.member.as_mut().unwrap().crc32 ^= 1;
        let e = f.fetch(&wrong_crc).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::RetriesExhausted { last, .. } if matches!(**last, FetchError::MemberMismatch { what: "crc32", .. })),
            "{e}"
        );
        let mut wrong_size = remote.clone();
        wrong_size.member.as_mut().unwrap().uncompressed_size += 1;
        let e = f.fetch(&wrong_size).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::RetriesExhausted { last, .. } if matches!(**last, FetchError::MemberMismatch { what: "size", .. })),
            "{e}"
        );
        assert!(f.cache().entries().unwrap().is_empty());

        // The offset does not point at a local header.
        let mut wrong_offset = remote.clone();
        wrong_offset.member.as_mut().unwrap().header_offset += 3;
        let e = f.fetch(&wrong_offset).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::RemoteZip { message, .. } if message.contains("local file header")),
            "{e}"
        );

        // Bytes that are not a deflate stream.
        let mut garbage = month.clone();
        let data_at = member.header_offset as usize + 30 + member.name.len();
        for b in &mut garbage[data_at..data_at + member.compressed_size as usize] {
            *b = 0xff;
        }
        let t2 = Arc::new(StaticTransport::default());
        t2.add(&zip_url, Canned::ok(&garbage));
        let f2 = quick(&t2, &dir.path().join("b"));
        let e = f2.fetch(remote).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::RemoteZip { message, .. } if message.contains("does not inflate")),
            "{e}"
        );

        // The body ends before the member does.
        let t3 = Arc::new(StaticTransport::default());
        t3.add(&zip_url, Canned::ok(&month[..data_at + 5]));
        let f3 = quick(&t3, &dir.path().join("c"));
        let e = f3.fetch(remote).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::RetriesExhausted { last, .. } if matches!(**last, FetchError::ContentLength { .. })),
            "{e}"
        );

        // Encrypted or exotically compressed members are named, not guessed at.
        let mut encrypted = remote.clone();
        encrypted.member.as_mut().unwrap().flags |= 1;
        let e = f.fetch(&encrypted).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::MemberUnsupported { reason, .. } if reason.contains("encrypted")),
            "{e}"
        );
        let mut bzip = remote.clone();
        bzip.member.as_mut().unwrap().method = 12;
        let e = f.fetch(&bzip).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::MemberUnsupported { reason, .. } if reason.contains("method 12")),
            "{e}"
        );

        // Offline: nothing cached means a clear refusal.
        let off = Fetcher::new(
            Arc::new(crate::transport::OfflineTransport),
            DiskCache::new(dir.path().join("d")),
            FetcherConfig::default(),
        );
        assert!(matches!(
            off.list_zip_members(&zip_url, 60).await,
            Err(FetchError::Offline { .. })
        ));
        // And the good member still fetches after all of that.
        assert_eq!(
            std::fs::read(f.fetch(remote).await.unwrap().path).unwrap(),
            region
        );
    }
}
