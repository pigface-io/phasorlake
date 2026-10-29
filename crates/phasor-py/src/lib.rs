//! The phasorlake Python package's native module. Each function is a thin call
//! into what the `phasor` command runs: the same kits, decoders and registry,
//! so what Python reads of a file is what `phasor ingest` writes of it.
//!
//! Tables cross to Python by the Arrow C data interface (the PyCapsule
//! protocol): pyarrow, polars or nanoarrow read them, and nothing here links
//! against any of them.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use arrow::ffi::FFI_ArrowSchema;
use arrow::ffi_stream::FFI_ArrowArrayStream;
use arrow_array::{ArrayRef, RecordBatch, RecordBatchIterator, new_null_array};
use arrow_schema::{Field, Schema, SchemaRef};
use phasor_cli::commands::common::table_id;
use phasor_core::config::Config;
use phasor_core::{Category, Diagnose as _, Diagnostic, Fingerprint};
use phasor_kit::{
    Block, BlockSink, DecodeOptions, DecodeStats, Flow, KitRegistry, RowAt, Strictness, TableRef,
    Transform,
};
use phasor_mms_conform::inspect::{InspectError, file as inspect_file};
use phasor_pipeline::ingest_file::ROW_ERRORS_KEPT;
use phasor_pipeline::run::local_file;
use phasor_pipeline::{
    DecodeFile, FileSink, decode_file, footer_mismatch, local_remote, local_remote_in,
};
use phasor_registry::Registry;
use pyo3::exceptions::{PyException, PyUserWarning};
use pyo3::prelude::*;
use pyo3::types::{PyCapsule, PyDict, PyList};

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
    "Something phasor noted while reading: a deviation, a value kept as text, a quarantined row."
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
    set(&err).unwrap_or_else(|e| e.restore(py));
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

/// A table on its way to Python: one schema, its batches in order.
#[pyclass(frozen, name = "ArrowTable", module = "phasorlake._native")]
struct ArrowTable {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
}

fn capsule<'py, T: Send + 'static>(
    py: Python<'py>,
    value: T,
    name: &'static std::ffi::CStr,
) -> PyResult<Bound<'py, PyCapsule>> {
    PyCapsule::new_with_value(py, value, name)
}

fn schema_capsule<'py>(py: Python<'py>, schema: &Schema) -> PyResult<Bound<'py, PyCapsule>> {
    let ffi = FFI_ArrowSchema::try_from(schema).map_err(|e| PyException::new_err(e.to_string()))?;
    capsule(py, ffi, c"arrow_schema")
}

#[pymethods]
impl ArrowTable {
    /// The schema, for `pyarrow.schema(...)`.
    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        schema_capsule(py, &self.schema)
    }

    /// The rows as a stream of batches, for `pyarrow.table(...)`.
    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        // The rows are what the file holds; a reader casts them if it wants to.
        let _ = requested_schema;
        let reader = RecordBatchIterator::new(
            self.batches.clone().into_iter().map(Ok),
            self.schema.clone(),
        );
        capsule(
            py,
            FFI_ArrowArrayStream::new(Box::new(reader)),
            c"arrow_array_stream",
        )
    }

    /// Rows.
    fn __len__(&self) -> usize {
        self.batches.iter().map(RecordBatch::num_rows).sum()
    }
}

/// A schema on its way to Python.
#[pyclass(frozen, name = "ArrowSchema", module = "phasorlake._native")]
struct ArrowSchema {
    schema: SchemaRef,
}

#[pymethods]
impl ArrowSchema {
    /// The schema, for `pyarrow.schema(...)`.
    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        schema_capsule(py, &self.schema)
    }
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
    current: Option<String>,
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
        self.current = Some(table.name.clone());
        self.tables.entry(table.name.clone()).or_default();
    }

    fn batch(&mut self, b: Block) -> Result<(), Box<Diagnostic>> {
        self.tables
            .entry(b.table.name.clone())
            .or_default()
            .batches
            .push(b.batch);
        Ok(())
    }

    fn warning(&mut self, mut d: Diagnostic) {
        if d.table.is_none() {
            d.table.clone_from(&self.current);
        }
        self.warnings.push(d);
    }

    fn bad_row(&mut self, _raw: &[u8], _at: RowAt, mut d: Diagnostic) -> Flow {
        if d.table.is_none() {
            d.table.clone_from(&self.current);
        }
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
    /// Every batch of `name` under one schema: the table's lake schema, every field
    /// nullable, plus any column a batch carries beside it (a `{col}__raw`), so a
    /// value that did not fit its type is not lost.
    fn table(&self, name: &str, g: &Gathered) -> Result<ArrowTable, arrow_schema::ArrowError> {
        let lake = table_id(self.registry, name).map(|id| self.registry.lake_schema(id));
        let mut fields: Vec<Field> = lake
            .map(|s| {
                s.fields()
                    .iter()
                    .map(|f| f.as_ref().clone().with_nullable(true))
                    .collect()
            })
            .unwrap_or_default();
        for b in &g.batches {
            for f in b.schema().fields() {
                if !fields.iter().any(|have| have.name() == f.name()) {
                    fields.push(f.as_ref().clone().with_nullable(true));
                }
            }
        }
        let metadata = lake.map(|s| s.metadata().clone()).unwrap_or_default();
        let schema = Arc::new(Schema::new_with_metadata(fields, metadata));
        let batches = g
            .batches
            .iter()
            .map(|b| align(b, &schema))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ArrowTable { schema, batches })
    }
}

/// `batch` under `schema`: its columns by name, a column it lacks all null.
fn align(batch: &RecordBatch, schema: &SchemaRef) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let columns: Vec<ArrayRef> = schema
        .fields()
        .iter()
        .map(|f| {
            batch
                .column_by_name(f.name())
                .cloned()
                .unwrap_or_else(|| new_null_array(f.data_type(), batch.num_rows()))
        })
        .collect();
    RecordBatch::try_new(schema.clone(), columns)
}

/// Every table in the file at `path`, as `phasor ingest` would write it.
///
/// A zip is read entry by entry, nested zips included. `kit` names the kit that
/// reads the file (`nem`, `wem`, `gas`) when its name alone does not say; `strict`
/// refuses a block the catalog does not describe exactly; `max_bad_rows` is how
/// many malformed rows an entry may have before it fails. Warnings are raised as
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
    let local = local_file(remote, &path).map_err(|e| error(py, &io_error(&path, &e)))?;
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
    let walked = decode_file(&file, &mut sink);
    if let Some(d) = &sink.error {
        return Err(error(py, d));
    }
    let failed = walked.map_err(|d| error(py, &d))?;
    for a in &failed {
        warn(py, &a.diagnostic())?;
    }
    for d in sink.warnings.iter().chain(&sink.row_errors) {
        warn(py, d)?;
    }
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
    let stack = stack()?;
    let tmp = tempfile::tempdir()?;
    let found = inspect_file(&stack.registry, &path, tmp.path()).map_err(|e| match e {
        InspectError::Open(e) => error(py, &io_error(&path, &e)),
        InspectError::Scan(d) => error(py, &d.with_file(path.display().to_string())),
        InspectError::Archive(a) => {
            error(py, &a.diagnostic().with_file(path.display().to_string()))
        }
    })?;
    for (entry, declared, read) in &found.footers {
        warn(
            py,
            &Diagnostic::warning(
                "csv.footer_count_mismatch",
                Category::CSV,
                format!("The footer says {declared} lines but {read} were read."),
            )
            .with_entry(entry.clone()),
        )?;
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
/// Keys: `name`, `market`, `description`, `load_mode`, `key`, `time_column`,
/// `roles` (`entity`, `valid` with its `column` and `kind`, `order`, `cutoff`),
/// `columns` (each with `name`, `type`, `role`, `field_id`, `nullable`,
/// `description`) and `schema`, the lake schema for `pyarrow.schema(...)`.
#[pyfunction]
fn table<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyDict>> {
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
    out.set_item(
        "load_mode",
        match t.load_mode {
            phasor_registry::file_model::LoadMode::Append => "append",
            phasor_registry::file_model::LoadMode::Snapshot => "snapshot",
        },
    )?;
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
    out.set_item(
        "schema",
        Bound::new(
            py,
            ArrowSchema {
                schema: r.lake_schema(id).clone(),
            },
        )?,
    )?;
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
    Ok(())
}
