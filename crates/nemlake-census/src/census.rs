//! The census: which shape every file of every month had.
//!
//! For each month the archive's file list is taken, and of each file only
//! its head: enough of the zip to read every I row near its start (see
//! [`read_blocks`]). Each block's columns are joined with the types the
//! month's control files load them as, which gives the block's
//! [`CanonicalShape`] and its [`ShapeId`]. Two files are written, both
//! metadata only, in an order that does not depend on the order of work:
//!
//! - `{root}/census/{YYYY-MM}.toml`: every file of the month and the id of
//!   each of its blocks;
//! - `{root}/shapes/{TABLE}.toml`: every distinct shape of a table, once,
//!   with the months it was first and last seen. A shape once recorded is
//!   never removed.
//!
//! A month whose census file exists is done; running again continues with
//! the months that are not.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nemlake_acquire::{FetchError, Fetcher, MmsdmArchive, Query, RemoteFile, Source};
use nemlake_core::{CanonicalShape, Category, Diagnose, Diagnostic, ShapeId, TypeClass, YearMonth};
use nemlake_registry::Registry;
use nemlake_registry::file_model::ctl::CtlFile;
use serde::{Deserialize, Serialize};

use crate::remote::{HeadError, inflate_head, list_ctls, trim_head};
use crate::sample::sample_reader;

/// How much of a zip is read for its I rows: `first` bytes, then four
/// times as many while the last block seen has no D row yet and the zip
/// goes on, up to `cap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadPolicy {
    /// Bytes of the first read.
    pub first: u64,
    /// Most bytes read of one zip.
    pub cap: u64,
}

impl Default for HeadPolicy {
    fn default() -> Self {
        Self {
            first: crate::remote::HEAD_BYTES,
            cap: 4 << 20,
        }
    }
}

/// Why the census of a month could not be taken or recorded.
#[derive(Debug, thiserror::Error)]
pub enum CensusError {
    /// Listing, fetching or reading a head.
    #[error(transparent)]
    Head(#[from] HeadError),
    /// A census or shapes file that cannot be read.
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
}

impl From<FetchError> for CensusError {
    fn from(e: FetchError) -> Self {
        Self::Head(e.into())
    }
}

impl Diagnose for CensusError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Head(e) => e.diagnostic(),
            Self::Read { path, .. } => {
                Diagnostic::error("schema.census_unreadable", Category::Schema, msg)
                    .with_file(path.clone())
            }
            Self::Write { path, source } => {
                Diagnostic::error("schema.census_write", Category::Schema, msg)
                    .with_file(path.clone())
                    .caused_by(source)
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
}

/// The census of one month: per file name, its blocks in file order. A
/// file whose head could not be read has none, and `problems` says why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonthCensus {
    /// The month.
    pub month: Option<YearMonth>,
    /// File name to blocks.
    pub files: BTreeMap<String, Vec<BlockShape>>,
    /// Files whose head could not be read, with the diagnostic.
    pub problems: Vec<Diagnostic>,
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
                let table = crate::remote::ctl_table(name)?;
                let ctl = nemlake_registry::file_model::ctl::parse(name, text).ok()?;
                Some((table, ctl))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Self(out)
    }

    /// Every control file in `dir`.
    pub fn read_dir(dir: &Path) -> Result<Self, CensusError> {
        let unreadable = |e: std::io::Error| CensusError::Read {
            path: dir.display().to_string(),
            message: e.to_string(),
        };
        let mut texts = Vec::new();
        for e in std::fs::read_dir(dir).map_err(unreadable)? {
            let p = e.map_err(unreadable)?.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ctl")) {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
                texts.push((
                    name.unwrap_or_default(),
                    std::fs::read_to_string(&p).map_err(unreadable)?,
                ));
            }
        }
        Ok(Self::parse(
            texts.iter().map(|(n, t)| (n.as_str(), t.as_str())),
        ))
    }

    /// The month's control files from the archive (fetched when they are
    /// not archived yet).
    pub async fn fetch(
        fetcher: &Fetcher,
        archive: &MmsdmArchive,
        month: YearMonth,
    ) -> Result<Self, CensusError> {
        let mut texts = Vec::new();
        for remote in list_ctls(fetcher, archive, month).await? {
            let local = fetcher.fetch(&remote).await?;
            let text = std::fs::read(&local.path).map_err(|e| CensusError::Read {
                path: local.path.display().to_string(),
                message: e.to_string(),
            })?;
            texts.push((
                remote.file_name(),
                String::from_utf8_lossy(&text).into_owned(),
            ));
        }
        Ok(Self::parse(
            texts.iter().map(|(n, t)| (n.as_str(), t.as_str())),
        ))
    }

    /// The control file that describes `table`'s file: one of the same
    /// table, else of a split of it, else the daily `_D` file's.
    fn for_table(&self, tables: &[&str]) -> Option<&CtlFile> {
        [false, true].into_iter().find_map(|siblings| {
            tables.iter().find_map(|t| {
                self.0
                    .iter()
                    .find(|(ct, _)| crate::remote::ctl_serves(ct, t, siblings))
                    .map(|(_, c)| c)
            })
        })
    }
}

/// The blocks near the start of a remote zip: report pair, version and
/// columns of each I row read, growing the read as `policy` says.
pub async fn read_blocks(
    fetcher: &Fetcher,
    remote: &RemoteFile,
    policy: HeadPolicy,
) -> Result<Vec<crate::sample::SampledBlock>, HeadError> {
    let identity = remote.url.as_str();
    let mut len = policy.first.max(1024);
    loop {
        let body = fetcher.fetch_head(remote, len).await?;
        let whole = (body.len() as u64) < len;
        let (_, mut inflated) = inflate_head(identity, &body)?;
        if remote.member.is_some() {
            inflated = inflate_head(identity, &inflated)?.1;
        }
        let head = trim_head(&inflated, 1);
        let blocks =
            sample_reader(identity, head.as_slice(), 1).map_err(|e| HeadError::NoIRow {
                url: format!("{identity}: {e}"),
                bytes: inflated.len(),
            })?;
        let settled = blocks.last().is_some_and(|b| b.rows > 0);
        if settled || whole || len >= policy.cap {
            if blocks.is_empty() {
                return Err(HeadError::NoIRow {
                    url: identity.to_owned(),
                    bytes: inflated.len(),
                });
            }
            return Ok(blocks);
        }
        len = (len * 4).min(policy.cap);
    }
}

/// The census of the files `remotes` of one month, typed by `ctls`.
/// Stops for good when the server refuses.
pub async fn census_files(
    fetcher: &Fetcher,
    registry: &Registry,
    remotes: &[RemoteFile],
    ctls: &Ctls,
    policy: HeadPolicy,
) -> Result<MonthCensus, CensusError> {
    let mut out = MonthCensus::default();
    for remote in remotes {
        let name = remote.file_name();
        let blocks = match read_blocks(fetcher, remote, policy).await {
            Ok(b) => b,
            Err(HeadError::Fetch(e @ FetchError::Blocked { .. })) => return Err(e.into()),
            Err(e) => {
                out.problems.push(e.diagnostic());
                out.files.insert(name, Vec::new());
                continue;
            }
        };
        let file_table = remote
            .table
            .clone()
            .or_else(|| nemlake_core::archive_name::parse(&name).map(|n| n.table));
        let shapes = blocks
            .into_iter()
            .map(|b| block_shape(registry, ctls, file_table.as_deref(), b))
            .collect();
        out.files.insert(name, shapes);
    }
    Ok(out)
}

/// A block's shape: its columns typed by the control file of the table
/// that claims its report pair, else of the table its file is named for.
pub fn block_shape(
    registry: &Registry,
    ctls: &Ctls,
    file_table: Option<&str>,
    b: crate::sample::SampledBlock,
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
        shape: CanonicalShape {
            columns: b.columns.iter().map(|c| (c.clone(), class(c))).collect(),
            report_type: b.report_type,
            report_subtype: b.report_subtype,
        },
    }
}

/// The census of `month`: its files from the archive listing, its control
/// files from `ctl_dir` when given, else from the archive.
pub async fn census_month(
    fetcher: &Fetcher,
    archive: &MmsdmArchive,
    registry: &Registry,
    month: YearMonth,
    ctl_dir: Option<&Path>,
    policy: HeadPolicy,
) -> Result<MonthCensus, CensusError> {
    let query = Query {
        exclude: Vec::new(),
        tables: Vec::new(),
        from: month,
        to: month,
    };
    let remotes = archive.list(&query, fetcher).await?;
    let ctls = match ctl_dir {
        Some(d) => Ctls::read_dir(d)?,
        None => Ctls::fetch(fetcher, archive, month).await?,
    };
    let mut census = census_files(fetcher, registry, &remotes, &ctls, policy).await?;
    census.month = Some(month);
    Ok(census)
}

/// One file's entry in a month's census: one id, or one per block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileEntry {
    /// A file of one block.
    One(ShapeId),
    /// A file of several blocks, or none that could be read.
    Many(Vec<ShapeId>),
}

impl FileEntry {
    /// The ids, in file order.
    pub fn ids(&self) -> Vec<ShapeId> {
        match self {
            Self::One(id) => vec![*id],
            Self::Many(ids) => ids.clone(),
        }
    }
}

/// One distinct shape of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeEntry {
    /// The shape's id.
    pub id: ShapeId,
    /// Report type.
    pub report_type: String,
    /// Report subtype.
    pub report_subtype: String,
    /// The version AEMO stamped on it.
    pub version: u16,
    /// Columns in file order: name and type class.
    pub columns: Vec<(String, TypeClass)>,
    /// The first month it was seen in.
    pub first_seen: YearMonth,
    /// The last month it was seen in.
    pub last_seen: YearMonth,
}

impl ShapeEntry {
    /// The canonical shape the entry records.
    pub fn shape(&self) -> CanonicalShape {
        CanonicalShape {
            report_type: self.report_type.clone(),
            report_subtype: self.report_subtype.clone(),
            columns: self.columns.clone(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ShapesFile {
    #[serde(default)]
    shape: Vec<ShapeEntry>,
}

const SHAPES_HEADER: &str =
    "# Generated by `nemlake schema census`: every distinct shape of the table, never removed.\n";

/// A shapes file: the header, then each shape with one column a line.
fn render_shapes(shapes: &[ShapeEntry]) -> String {
    use std::fmt::Write as _;
    let q = |s: &str| toml::Value::String(s.to_owned()).to_string();
    let mut out = String::from(SHAPES_HEADER);
    for e in shapes {
        let _ = write!(
            out,
            "\n[[shape]]\nid = \"{}\"\nreport_type = {}\nreport_subtype = {}\nversion = {}\nfirst_seen = \"{}\"\nlast_seen = \"{}\"\ncolumns = [\n",
            e.id,
            q(&e.report_type),
            q(&e.report_subtype),
            e.version,
            e.first_seen,
            e.last_seen
        );
        for (name, class) in &e.columns {
            let _ = writeln!(out, "    [{}, \"{class}\"],", q(name));
        }
        out.push_str("]\n");
    }
    out
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
    toml::from_str::<ShapesFile>(&text)
        .map(|f| f.shape)
        .map_err(|e| CensusError::Read {
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
        let ids: Vec<ShapeId> = blocks.iter().map(|b| b.shape.id()).collect();
        entries.insert(
            name,
            match ids.as_slice() {
                [one] => FileEntry::One(*one),
                _ => FileEntry::Many(ids),
            },
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
        "# Generated by `nemlake schema census`: the shape of every file of {month}.\n{}",
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
    use std::sync::Arc;
    use std::time::Duration;

    use nemlake_acquire::remotezip::ZipMember;
    use nemlake_acquire::transport::{Canned, StaticTransport};
    use nemlake_acquire::{DiskCache, FetcherConfig, FileKind};
    use nemlake_registry::TypeOptions;

    use super::*;

    fn zip_of(name: &str, csv: &[u8]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(csv).unwrap();
            w.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn remote(url: &str) -> RemoteFile {
        RemoteFile {
            url: url::Url::parse(url).unwrap(),
            source: "nem-mmsdm",
            table: None,
            period: YearMonth::new(2026, 8),
            kind: FileKind::MmsZip,
            size: None,
            modified: None,
            member: None,
        }
    }

    fn fetcher(dir: &Path, t: &Arc<StaticTransport>) -> Fetcher {
        Fetcher::new(
            Arc::new(t.clone()),
            DiskCache::new(dir),
            FetcherConfig {
                attempts: 1,
                backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(1),
                min_interval: Duration::ZERO,
                ..Default::default()
            },
        )
    }

    const SCADA_CTL: &str = r#"LOAD DATA INFILE "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.CSV" APPEND INTO TABLE DISPATCH_UNIT_SCADA
FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '"' TRAILING NULLCOLS
(row_type FILLER, report_type FILLER, report_subtype FILLER, report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", DUID CHAR(20), SCADAVALUE FLOAT EXTERNAL)
"#;

    fn scada_csv(rows: usize) -> Vec<u8> {
        let mut csv = b"C,NEMP.WORLD,DVD,AEMO,PUBLIC,2026/09/01,00:00:00,,,\r\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\n".to_vec();
        for i in 0..rows {
            csv.extend_from_slice(
                format!("D,DISPATCH,UNIT_SCADA,1,\"2026/08/01 00:05:00\",U{i},{i}.5\r\n")
                    .as_bytes(),
            );
        }
        csv
    }

    #[tokio::test]
    async fn every_block_gets_its_shape_typed_by_the_months_control_file() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        // A file of the newer naming scheme with two blocks, the second of
        // a table no control file of the month describes.
        let mut csv = scada_csv(3);
        csv.extend_from_slice(b"I,NEMLAKE_TEST,EXTRA,2,A,B\r\nD,NEMLAKE_TEST,EXTRA,2,x,y\r\n");
        let url =
            "https://h/PUBLIC_ARCHIVE%23DISPATCH_UNIT_SCADA%23ALL%23FILE01%23202608010000.zip";
        t.add(url, Canned::ok(&zip_of("A.CSV", &csv)));
        let f = fetcher(dir.path(), &t);
        let reg = Registry::load_embedded(TypeOptions::default());
        let ctls = Ctls::parse([(
            "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl",
            SCADA_CTL,
        )]);
        let mut census = census_files(&f, &reg, &[remote(url)], &ctls, HeadPolicy::default())
            .await
            .unwrap();
        assert!(census.problems.is_empty(), "{:?}", census.problems);
        let name = "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#ALL#FILE01#202608010000.zip";
        let blocks = &census.files[name];
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].version, 1);
        assert_eq!(
            blocks[0].shape.columns,
            [
                ("SETTLEMENTDATE".to_owned(), TypeClass::Timestamp),
                ("DUID".to_owned(), TypeClass::Varchar),
                ("SCADAVALUE".to_owned(), TypeClass::Number)
            ]
        );
        assert_eq!(blocks[1].shape.report_subtype, "EXTRA");
        assert!(
            blocks[1]
                .shape
                .columns
                .iter()
                .all(|(_, c)| *c == TypeClass::Unknown)
        );

        // Recorded: the census file names both ids, each table's shapes
        // file holds its shape once, and a second month only moves the
        // months it was seen in.
        census.month = YearMonth::new(2026, 8);
        let root = dir.path().join("schemas");
        assert_eq!(record(&root, &reg, &census).unwrap(), 2);
        let month = read_month(&root, YearMonth::new(2026, 8).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            month[name].ids(),
            [blocks[0].shape.id(), blocks[1].shape.id()]
        );
        let text =
            std::fs::read_to_string(census_path(&root, YearMonth::new(2026, 8).unwrap())).unwrap();
        assert!(
            text.starts_with("# Generated by `nemlake schema census`"),
            "{text}"
        );
        census.month = YearMonth::new(2026, 6);
        assert_eq!(record(&root, &reg, &census).unwrap(), 0);
        let shapes = read_shapes(&root, "DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].id, blocks[0].shape.id());
        assert_eq!(shapes[0].shape().id(), shapes[0].id);
        assert_eq!(shapes[0].first_seen.to_string(), "2026-06");
        assert_eq!(shapes[0].last_seen.to_string(), "2026-08");
        assert_eq!(read_shapes(&root, "NEMLAKE_TEST_EXTRA").unwrap().len(), 1);
        assert!(read_shapes(&root, "NOBODY").unwrap().is_empty());
        assert!(
            read_month(&root, YearMonth::new(2020, 1).unwrap())
                .unwrap()
                .is_none()
        );
        // The same census again writes the same bytes.
        let before =
            std::fs::read_to_string(shapes_dir(&root).join("DISPATCH_UNIT_SCADA.toml")).unwrap();
        record(&root, &reg, &census).unwrap();
        assert_eq!(
            std::fs::read_to_string(shapes_dir(&root).join("DISPATCH_UNIT_SCADA.toml")).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn a_member_of_a_whole_month_zip_is_read_from_its_offset_and_grown() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        // A per-table zip inside a month zip, stored, after 100 bytes of
        // something else. Its first block's C rows are long, so the first
        // read ends before any D row and the read is grown.
        let mut csv = b"C,".to_vec();
        // Letters that do not compress, so the head really is long.
        let mut seed = 7u32;
        csv.extend((0..3000).map(|_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            b'a' + (seed >> 16) as u8 % 26
        }));
        csv.extend_from_slice(b"\r\n");
        csv.extend_from_slice(&scada_csv(2));
        let inner = zip_of("PUBLIC_DVD_DISPATCH_UNIT_SCADA_201001010000.CSV", &csv);
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.start_file("DOCUMENTATION/README.txt", stored).unwrap();
            w.write_all(&[b'r'; 100]).unwrap();
            w.start_file(
                "DATA/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201001010000.zip",
                stored,
            )
            .unwrap();
            w.write_all(&inner).unwrap();
            w.finish().unwrap();
        }
        let month = cursor.into_inner();
        let offset = zip::ZipArchive::new(std::io::Cursor::new(&month))
            .unwrap()
            .by_index(1)
            .unwrap()
            .header_start();
        t.add("https://h/MMSDM_2010_01.zip", Canned::ok(&month));
        let f = fetcher(dir.path(), &t);
        let m = RemoteFile {
            member: Some(Box::new(ZipMember {
                name: "DATA/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201001010000.zip".into(),
                method: 0,
                crc32: 0,
                compressed_size: inner.len() as u64,
                uncompressed_size: inner.len() as u64,
                header_offset: offset,
                name_and_extra: 0,
                flags: 0,
            })),
            ..remote(
                "https://h/MMSDM_2010_01.zip#DATA/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201001010000.zip",
            )
        };
        let policy = HeadPolicy {
            first: 1024,
            cap: 64 << 10,
        };
        let blocks = read_blocks(&f, &m, policy).await.unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].columns, ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]);
        let ranges = t.ranges.lock().unwrap().clone();
        assert_eq!(ranges.len(), 2, "{ranges:?}");
        // Without a control file every class is unknown; a bad zip is a
        // problem of its own file and the census goes on.
        t.add(
            "https://h/bad.zip",
            Canned::ok(b"not a zip at all, not at all"),
        );
        let reg = Registry::load_embedded(TypeOptions::default());
        let census = census_files(
            &f,
            &reg,
            &[m, remote("https://h/bad.zip")],
            &Ctls::parse([]),
            policy,
        )
        .await
        .unwrap();
        assert_eq!(census.problems.len(), 1);
        assert_eq!(census.problems[0].code, "schema.head_not_zip");
        assert!(census.files["bad.zip"].is_empty());
        let scada = &census.files["PUBLIC_DVD_DISPATCH_UNIT_SCADA_201001010000.zip"][0];
        assert!(
            scada
                .shape
                .columns
                .iter()
                .all(|(_, c)| *c == TypeClass::Unknown)
        );
    }

    /// The committed heads of both months the registry is built from: the
    /// shape of every block resolves exactly, and in the anchor month
    /// every column is typed by its control file.
    #[test]
    fn the_bootstrap_heads_of_both_months_resolve_exactly() {
        use nemlake_core::IRow;
        let reg = Registry::load_embedded(TypeOptions::default());
        for month in ["MMSDM_2024_01", "MMSDM_2026_08"] {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testdata/bootstrap")
                .join(month);
            let ctls = Ctls::read_dir(&root.join("CTL")).unwrap();
            let mut seen = std::collections::BTreeSet::new();
            let mut untyped = Vec::new();
            for e in std::fs::read_dir(root.join("heads")).unwrap() {
                let p = e.unwrap().path();
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                let file_table = nemlake_core::archive_name::parse(&name).map(|n| n.table);
                let text = std::fs::read(&p).unwrap();
                for b in sample_reader(&name, text.as_slice(), 1).unwrap() {
                    let b = block_shape(&reg, &ctls, file_table.as_deref(), b);
                    let s = &b.shape;
                    let columns: Vec<&str> = s.columns.iter().map(|(c, _)| c.as_str()).collect();
                    let irow = IRow {
                        report_type: &s.report_type,
                        report_subtype: &s.report_subtype,
                        version: b.version,
                        columns: &columns,
                    };
                    let res = reg
                        .resolve(
                            &irow,
                            nemlake_registry::ResolveOptions {
                                mode: nemlake_registry::Mode::Strict,
                            },
                        )
                        .unwrap_or_else(|e| panic!("{month} {name}: {e}"));
                    assert_eq!(
                        res.status,
                        nemlake_registry::Status::Exact,
                        "{month} {name}"
                    );
                    untyped.extend(
                        s.columns
                            .iter()
                            .filter(|(_, c)| *c == TypeClass::Unknown)
                            .map(|(c, _)| format!("{name} {c}")),
                    );
                    seen.insert(s.id());
                }
            }
            assert!(seen.len() > 100, "{month}: {} shapes", seen.len());
            // The earlier month's fixtures hold only the control files the
            // anchor month lacks, so some of its columns stay untyped.
            if month == "MMSDM_2026_08" {
                assert!(untyped.is_empty(), "{month}: {untyped:?}");
            }
        }
    }

    #[test]
    fn control_files_come_from_a_directory_and_errors_have_codes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path()
                .join("PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl"),
            SCADA_CTL,
        )
        .unwrap();
        std::fs::write(dir.path().join("README.txt"), "x").unwrap();
        let ctls = Ctls::read_dir(dir.path()).unwrap();
        assert!(ctls.for_table(&["DISPATCH_UNIT_SCADA"]).is_some());
        assert!(ctls.for_table(&["NOPE"]).is_none());
        let e = Ctls::read_dir(&dir.path().join("none")).err().unwrap();
        assert_eq!(e.diagnostic().code, "schema.census_unreadable");
        let root = dir.path().join("schemas");
        std::fs::create_dir_all(census_dir(&root)).unwrap();
        std::fs::write(
            census_path(&root, YearMonth::new(2026, 8).unwrap()),
            "= nope",
        )
        .unwrap();
        let e = read_month(&root, YearMonth::new(2026, 8).unwrap()).unwrap_err();
        assert_eq!(e.diagnostic().code, "schema.census_unreadable");
        std::fs::create_dir_all(shapes_dir(&root)).unwrap();
        std::fs::write(shapes_dir(&root).join("T.toml"), "shape = 1").unwrap();
        assert!(read_shapes(&root, "T").is_err());
        let e = CensusError::Write {
            path: "p".into(),
            source: std::io::Error::other("full"),
        };
        assert_eq!(e.diagnostic().code, "schema.census_write");
        let e = CensusError::from(FetchError::Offline { url: "u".into() });
        assert_eq!(e.diagnostic().code, "net.offline");
    }
}
