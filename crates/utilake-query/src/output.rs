//! What a read returns, and how it is written out: resolved frames and
//! raw ones are different types, so a caller cannot pass one off as the
//! other; one Parquet, CSV or Arrow file with a JSON sidecar that names
//! the interval, the schema, the resolution and every source.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Int32Type, UInt32Type};
use arrow_array::{ArrayRef, DictionaryArray, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use serde::Serialize;
use utilake_core::Diagnostic;

use crate::error::QueryError;
use crate::query::{Bound, Resolution};
use crate::resolve::VALID_TO;
use crate::scan::{KNOWN, SOURCE_IX};
use crate::time::{ValidWindow, rfc3339};

/// The column of every output row that names its source, `url!entry`.
pub const SOURCE: &str = "_nl_source";

/// Rows with one row per fact (or per revision), as the roles say.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved(pub RecordBatch);

/// Rows as the files hold them.
#[derive(Debug, Clone, PartialEq)]
pub struct RawBatch(pub RecordBatch);

/// One batch of a read, of the kind the resolution gives.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Resolved.
    Resolved(Resolved),
    /// Raw.
    Raw(RawBatch),
}

impl Frame {
    /// The rows, whichever kind.
    pub fn batch(&self) -> &RecordBatch {
        match self {
            Self::Resolved(r) => &r.0,
            Self::Raw(r) => &r.0,
        }
    }
}

/// One source the read took rows from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRef {
    /// The URL.
    pub url: String,
    /// The entry.
    pub entry: String,
    /// The month of the source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_period: Option<String>,
    /// When it was published, RFC 3339 UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

/// What a read did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Rows read from the files.
    pub rows_read: u64,
    /// Rows returned.
    pub rows_out: u64,
    /// Rows outside the window.
    pub dropped_window: u64,
    /// Rows not known by `known_at`.
    pub dropped_unknown: u64,
    /// Rows whose rank was NaN.
    pub nan_rows: u64,
    /// Units resolved.
    pub units: u64,
    /// Files read.
    pub files: u64,
    /// What was noted.
    pub warnings: Vec<Diagnostic>,
    /// The sources, in stamp order.
    pub sources: Vec<SourceRef>,
    /// Where the time went.
    pub timing: Timing,
}

/// Microseconds spent in each stage of a read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Timing {
    /// Reading, projecting and trimming the files.
    pub scan_us: u64,
    /// Joining a unit's batches into one.
    pub concat_us: u64,
    /// The kernel.
    pub resolve_us: u64,
    /// The output schema, the source names, the projection.
    pub finish_us: u64,
}

/// A read's frames.
#[derive(Debug, Clone)]
pub struct Frames {
    /// The columns every frame has.
    pub schema: SchemaRef,
    /// The frames, in unit order.
    pub frames: Vec<Frame>,
    /// What the read did.
    pub report: Report,
}

/// The file an extract writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Parquet, zstd, with the lake's field ids and `utilake.*` footer keys.
    Parquet,
    /// CSV with a header row.
    Csv,
    /// Arrow IPC file.
    Arrow,
}

impl Format {
    /// The file extension.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Parquet => "parquet",
            Self::Csv => "csv",
            Self::Arrow => "arrow",
        }
    }
}

/// What was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// The data file.
    pub path: PathBuf,
    /// The JSON sidecar beside it.
    pub sidecar: PathBuf,
    /// Rows written.
    pub rows: u64,
    /// What the read did.
    pub report: Report,
}

/// The output schema: the kept target columns, then `_nl_source`, then
/// `_nl_known` and (on a `from` axis) `_nl_valid_to` for resolved rows.
pub fn output_schema(b: &Bound, resolved: &SchemaRef) -> SchemaRef {
    let mut fields: Vec<Field> = match &b.keep {
        Some(keep) => keep.iter().map(|i| b.schema.field(i.0).clone()).collect(),
        None => b
            .schema
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect(),
    };
    fields.push(Field::new(
        SOURCE,
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        false,
    ));
    if b.query.resolution != Resolution::Raw {
        for name in [KNOWN, VALID_TO] {
            if let Ok(i) = resolved.index_of(name) {
                fields.push(resolved.field(i).clone());
            }
        }
    }
    Arc::new(Schema::new(fields))
}

/// A scanned or resolved batch in the output schema: the source index
/// becomes a dictionary of source names, the projection is applied.
pub fn finish(
    batch: &RecordBatch,
    b: &Bound,
    out: &SchemaRef,
    sources: &ArrayRef,
) -> Result<RecordBatch, QueryError> {
    let err = |e| QueryError::arrow(&b.table, e);
    let schema = batch.schema();
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(out.fields().len());
    for f in out.fields() {
        let col = if f.name() == SOURCE {
            let ix = schema.index_of(SOURCE_IX).map_err(err)?;
            let keys: Int32Array = batch
                .column(ix)
                .as_primitive::<UInt32Type>()
                .iter()
                .map(|k| k.map(|k| k as i32))
                .collect();
            let dict = DictionaryArray::<Int32Type>::try_new(keys, sources.clone()).map_err(err)?;
            Arc::new(dict) as ArrayRef
        } else {
            let ix = schema.index_of(f.name()).map_err(err)?;
            batch.column(ix).clone()
        };
        columns.push(col);
    }
    RecordBatch::try_new(out.clone(), columns).map_err(err)
}

/// The names of every source, as the dictionary of `_nl_source`.
pub fn source_names(lake: &crate::Lake<'_>) -> ArrayRef {
    let names: Vec<String> = lake.files.iter().map(crate::LakeFile::source).collect();
    Arc::new(StringArray::from(names))
}

/// What the sidecar says.
#[derive(Debug, Serialize)]
struct Sidecar<'a> {
    table: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_id: Option<u32>,
    resolution: &'static str,
    window: WindowText,
    #[serde(skip_serializing_if = "Option::is_none")]
    known_at: Option<String>,
    rows: u64,
    rows_read: u64,
    dropped_window: u64,
    dropped_unknown: u64,
    nan_rows: u64,
    sources: &'a [SourceRef],
    warnings: Vec<&'a str>,
    utilake: &'a str,
}

/// The window, in the market's clock.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum WindowText {
    /// A range.
    Range {
        /// The start.
        after: String,
        /// The end.
        upto: String,
    },
    /// An instant.
    At {
        /// The instant.
        at: String,
    },
}

/// The window as text in the market's clock.
pub fn window_text(b: &Bound) -> WindowText {
    let off = b.offset.0;
    match b.query.window {
        ValidWindow::Range { after, upto } => WindowText::Range {
            after: rfc3339(after.get(), off),
            upto: rfc3339(upto.get(), off),
        },
        ValidWindow::At(t) => WindowText::At {
            at: rfc3339(t.get(), off),
        },
    }
}

/// Writes frames as one file of `format` at `path`, and the sidecar as
/// `{path}.json`.
pub fn write(
    frames: &Frames,
    b: &Bound,
    format: Format,
    path: &Path,
    version: &str,
) -> Result<Written, QueryError> {
    let io = |e| QueryError::io(path, e);
    let arrow = |e| QueryError::arrow(&path.display().to_string(), e);
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let file = File::create(path).map_err(io)?;
    let batches = frames.frames.iter().map(Frame::batch);
    let rows: u64 = frames
        .frames
        .iter()
        .map(|f| f.batch().num_rows() as u64)
        .sum();
    match format {
        Format::Parquet => {
            let known_at = match b.query.resolution {
                Resolution::Latest { known_at: Some(k) } => rfc3339(k.get(), 0),
                _ => String::new(),
            };
            let kv = vec![
                KeyValue::new("utilake.version".to_owned(), version.to_owned()),
                KeyValue::new("utilake.table".to_owned(), b.table.clone()),
                KeyValue::new(
                    "utilake.schema_id".to_owned(),
                    b.schema_id.map_or_else(String::new, |s| s.to_string()),
                ),
                KeyValue::new(
                    "utilake.resolution".to_owned(),
                    b.query.resolution.name().to_owned(),
                ),
                KeyValue::new(
                    "utilake.window".to_owned(),
                    serde_json::to_string(&window_text(b)).expect("a window is JSON"),
                ),
                KeyValue::new("utilake.known_at".to_owned(), known_at),
            ];
            let props = WriterProperties::builder()
                .set_compression(Compression::ZSTD(
                    ZstdLevel::try_new(3).expect("3 is a zstd level"),
                ))
                .set_statistics_enabled(EnabledStatistics::Page)
                .set_created_by(format!("utilake {version}"))
                .set_key_value_metadata(Some(kv))
                .build();
            let pq = |e| QueryError::write_parquet(path, e);
            let mut w =
                ArrowWriter::try_new(file, frames.schema.clone(), Some(props)).map_err(pq)?;
            for batch in batches {
                w.write(batch).map_err(pq)?;
            }
            w.close().map_err(pq)?;
        }
        Format::Csv => {
            let mut w = arrow_csv::WriterBuilder::new()
                .with_header(true)
                .build(file);
            for batch in batches {
                w.write(batch).map_err(arrow)?;
            }
        }
        Format::Arrow => {
            let mut w =
                arrow_ipc::writer::FileWriter::try_new(file, &frames.schema).map_err(arrow)?;
            for batch in batches {
                w.write(batch).map_err(arrow)?;
            }
            w.finish().map_err(arrow)?;
        }
    }
    let sidecar_path = path.with_extension(format!("{}.json", format.extension()));
    let report = &frames.report;
    let sidecar = Sidecar {
        table: &b.table,
        schema_id: b.schema_id,
        resolution: b.query.resolution.name(),
        window: window_text(b),
        known_at: match b.query.resolution {
            Resolution::Latest { known_at: Some(k) } => Some(rfc3339(k.get(), 0)),
            _ => None,
        },
        rows,
        rows_read: report.rows_read,
        dropped_window: report.dropped_window,
        dropped_unknown: report.dropped_unknown,
        nan_rows: report.nan_rows,
        sources: &report.sources,
        warnings: report.warnings.iter().map(|d| d.code.as_ref()).collect(),
        utilake: version,
    };
    let text = serde_json::to_string_pretty(&sidecar).expect("a sidecar is JSON");
    std::fs::write(&sidecar_path, text).map_err(|e| QueryError::io(&sidecar_path, e))?;
    Ok(Written {
        path: path.to_path_buf(),
        sidecar: sidecar_path,
        rows,
        report: report.clone(),
    })
}
