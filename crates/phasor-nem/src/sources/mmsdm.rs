//! The monthly NEM archive on nemweb. Directories are listed rather than URLs
//! guessed, because split tables and renamed files only show up in the listing.
//! From 2015 each month is a directory of per-table zips, plus `CTL/` and
//! `DOCUMENTATION/`:
//!
//! ```text
//! {base}/Data_Archive/Wholesale_Electricity/MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}/MMSDM_Historical_Data_SQLLoader/DATA/
//!     PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip     one per table; big tables split: BIDPEROFFER1, BIDPEROFFER2
//! ```
//!
//! July 2009 to December 2014 is one zip per month of 1.4 to 2.1 GB, read by its
//! directory or whole. Where a month has both, the directory is used.
//!
//! ```text
//! {base}/Data_Archive/Wholesale_Electricity/MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}.zip
//!     {MM}/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use phasor_core::YearMonth;
use url::Url;

use phasor_acquire::{Archive, FetchError, Held, listing};
use phasor_kit::{Entry, Listings, LocalFile, Pick, Picks, Query, RemoteFile, Source};

/// The source id.
pub const SOURCE_ID: &str = "nem-mmsdm";

/// The first year published as directories of per-table zips.
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
    /// Only one whole-month zip (`MMSDM_2012_01.zip`) holding the per-table zips, control files and README.
    WholeZip {
        /// The zip.
        url: Url,
    },
}

/// The files of one month: per-table data files or its whole zip, plus control files and README.
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
        let base = Url::parse(base).map_err(FetchError::parse(base, "base URL"))?;
        Ok(Self { base })
    }

    /// The year directory, e.g. `.../MMSDM/2012/`.
    pub fn year_root(&self, year: i32) -> Result<Url, FetchError> {
        let path = format!("/Data_Archive/Wholesale_Electricity/MMSDM/{year}/");
        self.base
            .join(&path)
            .map_err(FetchError::parse(path, "year URL"))
    }

    /// The month directory (`.../MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/`), from 2015 on.
    pub fn month_root(&self, ym: YearMonth) -> Result<Url, FetchError> {
        let path = format!(
            "/Data_Archive/Wholesale_Electricity/MMSDM/{}/MMSDM_{}_{:02}/MMSDM_Historical_Data_SQLLoader/",
            ym.year(),
            ym.year(),
            ym.month()
        );
        self.base
            .join(&path)
            .map_err(FetchError::parse(path, "month URL"))
    }

    /// Finds how a month is published by reading its year's listing.
    pub async fn month_layout(
        &self,
        ym: YearMonth,
        fetcher: &dyn Listings,
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
            let url = year_url
                .join(&e.href)
                .map_err(FetchError::parse(&e.href, "listing href"))?;
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
        fetcher: &dyn Listings,
    ) -> Result<MonthFiles, FetchError> {
        // From 2015 try the month directory first, falling back to the year listing.
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
                    period: Some(ym.into()),
                    size: None,
                    modified: None,
                    modified_at: None,
                }),
                support: Vec::new(),
            }),
        }
    }

    /// Every archived file of `month`, sorted, from its directory or whole zip;
    /// sidecars and partial downloads left out.
    pub fn archived_files(
        &self,
        archive: &Archive,
        month: YearMonth,
    ) -> Result<Vec<PathBuf>, FetchError> {
        let dir = archive.path_for(self.month_root(month)?.as_str());
        let whole_url = self
            .year_root(month.year())?
            .join(&format!("MMSDM_{}_{:02}.zip", month.year(), month.month()))
            .map_err(FetchError::parse(month.to_string(), "month zip URL"))?;
        let mut out: Vec<PathBuf> = phasor_acquire::archive::entries_under(archive, Some(&dir))
            .into_iter()
            .map(|e| e.path)
            .collect();
        // Entries fetched alone out of a whole-month zip sit beside it: `MMSDM_2012_01.zip!01/…`.
        let whole_path = archive.path_for(whole_url.as_str());
        if let (Some(year_dir), Some(zip_name)) = (
            whole_path.parent(),
            whole_path.file_name().and_then(|n| n.to_str()),
        ) {
            let prefix = format!("{zip_name}!");
            let mut picked: Vec<PathBuf> = std::fs::read_dir(year_dir)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with(&prefix))
                })
                .map(|e| e.path())
                .collect();
            picked.sort();
            for d in picked {
                out.extend(
                    phasor_acquire::archive::entries_under(archive, Some(&d))
                        .into_iter()
                        .map(|e| e.path),
                );
            }
        }
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
        fetcher: &dyn Listings,
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
                // A zip named by neither scheme is still listed; its I rows say its tables.
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
                    .map_err(FetchError::parse(&e.href, "listing href"))?;
                out.push((
                    table.clone(),
                    RemoteFile {
                        url,
                        source: SOURCE_ID,
                        table: named.then(|| table.clone()),
                        period: Some(ym.into()),
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
                let url = dir
                    .join(&e.href)
                    .map_err(FetchError::parse(&e.href, "listing href"))?;
                support.push(RemoteFile {
                    url,
                    source: SOURCE_ID,
                    table: None,
                    period: Some(ym.into()),
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

    /// The table a data zip carries under either naming scheme ([`crate::names`]):
    /// `PUBLIC_DVD_BIDPEROFFER1_202401010000.zip` and
    /// `PUBLIC_ARCHIVE#BIDPEROFFER#FILE01#202608010000.zip` are both `BIDPEROFFER`.
    pub fn parse_name(name: &str) -> Option<String> {
        if !name.to_ascii_lowercase().ends_with(".zip") {
            return None;
        }
        let parsed = crate::names::parse(name)?;
        if parsed.file.is_some() {
            return Some(parsed.table);
        }
        Some(parsed.split_table().0.to_owned())
    }
}

/// The archive month a data file's name says, for either naming scheme.
pub fn period_of_name(name: &str) -> Option<YearMonth> {
    crate::names::parse(name).map(|n| n.month)
}

/// The table and month a data file's name says; `None` when it follows neither scheme.
pub fn describe_name(url: Url) -> Option<RemoteFile> {
    // An entry fetched alone is named `{zip}!{entry}`: the entry's name says its table and month.
    let name = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())?
        .rsplit('!')
        .next()?
        .replace("%23", "#");
    let lower = name.to_ascii_lowercase();
    if !(lower.ends_with(".zip") || lower.ends_with(".csv")) {
        return None;
    }
    let parsed = crate::names::parse(&name)?;
    let table = if parsed.file.is_some() {
        parsed.table.clone()
    } else {
        parsed.split_table().0.to_owned()
    };
    Some(RemoteFile {
        url,
        source: SOURCE_ID,
        table: Some(table),
        period: Some(parsed.month.into()),
        size: None,
        modified: None,
        modified_at: None,
    })
}

#[async_trait]
impl Source for MmsdmArchive {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    /// The archived data files of the query's months, from their sidecars. A table
    /// filter applies to one-table files; a whole-month zip is always kept.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        let mut out = Vec::new();
        for month in query.range.months_iter() {
            for path in self.archived_files(archive, month)? {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let lower = name.to_ascii_lowercase();
                if !(lower.ends_with(".zip") || lower.ends_with(".csv")) {
                    continue;
                }
                let table = Self::parse_name(&name);
                if let Some(t) = &table {
                    let named = |names: &[String]| names.iter().any(|n| n.eq_ignore_ascii_case(t));
                    if (!query.tables.is_empty() && !named(&query.tables)) || named(&query.exclude)
                    {
                        continue;
                    }
                }
                let Some(provenance) = Archive::read(&path)? else {
                    continue;
                };
                let Ok(url) = Url::parse(provenance.url()) else {
                    continue;
                };
                let remote = RemoteFile {
                    url,
                    source: SOURCE_ID,
                    table,
                    period: Some(month.into()),
                    size: Some(provenance.content.size),
                    modified: None,
                    modified_at: provenance.listed(provenance.url()).and_then(|l| l.at),
                };
                out.push(LocalFile::from_held(
                    &remote,
                    Held { path, provenance },
                    true,
                ));
            }
        }
        Ok(out)
    }

    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        describe_name(url.clone())
    }

    /// A whole-month zip is read by its directory when tables are named.
    fn wants_ranged(&self, remote: &RemoteFile, query: &Query) -> bool {
        !query.tables.is_empty()
            && remote.table.is_none()
            && remote
                .url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .is_some_and(|n| {
                    n.starts_with("MMSDM_") && n.to_ascii_lowercase().ends_with(".zip")
                })
    }

    /// The nested zips of the tables asked for; the whole zip when none are, or one has no zip of its own.
    fn pick(&self, remote: &RemoteFile, entries: &[Entry], query: &Query) -> Picks {
        if query.tables.is_empty() {
            return Picks::Whole;
        }
        let wanted = |t: &str| {
            query.tables.iter().any(|w| w.eq_ignore_ascii_case(t))
                && !query.exclude.iter().any(|x| x.eq_ignore_ascii_case(t))
        };
        let picks: Vec<Pick> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_dir())
            .filter_map(|(index, e)| {
                let name = e.name.rsplit('/').next().unwrap_or(&e.name);
                let parsed = crate::names::parse(name)?;
                let table = if parsed.file.is_some() {
                    parsed.table.clone()
                } else {
                    parsed.split_table().0.to_owned()
                };
                if !wanted(&table) || !name.to_ascii_lowercase().ends_with(".zip") {
                    return None;
                }
                // A `#` in the newer names would start a URL fragment.
                let url =
                    Url::parse(&format!("{}!{}", remote.url, e.name.replace('#', "%23"))).ok()?;
                let mut remote_entry = describe_name(url)?;
                remote_entry.size = Some(e.size);
                remote_entry.modified = remote.modified.clone();
                remote_entry.modified_at = remote.modified_at;
                Some(Pick {
                    index,
                    remote: remote_entry,
                })
            })
            .collect();
        if picks.is_empty() {
            return Picks::Whole;
        }
        Picks::Entries(picks)
    }

    async fn list(
        &self,
        query: &Query,
        fetcher: &dyn Listings,
    ) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        let upper = |names: &[String]| -> Vec<String> {
            names.iter().map(|t| t.to_ascii_uppercase()).collect()
        };
        let wanted = upper(&query.tables);
        let excluded = upper(&query.exclude);
        // Tables come and go, so a named table is an error only when no month of the range has it.
        let mut found = vec![false; wanted.len()];
        let mut last_listing: Option<(String, Vec<String>)> = None;
        for ym in query.range.months_iter() {
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
                period: query.range.to_string(),
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
    use phasor_acquire::Fetcher;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use phasor_acquire::FetcherConfig;
    use phasor_acquire::transport::{Canned, StaticTransport};
    use phasor_core::TimeRange;

    #[test]
    fn a_name_says_its_table_and_month() {
        let ym = YearMonth::new(2024, 1).unwrap();
        assert_eq!(
            period_of_name("PUBLIC_DVD_REGION_202401010000.zip"),
            Some(ym)
        );
        assert_eq!(period_of_name("REGION_v1_head.csv"), None);
        assert_eq!(
            period_of_name("PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202608010000.zip"),
            YearMonth::new(2026, 8)
        );
        assert_eq!(period_of_name("PUBLIC_DVD_REGION_2024010100001.zip"), None);
        let r = describe_name(
            Url::parse("https://h/d/PUBLIC_DVD_BIDPEROFFER1_202401010000.zip").unwrap(),
        )
        .unwrap();
        assert_eq!(
            (r.table.as_deref(), r.period),
            (Some("BIDPEROFFER"), Some(ym.into()))
        );
        assert_eq!(r.source, SOURCE_ID);
        let r = describe_name(
            Url::parse(
                "https://h/PUBLIC_ARCHIVE%23P5MIN_UNITSOLUTION%23ALL%23FILE02%23202401010000.CSV",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(r.table.as_deref(), Some("P5MIN_UNITSOLUTION"));
        for bad in [
            "https://h/README.txt",
            "https://h/PUBLIC_DVD_REGION_202401010000.ctl",
            "https://h/",
            "https://h/other.zip",
        ] {
            assert!(describe_name(Url::parse(bad).unwrap()).is_none(), "{bad}");
        }
        let a = MmsdmArchive::new("https://h").unwrap();
        assert_eq!(a.id(), SOURCE_ID);
        assert!(
            a.describe(&Url::parse("https://h/PUBLIC_DVD_REGION_202401010000.zip").unwrap())
                .is_some()
        );
    }

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

    /// Newer-scheme `ALL` directories and zips named by neither scheme are both listed.
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
                range: TimeRange::months(ym, ym),
                instants: None,
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
            range: TimeRange::months(ym, ym),
            instants: None,
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
        assert_eq!(files[2].period, Some(ym.into()));
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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
                    range: TimeRange::months(ym2, ym2),
                    instants: None,
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
                    range: TimeRange::months(ym3, ym3),
                    instants: None,
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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

    /// A directory month lists its control files and README whatever tables are asked for.
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
                    range: TimeRange::months(ym, ym),
                    instants: None,
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

    /// Before 2015 a month is one zip, listed as one file and downloaded whole.
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
            range: TimeRange::months(ym, ym),
            instants: None,
        };
        let all = a.list(&query(&[]), &f).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].url.as_str(), zip_url);
        assert_eq!(all[0].table, None);
        assert_eq!(all[0].period, Some(ym.into()));
        assert_eq!(a.list(&query(&["REGION"]), &f).await.unwrap(), all);
        let local = f.fetch(&all[0]).await.unwrap();
        assert_eq!(std::fs::read(&local.path).unwrap(), b"PK month zip bytes");
        // The archive finds it as the month's file.
        let store = Archive::new(dir.path());
        assert_eq!(
            a.archived_files(&store, ym).unwrap(),
            std::slice::from_ref(&local.path)
        );
        // An entry fetched alone out of the month's zip is found too.
        let entry_url = format!(
            "{zip_url}!01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_201201010000.zip"
        );
        let entry = store.path_for(&entry_url);
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, b"PK entry").unwrap();
        let record = phasor_acquire::Provenance::new(
            phasor_acquire::Content::of(b"PK entry"),
            "test",
            phasor_acquire::Address {
                url: entry_url.clone(),
                seen: phasor_core::timetext::Stamp::new(1_792_454_400, 0),
                gone: None,
                via: phasor_acquire::Via::Fetched {
                    listed: None,
                    served: None,
                },
            },
        );
        Archive::write(&entry, &record).unwrap();
        assert_eq!(
            a.archived_files(&store, ym).unwrap(),
            [local.path.clone(), entry.clone()]
        );
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
                range: TimeRange::months(ym, ym),
                instants: None,
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

    #[test]
    fn an_entry_fetched_alone_is_described_by_its_own_name() {
        let r = describe_name(
            Url::parse("https://h/A/MMSDM_2014_01.zip!PUBLIC_DVD_REGION_201401010000.zip").unwrap(),
        )
        .unwrap();
        assert_eq!(r.table.as_deref(), Some("REGION"));
        assert_eq!(r.period, Some(YearMonth::new(2014, 1).unwrap().into()));
        assert!(
            describe_name(Url::parse("https://h/A/MMSDM_2014_01.zip!notes.txt").unwrap()).is_none()
        );
    }

    #[test]
    fn a_whole_month_zip_is_read_by_its_directory_for_the_tables_asked() {
        let a = MmsdmArchive::new("https://h").unwrap();
        let whole = RemoteFile {
            url: Url::parse(
                "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2014/MMSDM_2014_01.zip",
            )
            .unwrap(),
            source: SOURCE_ID,
            table: None,
            period: Some(YearMonth::new(2014, 1).unwrap().into()),
            size: None,
            modified: Some("m".into()),
            modified_at: None,
        };
        let ym = YearMonth::new(2014, 1).unwrap();
        let q = |tables: &[&str]| Query {
            tables: tables.iter().map(|t| (*t).to_owned()).collect(),
            exclude: Vec::new(),
            range: TimeRange::months(ym, ym),
            instants: None,
        };
        assert!(a.wants_ranged(&whole, &q(&["REGION"])));
        // Every table: the whole zip, its directory unread.
        assert!(!a.wants_ranged(&whole, &q(&[])));
        let per_table = RemoteFile {
            url: Url::parse("https://h/x/DATA/PUBLIC_DVD_REGION_201401010000.zip").unwrap(),
            table: Some("REGION".into()),
            ..whole.clone()
        };
        assert!(!a.wants_ranged(&per_table, &q(&["REGION"])));
        let entry = |name: &str| Entry {
            name: name.into(),
            size: 10,
            compressed: 8,
            crc32: 0,
            method: 0,
            dos_date: 0,
            dos_time: 0,
            local_header_offset: 0,
        };
        let entries = [
            entry(
                "MMSDM_2014_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_201401010000.zip",
            ),
            entry(
                "MMSDM_2014_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_DISPATCHPRICE_201401010000.zip",
            ),
            entry(
                "MMSDM_2014_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_BIDPEROFFER1_201401010000.zip",
            ),
            entry("MMSDM_2014_01/MMSDM_Historical_Data_SQLLoader/DATA/"),
            entry(
                "MMSDM_2014_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_201401010000.CSV",
            ),
            entry("MMSDM_2014_01/README.txt"),
            entry(
                "MMSDM_2014_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_ARCHIVE#DUDETAIL#FILE01#201401010000.zip",
            ),
        ];
        let q = |tables: &[&str], exclude: &[&str]| Query {
            tables: tables.iter().map(|t| (*t).to_owned()).collect(),
            exclude: exclude.iter().map(|t| (*t).to_owned()).collect(),
            range: TimeRange::months(
                YearMonth::new(2014, 1).unwrap(),
                YearMonth::new(2014, 1).unwrap(),
            ),
            instants: None,
        };
        assert_eq!(
            a.pick(&whole, &entries, &q(&[], &[])),
            Picks::Whole,
            "every table"
        );
        let Picks::Entries(p) = a.pick(&whole, &entries, &q(&["region", "BIDPEROFFER"], &[]))
        else {
            panic!("entries");
        };
        assert_eq!(
            p.iter().map(|p| p.index).collect::<Vec<_>>(),
            [0, 2],
            "zips only, the split digit ignored"
        );
        assert_eq!(p[0].remote.table.as_deref(), Some("REGION"));
        assert_eq!(p[0].remote.period, whole.period);
        assert_eq!(
            (p[0].remote.size, p[0].remote.modified.as_deref()),
            (Some(10), Some("m"))
        );
        assert!(p[0].remote.url.as_str().ends_with(".zip!MMSDM_2014_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_201401010000.zip"));
        let Picks::Entries(p) = a.pick(&whole, &entries, &q(&["DUDETAIL"], &[])) else {
            panic!("entries");
        };
        assert_eq!(p[0].index, 6, "the hash scheme names its table too");
        assert_eq!(
            a.pick(&whole, &entries, &q(&["REGION"], &["REGION"])),
            Picks::Whole,
            "nothing left to pick"
        );
        assert_eq!(
            a.pick(&whole, &entries, &q(&["NOPE"], &[])),
            Picks::Whole,
            "a table the zip lacks"
        );
    }
}
