//! The census of AEMO's report folders: every block of every archived file
//! under `Reports/ARCHIVE` and `Reports/CURRENT`, nested zips included.
//!
//! The MMSDM archive publishes a table a month at a time; the report
//! folders publish the same tables (and others) as they are produced, so a
//! layout AEMO has just changed shows there first. Each CSV's month comes
//! from the first date in its name (`PUBLIC_DISPATCHIS_202610200245_….CSV`
//! is 2026-10), else its zip's, else when AEMO published the file. Its
//! blocks are typed by the control files of that MMSDM month, or the last
//! month archived before it; a table no control file loads has columns of
//! unknown type.
//!
//! A folder holds many thousands of five-minute files, so the census of a
//! folder is a list of runs, not of files: CSVs in name order (their names
//! carry their time) that held a table in one shape and version, with the
//! first and last CSV, how many, and the block record of them all (rows,
//! time span, and each column's declared class and observed profile,
//! merged). A run ends at any change: another shape or version, or a
//! column whose values are of a wider kind or scale than the run's so far,
//! so the CSV where anything changed is kept. It is written to
//! `{root}/nem/census/reports/{ARCHIVE|CURRENT}/{folder}.toml`; the fold
//! (`crate::fold`) takes it in beside the MMSDM census. A folder already
//! recorded is skipped unless taken again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utilake_acquire::Archive;
use utilake_acquire::archive::sha256_hex;
use utilake_core::{Diagnostic, YearMonth};
use utilake_mms_registry::{Registry, TypeClass};
use utilake_nem::MmsdmArchive;

use crate::census::{
    BlockRecord, BlockShape, CensusError, Ctls, FileRecord, Span, block_shape, census_dir,
    read_blocks, table_of, write_text,
};
use crate::fold::Fold;
use crate::history::History;

/// The report roots under `Reports/`.
pub const ROOTS: [&str; 2] = ["ARCHIVE", "CURRENT"];

/// How far back a report month looks for an MMSDM month's control files.
const CTL_LOOKBACK_MONTHS: usize = 24;

/// CSVs in a row that held one table in one shape and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// The month of its first CSV.
    pub month: YearMonth,
    /// CSVs that held a block of it.
    pub csvs: u64,
    /// The first CSV, by name.
    pub first_csv: String,
    /// The last CSV, by name.
    pub last_csv: String,
    /// Their blocks as one: rows summed, span and profiles merged.
    #[serde(flatten)]
    pub block: BlockRecord,
}

/// A file of a folder that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unread {
    /// The archived file (and entry).
    pub file: String,
    /// Why.
    pub error: String,
}

/// The census of one folder, as written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderCensus {
    /// Archived files read.
    pub files: u64,
    /// CSVs read out of them.
    pub csvs: u64,
    /// Archived files that are neither zip nor CSV (a PDF, a web page).
    pub other: u64,
    /// Files or entries that could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<Unread>,
    /// Every run, in the order its first CSV was read.
    #[serde(default, rename = "block")]
    pub blocks: Vec<Tally>,
    /// Every file AEMO rewrote in place, with each of its versions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewritten: Vec<Rewritten>,
}

/// A file AEMO rewrote in place under the same name: nemweb-mirror keeps
/// each version with new bytes, the first under the file's name and each
/// later one beside it as `{name}@{listed time}`, and notes every listing
/// in the folder's journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rewritten {
    /// The file's name.
    pub file: String,
    /// Listings the mirror found with the bytes of a version it holds
    /// (journal `same`): a rewrite with nothing new.
    #[serde(default)]
    pub relisted: u64,
    /// Zips listed packed again with the same files (journal `repacked`).
    #[serde(default)]
    pub repacked: u64,
    /// Every version archived, the first first.
    #[serde(rename = "version")]
    pub versions: Vec<Version>,
}

/// One version of a file rewritten in place, against the ones before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// The archived file: `{name}`, or `{name}@{listed time}`.
    pub file: String,
    /// What changed from the version before.
    pub change: Change,
    /// For `reverted`, the earlier version whose data it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_data_as: Option<String>,
    /// When nemweb listed it, RFC 3339 with offset (AEST).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_at: Option<String>,
    /// When AEMO published it (HTTP `Last-Modified`), RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// sha256 hex of its bytes.
    pub sha256: String,
}

/// What a version changed from the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// The first version archived.
    First,
    /// Its data differs.
    Data,
    /// Only the report's own time differs: AEMO's `C,` header and footer
    /// rows, or a `current_date` column.
    ReportTime,
    /// Its data is that of an earlier version, not the last: published,
    /// then withdrawn.
    Reverted,
}

/// A version's base name and listed time: `x.csv@20261021T191200+1000`
/// is version `20261021T191200+1000` of `x.csv`. Any other `@` is part of
/// the name.
#[must_use]
pub fn version_of(name: &str) -> (&str, Option<&str>) {
    let stamp = |s: &str| {
        let b = s.as_bytes();
        let digits =
            |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
        digits(0..8)
            && b.get(8) == Some(&b'T')
            && digits(9..15)
            && (&s[15.min(s.len())..] == "Z"
                || (b.len() == 20 && matches!(b[15], b'+' | b'-') && digits(16..20)))
    };
    match name.rsplit_once('@') {
        Some((base, at)) if stamp(at) => (base, Some(at)),
        _ => (name, None),
    }
}

/// `text` without the report's own time: AEMO's `C,` rows, or else the
/// values of a `current_date` column its header names. Two versions of a
/// file the same without it differ only in when the report was made.
fn without_report_time(text: &[u8]) -> Vec<u8> {
    let lines: Vec<&[u8]> = text
        .split(|&c| c == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .collect();
    let keep: Vec<Vec<u8>> = if lines.first().is_some_and(|l| l.starts_with(b"C,")) {
        lines
            .into_iter()
            .filter(|l| !l.starts_with(b"C,"))
            .map(<[u8]>::to_vec)
            .collect()
    } else {
        let column = lines.first().and_then(|h| {
            h.split(|&c| c == b',')
                .position(|f| f.trim_ascii().eq_ignore_ascii_case(b"current_date"))
        });
        lines
            .into_iter()
            .map(|l| {
                let fields: Vec<&[u8]> = l
                    .split(|&c| c == b',')
                    .enumerate()
                    .filter(|(i, _)| Some(*i) != column)
                    .map(|(_, f)| f)
                    .collect();
                fields.join(&b","[..])
            })
            .collect()
    };
    keep.join(&b"\n"[..])
}

/// What a report file holds apart from when it was made, as one sha256:
/// each CSV (a zip's by entry name) without its report time. None when it
/// cannot be read.
fn data_print(path: &Path, zip: bool, tmp: &Path) -> Option<String> {
    let mut parts: Vec<(String, String)> = Vec::new();
    if zip {
        utilake_kit::container::walk_csv_entries::<std::io::Error>(
            path,
            tmp,
            &mut |e| {
                let mut b = Vec::new();
                std::io::Read::read_to_end(e.reader, &mut b)?;
                parts.push((e.name.clone(), sha256_hex(&without_report_time(&b))));
                Ok(())
            },
            &mut |_| {},
        )
        .ok()?;
        parts.sort();
    } else {
        let b = std::fs::read(path).ok()?;
        parts.push((String::new(), sha256_hex(&without_report_time(&b))));
    }
    let joined: Vec<String> = parts
        .into_iter()
        .map(|(n, h)| format!("{n}\t{h}"))
        .collect();
    Some(sha256_hex(joined.join("\n").as_bytes()))
}

/// How many listings of each file a folder's journal found with nothing
/// new, by URL: (`same`, `repacked`).
fn journal_counts(journal: Option<&Path>) -> BTreeMap<String, (u64, u64)> {
    let mut out: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let Some(text) = journal.and_then(|p| std::fs::read_to_string(p).ok()) else {
        return out;
    };
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(url), Some(event)) = (e["url"].as_str(), e["event"].as_str()) else {
            continue;
        };
        let c = out.entry(url.to_owned()).or_default();
        match event {
            "same" => c.0 += 1,
            "repacked" => c.1 += 1,
            _ => {}
        }
    }
    out
}

/// Each file of `files` with more than one version, or with listings the
/// journal found with nothing new, every version against the ones before.
fn rewritten(files: &[PathBuf], journal: Option<&Path>, tmp: &Path) -> Vec<Rewritten> {
    let mut groups: BTreeMap<String, Vec<(Option<String>, &PathBuf)>> = BTreeMap::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (base, at) = version_of(&name);
        groups
            .entry(base.to_owned())
            .or_default()
            .push((at.map(str::to_owned), path));
    }
    let counts = journal_counts(journal);
    let mut out = Vec::new();
    for (file, mut versions) in groups {
        versions.sort();
        let sidecars: Vec<_> = versions
            .iter()
            .map(|(_, p)| Archive::read_sidecar(p).ok().flatten())
            .collect();
        let (relisted, repacked) = sidecars
            .iter()
            .flatten()
            .next()
            .and_then(|s| counts.get(&s.url))
            .copied()
            .unwrap_or_default();
        if versions.len() < 2 && relisted + repacked == 0 {
            continue;
        }
        let zip = file.to_ascii_lowercase().ends_with(".zip");
        let mut seen: Vec<(Option<String>, String)> = Vec::new();
        let mut list = Vec::new();
        for ((_, path), sc) in versions.iter().zip(&sidecars) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let print = data_print(path, zip, tmp);
            let earlier = seen
                .iter()
                .rev()
                .skip(1)
                .find(|(p, _)| print.is_some() && *p == print)
                .map(|(_, n)| n.clone());
            let (change, same_data_as) = match seen.last() {
                None => (Change::First, None),
                Some((p, _)) if print.is_some() && *p == print => (Change::ReportTime, None),
                Some(_) if earlier.is_some() => (Change::Reverted, earlier),
                Some(_) => (Change::Data, None),
            };
            list.push(Version {
                file: name.clone(),
                change,
                same_data_as,
                listed_at: sc
                    .as_ref()
                    .and_then(|s| s.listed_modified_at)
                    .map(|t| t.rfc3339()),
                published_at: sc
                    .as_ref()
                    .and_then(|s| s.last_modified_at)
                    .map(|t| t.rfc3339()),
                sha256: sc.as_ref().map(|s| s.sha256.clone()).unwrap_or_default(),
            });
            seen.push((print, name));
        }
        out.push(Rewritten {
            file,
            relisted,
            repacked,
            versions: list,
        });
    }
    out
}

/// The first month a name gives: the first run of at least six digits
/// that starts with a year from 1990 to 2099 and a month 01 to 12.
#[must_use]
pub fn name_month(name: &str) -> Option<YearMonth> {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i - start >= 6 {
            let year: i32 = name[start..start + 4].parse().ok()?;
            let month: u8 = name[start + 4..start + 6].parse().ok()?;
            if (1990..2100).contains(&year)
                && let Some(m) = YearMonth::new(year, month)
            {
                return Some(m);
            }
        }
    }
    None
}

/// The month AEMO published an archived file in, from its download record:
/// as listed (market time), else as served, else when it was fetched.
fn published_month(path: &Path) -> Option<YearMonth> {
    let sc = Archive::read_sidecar(path).ok()??;
    let text = sc
        .listed_modified_at
        .or(sc.last_modified_at)
        .map_or(sc.fetched_at.clone(), |s| s.rfc3339());
    text.get(..7)?.parse().ok()
}

/// The month before `m`.
fn previous(m: YearMonth) -> Option<YearMonth> {
    if m.month() == 1 {
        YearMonth::new(m.year() - 1, 12)
    } else {
        YearMonth::new(m.year(), m.month() - 1)
    }
}

/// Control files per report month, read once from the MMSDM archive.
pub struct CtlSource<'a> {
    store: &'a Archive,
    nem: &'a MmsdmArchive,
    by_month: BTreeMap<Option<YearMonth>, Ctls>,
    read: BTreeMap<YearMonth, Option<Vec<(String, String)>>>,
}

impl<'a> CtlSource<'a> {
    /// Control files of the MMSDM months in `store`.
    #[must_use]
    pub fn new(store: &'a Archive, nem: &'a MmsdmArchive) -> Self {
        Self {
            store,
            nem,
            by_month: BTreeMap::new(),
            read: BTreeMap::new(),
        }
    }

    /// The control files of `month`, else of the last MMSDM month archived
    /// before it; none for no month.
    pub fn for_month(&mut self, month: Option<YearMonth>) -> &Ctls {
        if !self.by_month.contains_key(&month) {
            let mut texts = Vec::new();
            let mut m = month;
            for _ in 0..CTL_LOOKBACK_MONTHS {
                let Some(cur) = m else { break };
                if let Some(t) = self.month_texts(cur) {
                    texts = t;
                    break;
                }
                m = previous(cur);
            }
            let ctls = Ctls::parse(texts.iter().map(|(n, t)| (n.as_str(), t.as_str())));
            self.by_month.insert(month, ctls);
        }
        &self.by_month[&month]
    }

    fn month_texts(&mut self, month: YearMonth) -> Option<Vec<(String, String)>> {
        if !self.read.contains_key(&month) {
            // The loose control files of a month directory (2015 on): read
            // straight from its CTL folder, not by listing the month.
            let texts = self
                .nem
                .month_root(month)
                .ok()
                .map(|u| {
                    self.store
                        .path_for(u.as_str())
                        .join("MMSDM_Historical_Data_SQLLoader")
                        .join("CTL")
                })
                .and_then(|dir| std::fs::read_dir(dir).ok())
                .map(|rd| {
                    let mut t: Vec<(String, String)> = rd
                        .filter_map(Result::ok)
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            let wanted = !name.starts_with("._")
                                && name.to_ascii_lowercase().ends_with(".ctl");
                            wanted
                                .then(|| std::fs::read(e.path()).ok())
                                .flatten()
                                .map(|b| (name, String::from_utf8_lossy(&b).into_owned()))
                        })
                        .collect();
                    t.sort();
                    t
                })
                .filter(|t| !t.is_empty());
            self.read.insert(month, texts);
        }
        self.read[&month].clone()
    }
}

/// One run, gathering.
struct Acc {
    month: YearMonth,
    csvs: u64,
    first_csv: String,
    last_csv: String,
    merged: BlockShape,
}

/// What a folder's census gathers before it is written.
#[derive(Default)]
struct Gather {
    census: FolderCensus,
    runs: Vec<Acc>,
    /// The open run of each report pair.
    open: BTreeMap<(String, String), usize>,
    undated: u64,
}

/// Whether `b` continues run `a`: the same shape, version and declared
/// classes, and no column of a wider kind or scale than the run has seen.
fn continues(a: &Acc, b: &BlockShape) -> bool {
    let m = &a.merged;
    m.shape.id() == b.shape.id()
        && m.version == b.version
        && m.ctl == b.ctl
        && m.profile
            .iter()
            .zip(&b.profile)
            .all(|(p, q)| p.kind.join(q.kind) == p.kind && q.scale <= p.scale)
}

impl Gather {
    fn add(&mut self, month: YearMonth, csv: &str, b: BlockShape) {
        let key = (b.shape.report_type.clone(), b.shape.report_subtype.clone());
        let open = self.open.get(&key).copied();
        let Some(i) = open.filter(|i| continues(&self.runs[*i], &b)) else {
            self.open.insert(key, self.runs.len());
            self.runs.push(Acc {
                month,
                csvs: 1,
                first_csv: csv.to_owned(),
                last_csv: csv.to_owned(),
                merged: b,
            });
            return;
        };
        let a = &mut self.runs[i];
        a.csvs += 1;
        if csv < a.first_csv.as_str() {
            csv.clone_into(&mut a.first_csv);
        }
        if csv > a.last_csv.as_str() {
            csv.clone_into(&mut a.last_csv);
        }
        let m = &mut a.merged;
        m.rows += b.rows;
        m.span = match (m.span.take(), b.span) {
            (Some(x), Some(y)) => Some(Span {
                first: x.first.min(y.first),
                last: x.last.max(y.last),
                column: x.column,
            }),
            (x, y) => x.or(y),
        };
        for (p, q) in m.profile.iter_mut().zip(&b.profile) {
            p.merge(q);
        }
        for (c, d) in m.ctl.iter_mut().zip(b.ctl) {
            if d != TypeClass::Unknown {
                *c = d;
            }
        }
    }
}

/// The base name of an entry path, nested names joined with `!`.
fn base(name: &str) -> &str {
    name.rsplit(['/', '\\', '!']).next().unwrap_or(name)
}

/// The census of the archived `files` of one folder.
///
/// A version of a file AEMO rewrote in place (`{name}@{listed time}`) is
/// read as its name's kind and month, its blocks counted with the rest,
/// and each such file is listed in `rewritten` with every version against
/// the ones before it. `journal` is the folder's nemweb-mirror journal,
/// whose listings with nothing new each file counts.
pub fn census_folder(
    registry: &Registry,
    ctls: &mut CtlSource<'_>,
    files: &[PathBuf],
    journal: Option<&Path>,
    tmp: &Path,
) -> FolderCensus {
    let mut g = Gather::default();
    let mut scan_buf = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (base_name, _) = version_of(&name);
        let lower = base_name.to_ascii_lowercase();
        let outer_month = name_month(base_name).or_else(|| published_month(path));
        g.census.files += 1;
        let mut one_csv =
            |csv: &str, r: &mut dyn std::io::Read, g: &mut Gather| -> Result<(), Box<Diagnostic>> {
                let blocks = read_blocks(r, &mut scan_buf)?;
                g.census.csvs += 1;
                let month = name_month(base(csv)).or(outer_month);
                let Some(month) = month else {
                    g.undated += 1;
                    return Ok(());
                };
                let c = ctls.for_month(Some(month));
                for raw in blocks {
                    let b = block_shape(registry, c, None, raw);
                    g.add(month, base(csv), b);
                }
                Ok(())
            };
        if lower.ends_with(".csv") {
            let res = std::fs::File::open(path)
                .map_err(|e| e.to_string())
                .and_then(|mut f| one_csv(&name, &mut f, &mut g).map_err(|d| d.message.clone()));
            if let Err(error) = res {
                g.census.unread.push(Unread { file: name, error });
            }
        } else if lower.ends_with(".zip") {
            let mut entry_err: Option<Unread> = None;
            let walked = utilake_kit::container::walk_csv_entries::<Box<Diagnostic>>(
                path,
                tmp,
                &mut |e| {
                    one_csv(&e.name, e.reader, &mut g).inspect_err(|d| {
                        entry_err = Some(Unread {
                            file: format!("{name}!{}", e.name),
                            error: d.message.clone(),
                        });
                    })
                },
                &mut |_| {},
            );
            match walked {
                Ok(_)
                | Err(utilake_kit::container::WalkError::Archive(
                    utilake_kit::container::ArchiveError::NoCsv { .. },
                )) => {}
                Err(utilake_kit::container::WalkError::Archive(a)) => {
                    g.census.unread.push(Unread {
                        file: name,
                        error: a.to_string(),
                    });
                }
                Err(utilake_kit::container::WalkError::Callback(_)) => {
                    g.census.unread.extend(entry_err);
                }
            }
        } else {
            g.census.other += 1;
        }
    }
    if g.undated > 0 {
        g.census.unread.push(Unread {
            file: String::new(),
            error: format!("{} CSV(s) whose month no name or record gives", g.undated),
        });
    }
    for a in g.runs {
        g.census.blocks.push(Tally {
            month: a.month,
            csvs: a.csvs,
            first_csv: a.first_csv,
            last_csv: a.last_csv,
            block: BlockRecord::of(&a.merged),
        });
    }
    g.census.rewritten = rewritten(files, journal, tmp);
    g.census
}

/// The folders directly under `dir`, in name order (macOS `._` files
/// left out).
#[must_use]
pub fn top_folders(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| !n.to_string_lossy().starts_with("._"))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Every folder at or under `under` that holds archived files, with its
/// files in name order, keyed by its path below `dir` (`/`-separated).
#[must_use]
pub fn folders(store: &Archive, dir: &Path, under: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    let mut out: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for a in store.entries_under(Some(under)) {
        let Some(parent) = a.path.parent() else {
            continue;
        };
        let Ok(rel) = parent.strip_prefix(dir) else {
            continue;
        };
        let key = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.entry(key).or_default().push(a.path);
    }
    for files in out.values_mut() {
        files.sort();
    }
    out
}

/// Where the census of folder `rel` of report root `root` is written.
#[must_use]
pub fn folder_path(schemas: &Path, root: &str, rel: &str) -> PathBuf {
    let name = if rel.is_empty() { "_root" } else { rel };
    census_dir(schemas)
        .join("reports")
        .join(root)
        .join(format!("{name}.toml"))
}

/// Writes the census of a folder; the shapes are folded afterwards.
pub fn record_folder(
    schemas: &Path,
    root: &str,
    rel: &str,
    census: &FolderCensus,
) -> Result<(), CensusError> {
    let text = format!(
        "# Generated by `utilake-nem-census reports`: every shape of Reports/{root}/{rel}, per month, and what each column held.\n{}",
        toml::to_string(census).expect("a folder census serialises")
    );
    write_text(&folder_path(schemas, root, rel), &text)
}

/// Every folder census recorded under a report root, by its path below the
/// root (`/`-separated, without `.toml`).
fn recorded_folders(schemas: &Path, root: &str) -> Vec<String> {
    let base = census_dir(schemas).join("reports").join(root);
    let mut out = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for p in rd.filter_map(Result::ok).map(|e| e.path()) {
            if p.is_dir() {
                stack.push(p);
            } else if let Some(rel) = p
                .strip_prefix(&base)
                .ok()
                .and_then(|r| r.to_str())
                .and_then(|r| r.strip_suffix(".toml"))
            {
                out.push(rel.replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

/// Takes every recorded folder census into a fold: each tally is a run of
/// CSVs from its first to its last, of kind `reports/{ROOT}`.
pub fn fold_into(
    schemas: &Path,
    registry: &Registry,
    fold: &mut Fold,
    history: &mut History,
) -> Result<(), CensusError> {
    for root in ROOTS {
        for rel in recorded_folders(schemas, root) {
            let Some(census) = read_folder(schemas, root, &rel)? else {
                continue;
            };
            for t in &census.blocks {
                let b = &t.block;
                let table = table_of(registry, &b.report.0, &b.report.1);
                let file = FileRecord {
                    source: format!("Reports/{root}/{rel}"),
                    sha256: None,
                    published: None,
                    kind: format!("reports/{}", root.to_ascii_lowercase()),
                    block: Vec::new(),
                };
                fold.run(
                    &table,
                    t.month,
                    (&t.first_csv, &t.last_csv),
                    t.csvs,
                    &file,
                    b,
                );
                history.run(
                    &table,
                    t.month,
                    (&t.first_csv, &t.last_csv, t.csvs),
                    &file,
                    b,
                );
            }
        }
    }
    Ok(())
}

/// The census of a folder as written, if it was.
pub fn read_folder(
    schemas: &Path,
    root: &str,
    rel: &str,
) -> Result<Option<FolderCensus>, CensusError> {
    let path = folder_path(schemas, root, rel);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(CensusError::Read {
                path: path.display().to_string(),
                message: e.to_string(),
            });
        }
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| CensusError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::census::tests::{SCADA_CTL, put, three_tables, zip_of};
    use utilake_mms_registry::TypeClass;

    /// Archives `bytes` as version `at` of `url` (`None`: the first), with
    /// the listed time `at` gives.
    fn put_version(store: &Archive, url: &str, at: Option<&str>, bytes: &[u8]) -> PathBuf {
        let base = store.path_for(url);
        let path = at.map_or(base.clone(), |a| {
            let mut p = base.into_os_string();
            p.push(format!("@{a}"));
            PathBuf::from(p)
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let listed = at.map(|a| {
            format!(
                "{}-{}-{}T{}:{}:{}+10:00",
                &a[0..4],
                &a[4..6],
                &a[6..8],
                &a[9..11],
                &a[11..13],
                &a[13..15]
            )
        });
        let sidecar = utilake_acquire::Sidecar {
            url: url.to_owned(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            fetched_at: "2026-10-21T00:00:00Z".into(),
            etag: None,
            last_modified: Some("Wed, 21 Oct 2026 00:00:00 GMT".into()),
            last_modified_at: utilake_core::timetext::parse_rfc3339("2026-10-21T00:00:00Z"),
            listed_modified: None,
            listed_modified_at: listed.and_then(|l| utilake_core::timetext::parse_rfc3339(&l)),
            tool: "test".into(),
        };
        Archive::write_sidecar(&path, &sidecar).unwrap();
        path
    }

    #[test]
    fn a_version_name_gives_its_base_and_listed_time() {
        assert_eq!(
            version_of("int128_v4_actual_linepack_1.csv@20261021T191200+1000"),
            (
                "int128_v4_actual_linepack_1.csv",
                Some("20261021T191200+1000")
            )
        );
        assert_eq!(
            version_of("a.zip@20261021T091200Z"),
            ("a.zip", Some("20261021T091200Z"))
        );
        assert_eq!(version_of("a.zip"), ("a.zip", None));
        for odd in [
            "a@b.csv",
            "a.csv@0123456789abcdef",
            "a.csv@20261021X191200+1000",
            "a.csv@20261021T191200+10",
        ] {
            assert_eq!(version_of(odd), (odd, None), "{odd}");
        }
    }

    #[test]
    fn a_report_time_is_left_out_of_what_a_file_holds() {
        let aemo = b"C,NEMP.WORLD,X,AEMO,PUBLIC,2026/10/21,19:12:00,1\r\nI,A,B,1,X\r\nD,A,B,1,5\r\nC,END OF REPORT,3\r\n";
        let later = b"C,NEMP.WORLD,X,AEMO,PUBLIC,2026/10/21,19:42:00,1\r\nI,A,B,1,X\r\nD,A,B,1,5\r\nC,END OF REPORT,3\r\n";
        assert_eq!(without_report_time(aemo), without_report_time(later));
        assert_eq!(without_report_time(aemo), b"I,A,B,1,X\nD,A,B,1,5\n");
        let gas = b"gas_date,flow,current_date\n2026-10-21,5,21 Oct 2026 19:12:00\n";
        let gas_later = b"gas_date,flow,current_date\n2026-10-21,5,21 Oct 2026 19:42:00\n";
        assert_eq!(without_report_time(gas), without_report_time(gas_later));
        assert_eq!(without_report_time(gas), b"gas_date,flow\n2026-10-21,5\n");
        let plain = b"a,b\n1,2";
        assert_eq!(without_report_time(plain), plain);
    }

    #[test]
    fn each_version_of_a_file_rewritten_in_place_is_classed() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = Archive::new(dir.path());
        let folder = "https://h/Reports/CURRENT/VicGas";
        let csv = format!("{folder}/int128_v4_actual_linepack_1.csv");
        let row = |flow: u32, made: &str| {
            format!("gas_date,flow,current_date\n2026-10-21,{flow},{made}\n")
        };
        let mut files = vec![
            put_version(&store, &csv, None, row(5, "19:00").as_bytes()),
            put_version(
                &store,
                &csv,
                Some("20261021T191200+1000"),
                row(6, "19:12").as_bytes(),
            ),
            put_version(
                &store,
                &csv,
                Some("20261021T194200+1000"),
                row(6, "19:42").as_bytes(),
            ),
            put_version(
                &store,
                &csv,
                Some("20261021T201200+1000"),
                row(5, "20:12").as_bytes(),
            ),
        ];
        let zip = format!("{folder}/CurrentDay.zip");
        let inner = |made: &str| {
            crate::census::tests::zip_of(&[(
                "int029a.csv",
                format!("C,X,{made}\nD,A,1\nC,END\n").as_bytes(),
            )])
        };
        files.push(put_version(&store, &zip, None, &inner("1")));
        files.push(put_version(
            &store,
            &zip,
            Some("20261021T191200+1000"),
            &inner("2"),
        ));
        // Not a zip that reads: its data cannot be compared.
        files.push(put_version(
            &store,
            &zip,
            Some("20261021T194200+1000"),
            b"not a zip",
        ));
        let lone = format!("{folder}/int934_v4_ecgs_contacts_1.csv");
        files.push(put_version(&store, &lone, None, b"a,b\n1,2\n"));
        let quiet = format!("{folder}/int111_v5_sdpc_1.csv");
        files.push(put_version(&store, &quiet, None, b"a,b\n1,2\n"));
        files.sort();

        let journal = dir.path().join("changes.ndjson");
        let line =
            |url: &str, event: &str| format!("{{\"url\":\"{url}\",\"event\":\"{event}\"}}\n");
        let text = [
            line(&lone, "same"),
            line(&lone, "same"),
            line(&zip, "repacked"),
            line(&csv, "version"),
            "{\"event\":\"same\"}\n".to_owned(),
            "not json\n".to_owned(),
        ]
        .concat();
        std::fs::write(&journal, text).unwrap();

        let r = rewritten(&files, Some(&journal), tmp.path());
        let names: Vec<&str> = r.iter().map(|w| w.file.as_str()).collect();
        assert_eq!(
            names,
            [
                "CurrentDay.zip",
                "int128_v4_actual_linepack_1.csv",
                "int934_v4_ecgs_contacts_1.csv"
            ]
        );
        let changes = |w: &Rewritten| w.versions.iter().map(|v| v.change).collect::<Vec<_>>();
        assert_eq!(
            changes(&r[0]),
            [Change::First, Change::ReportTime, Change::Data]
        );
        assert_eq!((r[0].relisted, r[0].repacked), (0, 1));
        assert_eq!(
            changes(&r[1]),
            [
                Change::First,
                Change::Data,
                Change::ReportTime,
                Change::Reverted
            ]
        );
        assert_eq!(
            r[1].versions[3].same_data_as.as_deref(),
            Some("int128_v4_actual_linepack_1.csv")
        );
        assert_eq!(
            r[1].versions[1].listed_at.as_deref(),
            Some("2026-10-21T19:12:00+10:00")
        );
        assert_eq!(
            r[1].versions[0].published_at.as_deref(),
            Some("2026-10-21T00:00:00Z")
        );
        assert_eq!((r[2].relisted, r[2].versions.len()), (2, 1));
        // No journal: only the files with more than one version.
        assert_eq!(rewritten(&files, None, tmp.path()).len(), 2);
        assert!(journal_counts(Some(&dir.path().join("none.ndjson"))).is_empty());

        // The census reads every version as its name's kind, and lists them.
        let nem = MmsdmArchive::new("https://h").unwrap();
        let reg = Registry::load_embedded();
        let mut ctls = CtlSource::new(&store, &nem);
        let census = census_folder(&reg, &mut ctls, &files, Some(&journal), tmp.path());
        assert_eq!(census.other, 0);
        assert_eq!(census.rewritten, r);
        let text = toml::to_string(&census).unwrap();
        assert!(text.contains("[[rewritten.version]]"), "{text}");
        let back: FolderCensus = toml::from_str(&text).unwrap();
        assert_eq!(back, census);
    }

    #[test]
    fn a_name_gives_its_first_month() {
        let m = |s: &str| name_month(s).map(|m| m.to_string());
        assert_eq!(
            m("PUBLIC_DISPATCHIS_202610200245_0000000543803367.zip").as_deref(),
            Some("2026-10")
        );
        assert_eq!(m("PUBLIC_BILLING_20250902.zip").as_deref(), Some("2025-09"));
        assert_eq!(
            m("FILE01_123_202509.CSV").as_deref(),
            Some("2025-09"),
            "short runs are skipped"
        );
        assert_eq!(m("X_000000123456_201213.CSV"), None, "year 0000, month 13");
        assert_eq!(
            m("X_209913_200001.CSV").as_deref(),
            Some("2000-01"),
            "month 13 is not a month"
        );
        assert_eq!(m("PUBLIC_TER_DAILY.zip"), None);
        assert_eq!(
            previous(YearMonth::new(2026, 1).unwrap())
                .unwrap()
                .to_string(),
            "2025-12"
        );
        assert_eq!(
            previous(YearMonth::new(2026, 7).unwrap())
                .unwrap()
                .to_string(),
            "2026-06"
        );
    }

    #[test]
    fn a_folder_is_a_list_of_runs_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let store = Archive::new(dir.path());
        let nem = MmsdmArchive::new("https://h").unwrap();
        let reg = Registry::load_embedded();
        // The control file of 2026-08 types the SCADA block of 2026-08 and,
        // looking back, of 2026-10; nothing types 2026-07.
        let ctl_url = nem
            .month_root(YearMonth::new(2026, 8).unwrap())
            .unwrap()
            .join("MMSDM_Historical_Data_SQLLoader/CTL/PUBLIC_ARCHIVE%23DISPATCH_UNIT_SCADA%23FILE01%23202608.ctl")
            .unwrap()
            .to_string();
        put(&store, &ctl_url, SCADA_CTL.as_bytes());
        let folder = "https://h/Reports/ARCHIVE/Dispatch_SCADA";
        // A day's zip of five-minute zips, each a CSV of three blocks.
        let inner = |stamp: &str, rows: usize| {
            zip_of(&[(
                &format!("PUBLIC_DISPATCHSCADA_{stamp}_0000000001.CSV"),
                &three_tables(rows),
            )])
        };
        let day = zip_of(&[
            (
                "PUBLIC_DISPATCHSCADA_202608010005.zip",
                &inner("202608010005", 2),
            ),
            (
                "PUBLIC_DISPATCHSCADA_202608010010.zip",
                &inner("202608010010", 3),
            ),
        ]);
        put(
            &store,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_20260801.zip"),
            &day,
        );
        put(
            &store,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202610200005_1.CSV"),
            &three_tables(4),
        );
        put(
            &store,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202607310005_1.CSV"),
            &three_tables(1),
        );
        // A value with a second decimal place: a wider scale starts a run.
        let finer = String::from_utf8(three_tables(1))
            .unwrap()
            .replace(",0.5\r", ",0.25\r");
        put(
            &store,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202610200010_1.CSV"),
            finer.as_bytes(),
        );
        // No date in its name: the month AEMO published it.
        put(
            &store,
            &format!("{folder}/PUBLIC_UNDATED.CSV"),
            &three_tables(1),
        );
        put(&store, &format!("{folder}/notes.pdf"), b"%PDF");
        put(
            &store,
            &format!("{folder}/PUBLIC_BAD_20260801.zip"),
            b"not a zip",
        );
        put(
            &store,
            &format!("{folder}/PUBLIC_BAD_20260802.CSV"),
            b"D,A,B,1,x\n",
        );
        put(
            &store,
            &format!("{folder}/PUBLIC_BADENTRY_20260803.zip"),
            &zip_of(&[("PUBLIC_BADENTRY_20260803.CSV", b"D,A,B,1,x\n")]),
        );
        put(
            &store,
            "https://h/Reports/ARCHIVE/GSH/Benchmark_Price/PUBLIC_GSH_20260801.CSV",
            &three_tables(1),
        );

        let root_dir = store.path_for("https://h/Reports/ARCHIVE/x");
        let root_dir = root_dir.parent().unwrap();
        let tops = top_folders(root_dir);
        assert_eq!(tops.len(), 2);
        assert!(top_folders(&dir.path().join("none")).is_empty());
        let all = folders(&store, root_dir, &tops[0]);
        assert_eq!(all.keys().collect::<Vec<_>>(), ["Dispatch_SCADA"]);
        let gsh = folders(&store, root_dir, &tops[1]);
        assert_eq!(gsh.keys().collect::<Vec<_>>(), ["GSH/Benchmark_Price"]);
        assert!(folder_path(Path::new("s"), "ARCHIVE", "").ends_with("reports/ARCHIVE/_root.toml"));

        let tmp = tempfile::tempdir().unwrap();
        let mut ctls = CtlSource::new(&store, &nem);
        assert!(
            ctls.for_month(None)
                .for_table(&["DISPATCH_UNIT_SCADA"])
                .is_none()
        );
        let census = census_folder(&reg, &mut ctls, &all["Dispatch_SCADA"], None, tmp.path());
        assert_eq!((census.files, census.other), (9, 1));
        assert_eq!(census.csvs, 6, "two nested, four loose");
        let unread: Vec<&str> = census.unread.iter().map(|u| u.file.as_str()).collect();
        assert_eq!(
            unread,
            [
                "PUBLIC_BADENTRY_20260803.zip!PUBLIC_BADENTRY_20260803.CSV",
                "PUBLIC_BAD_20260801.zip",
                "PUBLIC_BAD_20260802.CSV"
            ],
            "{:?}",
            census.unread
        );
        let scada: Vec<&Tally> = census
            .blocks
            .iter()
            .filter(|t| t.block.report == ("DISPATCH".into(), "UNIT_SCADA".into()))
            .collect();
        // Runs in name order: July untyped; August's two nested CSVs and
        // October's first typed alike (October looks back to August's
        // control file); October's finer value and the undated CSV after it.
        let months: Vec<String> = scada.iter().map(|t| t.month.to_string()).collect();
        assert_eq!(months, ["2026-07", "2026-08", "2026-10"]);
        let aug = scada[1];
        assert_eq!((aug.csvs, aug.block.rows), (3, 9));
        assert_eq!(
            aug.first_csv,
            "PUBLIC_DISPATCHSCADA_202608010005_0000000001.CSV"
        );
        assert_eq!(aug.last_csv, "PUBLIC_DISPATCHSCADA_202610200005_1.CSV");
        // The CSVs' blocks are one record: rows summed, values merged.
        assert_eq!(aug.block.nulls, [0, 0, 0]);
        assert_eq!(aug.block.max[2], "3.5");
        assert_eq!(
            scada[2].first_csv,
            "PUBLIC_DISPATCHSCADA_202610200010_1.CSV"
        );
        assert_eq!(scada[2].last_csv, "PUBLIC_UNDATED.CSV");
        assert_eq!(scada[0].block.ctl_of(0), TypeClass::Unknown);
        assert_eq!(aug.block.ctl_of(0), TypeClass::Timestamp);

        // Recorded: the folder file, and each shape with its months.
        let schemas = dir.path().join("schemas");
        record_folder(&schemas, "ARCHIVE", "Dispatch_SCADA", &census).unwrap();
        let folded = crate::fold::fold(&schemas, &reg).unwrap();
        assert!(folded.shapes >= 3, "{folded:?}");
        // Each table's history as an Iceberg table: the three report runs
        // share one schema, so one run of six CSVs.
        let text = std::fs::read_to_string(
            crate::census::history_dir(&schemas).join("DISPATCH_UNIT_SCADA.json"),
        )
        .unwrap();
        let history: serde_json::Value = serde_json::from_str(&text).unwrap();
        let runs = history["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1, "{text}");
        assert_eq!(
            runs[0]["from"]["file"],
            "PUBLIC_DISPATCHSCADA_202607310005_1.CSV"
        );
        assert_eq!(runs[0]["files"], 6);
        assert_eq!(runs[0]["kinds"], serde_json::json!(["reports/archive"]));
        assert!(folded.runs >= 3, "{folded:?}");
        assert_eq!(
            read_folder(&schemas, "ARCHIVE", "Dispatch_SCADA")
                .unwrap()
                .unwrap(),
            census
        );
        assert_eq!(read_folder(&schemas, "ARCHIVE", "None").unwrap(), None);
        let recorded = crate::census::read_shapes(&schemas, "DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(
            recorded
                .iter()
                .map(|e| e.first_seen)
                .min()
                .unwrap()
                .to_string(),
            "2026-07"
        );
        assert_eq!(
            recorded
                .iter()
                .map(|e| e.last_seen)
                .max()
                .unwrap()
                .to_string(),
            "2026-10"
        );
        std::fs::write(folder_path(&schemas, "ARCHIVE", "Broken"), "files = [").unwrap();
        assert!(read_folder(&schemas, "ARCHIVE", "Broken").is_err());
        std::fs::create_dir_all(folder_path(&schemas, "ARCHIVE", "Dir")).unwrap();
        assert!(read_folder(&schemas, "ARCHIVE", "Dir").is_err());
    }
}
