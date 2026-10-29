//! The phasorlake Python package's native module. Each function is a thin call
//! into what the `phasor` command runs: the same kits, decoders and registry,
//! so what Python reads of a file is what `phasor ingest` writes of it.
//!
//! Tables cross to Python by the Arrow C data interface (the PyCapsule
//! protocol): pyarrow, polars or nanoarrow read them, and nothing here links
//! against any of them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use arrow::compute::{concat, concat_batches};
use arrow_array::{ArrayRef, RecordBatch, new_null_array};
use arrow_schema::{FieldRef, Schema, SchemaRef};
use phasor_cli::commands::common::table_id;
use phasor_core::config::Config;
use phasor_core::{Category, Diagnose as _, Diagnostic, Fingerprint};
use phasor_kit::{
    Block, BlockSink, DecodeOptions, DecodeStats, Flow, KitRegistry, LocalFile, RowAt, Strictness,
    TableRef, Transform,
};
use phasor_mms_conform::inspect::{InspectError, file as inspect_file};
use phasor_pipeline::ingest_file::ROW_ERRORS_KEPT;
use phasor_pipeline::{
    DecodeFile, FileSink, decode_file, footer_mismatch, local_remote, local_remote_in,
};
use phasor_registry::Registry;
use pyo3::exceptions::{PyException, PyUserWarning};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// Opens a zone of `zonetrace` until the end of the block, the heap counted
/// under its name too, with the feature `zones`; nothing without it.
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = (
            zonetrace::enter(&crate::zones::$z),
            crate::zones::Heap::enter(&crate::zones::$z),
        );
    };
}

#[cfg(feature = "zones")]
mod zones;

mod c_data;

use c_data::{ArrowSchema, ArrowTable};

pyo3::create_exception!(
    _native,
    PhasorError,
    PyException,
    "A failure of phasor. `code` is its diagnostic code (docs/errors.md); `message`, `file`, `entry`, `table` and `line` say where."
);

pyo3::create_exception!(
    _native,
    PhasorWarning,
    PyUserWarning,
    "Something phasor noted while reading: a deviation, a value kept as text, a malformed row dropped."
);

/// The registry and kits the `phasor` command is assembled with, built once.
struct Stack {
    registry: Arc<Registry>,
    kits: Arc<KitRegistry>,
}

fn stack() -> PyResult<&'static Stack> {
    static STACK: OnceLock<Result<Stack, String>> = OnceLock::new();
    STACK
        .get_or_init(|| {
            zone!(STACK);
            let registry = phasor_nem::registry();
            let kits = phasor_cli::kits::assemble(&Config::default(), registry.clone())
                .map_err(|e| e.to_string())?;
            Ok(Stack { registry, kits })
        })
        .as_ref()
        .map_err(|e| PyException::new_err(e.clone()))
}

/// A diagnostic as a `PhasorError`, its fields on the exception.
fn error(py: Python<'_>, d: &Diagnostic) -> PyErr {
    let err = PhasorError::new_err(d.render());
    let set = |err: &PyErr| -> PyResult<()> {
        let v = err.value(py);
        v.setattr("code", d.code.as_ref())?;
        v.setattr("message", d.message.as_str())?;
        v.setattr("file", d.file.as_deref())?;
        v.setattr("entry", d.entry.as_deref())?;
        v.setattr("table", d.table.as_deref())?;
        v.setattr("line", d.line)?;
        Ok(())
    };
    // An exception of our own takes any attribute: setting one cannot fail.
    set(&err).expect("a PhasorError takes its fields");
    err
}

/// A diagnostic as a `PhasorWarning`, raised through Python's `warnings`.
fn warn(py: Python<'_>, d: &Diagnostic) -> PyResult<()> {
    let text = CString::new(format!("{}: {}", d.code, d.render()).replace('\0', " "))
        .expect("no NUL remains");
    PyErr::warn(py, &py.get_type::<PhasorWarning>(), &text, 2)
}

fn io_error(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        "csv.read_failed",
        Category::CSV,
        format!("The file {} could not be opened.", path.display()),
    )
    .with_file(path.display().to_string())
    .caused_by(e)
}

/// The batches of one table, gathered from every block of every entry.
#[derive(Default)]
struct Gathered {
    batches: Vec<RecordBatch>,
}

/// Keeps every block's rows by table; what is said of them is kept for Python.
struct Collect<'r> {
    registry: &'r Registry,
    tables: BTreeMap<String, Gathered>,
    /// The table of the block being read, for what is said without one.
    current: Option<Arc<TableRef>>,
    warnings: Vec<Diagnostic>,
    row_errors: Vec<Diagnostic>,
    error: Option<Diagnostic>,
}

impl BlockSink for Collect<'_> {
    fn block_started(
        &mut self,
        table: &Arc<TableRef>,
        _version: u16,
        _fingerprint: Fingerprint,
        _transform: &Arc<Transform>,
        _status: &str,
    ) {
        self.current = Some(Arc::clone(table));
        self.gathered(&table.name);
    }

    fn batch(&mut self, b: Block) -> Result<(), Box<Diagnostic>> {
        self.gathered(&b.table.name).batches.push(b.batch);
        Ok(())
    }

    fn warning(&mut self, mut d: Diagnostic) {
        self.name(&mut d);
        self.warnings.push(d);
    }

    fn bad_row(&mut self, _raw: &[u8], _at: RowAt, mut d: Diagnostic) -> Flow {
        self.name(&mut d);
        if self.row_errors.len() < ROW_ERRORS_KEPT {
            self.row_errors.push(d);
        }
        Flow::Continue
    }
}

impl FileSink for Collect<'_> {
    fn entry_started(&mut self, _entry: &str, _zip_entry_modified: Option<i64>) {
        self.current = None;
    }

    fn entry_done(&mut self, entry: &str, result: Result<DecodeStats, Box<Diagnostic>>) -> Flow {
        match result {
            Ok(stats) => {
                if let Some(d) = footer_mismatch(&stats, phasor_core::Severity::Warning) {
                    self.warnings.push(d.with_entry(entry.to_owned()));
                }
                Flow::Continue
            }
            Err(d) => {
                self.error = Some(d.with_entry(entry.to_owned()));
                Flow::Stop
            }
        }
    }
}

impl Collect<'_> {
    /// The batches of `name`; its name is copied only the first time.
    fn gathered(&mut self, name: &str) -> &mut Gathered {
        if !self.tables.contains_key(name) {
            self.tables.insert(name.to_owned(), Gathered::default());
        }
        self.tables.get_mut(name).expect("inserted above")
    }

    /// `d` names the current block's table, unless it names one already.
    fn name(&self, d: &mut Diagnostic) {
        if d.table.is_none() {
            d.table = self.current.as_ref().map(|t| t.name.clone());
        }
    }

    /// Every batch of `name` under one schema: the table's lake schema, every field
    /// nullable, plus any column a batch carries beside it (a `{col}__raw`), so a
    /// value that did not fit its type is not lost. The registry's schema, its
    /// fields and every column are shared, not copied; a column a batch lacks is
    /// one null array, shared by every batch of its length.
    fn table(&self, name: &str, g: &Gathered) -> Result<ArrowTable, arrow_schema::ArrowError> {
        let lake = table_id(self.registry, name).map(|id| self.registry.lake_schema(id));
        // A block's batches share one schema: each shape is looked at once.
        let mut shapes: Vec<&SchemaRef> = Vec::new();
        for b in &g.batches {
            if !shapes.iter().any(|have| Arc::ptr_eq(have, b.schema_ref())) {
                shapes.push(b.schema_ref());
            }
        }
        let schema = match lake {
            Some(lake)
                if lake.fields().iter().all(|f| f.is_nullable()) && !widens(lake, &shapes) =>
            {
                Arc::clone(lake)
            }
            _ => Arc::new(widened(lake, &shapes)),
        };
        let mut nulls: Vec<ArrayRef> = Vec::new();
        let mut placed: Vec<Option<Vec<Option<usize>>>> = Vec::with_capacity(shapes.len());
        for shape in &shapes {
            placed.push((***shape != *schema).then(|| columns_of(shape, &schema)));
        }
        let mut aligned = Vec::with_capacity(g.batches.len());
        for b in &g.batches {
            let at = shapes
                .iter()
                .position(|have| Arc::ptr_eq(have, b.schema_ref()))
                .expect("every batch's shape was listed");
            aligned.push(match &placed[at] {
                // The batch has the table's shape already: it goes as it is.
                None => b.clone(),
                Some(columns) => align(b, columns, &schema, &mut nulls)?,
            });
        }
        let batches = coalesce(&schema, aligned)?;
        Ok(ArrowTable { schema, batches })
    }
}

/// `batch` copied into buffers of its own size. `concat` of one array hands
/// back a slice of it, room and all; beside an empty slice of itself, it is
/// copied, each buffer sized to what it holds.
fn compact(batch: &RecordBatch) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let mut arrays = Vec::with_capacity(batch.num_columns());
    for c in batch.columns() {
        arrays.push(concat(&[c.as_ref(), c.slice(0, 0).as_ref()])?);
    }
    RecordBatch::try_new(batch.schema(), arrays)
}

/// Rows a batch of small ones is gathered up to, at least.
const COALESCE_ROWS: usize = 8192;

/// `batches` with each run of small ones copied into one. A builder keeps
/// room for more rows than a small block fills, and its batch holds that
/// room for as long as Python holds the table: a zip of 5-minute files would
/// hold hundreds of times its rows, in as many chunks. A batch at least half
/// the size of the largest goes as it is, uncopied.
fn coalesce(
    schema: &SchemaRef,
    batches: Vec<RecordBatch>,
) -> Result<Vec<RecordBatch>, arrow_schema::ArrowError> {
    let full = batches
        .iter()
        .map(RecordBatch::num_rows)
        .max()
        .unwrap_or(0)
        .max(COALESCE_ROWS);
    let mut out = Vec::with_capacity(batches.len());
    let mut run: Vec<RecordBatch> = Vec::new();
    let mut run_rows = 0;
    let flush = |run: &mut Vec<RecordBatch>, out: &mut Vec<RecordBatch>| {
        match run.len() {
            0 => {}
            1 => out.push(compact(&run[0])?),
            _ => out.push(concat_batches(schema, run.iter())?),
        }
        run.clear();
        Ok::<(), arrow_schema::ArrowError>(())
    };
    for b in batches {
        let rows = b.num_rows();
        if rows * 2 >= full {
            flush(&mut run, &mut out)?;
            run_rows = 0;
            out.push(b);
            continue;
        }
        if run_rows + rows > full {
            flush(&mut run, &mut out)?;
            run_rows = 0;
        }
        run_rows += rows;
        run.push(b);
    }
    flush(&mut run, &mut out)?;
    Ok(out)
}

/// Whether a batch of one of `shapes` has a column `lake` lacks.
fn widens(lake: &Schema, shapes: &[&SchemaRef]) -> bool {
    shapes.iter().any(|s| {
        in_order(s, lake).is_none()
            && s.fields()
                .iter()
                .any(|f| lake.field_with_name(f.name()).is_err())
    })
}

/// Where each of `schema`'s fields is in `from`, when `from`'s fields are
/// some of `schema`'s in `schema`'s order, as a version's columns are its
/// table's: one pass, nothing looked up.
fn in_order(from: &Schema, schema: &Schema) -> Option<Vec<Option<usize>>> {
    let have = from.fields();
    let mut next = 0;
    let mut at = Vec::with_capacity(schema.fields().len());
    for f in schema.fields() {
        if have.get(next).is_some_and(|h| h.name() == f.name()) {
            at.push(Some(next));
            next += 1;
        } else {
            at.push(None);
        }
    }
    (next == have.len()).then_some(at)
}

/// The lake schema's fields, then every column of `shapes` it lacks, each
/// nullable; a field is copied only to make it so.
fn widened(lake: Option<&SchemaRef>, shapes: &[&SchemaRef]) -> Schema {
    let nullable = |f: &FieldRef| {
        if f.is_nullable() {
            Arc::clone(f)
        } else {
            Arc::new(f.as_ref().clone().with_nullable(true))
        }
    };
    let width = lake.map_or(0, |l| l.fields().len());
    let mut fields: Vec<FieldRef> = Vec::with_capacity(width + 1);
    let mut have: HashSet<&str> = HashSet::with_capacity(width + 1);
    for f in lake.into_iter().flat_map(|l| l.fields().iter()) {
        have.insert(f.name());
        fields.push(nullable(f));
    }
    for f in shapes.iter().flat_map(|s| s.fields().iter()) {
        if have.insert(f.name()) {
            fields.push(nullable(f));
        }
    }
    let metadata = lake.map(|s| s.metadata().clone()).unwrap_or_default();
    Schema::new_with_metadata(fields, metadata)
}

/// Where each of `schema`'s fields is in `from`, by name; `None` where `from` lacks it.
fn columns_of(from: &Schema, schema: &Schema) -> Vec<Option<usize>> {
    if let Some(at) = in_order(from, schema) {
        return at;
    }
    let at: HashMap<&str, usize> = from
        .fields()
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name().as_str(), i))
        .collect();
    schema
        .fields()
        .iter()
        .map(|f| at.get(f.name().as_str()).copied())
        .collect()
}

/// `batch` under `schema`, its columns where `columns` says. A column it lacks
/// is null: one array of each type and length, kept in `nulls` and shared.
fn align(
    batch: &RecordBatch,
    columns: &[Option<usize>],
    schema: &SchemaRef,
    nulls: &mut Vec<ArrayRef>,
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let rows = batch.num_rows();
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    for (f, at) in schema.fields().iter().zip(columns) {
        arrays.push(match at {
            Some(i) => Arc::clone(batch.column(*i)),
            None => {
                let shared = nulls
                    .iter()
                    .find(|n| n.len() == rows && n.data_type() == f.data_type());
                if let Some(n) = shared {
                    Arc::clone(n)
                } else {
                    let n = new_null_array(f.data_type(), rows);
                    nulls.push(Arc::clone(&n));
                    n
                }
            }
        });
    }
    RecordBatch::try_new(Arc::clone(schema), arrays)
}

/// Every table in the file at `path`, as `phasor ingest` would write it.
///
/// A zip is read entry by entry, nested zips included. `kit` names the kit that
/// reads the file (`nem`, `wem`, `gas`) when its name alone does not say; `strict`
/// refuses a block the catalog does not describe exactly; `max_bad_rows` is how
/// many malformed rows an entry may drop before it fails. The GIL is released
/// while the file is read. Warnings are raised as
/// `PhasorWarning`; a failure as `PhasorError`.
#[pyfunction]
#[pyo3(signature = (path, *, kit=None, strict=false, max_bad_rows=0))]
#[expect(
    clippy::needless_pass_by_value,
    reason = "pyo3 hands arguments over by value"
)]
fn decode<'py>(
    py: Python<'py>,
    path: PathBuf,
    kit: Option<&str>,
    strict: bool,
    max_bad_rows: u64,
) -> PyResult<Bound<'py, PyDict>> {
    zone!(DECODE);
    let stack = stack()?;
    let claimed = match kit {
        Some(k) => local_remote_in(&stack.kits, k, &path),
        None => local_remote(&stack.kits, &path),
    };
    let Some((kit_id, remote)) = claimed else {
        let d = Diagnostic::error(
            "config.no_decoder",
            Category::CONFIG,
            format!(
                "The file {} is not one any kit reads; no decoder accepts its name and it is not a zip.",
                path.display()
            ),
        )
        .with_file(path.display().to_string());
        return Err(error(py, &d));
    };
    let kit = stack
        .kits
        .kit(kit_id)
        .expect("the kit that claimed the file is assembled");
    // The pipeline hashes a file for its provenance; nothing here records one,
    // so the file is only looked at, for the error a missing one gets.
    let size = std::fs::metadata(&path)
        .map_err(|e| error(py, &io_error(&path, &e)))?
        .len();
    let local = LocalFile {
        remote,
        path: path.clone(),
        sha256: String::new(),
        size,
        cached: true,
        etag: None,
        last_modified: None,
        last_modified_at: None,
        fetched_at: None,
    };
    let tmp = tempfile::tempdir()?;
    let options = DecodeOptions {
        strictness: if strict {
            Strictness::Strict
        } else {
            Strictness::Permissive
        },
        max_bad_rows,
        batch_rows: phasor_pipeline::DEFAULT_BATCH_ROWS,
    };
    let file = DecodeFile {
        kit,
        local: &local,
        options: &options,
        tmp_dir: tmp.path(),
        skip: &[],
    };
    let mut sink = Collect {
        registry: &stack.registry,
        tables: BTreeMap::new(),
        current: None,
        warnings: Vec::new(),
        row_errors: Vec::new(),
        error: None,
    };
    // Other Python threads run while the file is read: decoding files on a
    // thread pool reads them in parallel.
    let walked = py.detach(|| {
        zone!(WALK);
        decode_file(&file, &mut sink)
    });
    if let Some(d) = &sink.error {
        return Err(error(py, d));
    }
    let failed = walked.map_err(|d| error(py, &d))?;
    {
        zone!(WARN);
        for a in &failed {
            warn(py, &a.diagnostic())?;
        }
        for d in sink.warnings.iter().chain(&sink.row_errors) {
            warn(py, d)?;
        }
    }
    zone!(GATHER);
    let out = PyDict::new(py);
    for (name, g) in &sink.tables {
        let table = sink
            .table(name, g)
            .map_err(|e| PyException::new_err(e.to_string()))?;
        out.set_item(name, Bound::new(py, table)?)?;
    }
    Ok(out)
}

/// Every block of every CSV in the file at `path`, as `phasor inspect` lists them.
///
/// Each item: `entry`, `line`, `report_type`, `report_subtype`, `version`,
/// `columns`, `fingerprint`, `rows`, `status` (`conforms`, `deviates`, `derived`
/// or the resolve error's code) and `table`.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "pyo3 hands arguments over by value"
)]
fn inspect<'py>(py: Python<'py>, path: PathBuf) -> PyResult<Bound<'py, PyList>> {
    zone!(INSPECT);
    let stack = stack()?;
    let tmp = tempfile::tempdir()?;
    let found = py.detach(|| inspect_file(&stack.registry, &path, tmp.path()));
    let found = found.map_err(|e| match e {
        InspectError::Open(e) => error(py, &io_error(&path, &e)),
        InspectError::Scan(d) => error(py, &d.with_file(path.display().to_string())),
        InspectError::Archive(a) => {
            error(py, &a.diagnostic().with_file(path.display().to_string()))
        }
    })?;
    for (entry, declared, read) in &found.footers {
        let d = Diagnostic::warning(
            "csv.footer_count_mismatch",
            Category::CSV,
            format!("The footer says {declared} lines but {read} were read."),
        )
        .with_entry(entry.clone());
        warn(py, &d)?;
    }
    for a in &found.failed {
        warn(py, &a.diagnostic())?;
    }
    let out = PyList::empty(py);
    for (entry, b) in &found.blocks {
        let d = PyDict::new(py);
        d.set_item("entry", entry)?;
        d.set_item("line", b.line)?;
        d.set_item("report_type", &b.report_type)?;
        d.set_item("report_subtype", &b.report_subtype)?;
        d.set_item("version", b.version)?;
        d.set_item("columns", &b.columns)?;
        d.set_item("fingerprint", b.fingerprint.to_string())?;
        d.set_item("rows", b.rows)?;
        d.set_item("status", &b.status)?;
        d.set_item("table", b.table.as_deref())?;
        out.append(d)?;
    }
    Ok(out)
}

/// What the registry says of table `name`, as `phasor list TABLE --json` does.
///
/// Keys: `name`, `market`, `description`, `load_mode`, `time_column`,
/// `columns` (each with `name`, `type`, `role`, `field_id`, `nullable`,
/// `description`) and `schema`, the lake schema for `pyarrow.schema(...)`.
#[pyfunction]
fn table<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyDict>> {
    zone!(TABLE);
    let stack = stack()?;
    let r = &stack.registry;
    let Some(id) = table_id(r, name) else {
        let d = Diagnostic::error(
            "schema.unknown_table",
            Category::SCHEMA,
            format!("No table is named {name}; `phasor list` names them."),
        )
        .with_table(name.to_owned());
        return Err(error(py, &d));
    };
    let t = r.table(id);
    let docs = t.published().docs;
    let out = PyDict::new(py);
    out.set_item("name", &t.name)?;
    out.set_item("market", t.market.to_string())?;
    out.set_item("description", t.description.as_deref())?;
    let load_mode = match t.load_mode {
        phasor_registry::file_model::LoadMode::Append => "append",
        phasor_registry::file_model::LoadMode::Snapshot => "snapshot",
    };
    out.set_item("load_mode", load_mode)?;
    out.set_item("time_column", t.time_column.as_deref())?;
    let columns = PyList::empty(py);
    for c in &t.columns {
        let d = PyDict::new(py);
        d.set_item("name", &c.name)?;
        d.set_item("type", c.ty.to_string())?;
        let time = t.time_column.as_deref() == Some(c.name.as_str());
        d.set_item("role", if time { "time" } else { "" })?;
        d.set_item("field_id", c.id.0)?;
        d.set_item("nullable", c.nullable)?;
        d.set_item("description", docs.get(&c.id.0))?;
        columns.append(d)?;
    }
    out.set_item("columns", columns)?;
    let schema = ArrowSchema {
        schema: r.lake_schema(id).clone(),
    };
    out.set_item("schema", Bound::new(py, schema)?)?;
    Ok(out)
}

/// The version of phasor this module was built from.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The module `phasorlake._native`.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(decode, m)?)?;
    m.add_function(wrap_pyfunction!(inspect, m)?)?;
    m.add_function(wrap_pyfunction!(table, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_class::<ArrowTable>()?;
    m.add_class::<ArrowSchema>()?;
    m.add("PhasorError", m.py().get_type::<PhasorError>())?;
    m.add("PhasorWarning", m.py().get_type::<PhasorWarning>())?;
    #[cfg(feature = "zones")]
    {
        m.add_function(wrap_pyfunction!(zones::_zone, m)?)?;
        m.add_function(wrap_pyfunction!(zones::_zones, m)?)?;
        m.add_function(wrap_pyfunction!(zones::_memory, m)?)?;
        m.add_class::<zones::PyZone>()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use arrow_array::{Array, Int64Array, StringArray};
    use arrow_schema::{DataType, Field};

    use super::*;

    fn field(name: &str, nullable: bool) -> Field {
        Field::new(name, DataType::Int64, nullable)
    }

    fn schema(names: &[&str]) -> SchemaRef {
        Arc::new(Schema::new(
            names.iter().map(|n| field(n, true)).collect::<Vec<_>>(),
        ))
    }

    fn batch(schema: &SchemaRef, rows: i64) -> RecordBatch {
        let columns = schema
            .fields()
            .iter()
            .map(|_| Arc::new(Int64Array::from_iter_values(0..rows)) as ArrayRef)
            .collect();
        RecordBatch::try_new(Arc::clone(schema), columns).expect("a batch")
    }

    #[test]
    fn a_version_s_columns_are_found_in_one_pass() {
        let table = schema(&["a", "b", "c"]);
        assert_eq!(
            in_order(&schema(&["a", "c"]), &table),
            Some(vec![Some(0), None, Some(1)])
        );
        // Out of the table's order: one pass cannot place them, a lookup does.
        let swapped = schema(&["c", "a"]);
        assert_eq!(in_order(&swapped, &table), None);
        assert_eq!(columns_of(&swapped, &table), vec![Some(1), None, Some(0)]);
    }

    #[test]
    fn a_column_the_lake_lacks_widens_it_and_every_field_is_nullable() {
        let lake = Arc::new(Schema::new(vec![field("a", false), field("b", true)]));
        let raw = schema(&["b", "a", "a__raw"]);
        assert!(widens(&lake, &[&raw]));
        assert!(!widens(&lake, &[&schema(&["a"])]));
        let wide = widened(Some(&lake), &[&raw]);
        let names: Vec<&str> = wide.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, ["a", "b", "a__raw"]);
        assert!(wide.fields().iter().all(|f| f.is_nullable()));
        // A field already nullable is the lake's own, not a copy.
        assert!(Arc::ptr_eq(&wide.fields()[1], &lake.fields()[1]));
    }

    #[test]
    fn missing_columns_share_one_null_array() {
        let table = schema(&["a", "b", "c"]);
        let part = schema(&["a"]);
        let mut nulls = Vec::new();
        let columns = columns_of(&part, &table);
        let first = align(&batch(&part, 4), &columns, &table, &mut nulls).expect("aligned");
        let second = align(&batch(&part, 4), &columns, &table, &mut nulls).expect("aligned");
        assert_eq!(first.column(1).null_count(), 4);
        assert!(Arc::ptr_eq(first.column(1), first.column(2)));
        assert!(Arc::ptr_eq(first.column(1), second.column(1)));
        assert_eq!(nulls.len(), 1);
    }

    #[test]
    fn small_batches_are_gathered_and_large_ones_go_as_they_are() {
        let s = schema(&["a"]);
        let large = batch(&s, 10_000);
        let batches = vec![batch(&s, 3), batch(&s, 4), large.clone(), batch(&s, 5)];
        let out = coalesce(&s, batches).expect("coalesced");
        let rows: Vec<usize> = out.iter().map(RecordBatch::num_rows).collect();
        assert_eq!(rows, [7, 10_000, 5]);
        assert!(Arc::ptr_eq(out[1].column(0), large.column(0)));
        // A large batch first: nothing gathered before it.
        let first = coalesce(&s, vec![large.clone(), batch(&s, 2)]).expect("coalesced");
        assert!(Arc::ptr_eq(first[0].column(0), large.column(0)));
        assert_eq!(first[1].num_rows(), 2);
        // A run of small batches past the largest's size starts another.
        let many = vec![batch(&s, 3000), batch(&s, 3000), batch(&s, 3000)];
        let rows: Vec<usize> = coalesce(&s, many)
            .expect("coalesced")
            .iter()
            .map(RecordBatch::num_rows)
            .collect();
        assert_eq!(rows, [6000, 3000]);
    }

    #[test]
    fn a_batch_alone_is_copied_to_its_own_size() {
        let s = Arc::new(Schema::new(vec![Field::new("t", DataType::Utf8, true)]));
        let mut b = arrow_array::builder::StringBuilder::with_capacity(4096, 65_536);
        b.append_value("x");
        let roomy =
            RecordBatch::try_new(Arc::clone(&s), vec![Arc::new(b.finish())]).expect("a batch");
        let small = compact(&roomy).expect("compacted");
        let text = small
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("text");
        assert_eq!(text.value(0), "x");
        assert!(small.get_array_memory_size() * 10 < roomy.get_array_memory_size());
    }
}
