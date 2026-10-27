//! The month a file's data is for, read from its URL, and the cut-off
//! before which the mirror keeps nothing (`keep_from` in the config, or
//! `--keep-from`).
//!
//! The month comes from, first match wins:
//! - `_YYYY_MM` in the last segment (`MMSDM_2015_01.zip`, `NEMDE_2015_01/`);
//! - the first `_YYYYMMDD…` stamp in it (`PUBLIC_YESTBID_201612220000_…`:
//!   the first stamp is the data's, a later one when it was published);
//! - the deepest `/YYYY/` folder in the path (`MMSDM/2015/…/CRLF.TXT`).
//!
//! A URL with none of these has no month and is always kept.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::crawl::{Mirror, RemoteKind};
use crate::error::MirrorError;
use crate::net::Archive;
use crate::stamp::Stamp;

/// A calendar month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Month {
    /// The year.
    pub year: u16,
    /// 1 to 12.
    pub month: u8,
}

impl Month {
    /// The month, when `year` and `month` are plausible for AEMO's data.
    pub fn new(year: u16, month: u8) -> Option<Self> {
        ((1990..=2100).contains(&year) && (1..=12).contains(&month)).then_some(Self { year, month })
    }
}

impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

/// Not a month in the form `YYYY-MM`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a month: expected YYYY-MM, e.g. 2020-01")]
pub struct BadMonth(pub String);

impl FromStr for Month {
    type Err = BadMonth;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || BadMonth(s.to_owned());
        let (y, m) = s.split_once('-').ok_or_else(bad)?;
        if y.len() != 4 || m.len() != 2 {
            return Err(bad());
        }
        Month::new(y.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?).ok_or_else(bad)
    }
}

impl Serialize for Month {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Month {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The month `url`'s data is for, if its URL says.
pub fn data_month(url: &str) -> Option<Month> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = path.trim_end_matches('/');
    let name = path.rsplit('/').next().unwrap_or(path);
    in_name(name).or_else(|| {
        path.split('/')
            .filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()))
            .filter_map(|s| Month::new(s.parse().ok()?, 1))
            .next_back()
    })
}

/// `_YYYY_MM`, or the first `_YYYYMMDD` stamp, in a file or folder name.
fn in_name(name: &str) -> Option<Month> {
    let b = name.as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let num = |from: usize, len: usize| name[from..from + len].parse::<u16>().ok();
    for (i, _) in name.match_indices('_') {
        let at = i + 1;
        let run = digits(at);
        let found = if run == 4 && b.get(at + 4) == Some(&b'_') && digits(at + 5) == 2 {
            Month::new(num(at, 4)?, num(at + 5, 2)? as u8)
        } else if run >= 8 {
            let day = num(at + 6, 2)?;
            Month::new(num(at, 4)?, num(at + 4, 2)? as u8).filter(|_| (1..=31).contains(&day))
        } else {
            None
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Whether the mirror keeps `url` when it keeps nothing before `from`.
pub fn keeps(url: &str, from: Option<Month>) -> bool {
    match (from, data_month(url)) {
        (Some(from), Some(month)) => month >= from,
        _ => true,
    }
}

/// What `prune` deleted, or with `apply` false would delete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PruneSummary {
    /// Nothing for a month before this is kept.
    pub before: Option<Month>,
    /// Whether anything was deleted: false for a dry run.
    pub applied: bool,
    /// Folders deleted whole: every file listed under them is before.
    pub folders: Vec<String>,
    /// Files listed before the month, in those folders or alone.
    pub files: usize,
    /// Their bytes as listed.
    pub listed_bytes: u64,
    /// Of those, files a download pass recorded held.
    pub held_files: usize,
    /// Their bytes on disk.
    pub held_bytes: u64,
}

/// One line of `pruned.ndjson`.
#[derive(Serialize)]
struct Pruned<'a> {
    url: &'a str,
    folder: bool,
    files: usize,
    listed_bytes: u64,
    before: Month,
    pruned_at: Stamp,
}

/// Deletes from `archive` every file the record lists for a month before
/// `mirror`'s [`Mirror::keep_from`], with its record, versions and
/// AppleDouble twins; with `apply` false only counts them.
///
/// A folder every file under which is before goes whole, one removal
/// rather than one per file (each costs seconds deep in a large folder on
/// exFAT); a file before in a folder that keeps others goes alone, its
/// folder read once. Each deletion is a line of `pruned.ndjson`. The
/// listings stay: the files are still nemweb's, just not kept.
pub fn prune(mirror: &Mirror, archive: &Archive, apply: bool) -> Result<PruneSummary, MirrorError> {
    let from = mirror.keep_from().ok_or(MirrorError::NoKeepFrom)?;
    let record = mirror.record()?;
    let held = mirror.held()?;
    let before = |url: &str| !keeps(url, Some(from));
    // A folder goes whole when it is before and nothing kept is under it.
    let kept_under = |dir: &str| {
        record
            .iter()
            .any(|r| r.url.starts_with(dir) && !before(&r.url))
    };
    let mut whole: Vec<&str> = record
        .iter()
        .filter(|r| r.kind == RemoteKind::Dir && before(&r.url) && !kept_under(&r.url))
        .map(|r| r.url.as_str())
        .collect();
    // Only the topmost: record is in URL order, so a folder's are after it.
    whole.dedup_by(|inner, outer| inner.starts_with(*outer));
    let mut summary = PruneSummary {
        before: Some(from),
        applied: apply,
        folders: whole.iter().map(|d| (*d).to_owned()).collect(),
        ..PruneSummary::default()
    };
    let mut lines: Vec<Pruned<'_>> = Vec::new();
    let mut per_folder: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut tally: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    for r in record
        .iter()
        .filter(|r| r.kind == RemoteKind::File && before(&r.url))
    {
        let size = r.size.unwrap_or(0);
        summary.files += 1;
        summary.listed_bytes += size;
        if let Some(h) = held.get(&r.url) {
            summary.held_files += 1;
            summary.held_bytes += h.size;
        }
        match whole.iter().find(|d| r.url.starts_with(**d)) {
            Some(d) => {
                let t = tally.entry(d).or_default();
                t.0 += 1;
                t.1 += size;
            }
            None => {
                // A file's path is under the archive, so has both.
                let path = archive.path_for(&r.url);
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                per_folder
                    .entry(path.parent().unwrap_or(archive.root()).to_owned())
                    .or_default()
                    .insert(name.into_owned());
                lines.push(Pruned {
                    url: &r.url,
                    folder: false,
                    files: 1,
                    listed_bytes: size,
                    before: from,
                    pruned_at: Stamp::now(),
                });
            }
        }
    }
    if !apply {
        return Ok(summary);
    }
    for d in &whole {
        let (files, listed_bytes) = tally.get(d).copied().unwrap_or_default();
        lines.push(Pruned {
            url: d,
            folder: true,
            files,
            listed_bytes,
            before: from,
            pruned_at: Stamp::now(),
        });
    }
    // The register first, so a cut-off prune leaves a note of what it
    // set out to delete.
    let register = mirror.pruned_path();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&register)
        .map_err(MirrorError::io(&register))?;
    for line in &lines {
        let mut text = serde_json::to_vec(line).expect("a Pruned always serialises");
        text.push(b'\n');
        f.write_all(&text).map_err(MirrorError::io(&register))?;
    }
    f.sync_all().map_err(MirrorError::io(&register))?;
    for d in &whole {
        let path = archive.path_for(d);
        remove_dir_retrying(&path, |p| std::fs::remove_dir_all(p))?;
    }
    for (dir, names) in &per_folder {
        remove_named(dir, names)?;
    }
    Ok(summary)
}

/// Removes the folder at `path` with `remove`, again while it is left
/// not empty: on exFAT under FSKit a folder read while its entries are
/// deleted passes some over, so each pass leaves fewer. A folder already
/// gone is fine.
fn remove_dir_retrying(
    path: &Path,
    mut remove: impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<(), MirrorError> {
    const PASSES: usize = 50;
    let mut pass = 1;
    loop {
        match remove(path) {
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty && pass < PASSES => {
                pass += 1;
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(MirrorError::io(path)(e));
            }
            _ => return Ok(()),
        }
    }
}

/// Removes from `dir` each of `names` with what sits beside it: its
/// record, versions (`{name}@…`), part file, and the AppleDouble twin of
/// each. The folder is read once.
fn remove_named(dir: &Path, names: &BTreeSet<String>) -> Result<(), MirrorError> {
    // Every name first, then the deletions: a folder read while its
    // entries go may pass some over.
    let entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(dir) {
        Ok(e) => e.collect::<Result<_, _>>().map_err(MirrorError::io(dir))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(MirrorError::io(dir)(e)),
    };
    let belongs = |entry: &str| {
        let base = entry.strip_prefix("._").unwrap_or(entry);
        names.iter().any(|n| {
            base.strip_prefix(n.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '@']))
        })
    };
    for entry in entries {
        let name = entry.file_name();
        if !belongs(&name.to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(MirrorError::io(&path)(e));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Month {
        s.parse().unwrap()
    }

    #[test]
    fn a_month_is_read_from_the_name_then_the_year_folder() {
        let base = "https://h/Data_Archive/Wholesale_Electricity";
        let cases = [
            ("MMSDM/2015/MMSDM_2015_01.zip", Some("2015-01")),
            ("NEMDE/2015/NEMDE_2015_02/", Some("2015-02")),
            (
                "FCAS_Causer_Pays/2015/FCAS_Causer_Pays_2015_03.zip",
                Some("2015-03"),
            ),
            (
                "MMSDM/2019/MMSDM_2019_06/MMSDM_Historical_Data_SQLLoader/UTILITIES/CRLF.TXT",
                Some("2019-01"),
            ),
            ("MMSDM/2015/", Some("2015-01")),
            ("MMSDM/", None),
            ("MMSDM/MTPASA_DATA_EXPORT/", None),
        ];
        for (path, want) in cases {
            assert_eq!(data_month(&format!("{base}/{path}")), want.map(m), "{path}");
        }
        let reports = [
            (
                "https://h/Reports/CURRENT/Yesterdays_Bids_Reports/PUBLIC_YESTBID_201612220000_20161223040519.zip",
                Some("2016-12"),
            ),
            (
                "https://h/Reports/CURRENT/WDR_CAPACITY_NO_SCADA/PUBLIC_WDR_CAPACITY_NO_SCADA_20261024_0000000544726949.zip",
                Some("2026-10"),
            ),
            // An id of zeros is not a date, nor a month 13 or a day 32.
            ("https://h/R/PUBLIC_X_0000000544726949.zip", None),
            ("https://h/R/PUBLIC_X_20261301.zip", None),
            ("https://h/R/PUBLIC_X_20261032.zip", None),
            ("https://h/R/PUBLIC_X_2026_13.zip", None),
            ("https://h/R/X_2026.zip", None),
            ("https://h/R/Weekly_Constraint_Reports/2026.zip", None),
            ("https://h/R/Current_CCR.xlsx", None),
            ("https://h/R/a.zip?x=1#y", None),
            ("https://h/R/A_2019_04.zip?x=1", Some("2019-04")),
        ];
        for (url, want) in reports {
            assert_eq!(data_month(url), want.map(m), "{url}");
        }
    }

    #[test]
    fn a_cut_off_keeps_its_month_on_and_every_undated_file() {
        let from = Some(m("2020-01"));
        assert!(!keeps("https://h/A/2019/MMSDM_2019_12.zip", from));
        assert!(keeps("https://h/A/2020/MMSDM_2020_01.zip", from));
        assert!(keeps("https://h/A/2026/x.csv", from));
        assert!(keeps("https://h/A/logo.gif", from));
        assert!(keeps("https://h/A/2015/MMSDM_2015_01.zip", None));
    }

    #[test]
    fn a_month_reads_and_writes_as_yyyy_mm() {
        assert_eq!(
            m("2020-01"),
            Month {
                year: 2020,
                month: 1
            }
        );
        assert_eq!(m("2020-01").to_string(), "2020-01");
        for bad in [
            "2020", "2020-1", "20-01", "2020-13", "1989-01", "x-01", "2020-xx",
        ] {
            let e = bad.parse::<Month>().unwrap_err();
            assert!(e.to_string().contains("YYYY-MM"), "{bad}: {e}");
        }
        let json = serde_json::to_string(&m("2021-07")).unwrap();
        assert_eq!(json, "\"2021-07\"");
        assert_eq!(serde_json::from_str::<Month>(&json).unwrap(), m("2021-07"));
        assert!(serde_json::from_str::<Month>("\"2021\"").is_err());
    }

    fn remote(url: &str, size: Option<u64>) -> crate::crawl::Remote {
        let kind = if url.ends_with('/') {
            RemoteKind::Dir
        } else {
            RemoteKind::File
        };
        let trimmed = url.trim_end_matches('/');
        let listed_on = format!("{}/", &trimmed[..trimmed.rfind('/').unwrap()]);
        crate::crawl::Remote {
            url: url.to_owned(),
            kind,
            size,
            modified: "m".to_owned(),
            modified_at: None,
            listed_on,
            crawled_at: None,
        }
    }

    fn put(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn prune_deletes_whole_folders_before_the_month_and_single_files_in_kept_ones() {
        let home = tempfile::tempdir().unwrap();
        let archive = Archive::new(home.path().join("archive"));
        let all = Mirror::new(home.path().join("state"));
        let urls = [
            ("https://h/A/", None),
            ("https://h/A/2015/", None),
            ("https://h/A/2015/M_2015_01.zip", Some(10)),
            ("https://h/A/2015/M_2015_01/", None),
            ("https://h/A/2015/M_2015_01/x.csv", Some(3)),
            ("https://h/A/2016/", None),
            ("https://h/A/2016/M_2016_01.zip", Some(5)),
            // December's folder holds a file stamped for January after it.
            ("https://h/A/2019/", None),
            ("https://h/A/2019/P_201912010000.zip", Some(7)),
            ("https://h/A/2019/P_202001010000.zip", Some(8)),
            ("https://h/A/logo.gif", Some(1)),
            ("https://h/R/", None),
            ("https://h/R/PUBLIC_X_20191231_1.zip", Some(2)),
            ("https://h/R/PUBLIC_X_20200101_2.zip", Some(4)),
        ];
        all.write_record(urls.iter().map(|(u, s)| remote(u, *s)))
            .unwrap();
        for (url, size) in urls.iter().filter(|(_, s)| s.is_some()) {
            let path = archive.path_for(url);
            put(&path, &vec![0; size.unwrap() as usize]);
            put(&Archive::record_path(&path), b"{}");
        }
        // Beside a pruned file: a version, its record, a part file and an
        // AppleDouble twin; beside a kept one, a name that only starts alike.
        let p = archive.path_for("https://h/R/PUBLIC_X_20191231_1.zip");
        let r = p.parent().unwrap();
        for name in [
            "PUBLIC_X_20191231_1.zip@20200101T000000+1000",
            "PUBLIC_X_20191231_1.zip@20200101T000000+1000.provenance.json",
            "PUBLIC_X_20191231_1.zip.part",
            "._PUBLIC_X_20191231_1.zip",
            "PUBLIC_X_20191231_10.zip",
        ] {
            put(&r.join(name), b"v");
        }
        let held = all.held_path();
        put(
            &held,
            b"{\"url\":\"https://h/A/2015/M_2015_01.zip\",\"size\":10}\n",
        );

        // With no month to keep from there is nothing to prune by.
        assert!(matches!(
            prune(&all, &archive, false).unwrap_err(),
            MirrorError::NoKeepFrom
        ));

        let mirror = all.clone().keeping(Some(m("2020-01")));
        let kept: Vec<String> = mirror.kept().unwrap().into_iter().map(|r| r.url).collect();
        assert_eq!(
            kept,
            [
                "https://h/A/",
                "https://h/A/2019/P_202001010000.zip",
                "https://h/A/logo.gif",
                "https://h/R/",
                "https://h/R/PUBLIC_X_20200101_2.zip",
            ]
        );

        let dry = prune(&mirror, &archive, false).unwrap();
        let want = PruneSummary {
            before: Some(m("2020-01")),
            applied: false,
            folders: vec!["https://h/A/2015/".into(), "https://h/A/2016/".into()],
            files: 5,
            listed_bytes: 10 + 3 + 5 + 7 + 2,
            held_files: 1,
            held_bytes: 10,
        };
        assert_eq!(dry, want);
        assert!(archive.path_for("https://h/A/2015/M_2015_01.zip").exists());
        assert!(!mirror.pruned_path().exists());

        let done = prune(&mirror, &archive, true).unwrap();
        assert_eq!(
            done,
            PruneSummary {
                applied: true,
                ..want
            }
        );
        let gone = |u: &str| !archive.path_for(u).exists();
        assert!(gone("https://h/A/2015/") && gone("https://h/A/2016/"));
        assert!(gone("https://h/A/2019/P_201912010000.zip"));
        assert!(
            !Archive::record_path(&archive.path_for("https://h/A/2019/P_201912010000.zip"))
                .exists()
        );
        for u in [
            "https://h/A/2019/P_202001010000.zip",
            "https://h/A/logo.gif",
            "https://h/R/PUBLIC_X_20200101_2.zip",
        ] {
            assert!(!gone(u), "{u}");
        }
        let mut left: Vec<String> = std::fs::read_dir(r)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "PUBLIC_X_20191231_10.zip",
                "PUBLIC_X_20200101_2.zip",
                "PUBLIC_X_20200101_2.zip.provenance.json",
            ]
        );
        let register = std::fs::read_to_string(mirror.pruned_path()).unwrap();
        let lines: Vec<serde_json::Value> = register
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        let folder_line = lines
            .iter()
            .find(|l| l["url"] == "https://h/A/2015/")
            .unwrap();
        assert_eq!(folder_line["folder"], true);
        assert_eq!(folder_line["files"], 2);
        assert_eq!(folder_line["listed_bytes"], 13);
        assert_eq!(folder_line["before"], "2020-01");
        crate::stamp::check::assert_aware(folder_line);
        // The listings stay: nemweb still lists the files.
        assert_eq!(all.record().unwrap().len(), urls.len());

        // Again: nothing left to delete, and folders already gone are fine.
        let again = prune(&mirror, &archive, true).unwrap();
        assert_eq!(again.files, 5);
        assert!(remove_named(&home.path().join("none"), &BTreeSet::new()).is_ok());
    }

    #[test]
    fn a_folder_left_not_empty_is_removed_again_until_it_goes() {
        let not_empty = || std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty);
        let path = Path::new("/nowhere");
        let mut tries = 0;
        remove_dir_retrying(path, |_: &Path| {
            tries += 1;
            if tries < 3 { Err(not_empty()) } else { Ok(()) }
        })
        .unwrap();
        assert_eq!(tries, 3);
        let mut tries = 0;
        let e = remove_dir_retrying(path, |_: &Path| {
            tries += 1;
            Err(not_empty())
        })
        .unwrap_err();
        assert!(matches!(e, MirrorError::Io { .. }));
        assert_eq!(tries, 50);
    }

    #[test]
    fn prune_reports_a_folder_or_file_it_cannot_delete() {
        let home = tempfile::tempdir().unwrap();
        let archive = Archive::new(home.path().join("archive"));
        let mirror = Mirror::new(home.path().join("state")).keeping(Some(m("2020-01")));
        mirror
            .write_record([
                remote("https://h/A/", None),
                remote("https://h/A/2015/", None),
                remote("https://h/A/2015/a.csv", Some(1)),
                remote("https://h/A/b_20190101.csv", Some(1)),
                remote("https://h/A/c_20200101.csv", Some(1)),
            ])
            .unwrap();
        // A file where its folder should be.
        put(&archive.path_for("https://h/A/c_20200101.csv"), b"x");
        let path = archive.path_for("https://h/A/2015/");
        std::fs::write(&path, b"not a folder").unwrap();
        assert!(matches!(
            prune(&mirror, &archive, true).unwrap_err(),
            MirrorError::Io { .. }
        ));
        std::fs::remove_file(&path).unwrap();

        // A folder where a file of that name sits beside a pruned one.
        let b = archive.path_for("https://h/A/b_20190101.csv");
        std::fs::create_dir_all(b.with_file_name("b_20190101.csv@x")).unwrap();
        std::fs::write(b.with_file_name("b_20190101.csv@x").join("f"), b"").unwrap();
        assert!(matches!(
            prune(&mirror, &archive, true).unwrap_err(),
            MirrorError::Io { .. }
        ));

        // A folder that is a file cannot be read for its names.
        let file = home.path().join("plain");
        std::fs::write(&file, b"").unwrap();
        let names = BTreeSet::from(["x".to_owned()]);
        assert!(matches!(
            remove_named(&file, &names).unwrap_err(),
            MirrorError::Io { .. }
        ));
    }
}
