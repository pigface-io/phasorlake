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
//! utilake downloads everything once, so such a month is one file: the
//! whole zip, read locally with its per-table zips nested inside. Where a
//! month has both a directory and a whole-month zip, the directory is used.
//! A directory month also lists its control files (`CTL/`) and its README
//! (`DOCUMENTATION/`), which the census and the schema tools read.

use std::path::PathBuf;

use async_trait::async_trait;
use url::Url;
use utilake_core::YearMonth;

use crate::archive::Archive;
use crate::error::FetchError;
use crate::fetcher::Fetcher;
use crate::listing;
use crate::source::{Query, RemoteFile, Source};

/// The source id.
pub const SOURCE_ID: &str = "nem-mmsdm";

/// The first year whose months are published as directories of per-table
/// zips. Earlier months exist only as one whole-month zip.
const FIRST_DIRECTORY_YEAR: i32 = 2015;

/// The sub-directories of a month that hold per-table data zips.
const DATA_DIRS: [&str; 3] = ["DATA/", "P5MIN_ALL_DATA/", "PREDISP_ALL_DATA/"];

/// The sub-directories of a month that hold its control files and README.
const SUPPORT_DIRS: [&str; 2] = ["CTL/", "DOCUMENTATION/"];

/// How the archive publishes one month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonthLayout {
    /// A directory of per-table zips (`MMSDM_2024_01/.../DATA/`).
    Directory,
    /// Only one zip of the whole month (`MMSDM_2012_01.zip`) holding the
    /// per-table zips, the control files and the README.
    WholeZip {
        /// The zip.
        url: Url,
    },
}

/// The files of one month: its per-table data files with the table each
/// carries, or its whole zip; and its control files and README.
struct MonthFiles {
    /// The URL listed, for a message about a table not found.
    listed_url: String,
    data: Vec<(String, RemoteFile)>,
    whole: Option<RemoteFile>,
    support: Vec<RemoteFile>,
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
        let body = match fetcher.fetch_listing(year_url.as_str()).await {
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
            return Ok(MonthLayout::WholeZip { url });
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

    /// The files of a month, from whichever layout it is published in.
    async fn month_files(
        &self,
        ym: YearMonth,
        fetcher: &Fetcher,
    ) -> Result<MonthFiles, FetchError> {
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
            MonthLayout::WholeZip { url } => Ok(MonthFiles {
                listed_url: url.to_string(),
                data: Vec::new(),
                whole: Some(RemoteFile {
                    url,
                    source: SOURCE_ID,
                    table: None,
                    period: Some(ym),
                    size: None,
                    modified: None,
                    modified_at: None,
                }),
                support: Vec::new(),
            }),
        }
    }

    /// Every archived file of `month`, sorted: under the month's directory
    /// (2015 on) and the month's whole zip (before): data zips and CSVs,
    /// control files and the README. Sidecars and partial downloads are
    /// left out.
    pub fn archived_files(
        &self,
        archive: &Archive,
        month: YearMonth,
    ) -> Result<Vec<PathBuf>, FetchError> {
        let dir = archive.path_for(self.month_root(month)?.as_str());
        let whole_url = self
            .year_root(month.year())?
            .join(&format!("MMSDM_{}_{:02}.zip", month.year(), month.month()))
            .map_err(|e| FetchError::Parse {
                url: month.to_string(),
                what: "month zip URL",
                message: e.to_string(),
            })?;
        let mut out: Vec<PathBuf> = archive
            .entries_under(Some(&dir))
            .into_iter()
            .map(|e| e.path)
            .collect();
        if let Some(whole) = archive.lookup(whole_url.as_str())? {
            out.push(whole.path);
        }
        out.sort();
        Ok(out)
    }

    /// The per-table files of a month published as a directory.
    async fn directory_files(
        &self,
        ym: YearMonth,
        fetcher: &Fetcher,
    ) -> Result<MonthFiles, FetchError> {
        let dirs = self.data_dirs(ym)?;
        let mut out = Vec::new();
        for (i, dir) in dirs.iter().enumerate() {
            let body = match fetcher.fetch_listing(dir.as_str()).await {
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
                // A zip whose name follows neither scheme is still a file
                // of the month: it is listed, under its own name, and its
                // tables are whatever its I rows say.
                let (table, named) = match Self::parse_name(&e.name) {
                    Some(table) => (table, true),
                    None if e.name.to_ascii_lowercase().ends_with(".zip") => {
                        (e.name.to_ascii_uppercase(), false)
                    }
                    None => continue,
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
                        table: named.then(|| table.clone()),
                        period: Some(ym),
                        size: e.size,
                        modified: e.modified.map(|m| m.text().to_owned()),
                        modified_at: e.modified.and_then(|m| m.stamp()),
                    },
                ));
            }
        }
        let root = self.month_root(ym)?;
        let mut support = Vec::new();
        for d in SUPPORT_DIRS {
            let Ok(dir) = root.join(d) else { continue };
            let body = match fetcher.fetch_listing(dir.as_str()).await {
                Ok(b) => b,
                Err(FetchError::HttpStatus { status: 404, .. }) => continue,
                Err(e) => return Err(e),
            };
            for e in listing::parse(&body) {
                let lower = e.name.to_ascii_lowercase();
                if e.is_dir || !(lower.ends_with(".ctl") || lower == "readme.txt") {
                    continue;
                }
                let url = dir.join(&e.href).map_err(|er| FetchError::Parse {
                    url: e.href.clone(),
                    what: "listing href",
                    message: er.to_string(),
                })?;
                support.push(RemoteFile {
                    url,
                    source: SOURCE_ID,
                    table: None,
                    period: Some(ym),
                    size: e.size,
                    modified: e.modified.map(|m| m.text().to_owned()),
                    modified_at: e.modified.and_then(|m| m.stamp()),
                });
            }
        }
        Ok(MonthFiles {
            listed_url: dirs[0].to_string(),
            data: out,
            whole: None,
            support,
        })
    }

    /// The directories to list for a month published as a directory.
    pub fn data_dirs(&self, ym: YearMonth) -> Result<Vec<Url>, FetchError> {
        let root = self.month_root(ym)?;
        Ok(DATA_DIRS.iter().filter_map(|d| root.join(d).ok()).collect())
    }

    /// The table a data zip carries, for either naming scheme (see
    /// [`utilake_core::archive_name`]):
    /// `PUBLIC_DVD_BIDPEROFFER1_202401010000.zip` and
    /// `PUBLIC_ARCHIVE#BIDPEROFFER#FILE01#202608010000.zip` are both
    /// `BIDPEROFFER`.
    pub fn parse_name(name: &str) -> Option<String> {
        if !name.to_ascii_lowercase().ends_with(".zip") {
            return None;
        }
        let parsed = utilake_core::archive_name::parse(name)?;
        if parsed.file.is_some() {
            return Some(parsed.table);
        }
        Some(parsed.split_table().0.to_owned())
    }
}

#[async_trait]
impl Source for MmsdmArchive {
    async fn list(&self, query: &Query, fetcher: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        let upper = |names: &[String]| -> Vec<String> {
            names.iter().map(|t| t.to_ascii_uppercase()).collect()
        };
        let wanted = upper(&query.tables);
        let excluded = upper(&query.exclude);
        // Tables come and go over the years, so a table asked for by name
        // may be missing from some months of a range. It is an error only
        // when no month of the range has it: then the name is likely wrong.
        let mut found = vec![false; wanted.len()];
        let mut last_listing: Option<(String, Vec<String>)> = None;
        for ym in query.from.range_inclusive(query.to) {
            let month = self.month_files(ym, fetcher).await?;
            // A whole-month zip holds every table of the month.
            if let Some(whole) = month.whole {
                found.iter_mut().for_each(|f| *f = true);
                out.push(whole);
            }
            out.extend(month.support);
            for (table, remote) in &month.data {
                if !wanted.is_empty() {
                    let mut any = false;
                    for (i, w) in wanted.iter().enumerate() {
                        if table.eq_ignore_ascii_case(w) {
                            found[i] = true;
                            any = true;
                        }
                    }
                    if !any {
                        continue;
                    }
                }
                // Leaving a table out wins over asking for it.
                if excluded.iter().any(|x| table.eq_ignore_ascii_case(x)) {
                    continue;
                }
                out.push(remote.clone());
            }
            last_listing = Some((
                month.listed_url,
                month.data.into_iter().map(|(t, _)| t).collect(),
            ));
        }
        if let (Some(i), Some((url, mut names))) = (found.iter().position(|f| !f), last_listing) {
            let w = &wanted[i];
            let entries = names.len();
            // Nearest by longest shared prefix, then by name.
            names.sort();
            names.dedup();
            names.sort_by_key(|n| {
                std::cmp::Reverse(n.bytes().zip(w.bytes()).take_while(|(a, b)| a == b).count())
            });
            let nearest: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
            return Err(FetchError::NoMatch {
                url,
                entries,
                table: w.clone(),
                period: if query.from == query.to {
                    query.from.to_string()
                } else {
                    format!("{}..{}", query.from, query.to)
                },
                nearest: if nearest.is_empty() {
                    "none".into()
                } else {
                    nearest.join(", ")
                },
            });
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
    use crate::archive::Archive;
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
        assert!(MmsdmArchive::new("not a url").is_err());
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip"),
            Some("DISPATCH_UNIT_SCADA".into())
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_BIDPEROFFER1_202401010000.zip"),
            Some("BIDPEROFFER".into())
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_P5MIN_CONSTRAINTSOLUTION4_ALL_202401010000.zip"),
            Some("P5MIN_CONSTRAINTSOLUTION".into())
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_P5MIN_UNITSOLUTION_ALL_202401010000.zip"),
            Some("P5MIN_UNITSOLUTION".into())
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_MARKETNOTICEDATA_202401.DMP"),
            None
        );
        assert_eq!(MmsdmArchive::parse_name("PUBLIC_DVD_X_2024.zip"), None);
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_DVD_123_202401010000.zip"),
            Some("123".into())
        );
    }

    /// The big-table directories of the newer scheme put `ALL` in the
    /// name, and a zip named by neither scheme is still a file of the
    /// month: both are listed, nothing is passed over in silence.
    #[tokio::test]
    async fn every_zip_of_a_month_is_listed_whatever_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let a = MmsdmArchive::new("https://h").unwrap();
        let ym = YearMonth::new(2026, 8).unwrap();
        let dirs = a.data_dirs(ym).unwrap();
        let link = |d: &Url, name: &str| {
            format!(
                r#" 9/13/2026 8:27 AM 12 <A HREF="{}{}">{name}</A><br>"#,
                d.path(),
                name.replace('#', "%23")
            )
        };
        let data = format!(
            "<pre>{}{}{}</pre>",
            link(
                &dirs[0],
                "PUBLIC_ARCHIVE#P5MIN_REGIONSOLUTION#FILE01#202608010000.zip"
            ),
            link(&dirs[0], "SOMETHING_NEW_202608.zip"),
            link(&dirs[0], "Readme.htm")
        );
        let all = format!(
            "<pre>{}</pre>",
            link(
                &dirs[1],
                "PUBLIC_ARCHIVE#P5MIN_REGIONSOLUTION#ALL#FILE01#202608010000.zip"
            )
        );
        t.add(dirs[0].as_str(), Canned::ok(data.as_bytes()));
        t.add(dirs[1].as_str(), Canned::ok(all.as_bytes()));
        t.add(dirs[2].as_str(), Canned::status(404));
        let f = Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(dir.path()),
            FetcherConfig {
                attempts: 1,
                min_interval: Duration::ZERO,
                ..Default::default()
            },
        );
        let list = |tables: &[&str], exclude: &[&str]| {
            let q = Query {
                exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
                tables: tables.iter().map(|s| (*s).to_owned()).collect(),
                from: ym,
                to: ym,
            };
            let (a, f) = (&a, &f);
            async move { a.list(&q, f).await.unwrap() }
        };
        let files = list(&[], &[]).await;
        let names: Vec<String> = files.iter().map(RemoteFile::file_name).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names.contains(&"SOMETHING_NEW_202608.zip".to_owned()));
        let odd = files
            .iter()
            .find(|r| r.file_name() == "SOMETHING_NEW_202608.zip")
            .unwrap();
        assert_eq!(odd.table, None, "its tables are whatever its I rows say");
        let both: Vec<&RemoteFile> = files
            .iter()
            .filter(|r| r.table.as_deref() == Some("P5MIN_REGIONSOLUTION"))
            .collect();
        assert_eq!(both.len(), 2, "the DATA file and the ALL file");
        assert!(
            both.iter()
                .any(|r| r.url.path().contains("/P5MIN_ALL_DATA/"))
        );
        // By table name: both files of the table, not the odd one.
        assert_eq!(list(&["P5MIN_REGIONSOLUTION"], &[]).await.len(), 2);
        // Leaving a table out, and the odd file by its own name.
        assert_eq!(list(&[], &["P5MIN_REGIONSOLUTION"]).await.len(), 1);
        assert_eq!(list(&[], &["something_new_202608.zip"]).await.len(), 2);
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
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), cfg);
        let q = Query {
            exclude: Vec::new(),
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
                    exclude: Vec::new(),
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
                    exclude: Vec::new(),
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
        // A table asked for beside others that exist is still an error
        // when no month of the range has it.
        let e = a
            .list(
                &Query {
                    exclude: vec![],
                    tables: vec!["DISPATCH_UNIT_SCADA".into(), "NOPE".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(e, FetchError::NoMatch { ref table, ref period, .. } if table == "NOPE" && period == "2024-01"),
            "{e}"
        );
        // Tables left out, with every table and with named ones.
        let without = a
            .list(
                &Query {
                    exclude: vec!["bidperoffer".into(), "PREDISPATCHPRICE".into()],
                    tables: vec![],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        assert!(without.len() < 148 - 2);
        assert!(
            without
                .iter()
                .all(|r| !matches!(r.table.as_deref(), Some("BIDPEROFFER" | "PREDISPATCHPRICE")))
        );
        let named = a
            .list(
                &Query {
                    exclude: vec!["BIDPEROFFER".into()],
                    tables: vec!["DISPATCH_UNIT_SCADA".into(), "BIDPEROFFER".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        // Asked for and left out at once: left out, and not a wrong name.
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
        // A month with no directory is looked up in its year's listing, and
        // a year that lists nothing for it is a missing month.
        let ym2 = YearMonth::new(2024, 2).unwrap();
        let e = a
            .list(
                &Query {
                    exclude: Vec::new(),
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
                    exclude: Vec::new(),
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
                    exclude: Vec::new(),
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
                    exclude: Vec::new(),
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
                    exclude: Vec::new(),
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
            Some("BIDOFFERPERIOD".into())
        );
        assert_eq!(
            MmsdmArchive::parse_name("PUBLIC_ARCHIVE#X#FILE01#202608.ctl"),
            None
        );
        // The cache keeps the bytes under a name without the #.
        t.add(r.url.as_str(), Canned::ok(b"zip bytes"));
        let local = f.fetch(r).await.unwrap();
        // The archive keeps the file under its real name, # and all.
        assert_eq!(
            local.path.file_name().unwrap().to_str().unwrap(),
            "PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202608010000.zip"
        );
        assert_eq!(local.remote.file_name(), r.file_name());
    }

    const YEAR_2012: &str =
        "https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2012/";

    fn year_listing() -> Vec<u8> {
        br#"<pre><A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/">[To Parent Directory]</A><br><br> 2/21/2012  3:10 PM   1912843433 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2012/MMSDM_2012_01.zip">MMSDM_2012_01.zip</A><br> 3/21/2012  3:10 PM   1784225268 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2012/MMSDM_2012_02.zip">MMSDM_2012_02.zip</A><br></pre>"#.to_vec()
    }

    fn quick(t: &Arc<StaticTransport>, dir: &std::path::Path) -> Fetcher {
        Fetcher::new(
            Arc::new(Arc::clone(t)),
            Archive::new(dir),
            FetcherConfig {
                attempts: 2,
                backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(1),
                min_interval: Duration::ZERO,
            },
        )
    }

    /// A directory month lists its control files and its README beside
    /// its data, whatever tables are asked for: the census reads them.
    #[tokio::test]
    async fn a_month_lists_its_control_files_and_readme() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let a = MmsdmArchive::new("https://h").unwrap();
        let ym = YearMonth::new(2024, 1).unwrap();
        let root = a.month_root(ym).unwrap();
        let link = |sub: &str, name: &str| {
            format!(
                r#" 2/13/2024 8:27 AM 12 <A HREF="{}{sub}{name}">{name}</A><br>"#,
                root.path()
            )
        };
        let data = root.join("DATA/").unwrap();
        t.add(
            data.as_str(),
            Canned::ok(
                format!(
                    "<pre>{}</pre>",
                    link("DATA/", "PUBLIC_DVD_REGION_202401010000.zip")
                )
                .as_bytes(),
            ),
        );
        t.add(
            root.join("CTL/").unwrap().as_str(),
            Canned::ok(
                format!(
                    "<pre>{}{}</pre>",
                    link("CTL/", "PUBLIC_DVD_REGION_202401.ctl"),
                    link("CTL/", "notes.htm")
                )
                .as_bytes(),
            ),
        );
        t.add(
            root.join("DOCUMENTATION/").unwrap().as_str(),
            Canned::ok(format!("<pre>{}</pre>", link("DOCUMENTATION/", "README.txt")).as_bytes()),
        );
        let f = quick(&t, dir.path());
        let files = a
            .list(
                &Query {
                    exclude: Vec::new(),
                    tables: vec!["REGION".into()],
                    from: ym,
                    to: ym,
                },
                &f,
            )
            .await
            .unwrap();
        let names: Vec<String> = files.iter().map(RemoteFile::file_name).collect();
        assert_eq!(
            names,
            [
                "PUBLIC_DVD_REGION_202401.ctl",
                "README.txt",
                "PUBLIC_DVD_REGION_202401010000.zip"
            ],
            "{names:?}"
        );
    }

    /// Before 2015 a month is one zip: it is listed as one file, whatever
    /// tables are asked for, and downloaded whole.
    #[tokio::test]
    async fn a_whole_month_zip_is_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let zip_url = format!("{YEAR_2012}MMSDM_2012_01.zip");
        let t = Arc::new(StaticTransport::default());
        t.add(YEAR_2012, Canned::ok(&year_listing()));
        t.add(&zip_url, Canned::ok(b"PK month zip bytes"));
        let f = quick(&t, dir.path());
        let a = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
        let ym = YearMonth::new(2012, 1).unwrap();
        assert_eq!(
            a.month_layout(ym, &f).await.unwrap(),
            MonthLayout::WholeZip {
                url: Url::parse(&zip_url).unwrap(),
            }
        );
        let query = |tables: &[&str]| Query {
            exclude: Vec::new(),
            tables: tables.iter().map(|s| (*s).to_owned()).collect(),
            from: ym,
            to: ym,
        };
        let all = a.list(&query(&[]), &f).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].url.as_str(), zip_url);
        assert_eq!(all[0].table, None);
        assert_eq!(all[0].period, Some(ym));
        assert_eq!(a.list(&query(&["REGION"]), &f).await.unwrap(), all);
        let local = f.fetch(&all[0]).await.unwrap();
        assert_eq!(std::fs::read(&local.path).unwrap(), b"PK month zip bytes");
        // The archive finds it as the month's file.
        let store = Archive::new(dir.path());
        assert_eq!(a.archived_files(&store, ym).unwrap(), [local.path]);
        assert!(
            a.archived_files(&store, YearMonth::new(2012, 2).unwrap())
                .unwrap()
                .is_empty()
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
                exclude: Vec::new(),
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
    }
}
