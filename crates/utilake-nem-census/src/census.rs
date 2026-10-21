//! The census: which shape every block of every file of every month had.
//!
//! utilake downloads every file of the archive once (`utilake fetch nem`).
//! The census then reads each archived file of a month in full, offline,
//! nested zips included, and records every I row it holds: a file of many
//! tables has a block per table, anywhere in the file, and all are seen.
//! Each block's columns are joined with the types the month's control
//! files load them as, which gives the block's [`CanonicalShape`] and its
//! [`ShapeId`]. Two files are written, both metadata only, in an order that
//! does not depend on the order of work:
//!
//! - `{root}/census/{YYYY-MM}.toml`: every file of the month and each of its
//!   blocks, in file order, with its row count;
//! - `{root}/shapes/{TABLE}.toml`: every distinct shape of a table, once,
//!   with the months it was first and last seen. A shape once recorded is
//!   never removed.
//!
//! A month whose census file exists is done; running again continues with
//! the months that are not.

use crate::profile::ColumnProfile;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utilake_acquire::Archive;
use utilake_core::{Category, Diagnose, Diagnostic, YearMonth};
use utilake_mms_decode::IRow;
use utilake_mms_decode::scan::{Flow, Pos, ScanOptions, Sink, scan_with};
use utilake_mms_decode::{Row, RowError, RowFailure, SinkError};
use utilake_mms_registry::Registry;
use utilake_mms_registry::file_model::ctl::CtlFile;
use utilake_mms_registry::file_model::shapes::{ShapeEntry, parse_shapes};
use utilake_mms_registry::{CanonicalShape, ShapeId, TypeClass};
use utilake_nem::MmsdmArchive;

/// Why the census of a month could not be taken or recorded.
#[derive(Debug, thiserror::Error)]
pub enum CensusError {
    /// A census, shapes or archived file that cannot be read.
    #[error("The census file {path} cannot be read: {message}")]
    Read {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// A census or shapes file that cannot be written.
    #[error("The census file {path} could not be written.")]
    Write {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// No file of the month is in the archive.
    #[error(
        "No file of {month} is in the archive under {path}; download the month first with `utilake fetch nem --from {month}`."
    )]
    NotArchived {
        /// The month.
        month: YearMonth,
        /// Where its files would be.
        path: String,
    },
}

impl Diagnose for CensusError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Read { path, .. } => {
                Diagnostic::error("schema.census_unreadable", Category::Schema, msg)
                    .with_file(path.clone())
            }
            Self::Write { path, source } => {
                Diagnostic::error("schema.census_write", Category::Schema, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::NotArchived { path, .. } => {
                Diagnostic::error("schema.census_not_archived", Category::Schema, msg)
                    .with_file(path.clone())
            }
        }
    }
}

/// One block of one file, as the census saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockShape {
    /// The block's shape: its report pair and column names.
    pub shape: CanonicalShape,
    /// The version AEMO stamped on it.
    pub version: u16,
    /// Its D rows.
    pub rows: u64,
    /// Where its rows lie in time, when a date-like column was there.
    pub span: Option<Span>,
    /// What the month's control file declares each column as, in column
    /// order; `unknown` where none does.
    pub ctl: Vec<TypeClass>,
    /// What each column's values were, in column order.
    pub profile: Vec<ColumnProfile>,
}

/// One file of a month and where it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileCensus {
    /// The archived file it was read out of: itself, or a whole-month zip.
    pub source: String,
    /// The sha256 of that archived file, from its sidecar.
    pub sha256: Option<String>,
    /// When AEMO published it (HTTP `Last-Modified`), RFC 3339, from its
    /// sidecar.
    pub published: Option<String>,
    /// The kind of file by its name: `dvd`, `archive` or `other`, with
    /// `#all` for a file of every run.
    pub kind: String,
    /// Its blocks, in file order.
    pub blocks: Vec<BlockShape>,
}

/// The census of one month: per file name, its blocks in file order. A
/// file that could not be read has none, and `problems` says why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonthCensus {
    /// The month.
    pub month: Option<YearMonth>,
    /// File name to what was read of it.
    pub files: BTreeMap<String, FileCensus>,
    /// Files that could not be read, with the diagnostic.
    pub problems: Vec<Diagnostic>,
}

/// The table a control file loads, from its name in either archive naming
/// scheme, upper-case. A split digit of the old scheme stays on the name.
#[must_use]
pub fn ctl_table(file_name: &str) -> Option<String> {
    if !file_name.to_ascii_lowercase().ends_with(".ctl") {
        return None;
    }
    utilake_nem::names::parse(file_name).map(|n| n.table)
}

/// The names under which a control file or a data stem may be looked up:
/// as written, and without a split digit.
pub(crate) fn table_keys(table: &str) -> Vec<String> {
    let upper = table.to_ascii_uppercase();
    let trimmed = upper.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.is_empty() || trimmed == upper {
        vec![upper]
    } else {
        vec![trimmed.to_owned(), upper]
    }
}

/// Whether the control file for `ctl_table` describes the data file for
/// `data_table`: the same name, a split of it, or with `siblings` the
/// daily `_D` file of the same table.
fn ctl_serves(ctl_table: &str, data_table: &str, siblings: bool) -> bool {
    let wanted = table_keys(data_table);
    let mut have = table_keys(ctl_table);
    if siblings && let Some(base) = ctl_table.to_ascii_uppercase().strip_suffix("_D") {
        have.extend(table_keys(base));
    }
    have.iter().any(|k| wanted.contains(k))
}

/// The control files of a month, with the table each one's file name says.
pub struct Ctls(Vec<(String, CtlFile)>);

impl Ctls {
    /// Control files parsed from `(file name, text)` pairs; ones that do
    /// not parse are left out.
    pub fn parse<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut out: Vec<(String, CtlFile)> = files
            .into_iter()
            .filter_map(|(name, text)| {
                let table = ctl_table(name)?;
                let ctl = utilake_mms_registry::file_model::ctl::parse(name, text).ok()?;
                Some((table, ctl))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Self(out)
    }

    /// The control file that describes `table`'s file: one of the same
    /// table, else of a split of it, else the daily `_D` file's.
    pub(crate) fn for_table(&self, tables: &[&str]) -> Option<&CtlFile> {
        [false, true].into_iter().find_map(|siblings| {
            tables.iter().find_map(|t| {
                self.0
                    .iter()
                    .find(|(ct, _)| ctl_serves(ct, t, siblings))
                    .map(|(_, c)| c)
            })
        })
    }
}

/// A block as read, before it is typed: report pair, version, columns and
/// rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    /// Report type.
    pub report_type: String,
    /// Report subtype.
    pub report_subtype: String,
    /// Version.
    pub version: u16,
    /// Column names, normalised.
    pub columns: Vec<String>,
    /// D rows.
    pub rows: u64,
    /// For every column whose first value is written as a date
    /// (`YYYY/MM/DD…`): its index, and the least and greatest value over
    /// every D row, as written.
    pub bounds: Vec<(usize, Vec<u8>, Vec<u8>)>,
    /// Every column's profile, in column order, over every D row.
    pub profile: Vec<ColumnProfile>,
}

/// Where a block's rows lie in time: the least and greatest value of its
/// time column over every D row, as written (AEMO's `YYYY/MM/DD HH:MM:SS`
/// orders as text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The least value.
    pub first: String,
    /// The greatest value.
    pub last: String,
    /// The column the values are of.
    pub column: String,
}

/// Columns that name the time an observation is for, most specific first:
/// the span of a block whose table declares no time column is of the first
/// of these it has.
const TARGET_TIME: [&str; 9] = [
    "INTERVAL_DATETIME",
    "DATETIME",
    "SETTLEMENTDATE",
    "TRADINGDATE",
    "DAY",
    "EFFECTIVEDATE",
    "START_DATE",
    "STARTDATE",
    "RUN_DATETIME",
];

/// Whether a field is written as a date: `YYYY/MM/DD` at least.
fn date_like(v: &[u8]) -> bool {
    v.len() >= 10
        && v[4] == b'/'
        && v[7] == b'/'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| v[i].is_ascii_digit())
}

/// A sink that keeps the I rows, counts the D rows of each block and
/// tracks the least and greatest value of every date-like column.
#[derive(Default)]
struct Blocks {
    blocks: Vec<RawBlock>,
    /// Whether the current block's first D row has been seen.
    probed: bool,
}

impl Sink for Blocks {
    fn comment(&mut self, _: &Row<'_>, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
    fn header(&mut self, irow: &IRow<'_>, _: Pos) -> Result<(), SinkError> {
        self.blocks.push(RawBlock {
            report_type: irow.report_type.to_owned(),
            report_subtype: irow.report_subtype.to_owned(),
            version: irow.version,
            columns: irow.columns.iter().map(|c| (*c).to_owned()).collect(),
            rows: 0,
            bounds: Vec::new(),
            profile: vec![ColumnProfile::default(); irow.columns.len()],
        });
        self.probed = false;
        Ok(())
    }
    fn row(&mut self, row: &Row<'_>, _: Pos) -> Result<(), RowFailure> {
        let Some(b) = self.blocks.last_mut() else {
            return Ok(());
        };
        b.rows += 1;
        // Data fields follow the four D,type,subtype,version cells.
        for (j, p) in b.profile.iter_mut().enumerate() {
            p.observe(row.get(4 + j));
        }
        if !self.probed {
            self.probed = true;
            for j in 0..b.columns.len() {
                if let Some(v) = row.get(4 + j)
                    && date_like(v)
                {
                    b.bounds.push((j, v.to_vec(), v.to_vec()));
                }
            }
            return Ok(());
        }
        for (j, min, max) in &mut b.bounds {
            if let Some(v) = row.get(4 + *j) {
                if v < min.as_slice() {
                    min.clear();
                    min.extend_from_slice(v);
                } else if v > max.as_slice() {
                    max.clear();
                    max.extend_from_slice(v);
                }
            }
        }
        Ok(())
    }
    fn bad_row(&mut self, _: &[u8], _: Pos, _: &RowError) -> Flow {
        Flow::Continue
    }
    fn footer(&mut self, _: Option<u64>, _: u64, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
}

/// Every block of one CSV stream, in order: I rows, D rows counted and
/// their date-like columns bounded.
pub fn read_blocks(
    reader: impl std::io::Read,
    buf: &mut Vec<u8>,
) -> Result<Vec<RawBlock>, Box<Diagnostic>> {
    let mut blocks = Blocks::default();
    scan_with(
        reader,
        &mut blocks,
        &ScanOptions {
            max_bad_rows: u64::MAX,
            ..Default::default()
        },
        buf,
    )
    .map_err(|e| Box::new(e.diagnostic()))?;
    Ok(blocks.blocks)
}

/// Every archived file of `month` (see [`MmsdmArchive::archived_files`]);
/// an error when there is none, which says to download the month first.
pub fn month_files(
    archive: &Archive,
    nem: &MmsdmArchive,
    month: YearMonth,
) -> Result<Vec<PathBuf>, CensusError> {
    let files = nem
        .archived_files(archive, month)
        .map_err(|e| CensusError::Read {
            path: month.to_string(),
            message: e.to_string(),
        })?;
    if files.is_empty() {
        let dir = nem
            .month_root(month)
            .map(|u| archive.path_for(u.as_str()).display().to_string())
            .unwrap_or_default();
        return Err(CensusError::NotArchived { month, path: dir });
    }
    Ok(files)
}

/// The file a walked entry belongs to: the innermost zip it was read out
/// of (`MMSDM_2012_01.zip!…/PUBLIC_DVD_X_201201010000.zip!X.CSV` is
/// `PUBLIC_DVD_X_201201010000.zip`), else the file itself.
fn file_key(outer: &Path, entry: &str) -> String {
    match entry.rsplit_once('!') {
        Some((zip, _)) => zip
            .rsplit(['/', '\\', '!'])
            .next()
            .unwrap_or(zip)
            .to_owned(),
        None => outer
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// The blocks of each file of a month, per archived file they were read
/// out of: a month published both as a directory and as a whole-month zip
/// (MMSDM 2020-02) holds most files twice.
#[derive(Default)]
struct Copies(BTreeMap<String, Vec<(String, Vec<RawBlock>)>>);

impl Copies {
    /// Blocks of file `key` read out of the archived file `source`; many
    /// CSVs of one zip add to one copy.
    fn add(&mut self, key: &str, source: &str, blocks: Vec<RawBlock>) {
        let copies = self.0.entry(key.to_owned()).or_default();
        match copies.iter_mut().find(|(s, _)| s == source) {
            Some((_, b)) => b.extend(blocks),
            None => copies.push((source.to_owned(), blocks)),
        }
    }

    /// One entry per file: the copy that is the file itself (a month
    /// directory's zip) else the first by source name. Another copy with
    /// the same blocks (shape, version and rows of each) is the same file
    /// and is dropped; one that differs is kept as `{source}!{key}`.
    fn settle(self) -> BTreeMap<String, (String, Vec<RawBlock>)> {
        let mut out = BTreeMap::new();
        for (key, mut copies) in self.0 {
            copies.sort_by(|a, b| (a.0 != key, &a.0).cmp(&(b.0 != key, &b.0)));
            let mut kept: Vec<Vec<RawBlock>> = Vec::new();
            for (i, (source, blocks)) in copies.into_iter().enumerate() {
                if kept.contains(&blocks) {
                    continue;
                }
                let name = if i == 0 {
                    key.clone()
                } else {
                    format!("{source}!{key}")
                };
                out.insert(name, (source, blocks.clone()));
                kept.push(blocks);
            }
        }
        out
    }
}

/// The census of `month` from its archived `files`: every block of every
/// data file, typed by the control files among them (loose, or inside a
/// whole-month zip). A file that cannot be read is a problem of its own
/// and the census goes on. `tmp` takes nested zips while they are read.
pub fn census_month(
    registry: &Registry,
    month: YearMonth,
    files: &[PathBuf],
    tmp: &Path,
) -> MonthCensus {
    let mut raw = Copies::default();
    let mut sources: BTreeMap<String, &Path> = BTreeMap::new();
    // One read buffer for every file and entry of the month.
    let mut scan_buf = Vec::new();
    let mut ctls: Vec<(String, String)> = Vec::new();
    let mut problems = Vec::new();
    let read_text = |name: String, r: &mut dyn std::io::Read, ctls: &mut Vec<(String, String)>| {
        let mut text = Vec::new();
        if r.read_to_end(&mut text).is_ok() {
            ctls.push((name, String::from_utf8_lossy(&text).into_owned()));
        }
    };
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        // The month's documentation (the data model's create and upgrade
        // scripts and their reports, recorded by `models`) is not market
        // data.
        if path
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("DOCUMENTATION"))
        {
            continue;
        }
        sources.insert(name.clone(), path);
        if lower.ends_with(".ctl") {
            if let Ok(mut f) = std::fs::File::open(path) {
                read_text(name, &mut f, &mut ctls);
            }
        } else if lower.ends_with(".csv") {
            match std::fs::File::open(path).map_err(|e| {
                Diagnostic::error(
                    "schema.census_unreadable",
                    Category::Schema,
                    format!("The archived file {} cannot be read.", path.display()),
                )
                .caused_by(&e)
            }) {
                Ok(f) => match read_blocks(f, &mut scan_buf) {
                    Ok(b) => raw.add(&name, &name, b),
                    Err(d) => {
                        problems.push(d.with_file(path.display().to_string()));
                        raw.add(&name, &name, Vec::new());
                    }
                },
                Err(d) => problems.push(d),
            }
        } else if lower.ends_with(".zip") {
            let walked = utilake_kit::container::walk_csv_entries::<Box<Diagnostic>>(
                path,
                tmp,
                &mut |e| {
                    let key = file_key(path, &e.name);
                    let blocks = read_blocks(e.reader, &mut scan_buf).map_err(|d| {
                        d.with_file(path.display().to_string())
                            .with_entry(e.name.clone())
                    })?;
                    raw.add(&key, &name, blocks);
                    Ok(())
                },
                &mut |e| {
                    let base = e
                        .name
                        .rsplit(['/', '\\', '!'])
                        .next()
                        .unwrap_or(&e.name)
                        .to_owned();
                    if base.to_ascii_lowercase().ends_with(".ctl") {
                        read_text(base, e.reader, &mut ctls);
                    }
                },
            );
            match walked {
                Ok(_) => {}
                // A whole-month zip holds control files beside data zips;
                // a zip of control files only has no CSV, and that is fine.
                Err(utilake_kit::container::WalkError::Archive(
                    utilake_kit::container::ArchiveError::NoCsv { .. },
                )) => {}
                Err(utilake_kit::container::WalkError::Archive(a)) => {
                    problems.push(a.diagnostic());
                    raw.add(&name, &name, Vec::new());
                }
                Err(utilake_kit::container::WalkError::Callback(d)) => {
                    problems.push(*d);
                    raw.add(&name, &name, Vec::new());
                }
            }
        }
    }
    let ctls = Ctls::parse(ctls.iter().map(|(n, t)| (n.as_str(), t.as_str())));
    let files = raw
        .settle()
        .into_iter()
        .map(|(name, (source, blocks))| {
            let file_table = utilake_nem::names::parse(&name).map(|n| n.table);
            let blocks = blocks
                .into_iter()
                .map(|b| block_shape(registry, &ctls, file_table.as_deref(), b))
                .collect();
            let sidecar = sources
                .get(&source)
                .and_then(|p| Archive::read_sidecar(p).ok().flatten());
            let kind = file_kind(&name);
            (
                name,
                FileCensus {
                    sha256: sidecar.as_ref().map(|s| s.sha256.clone()),
                    published: sidecar.and_then(|s| s.last_modified_at.map(|t| t.to_string())),
                    source,
                    kind,
                    blocks,
                },
            )
        })
        .collect();
    MonthCensus {
        month: Some(month),
        files,
        problems,
    }
}

/// A block's shape: its columns typed by the control file of the table
/// that claims its report pair, else of the table its file is named for.
pub fn block_shape(
    registry: &Registry,
    ctls: &Ctls,
    file_table: Option<&str>,
    b: RawBlock,
) -> BlockShape {
    let declared = registry
        .declared_by_pair(&b.report_type, &b.report_subtype)
        .map(|id| registry.table(id));
    let table = declared.map(|t| t.name.as_str());
    let keys: Vec<&str> = table.into_iter().chain(file_table).collect();
    let ctl = ctls.for_table(&keys);
    // The span is of the table's time column when it is there and dated,
    // else of the first time-naming column, else of the first dated column.
    let bounded = |name: &str| {
        b.columns
            .iter()
            .position(|c| c == name)
            .and_then(|j| b.bounds.iter().find(|(i, ..)| *i == j))
    };
    let span = declared
        .and_then(|t| t.time_column.as_deref())
        .and_then(bounded)
        .or_else(|| TARGET_TIME.iter().find_map(|n| bounded(n)))
        .or_else(|| b.bounds.first())
        .map(|(j, min, max)| Span {
            first: String::from_utf8_lossy(min).into_owned(),
            last: String::from_utf8_lossy(max).into_owned(),
            column: b.columns[*j].clone(),
        });
    let class = |column: &str| {
        ctl.and_then(|c| {
            c.data_columns()
                .find(|d| d.name.eq_ignore_ascii_case(column))
                .and_then(|d| d.ty.type_class())
        })
        .unwrap_or(TypeClass::Unknown)
    };
    BlockShape {
        version: b.version,
        rows: b.rows,
        span,
        ctl: b.columns.iter().map(|c| class(c)).collect(),
        profile: b.profile,
        shape: CanonicalShape {
            columns: b.columns,
            report_type: b.report_type,
            report_subtype: b.report_subtype,
        },
    }
}

/// The kind of a file by its name: `archive` (`PUBLIC_ARCHIVE#…`), `dvd`
/// (`PUBLIC_DVD_…`) or `other`, with `#all` when it holds every run of a
/// forecast (`#ALL#`, `_ALL_`).
pub fn file_kind(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    let naming = if upper.starts_with("PUBLIC_ARCHIVE#") {
        "archive"
    } else if upper.starts_with("PUBLIC_DVD_") {
        "dvd"
    } else {
        "other"
    };
    if upper.contains("#ALL#") || upper.contains("_ALL_") {
        format!("{naming}#all")
    } else {
        naming.to_owned()
    }
}

/// One file's entry in a month's census.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    /// The archived file it was read out of.
    pub source: String,
    /// The sha256 of that archived file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// When AEMO published it, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// `dvd`, `archive` or `other`, with `#all` for a file of every run.
    pub kind: String,
    /// Its blocks, in file order.
    #[serde(default)]
    pub block: Vec<BlockRecord>,
}

/// One block of a file in a month's census: its shape, and per column (in
/// column order) what the control file declared and what the values were.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockRecord {
    /// Its shape.
    pub shape: ShapeId,
    /// Its report type and subtype.
    pub report: (String, String),
    /// The version AEMO stamped on it.
    pub version: u16,
    /// Its D rows.
    pub rows: u64,
    /// The time column its span is of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// The least value of that column, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    /// The greatest value of that column, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    /// The column names.
    pub columns: Vec<String>,
    /// The declared class of each column, one letter each
    /// (`TypeClass::letter`).
    pub ctl: String,
    /// The observed kind of each column, one letter each (`Kind::letter`).
    pub kinds: String,
    /// Rows without a value, per column.
    pub nulls: Vec<u64>,
    /// The most digits before the point, per column.
    pub int_digits: Vec<u8>,
    /// The most digits after it, per column.
    pub scale: Vec<u8>,
    /// The most significant digits of any one value, per column.
    #[serde(default)]
    pub sig_digits: Vec<u8>,
    /// The longest value in bytes, per column.
    pub max_len: Vec<u32>,
    /// The least number or time, per column; empty for none.
    pub min: Vec<String>,
    /// The greatest number or time, per column; empty for none.
    pub max: Vec<String>,
}

impl BlockRecord {
    /// The record of a block.
    pub fn of(b: &BlockShape) -> Self {
        let text = |v: &Option<Vec<u8>>| {
            v.as_deref()
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .unwrap_or_default()
        };
        Self {
            shape: b.shape.id(),
            report: (b.shape.report_type.clone(), b.shape.report_subtype.clone()),
            version: b.version,
            rows: b.rows,
            time: b.span.as_ref().map(|s| s.column.clone()),
            first: b.span.as_ref().map(|s| s.first.clone()),
            last: b.span.as_ref().map(|s| s.last.clone()),
            columns: b.shape.columns.clone(),
            ctl: b.ctl.iter().map(|c| c.letter()).collect(),
            kinds: b.profile.iter().map(|p| p.kind.letter()).collect(),
            nulls: b.profile.iter().map(|p| p.nulls).collect(),
            int_digits: b.profile.iter().map(|p| p.int_digits).collect(),
            scale: b.profile.iter().map(|p| p.scale).collect(),
            sig_digits: b.profile.iter().map(|p| p.sig_digits).collect(),
            max_len: b.profile.iter().map(|p| p.max_len).collect(),
            min: b.profile.iter().map(|p| text(&p.min)).collect(),
            max: b.profile.iter().map(|p| text(&p.max)).collect(),
        }
    }

    /// The declared class of column `j`.
    pub fn ctl_of(&self, j: usize) -> TypeClass {
        self.ctl
            .chars()
            .nth(j)
            .and_then(TypeClass::of_letter)
            .unwrap_or(TypeClass::Unknown)
    }

    /// The profile of column `j`, as recorded.
    pub fn profile_of(&self, j: usize) -> ColumnProfile {
        let some = |v: Option<&String>| v.filter(|s| !s.is_empty()).map(|s| s.as_bytes().to_vec());
        ColumnProfile {
            nulls: self.nulls.get(j).copied().unwrap_or(0),
            kind: self
                .kinds
                .chars()
                .nth(j)
                .and_then(utilake_iceberg::Kind::of_letter)
                .unwrap_or_default(),
            int_digits: self.int_digits.get(j).copied().unwrap_or(0),
            scale: self.scale.get(j).copied().unwrap_or(0),
            sig_digits: self.sig_digits.get(j).copied().unwrap_or(0),
            max_len: self.max_len.get(j).copied().unwrap_or(0),
            min: some(self.min.get(j)),
            max: some(self.max.get(j)),
        }
    }
}

impl FileRecord {
    /// The record of a file.
    pub fn of(f: &FileCensus) -> Self {
        Self {
            source: f.source.clone(),
            sha256: f.sha256.clone(),
            published: f.published.clone(),
            kind: f.kind.clone(),
            block: f.blocks.iter().map(BlockRecord::of).collect(),
        }
    }

    /// The ids, in file order.
    pub fn ids(&self) -> Vec<ShapeId> {
        self.block.iter().map(|b| b.shape).collect()
    }
}

/// The directory of a market's census files under the schemas root.
pub fn census_dir(root: &Path) -> PathBuf {
    root.join("nem").join("census")
}

/// The directory of a market's shapes files under the schemas root.
pub fn shapes_dir(root: &Path) -> PathBuf {
    root.join("nem").join("shapes")
}

/// Where each table's history as an Iceberg table is written.
#[must_use]
pub fn history_dir(root: &Path) -> PathBuf {
    root.join("nem").join("iceberg")
}

/// The census file of `month`.
pub fn census_path(root: &Path, month: YearMonth) -> PathBuf {
    census_dir(root).join(format!("{month}.toml"))
}

fn read_text(path: &Path) -> Result<Option<String>, CensusError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CensusError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        }),
    }
}

pub(crate) fn write_text(path: &Path, text: &str) -> Result<(), CensusError> {
    let err = |source| CensusError::Write {
        path: path.display().to_string(),
        source,
    };
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(err)?;
    }
    std::fs::write(path, text).map_err(err)
}

/// The census of `month` as recorded, file name to entry.
pub fn read_month(
    root: &Path,
    month: YearMonth,
) -> Result<Option<BTreeMap<String, FileRecord>>, CensusError> {
    let path = census_path(root, month);
    let Some(text) = read_text(&path)? else {
        return Ok(None);
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| CensusError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        })
}

/// Every shape recorded for `table`.
pub fn read_shapes(root: &Path, table: &str) -> Result<Vec<ShapeEntry>, CensusError> {
    let path = shapes_dir(root).join(format!("{table}.toml"));
    let Some(text) = read_text(&path)? else {
        return Ok(Vec::new());
    };
    parse_shapes(&path, &text).map_err(|e| CensusError::Read {
        path: path.display().to_string(),
        message: e.to_string(),
    })
}

/// The table a block of this report pair is recorded under: the one the
/// registry declares for the pair, else `{TYPE}_{SUBTYPE}`.
pub fn table_of(registry: &Registry, report_type: &str, report_subtype: &str) -> String {
    registry
        .declared_by_pair(report_type, report_subtype)
        .map_or_else(
            || format!("{report_type}_{report_subtype}"),
            |id| registry.table(id).name.clone(),
        )
}

/// Records a month's census in its census file; the shapes are folded from
/// every census file afterwards (`crate::fold`). Returns how many blocks
/// were recorded.
pub fn record(root: &Path, census: &MonthCensus) -> Result<usize, CensusError> {
    let month = census.month.expect("a month's census knows its month");
    let entries: BTreeMap<&str, FileRecord> = census
        .files
        .iter()
        .map(|(name, f)| (name.as_str(), FileRecord::of(f)))
        .collect();
    let text = format!(
        "# Generated by `utilake-nem-census census`: every block of every file of {month}, where the file came from, and what each column held.\n{}",
        toml::to_string(&entries).expect("a census serialises")
    );
    write_text(&census_path(root, month), &text)?;
    Ok(census.files.values().map(|f| f.blocks.len()).sum())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write as _;

    use super::*;

    pub(crate) const SCADA_CTL: &str = r#"LOAD DATA INFILE "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.CSV" APPEND INTO TABLE DISPATCH_UNIT_SCADA
FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '"' TRAILING NULLCOLS
(row_type FILLER, report_type FILLER, report_subtype FILLER, report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", DUID CHAR(20), SCADAVALUE FLOAT EXTERNAL)
"#;

    pub(crate) fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            for (name, bytes) in entries {
                w.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        cursor.into_inner()
    }

    /// A file of three tables, the second and third starting after more
    /// than 4 MiB of the first: what a head-only census never saw.
    pub(crate) fn three_tables(scada_rows: usize) -> Vec<u8> {
        let mut csv = b"C,NEMP.WORLD,DVD,AEMO,PUBLIC,2026/09/01,00:00:00,,,\r\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\n".to_vec();
        for i in 0..scada_rows {
            csv.extend_from_slice(
                format!("D,DISPATCH,UNIT_SCADA,1,\"2026/08/01 00:05:00\",UNIT{i:08},{i}.5\r\n")
                    .as_bytes(),
            );
        }
        csv.extend_from_slice(b"I,UTILAKE_TEST,EXTRA,2,A,B\r\nD,UTILAKE_TEST,EXTRA,2,x,y\r\nD,UTILAKE_TEST,EXTRA,2,x,z\r\n");
        csv.extend_from_slice(b"I,UTILAKE_TEST,LAST,1,C\r\nD,UTILAKE_TEST,LAST,1,q\r\n");
        csv.extend_from_slice(format!("C,\"END OF REPORT\",{}\r\n", scada_rows + 8).as_bytes());
        csv
    }

    /// Archives `bytes` as the download of `url`, sidecar and all.
    pub(crate) fn put(store: &Archive, url: &str, bytes: &[u8]) {
        let path = store.path_for(url);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let sidecar = utilake_acquire::Sidecar {
            url: url.to_owned(),
            size: bytes.len() as u64,
            sha256: utilake_acquire::archive::sha256_hex(bytes),
            fetched_at: "2026-10-05T00:00:00Z".into(),
            etag: None,
            last_modified: None,
            last_modified_at: None,
            listed_modified: None,
            listed_modified_at: None,
            tool: "test".into(),
        };
        Archive::write_sidecar(&path, &sidecar).unwrap();
    }

    /// The span of a block is the least and greatest value of its time
    /// column over every row, whatever order the rows come in; the column
    /// is the table's declared one, else the first time-naming one, else
    /// the first dated one.
    #[test]
    fn a_block_spans_its_rows_by_the_time_column() {
        let reg = Registry::load_embedded();
        let csv = b"C,x,y\r\n\
I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE,LASTCHANGED\r\n\
D,DISPATCH,UNIT_SCADA,1,\"2026/08/02 00:05:00\",U1,1.5,\"2026/09/01 10:00:00\"\r\n\
D,DISPATCH,UNIT_SCADA,1,\"2026/08/01 00:05:00\",U2,2.5,\"2026/09/02 10:00:00\"\r\n\
D,DISPATCH,UNIT_SCADA,1,\"2026/08/31 23:55:00\",U3,3.5,\"2026/08/31 10:00:00\"\r\n\
I,UTILAKE_TEST,BYVOCAB,1,PRICE,TRADINGDATE,EFFECTIVEDATE\r\n\
D,UTILAKE_TEST,BYVOCAB,1,1,2026/08/03,2020/01/01\r\n\
D,UTILAKE_TEST,BYVOCAB,1,2,2026/08/01,2021/01/01\r\n\
I,UTILAKE_TEST,FIRSTDATED,1,A,WHEN,B\r\n\
D,UTILAKE_TEST,FIRSTDATED,1,x,2026/08/05,2026/08/09\r\n\
D,UTILAKE_TEST,FIRSTDATED,1,y,2026/08/04,2026/08/08\r\n\
I,UTILAKE_TEST,UNDATED,1,A,B\r\n\
D,UTILAKE_TEST,UNDATED,1,1,2\r\n\
C,\"END OF REPORT\",14\r\n";
        let mut buf = Vec::new();
        let blocks = read_blocks(&csv[..], &mut buf).unwrap();
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].rows, 3);
        assert_eq!(
            blocks[0].bounds.len(),
            2,
            "SETTLEMENTDATE and LASTCHANGED are dated"
        );
        // Every column is profiled from its rows, quoted or not.
        let kinds: Vec<_> = blocks[0].profile.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                utilake_iceberg::Kind::Timestamp,
                utilake_iceberg::Kind::Text,
                utilake_iceberg::Kind::Decimal,
                utilake_iceberg::Kind::Timestamp
            ]
        );
        assert_eq!(blocks[1].profile[1].kind, utilake_iceberg::Kind::Date);
        assert_eq!(blocks[3].profile[0].nulls, 0);
        let ctls = Ctls::parse([(
            "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl",
            SCADA_CTL,
        )]);
        let spans: Vec<Option<Span>> = blocks
            .into_iter()
            .map(|b| block_shape(&reg, &ctls, None, b).span)
            .collect();
        assert_eq!(
            spans[0],
            Some(Span {
                first: "2026/08/01 00:05:00".into(),
                last: "2026/08/31 23:55:00".into(),
                column: "SETTLEMENTDATE".into()
            }),
            "the declared time column, min and max over unordered rows"
        );
        assert_eq!(
            spans[1],
            Some(Span {
                first: "2026/08/01".into(),
                last: "2026/08/03".into(),
                column: "TRADINGDATE".into()
            }),
            "no declaration: TRADINGDATE outranks EFFECTIVEDATE in the vocabulary"
        );
        assert_eq!(
            spans[2],
            Some(Span {
                first: "2026/08/04".into(),
                last: "2026/08/05".into(),
                column: "WHEN".into()
            }),
            "no known name: the first dated column"
        );
        assert_eq!(spans[3], None);
        assert!(date_like(b"2026/08/01") && !date_like(b"2026-08-01") && !date_like(b"1.5"));
    }

    #[test]
    fn every_block_of_every_archived_file_is_recorded_with_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = Archive::new(dir.path());
        let nem = MmsdmArchive::new("https://h").unwrap();
        let reg = Registry::load_embedded();
        let month = YearMonth::new(2026, 8).unwrap();
        let root = nem.month_root(month).unwrap();
        // Listings give `#` as `%23`; the archive keeps the decoded name.
        let at = |rel: &str| root.join(&rel.replace('#', "%23")).unwrap().to_string();
        let data = |name: &str| at(&format!("DATA/{name}"));
        let scada_rows = 110_000;
        let csv = three_tables(scada_rows);
        assert!(csv.len() > 4 << 20);
        let zip_name = "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.zip";
        put(&store, &data(zip_name), &zip_of(&[("A.CSV", &csv)]));
        // A partial download, and a file without a sidecar, are not
        // archived files of the month.
        let stray = store.path_for(&data("PUBLIC_ARCHIVE#X#FILE01#202608010000.zip"));
        std::fs::write(Archive::part_path(&stray), b"x").unwrap();
        std::fs::write(&stray, b"x").unwrap();
        put(
            &store,
            &data("PUBLIC_ARCHIVE#BAD#FILE01#202608010000.zip"),
            b"not a zip",
        );
        put(
            &store,
            &at("CTL/PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl"),
            SCADA_CTL.as_bytes(),
        );
        // The month's documentation is not market data.
        put(
            &store,
            &at("DOCUMENTATION/MMS%20Data%20Model/v5.6/MMSDM_create_v5.6.zip"),
            &zip_of(&[("PDR.CSV", &three_tables(1))]),
        );
        let files = month_files(&store, &nem, month).unwrap();
        assert_eq!(files.len(), 4, "{files:?}");
        let tmp = tempfile::tempdir().unwrap();
        let census = census_month(&reg, month, &files, tmp.path());
        assert_eq!(census.problems.len(), 1, "{:?}", census.problems);
        assert!(!census.files.contains_key("MMSDM_create_v5.6.zip"));
        assert!(
            census.files["PUBLIC_ARCHIVE#BAD#FILE01#202608010000.zip"]
                .blocks
                .is_empty(),
            "{:?}",
            census.files.keys().collect::<Vec<_>>()
        );
        let file = &census.files[zip_name];
        assert_eq!(
            (file.source.as_str(), file.kind.as_str()),
            (zip_name, "archive")
        );
        assert_eq!(
            file.sha256.as_ref().map(String::len),
            Some(64),
            "from the sidecar"
        );
        let blocks = &file.blocks;
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks.iter().map(|b| b.rows).collect::<Vec<_>>(),
            [scada_rows as u64, 2, 1]
        );
        assert_eq!(
            blocks[0].span,
            Some(Span {
                first: "2026/08/01 00:05:00".into(),
                last: "2026/08/01 00:05:00".into(),
                column: "SETTLEMENTDATE".into()
            })
        );
        assert!(
            blocks[1].span.is_none() && blocks[2].span.is_none(),
            "no dated column"
        );
        assert_eq!(
            blocks[0].shape.columns,
            ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]
        );
        assert_eq!(
            blocks[0].ctl,
            [TypeClass::Timestamp, TypeClass::Varchar, TypeClass::Number]
        );
        assert_eq!(blocks[0].profile[2].kind, utilake_iceberg::Kind::Decimal);
        assert_eq!(blocks[1].shape.report_subtype, "EXTRA");
        assert_eq!(blocks[2].shape.report_subtype, "LAST");
        assert!(blocks[2].ctl.iter().all(|c| *c == TypeClass::Unknown));

        // Recorded: every block with its rows and profile, read back as
        // it was; the fold gives each table's shape once, from the first
        // month to the last.
        let schemas = dir.path().join("schemas");
        assert_eq!(record(&schemas, &census).unwrap(), 3);
        let recorded = read_month(&schemas, month).unwrap().unwrap();
        assert_eq!(recorded[zip_name], FileRecord::of(file));
        assert_eq!(recorded[zip_name].ids().len(), 3);
        let b = &recorded[zip_name].block[0];
        assert_eq!(b.profile_of(2), blocks[0].profile[2]);
        assert_eq!(b.ctl_of(0), TypeClass::Timestamp);
        assert_eq!(b.ctl_of(9), TypeClass::Unknown);
        assert_eq!(b.profile_of(9), ColumnProfile::default());
        let mut earlier = census.clone();
        earlier.month = YearMonth::new(2026, 6);
        assert_eq!(record(&schemas, &earlier).unwrap(), 3);
        let folded = crate::fold::fold(&schemas, &reg).unwrap();
        assert_eq!((folded.months, folded.tables, folded.shapes), (2, 3, 3));
        let shapes = read_shapes(&schemas, "DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].first_seen.to_string(), "2026-06");
        assert_eq!(shapes[0].last_seen.to_string(), "2026-08");
        assert_eq!(shapes[0].files, 2);
        assert_eq!(shapes[0].rows, 2 * scada_rows as u64);
        assert_eq!(
            shapes[0].from.data_time.as_deref(),
            Some("2026-08-01T00:05:00+10:00")
        );
        assert_eq!(read_shapes(&schemas, "UTILAKE_TEST_LAST").unwrap().len(), 1);
        assert!(read_shapes(&schemas, "NOBODY").unwrap().is_empty());
        // The same census again folds to the same bytes; a month removed
        // takes its months away.
        let before =
            std::fs::read_to_string(shapes_dir(&schemas).join("DISPATCH_UNIT_SCADA.toml")).unwrap();
        crate::fold::fold(&schemas, &reg).unwrap();
        assert_eq!(
            std::fs::read_to_string(shapes_dir(&schemas).join("DISPATCH_UNIT_SCADA.toml")).unwrap(),
            before
        );
        std::fs::remove_file(census_path(&schemas, YearMonth::new(2026, 6).unwrap())).unwrap();
        crate::fold::fold(&schemas, &reg).unwrap();
        let shapes = read_shapes(&schemas, "DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(shapes[0].first_seen.to_string(), "2026-08");
        // A month not in the archive says to download it.
        let e = month_files(&store, &nem, YearMonth::new(2020, 1).unwrap()).unwrap_err();
        assert_eq!(e.diagnostic().code, "schema.census_not_archived");
        assert!(e.to_string().contains("utilake fetch nem --from 2020-01"));
    }

    #[test]
    fn a_whole_month_zip_is_read_through_its_nested_zips_and_control_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Archive::new(dir.path());
        let nem = MmsdmArchive::new("https://h").unwrap();
        let reg = Registry::load_embedded();
        let month = YearMonth::new(2012, 1).unwrap();
        let inner = zip_of(&[(
            "PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.CSV",
            &three_tables(3),
        )]);
        let ctl = SCADA_CTL.replace(
            "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.CSV",
            "PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.CSV",
        );
        let prefix = "MMSDM_2012_01/MMSDM_Historical_Data_SQLLoader";
        let whole = zip_of(&[
            (
                &format!("{prefix}/DOCUMENTATION/README.txt"),
                b"Static/Standing data tables".as_slice(),
            ),
            (
                &format!("{prefix}/CTL/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201.ctl"),
                ctl.as_bytes(),
            ),
            (
                &format!("{prefix}/DATA/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.zip"),
                &inner,
            ),
        ]);
        put(
            &store,
            "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2012/MMSDM_2012_01.zip",
            &whole,
        );
        let files = month_files(&store, &nem, month).unwrap();
        assert_eq!(files.len(), 1);
        let tmp = tempfile::tempdir().unwrap();
        let census = census_month(&reg, month, &files, tmp.path());
        assert!(census.problems.is_empty(), "{:?}", census.problems);
        let file = &census.files["PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.zip"];
        assert_eq!(
            (file.source.as_str(), file.kind.as_str()),
            ("MMSDM_2012_01.zip", "dvd")
        );
        let blocks = &file.blocks;
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].rows, 3);
        assert_eq!(blocks[0].ctl[0], TypeClass::Timestamp);

        // The fixtures of the month come out of the same zip.
        let out = dir.path().join("fixtures");
        let w = crate::fixtures::write_fixtures(&files, tmp.path(), &out, 2).unwrap();
        assert!(w.problems.is_empty(), "{:?}", w.problems);
        assert_eq!((w.heads, w.ctls, w.readme), (1, 1, true));
        let head = std::fs::read_to_string(
            out.join("heads/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.CSV"),
        )
        .unwrap();
        assert_eq!(head.matches("\nI,").count(), 3, "{head}");
        assert_eq!(head.matches("D,DISPATCH,UNIT_SCADA").count(), 2);
        assert!(
            out.join("CTL/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201.ctl")
                .is_file()
        );
        assert!(out.join("README.txt").is_file());
    }

    #[test]
    fn a_month_both_a_directory_and_a_zip_is_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Archive::new(dir.path());
        let nem = MmsdmArchive::new("https://h").unwrap();
        let reg = Registry::load_embedded();
        let month = YearMonth::new(2020, 2).unwrap();
        let root = nem.month_root(month).unwrap();
        let data = |name: &str| {
            root.join(&format!("MMSDM_Historical_Data_SQLLoader/DATA/{name}"))
                .unwrap()
                .to_string()
        };
        let same = "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202002010000.zip";
        let other = "PUBLIC_DVD_P5MIN_X_202002010000.zip";
        let scada = zip_of(&[("A.CSV", &three_tables(3))]);
        put(&store, &data(same), &scada);
        put(
            &store,
            &data(other),
            &zip_of(&[("B.CSV", &three_tables(2))]),
        );
        // The whole-month zip names its entries with backslashes; one of
        // its copies differs from the directory's.
        let prefix = r"MMSDM_2020_02\MMSDM_Historical_Data_SQLLoader\DATA";
        let whole = zip_of(&[
            (&format!(r"{prefix}\{same}"), scada.as_slice()),
            (
                &format!(r"{prefix}\{other}"),
                &zip_of(&[("B.CSV", &three_tables(5))]),
            ),
        ]);
        put(
            &store,
            "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2020/MMSDM_2020_02.zip",
            &whole,
        );
        let files = month_files(&store, &nem, month).unwrap();
        assert_eq!(files.len(), 3, "{files:?}");
        let tmp = tempfile::tempdir().unwrap();
        let census = census_month(&reg, month, &files, tmp.path());
        assert!(census.problems.is_empty(), "{:?}", census.problems);
        let names: Vec<&str> = census.files.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [&format!("MMSDM_2020_02.zip!{other}"), same, other],
            "the same copy once, a differing copy under its source"
        );
        assert_eq!(census.files[same].blocks.len(), 3);
        assert_eq!(census.files[other].blocks[0].rows, 2);
        let copy = &census.files[&format!("MMSDM_2020_02.zip!{other}")];
        assert_eq!(
            (copy.blocks[0].rows, copy.source.as_str()),
            (5, "MMSDM_2020_02.zip")
        );
    }

    #[test]
    fn names_tell_kinds_and_tables() {
        assert_eq!(
            file_kind("PUBLIC_ARCHIVE#DISPATCHLOAD#FILE01#202608010000.zip"),
            "archive"
        );
        assert_eq!(
            file_kind("PUBLIC_ARCHIVE#PREDISPATCHPRICE#ALL#FILE01#202608010000.zip"),
            "archive#all"
        );
        assert_eq!(
            file_kind("PUBLIC_DVD_P5MIN_REGIONSOLUTION_ALL_201901010000.zip"),
            "dvd#all"
        );
        assert_eq!(file_kind("MMSDM_2020_02.zip"), "other");
        assert!(ctl_table("readme.txt").is_none());
        assert!(ctl_serves("PREDISPATCHPRICE_D", "PREDISPATCHPRICE", true));
        assert!(!ctl_serves("REGIONAPC", "REGION", true));
        assert_eq!(table_keys("BIDPEROFFER1"), ["BIDPEROFFER", "BIDPEROFFER1"]);
    }
}
