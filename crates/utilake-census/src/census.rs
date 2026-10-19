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
use utilake_mms_registry::file_model::shapes::{ShapeEntry, parse_shapes, render_shapes};
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
    /// The block's shape.
    pub shape: CanonicalShape,
    /// The version AEMO stamped on it.
    pub version: u16,
    /// Its D rows.
    pub rows: u64,
}

/// The census of one month: per file name, its blocks in file order. A
/// file that could not be read has none, and `problems` says why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonthCensus {
    /// The month.
    pub month: Option<YearMonth>,
    /// File name to blocks.
    pub files: BTreeMap<String, Vec<BlockShape>>,
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
    fn for_table(&self, tables: &[&str]) -> Option<&CtlFile> {
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
}

/// A sink that keeps only the I rows of a header-only scan.
#[derive(Default)]
struct Blocks(Vec<RawBlock>);

impl Sink for Blocks {
    fn comment(&mut self, _: &Row<'_>, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
    fn header(&mut self, irow: &IRow<'_>, _: Pos) -> Result<(), SinkError> {
        self.0.push(RawBlock {
            report_type: irow.report_type.to_owned(),
            report_subtype: irow.report_subtype.to_owned(),
            version: irow.version,
            columns: irow.columns.iter().map(|c| (*c).to_owned()).collect(),
            rows: 0,
        });
        Ok(())
    }
    fn row(&mut self, _: &Row<'_>, _: Pos) -> Result<(), RowFailure> {
        Ok(())
    }
    fn bad_row(&mut self, _: &[u8], _: Pos, _: &RowError) -> Flow {
        Flow::Continue
    }
    fn footer(&mut self, _: Option<u64>, _: u64, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
    fn block_rows(&mut self, rows: u64) {
        if let Some(b) = self.0.last_mut() {
            b.rows = rows;
        }
    }
}

/// Every block of one CSV stream, in order: I rows only, D rows counted.
pub fn read_blocks(
    reader: impl std::io::Read,
    buf: &mut Vec<u8>,
) -> Result<Vec<RawBlock>, Box<Diagnostic>> {
    let mut blocks = Blocks::default();
    scan_with(
        reader,
        &mut blocks,
        &ScanOptions {
            headers_only: true,
            max_bad_rows: u64::MAX,
            ..Default::default()
        },
        buf,
    )
    .map_err(|e| Box::new(e.diagnostic()))?;
    Ok(blocks.0)
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
        Some((zip, _)) => zip.rsplit(['/', '!']).next().unwrap_or(zip).to_owned(),
        None => outer
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
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
    let mut raw: BTreeMap<String, Vec<RawBlock>> = BTreeMap::new();
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
                    Ok(b) => {
                        raw.insert(name, b);
                    }
                    Err(d) => {
                        problems.push(d.with_file(path.display().to_string()));
                        raw.insert(name, Vec::new());
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
                    raw.entry(key).or_default().extend(blocks);
                    Ok(())
                },
                &mut |e| {
                    let base = e
                        .name
                        .rsplit(['/', '!'])
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
                    raw.entry(name).or_default();
                }
                Err(utilake_kit::container::WalkError::Callback(d)) => {
                    problems.push(*d);
                    raw.entry(name).or_default();
                }
            }
        }
    }
    let ctls = Ctls::parse(ctls.iter().map(|(n, t)| (n.as_str(), t.as_str())));
    let files = raw
        .into_iter()
        .map(|(name, blocks)| {
            let file_table = utilake_nem::names::parse(&name).map(|n| n.table);
            let shapes = blocks
                .into_iter()
                .map(|b| block_shape(registry, &ctls, file_table.as_deref(), b))
                .collect();
            (name, shapes)
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
    let table = registry
        .declared_by_pair(&b.report_type, &b.report_subtype)
        .map(|id| registry.table(id).name.as_str());
    let keys: Vec<&str> = table.into_iter().chain(file_table).collect();
    let ctl = ctls.for_table(&keys);
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
        shape: CanonicalShape {
            columns: b.columns.iter().map(|c| (c.clone(), class(c))).collect(),
            report_type: b.report_type,
            report_subtype: b.report_subtype,
        },
    }
}

/// One file's entry in a month's census: its blocks, in file order. The
/// census of a month taken before row counts were kept has only ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileEntry {
    /// A file of one block, without its rows.
    One(ShapeId),
    /// A file of several blocks, or none that could be read, without rows.
    Many(Vec<ShapeId>),
    /// Every block with its rows.
    Blocks(Vec<BlockEntry>),
}

/// One block of a file in a month's census.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEntry {
    /// Its shape.
    pub shape: ShapeId,
    /// Its D rows.
    pub rows: u64,
}

impl FileEntry {
    /// The ids, in file order.
    pub fn ids(&self) -> Vec<ShapeId> {
        match self {
            Self::One(id) => vec![*id],
            Self::Many(ids) => ids.clone(),
            Self::Blocks(blocks) => blocks.iter().map(|b| b.shape).collect(),
        }
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

fn write_text(path: &Path, text: &str) -> Result<(), CensusError> {
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
) -> Result<Option<BTreeMap<String, FileEntry>>, CensusError> {
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

/// Records a month's census: its census file, and every shape it saw in
/// its table's shapes file. Returns how many shapes were new.
pub fn record(
    root: &Path,
    registry: &Registry,
    census: &MonthCensus,
) -> Result<usize, CensusError> {
    let month = census.month.expect("a month's census knows its month");
    let mut entries: BTreeMap<&str, FileEntry> = BTreeMap::new();
    let mut by_table: BTreeMap<String, Vec<&BlockShape>> = BTreeMap::new();
    for (name, blocks) in &census.files {
        entries.insert(
            name,
            FileEntry::Blocks(
                blocks
                    .iter()
                    .map(|b| BlockEntry {
                        shape: b.shape.id(),
                        rows: b.rows,
                    })
                    .collect(),
            ),
        );
        for b in blocks {
            let s = &b.shape;
            let table = registry
                .declared_by_pair(&s.report_type, &s.report_subtype)
                .map_or_else(
                    || format!("{}_{}", s.report_type, s.report_subtype),
                    |id| registry.table(id).name.clone(),
                );
            by_table.entry(table).or_default().push(b);
        }
    }
    let text = format!(
        "# Generated by `utilake-census census`: the blocks of every file of {month}.\n{}",
        toml::to_string(&entries).expect("a census serialises")
    );
    let mut added = 0;
    for (table, blocks) in by_table {
        let mut shapes = read_shapes(root, &table)?;
        for b in blocks {
            let id = b.shape.id();
            match shapes
                .iter_mut()
                .find(|e| e.id == id && e.version == b.version)
            {
                Some(e) => {
                    e.first_seen = e.first_seen.min(month);
                    e.last_seen = e.last_seen.max(month);
                }
                None => {
                    added += 1;
                    shapes.push(ShapeEntry {
                        id,
                        report_type: b.shape.report_type.clone(),
                        report_subtype: b.shape.report_subtype.clone(),
                        version: b.version,
                        columns: b.shape.columns.clone(),
                        first_seen: month,
                        last_seen: month,
                    });
                }
            }
        }
        shapes.sort_by_key(|e| (e.first_seen, e.version, e.id));
        write_text(
            &shapes_dir(root).join(format!("{table}.toml")),
            &render_shapes(&shapes),
        )?;
    }
    // The census file last: a month is done once it exists.
    write_text(&census_path(root, month), &text)?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    const SCADA_CTL: &str = r#"LOAD DATA INFILE "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.CSV" APPEND INTO TABLE DISPATCH_UNIT_SCADA
FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '"' TRAILING NULLCOLS
(row_type FILLER, report_type FILLER, report_subtype FILLER, report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", DUID CHAR(20), SCADAVALUE FLOAT EXTERNAL)
"#;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
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
    fn three_tables(scada_rows: usize) -> Vec<u8> {
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
    fn put(store: &Archive, url: &str, bytes: &[u8]) {
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
        let files = month_files(&store, &nem, month).unwrap();
        assert_eq!(files.len(), 3, "{files:?}");
        let tmp = tempfile::tempdir().unwrap();
        let census = census_month(&reg, month, &files, tmp.path());
        assert_eq!(census.problems.len(), 1, "{:?}", census.problems);
        assert!(
            census.files["PUBLIC_ARCHIVE#BAD#FILE01#202608010000.zip"].is_empty(),
            "{:?}",
            census.files.keys().collect::<Vec<_>>()
        );
        let blocks = &census.files[zip_name];
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks.iter().map(|b| b.rows).collect::<Vec<_>>(),
            [scada_rows as u64, 2, 1]
        );
        assert_eq!(
            blocks[0].shape.columns,
            [
                ("SETTLEMENTDATE".to_owned(), TypeClass::Timestamp),
                ("DUID".to_owned(), TypeClass::Varchar),
                ("SCADAVALUE".to_owned(), TypeClass::Number)
            ]
        );
        assert_eq!(blocks[1].shape.report_subtype, "EXTRA");
        assert_eq!(blocks[2].shape.report_subtype, "LAST");
        assert!(
            blocks[2]
                .shape
                .columns
                .iter()
                .all(|(_, c)| *c == TypeClass::Unknown)
        );

        // Recorded: every block with its rows; each table's shapes file
        // holds its shape once; a second month only moves the months seen.
        let schemas = dir.path().join("schemas");
        assert_eq!(record(&schemas, &reg, &census).unwrap(), 3);
        let recorded = read_month(&schemas, month).unwrap().unwrap();
        assert_eq!(
            recorded[zip_name],
            FileEntry::Blocks(
                blocks
                    .iter()
                    .map(|b| BlockEntry {
                        shape: b.shape.id(),
                        rows: b.rows
                    })
                    .collect()
            )
        );
        let mut earlier = census.clone();
        earlier.month = YearMonth::new(2026, 6);
        assert_eq!(record(&schemas, &reg, &earlier).unwrap(), 0);
        let shapes = read_shapes(&schemas, "DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].first_seen.to_string(), "2026-06");
        assert_eq!(shapes[0].last_seen.to_string(), "2026-08");
        assert_eq!(read_shapes(&schemas, "UTILAKE_TEST_LAST").unwrap().len(), 1);
        assert!(read_shapes(&schemas, "NOBODY").unwrap().is_empty());
        // The same census again writes the same bytes.
        let before =
            std::fs::read_to_string(shapes_dir(&schemas).join("DISPATCH_UNIT_SCADA.toml")).unwrap();
        record(&schemas, &reg, &census).unwrap();
        assert_eq!(
            std::fs::read_to_string(shapes_dir(&schemas).join("DISPATCH_UNIT_SCADA.toml")).unwrap(),
            before
        );
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
        let blocks = &census.files["PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.zip"];
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].rows, 3);
        assert_eq!(blocks[0].shape.columns[0].1, TypeClass::Timestamp);

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
    fn a_census_recorded_before_row_counts_still_reads() {
        let old: BTreeMap<String, FileEntry> =
            toml::from_str("\"a.zip\" = \"9c77fb0f8b590880\"\n\"b.zip\" = [\"9c77fb0f8b590880\", \"af3df42662b718de\"]\n")
                .unwrap();
        assert_eq!(old["a.zip"].ids().len(), 1);
        assert_eq!(old["b.zip"].ids().len(), 2);
        assert!(ctl_table("readme.txt").is_none());
        assert!(ctl_serves("PREDISPATCHPRICE_D", "PREDISPATCHPRICE", true));
        assert!(!ctl_serves("REGIONAPC", "REGION", true));
        assert_eq!(table_keys("BIDPEROFFER1"), ["BIDPEROFFER", "BIDPEROFFER1"]);
    }
}
