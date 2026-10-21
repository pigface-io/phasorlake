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
pub fn census_folder(
    registry: &Registry,
    ctls: &mut CtlSource<'_>,
    files: &[PathBuf],
    tmp: &Path,
) -> FolderCensus {
    let mut g = Gather::default();
    let mut scan_buf = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        let outer_month = name_month(&name).or_else(|| published_month(path));
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
        let census = census_folder(&reg, &mut ctls, &all["Dispatch_SCADA"], tmp.path());
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
