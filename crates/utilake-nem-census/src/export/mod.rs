//! `export`: every table and shape as JSON Schema, for the public
//! `nem-schemas` repository.
//!
//! Per table, `nem/{TABLE}/table.json` is the one place the table is
//! described: its declaration and key, the key's history as the archived
//! data-model releases declare it, every declared column with its lake
//! type, the model's type and precision and its comment, every increment
//! with the loader type of each column from AEMO's control file, and the
//! list of its shapes. Per shape, `nem/{TABLE}/{shape}.schema.json` is a
//! JSON Schema a row of that shape validates against (types, formats,
//! what is required) with the shape's facts (`x-shape`, `x-seen`,
//! `x-increment`) and a reference to the table document; column metadata
//! is not repeated there. `x-seen` says every month the census saw the
//! shape in, its rows, its place among the month's shapes, and the first
//! and last time value its rows carried, so a shape's span is known to the
//! row, not the month. Beside them: `manifest.json` (content only, so a
//! second run gives the same bytes), `provenance.json` (which commit wrote
//! it, never compared) and `LICENCE.md`. The writer owns `nem/` whole: a
//! shape that disappears upstream disappears here.
//!
//! Everything is read from `schemas/` and the data-model JSON; nothing is
//! typed by hand, so the output is regenerable by anyone with the archive.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use utilake_core::{Category, Diagnose, Diagnostic, YearMonth};
use utilake_mms_registry::file_model::blob::SchemaSet;
use utilake_mms_registry::file_model::ctl;
use utilake_mms_registry::file_model::model::{Increment, Table};
use utilake_mms_registry::file_model::shapes::{ShapeColumn, ShapeEntry};
use utilake_mms_registry::shape::ShapeId;

use crate::census::{FileRecord, Span, census_dir, read_month};
use crate::datamodel::{DataModel, DmTable};
use crate::models::{self, Model};

mod doc;
mod iceberg;
mod manifest;
mod write;

pub use write::{Stale, Written, check, write};

/// The generation of the documents' layout: the files, the `x-` keys and
/// their meaning. Raised when a reader of this format would misread a document.
pub const FORMAT: u32 = 2;

/// Where the documents are published: `$id` is this plus the file's path.
pub const DEFAULT_BASE_URL: &str = "https://raw.githubusercontent.com/pigface-io/nem-schemas/main/";

/// The licence file written beside the documents.
pub const LICENCE: &str = include_str!("../../templates/nem-schemas/LICENCE.md");

/// What the export reads.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// The schemas directory.
    pub schemas: PathBuf,
    /// The data-model report JSON, for packages, visibility, comments and
    /// types where no archived create script covers a table.
    pub datamodel: Option<PathBuf>,
    /// The URL the documents will live under.
    pub base_url: String,
}

/// What the census saw of a shape in one month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenMonth {
    /// Rows over every block of the shape; `None` when the month's census
    /// predates row counts.
    pub rows: Option<u64>,
    /// Position of the shape among the month's shapes, by first appearance
    /// in file then block order: 0 for the first shape the month's files hold.
    pub order: u16,
    /// The least time value over the shape's blocks that month, as written,
    /// when the census recorded spans.
    pub first: Option<String>,
    /// The greatest time value over the shape's blocks that month.
    pub last: Option<String>,
    /// The column the values are of.
    pub column: Option<String>,
}

/// Every month a shape was seen in.
pub type Seen = BTreeMap<YearMonth, SeenMonth>;

/// One shape of one table, every version of it folded together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeDoc<'a> {
    /// The table the shape file is named for.
    pub table: &'a str,
    /// The table's declaration, when it has one.
    pub declared: Option<&'a Table>,
    /// The shape's id.
    pub id: ShapeId,
    /// Report type.
    pub report_type: &'a str,
    /// Report subtype.
    pub report_subtype: &'a str,
    /// Every version the id was seen under, ascending.
    pub versions: Vec<u16>,
    /// The first month any version was seen.
    pub first: YearMonth,
    /// The last month any version was seen.
    pub last: YearMonth,
    /// Columns in file order.
    pub columns: &'a [ShapeColumn],
    /// Months and rows from the census.
    pub seen: &'a Seen,
}

/// One increment with its control file read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementDoc<'a> {
    /// The increment.
    pub increment: &'a Increment,
    /// The data-model version the control file's history line names.
    pub model_version: Option<String>,
    /// Column name to loader type, as the control file writes it.
    pub loader_types: BTreeMap<String, String>,
}

/// What the data-model releases say about one table: the key per release,
/// first release first, and the newest release's columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelFacts<'a> {
    /// `(first month the release shipped, key columns, release version)`.
    pub keys: Vec<(YearMonth, &'a [String], &'a str)>,
    /// The newest release that declares the table.
    pub latest: Option<(&'a str, &'a models::ModelTable)>,
}

/// The export, loaded and joined; [`Export::render`] writes it as text.
#[derive(Debug)]
pub struct Export {
    tables: BTreeMap<String, Table>,
    shapes: BTreeMap<String, Vec<ShapeEntry>>,
    seen: BTreeMap<ShapeId, Seen>,
    months: BTreeSet<YearMonth>,
    datamodel: DataModel,
    datamodel_path: Option<PathBuf>,
    models: Vec<Model>,
    schemas: PathBuf,
    base_url: String,
    /// Census blocks that name a shape no shapes file records.
    problems: Vec<Diagnostic>,
    /// Each table's history as an Iceberg table, as the fold wrote it.
    histories: BTreeMap<String, serde_json::Value>,
}

/// Everything one run produces: the files the generator owns, by path
/// relative to the output directory, and the problems met joining them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// Path to text, every file compared by `--check`.
    pub files: BTreeMap<String, String>,
    /// `provenance.json`, written but never compared.
    pub provenance: String,
    /// Problems that do not stop the export: unknown shapes.
    pub problems: Vec<Diagnostic>,
}

/// Why a file could not be read or written.
#[derive(Debug, thiserror::Error)]
#[error("The export could not read or write {path}.")]
pub struct IoError {
    path: String,
    #[source]
    source: std::io::Error,
}

impl IoError {
    fn new(path: &Path, source: std::io::Error) -> Self {
        Self {
            path: path.display().to_string(),
            source,
        }
    }
}

impl Diagnose for IoError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("schema.export_io", Category::Schema, self.to_string())
            .with_file(self.path.clone())
            .caused_by(&self.source)
    }
}

impl Export {
    /// Reads the schemas, every census month, the recorded data-model
    /// releases and the data-model report.
    pub fn load(inputs: &Inputs) -> Result<Self, Box<Diagnostic>> {
        let set = SchemaSet::read_dir(&inputs.schemas).map_err(|e| Box::new(e.diagnostic()))?;
        let datamodel = match &inputs.datamodel {
            Some(p) => DataModel::load(p).map_err(|e| Box::new(e.diagnostic()))?,
            None => DataModel::default(),
        };
        let recorded =
            models::read_models(&inputs.schemas).map_err(|e| Box::new(e.diagnostic()))?;
        let models: Vec<Model> = models::by_first_seen(&recorded)
            .into_iter()
            .cloned()
            .collect();
        // Which shapes file holds each shape: the table a shape belongs to.
        let table_of: BTreeMap<ShapeId, &str> = set
            .shapes
            .iter()
            .flat_map(|(t, es)| es.iter().map(move |e| (e.id, t.as_str())))
            .collect();
        let mut seen: BTreeMap<ShapeId, Seen> = BTreeMap::new();
        let mut months = BTreeSet::new();
        let mut problems = Vec::new();
        for month in census_months(&inputs.schemas)? {
            // Listed a moment ago, so it is there; one that vanished meanwhile is empty.
            let files = read_month(&inputs.schemas, month)
                .map_err(|e| Box::new(e.diagnostic()))?
                .unwrap_or_default();
            months.insert(month);
            // Per table, its shapes in the order the month's files and
            // blocks hold them.
            let mut order: BTreeMap<&str, BTreeMap<ShapeId, u16>> = BTreeMap::new();
            for (file, entry) in &files {
                for block in blocks(entry) {
                    let id = block.id;
                    let Some(table) = table_of.get(&id) else {
                        problems.push(unknown_shape(month, file, id));
                        continue;
                    };
                    let of_table = order.entry(table).or_default();
                    let next = of_table.len() as u16;
                    let position = *of_table.entry(id).or_insert(next);
                    let slot = seen
                        .entry(id)
                        .or_default()
                        .entry(month)
                        .or_insert(SeenMonth {
                            rows: Some(0),
                            order: position,
                            first: None,
                            last: None,
                            column: None,
                        });
                    slot.rows = slot.rows.map(|have| have + block.rows);
                    if let Some(span) = block.span {
                        slot.first = Some(
                            slot.first
                                .take()
                                .map_or(span.first.clone(), |f| f.min(span.first)),
                        );
                        slot.last = Some(
                            slot.last
                                .take()
                                .map_or(span.last.clone(), |l| l.max(span.last)),
                        );
                        slot.column.get_or_insert(span.column);
                    }
                }
            }
        }
        Ok(Self {
            tables: set
                .tables
                .into_iter()
                .map(|t| (t.name.clone(), t))
                .collect(),
            shapes: set.shapes,
            seen,
            months,
            datamodel,
            datamodel_path: inputs.datamodel.clone(),
            models,
            schemas: inputs.schemas.clone(),
            base_url: inputs.base_url.clone(),
            problems,
            histories: histories(&inputs.schemas),
        })
    }

    /// Renders every file.
    pub fn render(&self) -> Result<Rendered, Box<Diagnostic>> {
        let mut out = Rendered {
            problems: self.problems.clone(),
            ..Rendered::default()
        };
        let empty = Seen::new();
        let report_version = self.report_version();
        let mut shape_count = 0usize;
        let mut schema_count = 0usize;
        for (table, entries) in &self.shapes {
            let declared = self.tables.get(table);
            let dm = self.datamodel.tables.get(table);
            let facts = self.model_facts(table);
            let increments = match declared {
                Some(t) => self.increments(table, t)?,
                None => Vec::new(),
            };
            let mut docs = Vec::new();
            for doc in fold(table, entries, &self.seen, &empty) {
                let doc = ShapeDoc { declared, ..doc };
                let matched = doc::increment(&doc);
                let path = format!("nem/{table}/{}.schema.json", doc.id);
                let id = format!("{}{path}", self.base_url);
                let description = declared
                    .and_then(|t| t.description.as_deref())
                    .or_else(|| dm.and_then(|m| m.description.as_deref()));
                out.files
                    .insert(path, doc::render(&doc, &id, description, matched));
                docs.push((doc, matched));
                shape_count += 1;
            }
            let history = self.histories.get(table).map(|h| {
                let (files, block) = iceberg::render(&self.base_url, table, h);
                schema_count += files.len();
                out.files.extend(files);
                block
            });
            let text = doc::table(&doc::TableDoc {
                name: table,
                declared,
                model: dm,
                facts: &facts,
                report_version: report_version.as_deref(),
                increments: &increments,
                shapes: &docs,
                history: history.as_ref(),
            });
            out.files.insert(format!("nem/{table}/table.json"), text);
        }
        let counts = manifest::Counts {
            tables: self.shapes.len(),
            declared_tables: self
                .shapes
                .keys()
                .filter(|t| self.tables.contains_key(*t))
                .count(),
            shapes: shape_count,
            schemas: schema_count,
        };
        let versions: Vec<&str> = self.models.iter().map(|m| m.version.as_str()).collect();
        let manifest =
            manifest::render(&self.base_url, &self.months, counts, &versions, &out.files);
        out.files.insert("manifest.json".to_owned(), manifest);
        out.files
            .insert("LICENCE.md".to_owned(), LICENCE.to_owned());
        out.provenance = manifest::provenance(&self.schemas, self.datamodel_path.as_deref());
        Ok(out)
    }

    /// The data-model report's version, from its file name
    /// (`mms_data_model_v5.0.0.1.json` -> `v5.0.0.1`).
    fn report_version(&self) -> Option<String> {
        let stem = self.datamodel_path.as_ref()?.file_stem()?.to_str()?;
        Some(
            stem.strip_prefix("mms_data_model_")
                .unwrap_or(stem)
                .to_owned(),
        )
    }

    /// What the releases declare for a table.
    fn model_facts(&self, table: &str) -> ModelFacts<'_> {
        let mut facts = ModelFacts::default();
        for m in &self.models {
            if let Some(t) = m.tables.get(table) {
                facts.keys.push((m.first_seen, &t.primary_key, &m.version));
                facts.latest = Some((&m.version, t));
            }
        }
        facts
    }

    /// Each increment with its control file read: the model version its
    /// history line names and the loader type of every column.
    fn increments<'a>(
        &self,
        table: &str,
        t: &'a Table,
    ) -> Result<Vec<IncrementDoc<'a>>, Box<Diagnostic>> {
        let mut out = Vec::with_capacity(t.increments.len());
        for i in &t.increments {
            let mut doc = IncrementDoc {
                increment: i,
                model_version: None,
                loader_types: BTreeMap::new(),
            };
            if let Some(name) = &i.ctl {
                let path = self.schemas.join("nem").join(table).join("ctl").join(name);
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| Box::new(IoError::new(&path, e).diagnostic()))?;
                doc.model_version = model_version(&text);
                // A control file that does not parse types nothing; the
                // registry's own gate reports it.
                doc.loader_types = match ctl::parse(&path.display().to_string(), &text) {
                    Ok(p) => p
                        .data_columns()
                        .map(|c| (c.name.to_ascii_uppercase(), c.ty.render()))
                        .collect(),
                    Err(_) => BTreeMap::new(),
                };
            }
            out.push(doc);
        }
        Ok(out)
    }
}

/// The months with a census file, ascending.
/// Each table's history as the fold wrote it, by table name, every key
/// kept; none when the census was not folded. The schema set read and
/// parsed each file already ([`SchemaSet::read_dir`] refuses one that does
/// not parse), so this only takes them in whole.
fn histories(schemas: &Path) -> BTreeMap<String, serde_json::Value> {
    let Ok(rd) = std::fs::read_dir(crate::census::history_dir(schemas)) else {
        return BTreeMap::new();
    };
    rd.filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter_map(|v| Some((v["table"].as_str()?.to_owned(), v)))
        .collect()
}

fn census_months(schemas: &Path) -> Result<Vec<YearMonth>, Box<Diagnostic>> {
    let dir = census_dir(schemas);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Box::new(IoError::new(&dir, e).diagnostic())),
    };
    let mut months = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| Box::new(IoError::new(&dir, e).diagnostic()))?
            .path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        if let Some(m) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse().ok())
        {
            months.push(m);
        }
    }
    months.sort_unstable();
    Ok(months)
}

/// One block of a census file.
struct Block {
    id: ShapeId,
    rows: u64,
    span: Option<Span>,
}

/// A file's blocks.
fn blocks(entry: &FileRecord) -> Vec<Block> {
    entry
        .block
        .iter()
        .map(|b| Block {
            id: b.shape,
            rows: b.rows,
            span: match (&b.first, &b.last, &b.time) {
                (Some(first), Some(last), Some(column)) => Some(Span {
                    first: first.clone(),
                    last: last.clone(),
                    column: column.clone(),
                }),
                _ => None,
            },
        })
        .collect()
}

fn unknown_shape(month: YearMonth, file: &str, id: ShapeId) -> Diagnostic {
    Diagnostic::warning(
        "schema.export_unknown_shape",
        Category::Schema,
        format!("The census of {month} records shape {id} in {file}, which no shapes file knows; the shape is left out."),
    )
    .with_file(file.to_owned())
}

/// One document per shape: a shapes file holds each id once, with every
/// version it was seen under.
fn fold<'a>(
    table: &'a str,
    entries: &'a [ShapeEntry],
    seen: &'a BTreeMap<ShapeId, Seen>,
    empty: &'a Seen,
) -> Vec<ShapeDoc<'a>> {
    let mut docs: Vec<ShapeDoc<'a>> = entries
        .iter()
        .map(|e| ShapeDoc {
            table,
            declared: None,
            id: e.id,
            report_type: &e.report_type,
            report_subtype: &e.report_subtype,
            versions: e.versions.clone(),
            first: e.first_seen,
            last: e.last_seen,
            columns: &e.columns,
            seen: seen.get(&e.id).unwrap_or(empty),
        })
        .collect();
    docs.sort_by_key(|d| (d.first, d.id));
    docs
}

/// The data-model version a control file's history line names: the line
/// `--      PUBLIC   v4.23`, three words, the last `v` and a number. `None`
/// when the line is absent or its version blank (`--      PUBLIC   v`).
pub fn model_version(ctl: &str) -> Option<String> {
    ctl.lines().find_map(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let [dashes, scope, version] = words.as_slice() else {
            return None;
        };
        let number = version.strip_prefix('v')?;
        let shaped = *dashes == "--"
            && scope.bytes().all(|b| b.is_ascii_uppercase())
            && number.starts_with(|c: char| c.is_ascii_digit())
            && number.bytes().all(|b| b.is_ascii_digit() || b == b'.');
        shaped.then(|| (*version).to_owned())
    })
}

/// A data-model report type as the create scripts spell it, so the two
/// sources read alike: `NUMBER` is `NUMERIC`, `VARCHAR2` is `VARCHAR`,
/// `DATE` is `TIMESTAMP(0)`; everything is upper case without spaces.
pub fn logical_type(oracle: &str) -> String {
    let compact: String = oracle
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if compact == "DATE" {
        return "TIMESTAMP(0)".to_owned();
    }
    if let Some(rest) = compact.strip_prefix("NUMBER") {
        return format!("NUMERIC{rest}");
    }
    if let Some(rest) = compact.strip_prefix("VARCHAR2") {
        return format!("VARCHAR{rest}");
    }
    compact
}

#[cfg(test)]
mod tests;
