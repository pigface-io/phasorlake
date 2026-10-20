//! `export`: every table and shape as JSON Schema, for the public
//! `nem-schemas` repository.
//!
//! One document per table per shape (`nem/{TABLE}/{shape}.schema.json`):
//! the shape's columns as properties, typed by the type class of the month's
//! control file and, where the shape belongs to a declared table, by our
//! lake type; the table's key, description and temporal roles; the increment
//! the shape conforms to; and every month the census saw it in, with its
//! rows. Beside them: `nem/{TABLE}/index.json`, `manifest.json` (content
//! only, so a second run gives the same bytes), `provenance.json` (which
//! commit wrote it, never compared) and `LICENCE.md`. The writer owns
//! `nem/` whole: a shape that disappears upstream disappears here.
//!
//! Everything is read from `schemas/` and the data-model JSON; nothing is
//! typed by hand, so the output is regenerable by anyone with the archive.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use utilake_core::{Category, Diagnose, Diagnostic, YearMonth};
use utilake_mms_registry::file_model::blob::SchemaSet;
use utilake_mms_registry::file_model::model::Table;
use utilake_mms_registry::file_model::shapes::ShapeEntry;
use utilake_mms_registry::shape::{ShapeId, TypeClass};

use crate::census::{FileEntry, census_dir, read_month};
use crate::datamodel::{DataModel, DmTable};

mod doc;
mod manifest;
mod write;

pub use write::{Stale, Written, check, write};

/// The generation of the documents' layout: the `x-` keys and their
/// meaning. Raised when a reader of format 1 would misread a document.
pub const FORMAT: u32 = 1;

/// Where the documents are published: `$id` is this plus the file's path.
pub const DEFAULT_BASE_URL: &str = "https://raw.githubusercontent.com/pigface-io/nem-schemas/main/";

/// The licence file written beside the documents.
pub const LICENCE: &str = include_str!("../../templates/nem-schemas/LICENCE.md");

/// What the export reads.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// The schemas directory.
    pub schemas: PathBuf,
    /// The data-model JSON, for packages, visibility and column comments.
    pub datamodel: Option<PathBuf>,
    /// The URL the documents will live under.
    pub base_url: String,
}

/// Every month a shape was seen in, with the rows the census counted. `None`
/// rows: the month's census predates row counts.
pub type Seen = BTreeMap<YearMonth, Option<u64>>;

/// One shape of one table, every version of it folded together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeDoc<'a> {
    /// The table the shape file is named for.
    pub table: &'a str,
    /// The table's declaration, when it has one.
    pub declared: Option<&'a Table>,
    /// The data model's entry, when it has one.
    pub model: Option<&'a DmTable>,
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
    pub columns: &'a [(String, TypeClass)],
    /// Months and rows from the census.
    pub seen: &'a Seen,
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
    schemas: PathBuf,
    base_url: String,
    /// Census blocks that name a shape no shapes file records.
    problems: Vec<Diagnostic>,
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
    /// Reads the schemas, every census month and the data model.
    pub fn load(inputs: &Inputs) -> Result<Self, Box<Diagnostic>> {
        let set = SchemaSet::read_dir(&inputs.schemas).map_err(|e| Box::new(e.diagnostic()))?;
        let datamodel = match &inputs.datamodel {
            Some(p) => DataModel::load(p).map_err(|e| Box::new(e.diagnostic()))?,
            None => DataModel::default(),
        };
        let known: BTreeSet<ShapeId> = set.shapes.values().flatten().map(|e| e.id).collect();
        let mut seen: BTreeMap<ShapeId, Seen> = BTreeMap::new();
        let mut months = BTreeSet::new();
        let mut problems = Vec::new();
        for month in census_months(&inputs.schemas)? {
            // Listed a moment ago, so it is there; one that vanished meanwhile is empty.
            let files = read_month(&inputs.schemas, month)
                .map_err(|e| Box::new(e.diagnostic()))?
                .unwrap_or_default();
            months.insert(month);
            for (file, entry) in &files {
                for (id, rows) in blocks(entry) {
                    if !known.contains(&id) {
                        problems.push(unknown_shape(month, file, id));
                        continue;
                    }
                    let slot = seen.entry(id).or_default().entry(month).or_insert(Some(0));
                    *slot = match (*slot, rows) {
                        (Some(have), Some(more)) => Some(have + more),
                        _ => None,
                    };
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
            schemas: inputs.schemas.clone(),
            base_url: inputs.base_url.clone(),
            problems,
        })
    }

    /// Renders every file.
    pub fn render(&self) -> Result<Rendered, Box<Diagnostic>> {
        let mut out = Rendered {
            problems: self.problems.clone(),
            ..Rendered::default()
        };
        let empty = Seen::new();
        let mut shape_count = 0usize;
        for (table, entries) in &self.shapes {
            let declared = self.tables.get(table);
            let model = self.datamodel.tables.get(table);
            let mut docs = Vec::new();
            for doc in fold(table, entries, &self.seen, &empty) {
                let doc = ShapeDoc {
                    declared,
                    model,
                    ..doc
                };
                let matched = doc::increment(&doc);
                let version = match matched {
                    Some(m) => self.model_version(table, m.increment.ctl.as_deref())?,
                    None => None,
                };
                let path = format!("nem/{table}/{}.schema.json", doc.id);
                let id = format!("{}{path}", self.base_url);
                let text = doc::render(&doc, &id, matched.map(|m| (m, version.as_deref())));
                out.files.insert(path, text);
                docs.push((doc, matched.map(|m| m.n())));
                shape_count += 1;
            }
            out.files.insert(
                format!("nem/{table}/index.json"),
                doc::index(table, declared.is_some(), &docs),
            );
        }
        let counts = manifest::Counts {
            tables: self.shapes.len(),
            declared_tables: self
                .shapes
                .keys()
                .filter(|t| self.tables.contains_key(*t))
                .count(),
            shapes: shape_count,
        };
        let manifest = manifest::render(&self.base_url, &self.months, counts, &out.files);
        out.files.insert("manifest.json".to_owned(), manifest);
        out.files
            .insert("LICENCE.md".to_owned(), LICENCE.to_owned());
        out.provenance = manifest::provenance(&self.schemas, self.datamodel_path.as_deref());
        Ok(out)
    }

    /// The data-model version the control file names in its history line.
    fn model_version(
        &self,
        table: &str,
        ctl: Option<&str>,
    ) -> Result<Option<String>, Box<Diagnostic>> {
        let Some(ctl) = ctl else { return Ok(None) };
        let path = self.schemas.join("nem").join(table).join("ctl").join(ctl);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Box::new(IoError::new(&path, e).diagnostic()))?;
        Ok(model_version(&text))
    }
}

/// The months with a census file, ascending.
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

/// A file's blocks as `(shape, rows)`; a census taken before rows were
/// counted gives `None`.
fn blocks(entry: &FileEntry) -> Vec<(ShapeId, Option<u64>)> {
    match entry {
        FileEntry::One(id) => vec![(*id, None)],
        FileEntry::Many(ids) => ids.iter().map(|id| (*id, None)).collect(),
        FileEntry::Blocks(blocks) => blocks.iter().map(|b| (b.shape, Some(b.rows))).collect(),
    }
}

fn unknown_shape(month: YearMonth, file: &str, id: ShapeId) -> Diagnostic {
    Diagnostic::warning(
        "schema.export_unknown_shape",
        Category::Schema,
        format!("The census of {month} records shape {id} in {file}, which no shapes file knows; the shape is left out."),
    )
    .with_file(file.to_owned())
}

/// One document per id: the versions of a shape, folded.
fn fold<'a>(
    table: &'a str,
    entries: &'a [ShapeEntry],
    seen: &'a BTreeMap<ShapeId, Seen>,
    empty: &'a Seen,
) -> Vec<ShapeDoc<'a>> {
    let mut by_id: BTreeMap<ShapeId, ShapeDoc<'a>> = BTreeMap::new();
    for e in entries {
        by_id
            .entry(e.id)
            .and_modify(|d| {
                d.versions.push(e.version);
                d.first = d.first.min(e.first_seen);
                d.last = d.last.max(e.last_seen);
            })
            .or_insert_with(|| ShapeDoc {
                table,
                declared: None,
                model: None,
                id: e.id,
                report_type: &e.report_type,
                report_subtype: &e.report_subtype,
                versions: vec![e.version],
                first: e.first_seen,
                last: e.last_seen,
                columns: &e.columns,
                seen: seen.get(&e.id).unwrap_or(empty),
            });
    }
    let mut docs: Vec<ShapeDoc<'a>> = by_id.into_values().collect();
    for d in &mut docs {
        d.versions.sort_unstable();
        d.versions.dedup();
    }
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

#[cfg(test)]
mod tests;
