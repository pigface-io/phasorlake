//! What a read returns, and how it is written out: resolved frames and
//! raw ones are different types, so a caller cannot pass one off as the
//! other; Parquet, CSV or Arrow, as one file or as parts under a
//! directory, with a JSON sidecar that names the interval, the schema, the
//! resolution, every source and every file.

use std::cell::Cell;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Int32Type, UInt32Type};
use arrow_array::{ArrayRef, DictionaryArray, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use serde::Serialize;
use utilake_core::Diagnostic;
use utilake_mms_registry::file_model::ValidAxis;

use crate::error::QueryError;
use crate::intervals::Intervals;
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
    /// On an interval table, the intervals the window holds and lacks.
    pub intervals: Option<Intervals>,
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
    /// Parquet, with the lake's field ids and `utilake.*` footer keys.
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

    /// The format a file extension names, if it names one.
    pub fn of_extension(ext: &str) -> Option<Self> {
        match ext {
            "parquet" => Some(Self::Parquet),
            "csv" => Some(Self::Csv),
            "arrow" => Some(Self::Arrow),
            _ => None,
        }
    }
}

/// How a Parquet file is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Codec {
    /// zstd at level 3: what the lake writes, read by every current reader.
    #[default]
    Zstd,
    /// Snappy: larger files, read by older tools too.
    Snappy,
    /// No compression.
    None,
}

impl Codec {
    /// The codec's name on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Snappy => "snappy",
            Self::None => "none",
        }
    }

    fn parquet(self) -> Compression {
        match self {
            Self::Zstd => Compression::ZSTD(ZstdLevel::try_new(3).expect("3 is a zstd level")),
            Self::Snappy => Compression::SNAPPY,
            Self::None => Compression::UNCOMPRESSED,
        }
    }
}

/// How a read is written out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// The format (a path with a `.parquet`, `.csv` or `.arrow` extension
    /// names its own).
    pub format: Format,
    /// Compression, for Parquet.
    pub codec: Codec,
    /// Under a directory, a new part begins once a part reaches this many
    /// bytes (a Parquet part may run over by one row group).
    pub target_file_bytes: u64,
    /// Rows per Parquet row group.
    pub row_group_rows: usize,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            format: Format::Parquet,
            codec: Codec::Zstd,
            target_file_bytes: 128 << 20,
            row_group_rows: 262_144,
        }
    }
}

/// What was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// The data files, in order: one, or the parts under the directory.
    pub parts: Vec<PathBuf>,
    /// The JSON sidecar beside them.
    pub sidecar: PathBuf,
    /// Rows written.
    pub rows: u64,
    /// What the read did.
    pub report: Report,
}

/// The output schema: the kept target columns, then `_nl_source`, then,
/// for resolved rows, `_nl_known` and (on a `from` axis) `_nl_valid_to`.
/// A function of the bound query alone, so a writer can open before the
/// first row is read.
pub fn output_schema(b: &Bound) -> SchemaRef {
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
        let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from(b.offset.tz())));
        fields.push(Field::new(KNOWN, ts.clone(), true));
        if b.spec
            .as_ref()
            .is_some_and(|s| matches!(s.valid, ValidAxis::From(_)))
        {
            fields.push(Field::new(VALID_TO, ts, true));
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
    /// The data files, by name, beside the sidecar.
    files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    intervals: Option<&'a Intervals>,
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

/// Where a read's rows land: one file, or parts under a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Layout {
    /// One file, whatever its size.
    File(PathBuf),
    /// `part-00000.{ext}`, `part-00001.{ext}`, … under the directory.
    Dir(PathBuf),
}

/// A writer counting the bytes it passes on.
struct Counting {
    inner: File,
    bytes: Rc<Cell<u64>>,
}

impl Write for Counting {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes.set(self.bytes.get() + n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// One open part.
enum Encoder {
    Parquet(ArrowWriter<File>),
    Csv(arrow_csv::Writer<Counting>, Rc<Cell<u64>>),
    Arrow(arrow_ipc::writer::FileWriter<Counting>, Rc<Cell<u64>>),
}

struct Part {
    /// Where it is written: `{final}.tmp`, renamed when closed.
    tmp: PathBuf,
    /// Where it ends up.
    path: PathBuf,
    encoder: Encoder,
    /// Rows written to it.
    rows: u64,
}

/// Writes a read's frames as they come: one file, or parts of about
/// `target_file_bytes` under a directory, each written whole to a `.tmp`
/// name and renamed, then the sidecar.
pub struct Writer<'a> {
    b: &'a Bound,
    schema: SchemaRef,
    opts: WriteOptions,
    layout: Layout,
    version: &'a str,
    parts: Vec<PathBuf>,
    open: Option<Part>,
    rows: u64,
}

impl<'a> Writer<'a> {
    /// A writer for `b`'s rows in `schema` at `path`: a file when the
    /// path's extension names a format (which is then the format), else a
    /// directory of parts.
    pub fn open(
        b: &'a Bound,
        schema: SchemaRef,
        opts: WriteOptions,
        path: &Path,
        version: &'a str,
    ) -> Result<Self, QueryError> {
        let named = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(Format::of_extension);
        let (layout, format) = match named {
            Some(f) => (Layout::File(path.to_path_buf()), f),
            None => (Layout::Dir(path.to_path_buf()), opts.format),
        };
        let dir = match &layout {
            Layout::File(p) => p.parent().filter(|d| !d.as_os_str().is_empty()),
            Layout::Dir(d) => Some(d.as_path()),
        };
        if let Some(d) = dir {
            std::fs::create_dir_all(d).map_err(|e| QueryError::io(d, e))?;
        }
        Ok(Self {
            b,
            schema,
            opts: WriteOptions { format, ..opts },
            layout,
            version,
            parts: Vec::new(),
            open: None,
            rows: 0,
        })
    }

    /// The path of the next part.
    fn next_path(&self) -> PathBuf {
        match &self.layout {
            Layout::File(p) => p.clone(),
            Layout::Dir(d) => d.join(format!(
                "part-{:05}.{}",
                self.parts.len(),
                self.opts.format.extension()
            )),
        }
    }

    fn footer(&self) -> Vec<KeyValue> {
        let b = self.b;
        let known_at = match b.query.resolution {
            Resolution::Latest { known_at: Some(k) } => rfc3339(k.get(), 0),
            _ => String::new(),
        };
        vec![
            KeyValue::new("utilake.version".to_owned(), self.version.to_owned()),
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
        ]
    }

    fn begin(&mut self) -> Result<&mut Part, QueryError> {
        let path = self.next_path();
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        let file = File::create(&tmp).map_err(|e| QueryError::io(&tmp, e))?;
        let count = Rc::new(Cell::new(0u64));
        let counting = Counting {
            inner: file,
            bytes: count.clone(),
        };
        let encoder = match self.opts.format {
            Format::Parquet => {
                let props = WriterProperties::builder()
                    .set_writer_version(WriterVersion::PARQUET_2_0)
                    .set_compression(self.opts.codec.parquet())
                    .set_max_row_group_row_count(Some(self.opts.row_group_rows))
                    .set_statistics_enabled(EnabledStatistics::Page)
                    .set_created_by(format!("utilake {}", self.version))
                    .set_key_value_metadata(Some(self.footer()))
                    .build();
                Encoder::Parquet(
                    ArrowWriter::try_new(counting.inner, self.schema.clone(), Some(props))
                        .map_err(|e| QueryError::write_parquet(&tmp, e))?,
                )
            }
            Format::Csv => Encoder::Csv(
                arrow_csv::WriterBuilder::new()
                    .with_header(true)
                    .build(counting),
                count,
            ),
            Format::Arrow => Encoder::Arrow(
                arrow_ipc::writer::FileWriter::try_new(counting, &self.schema)
                    .map_err(|e| QueryError::arrow(&tmp.display().to_string(), e))?,
                count,
            ),
        };
        self.open = Some(Part {
            tmp,
            path,
            encoder,
            rows: 0,
        });
        Ok(self.open.as_mut().expect("just opened"))
    }

    /// Writes one frame, beginning a new part when the open one is full.
    pub fn write(&mut self, frame: &Frame) -> Result<(), QueryError> {
        let batch = frame.batch();
        let target = self.opts.target_file_bytes;
        let rolls = matches!(self.layout, Layout::Dir(_));
        if self.open.is_none() {
            self.begin()?;
        }
        let part = self.open.as_mut().expect("opened above");
        let tmp = part.tmp.clone();
        let bytes = match &mut part.encoder {
            Encoder::Parquet(w) => {
                w.write(batch)
                    .map_err(|e| QueryError::write_parquet(&tmp, e))?;
                (w.bytes_written() + w.in_progress_size()) as u64
            }
            Encoder::Csv(w, n) => {
                w.write(batch)
                    .map_err(|e| QueryError::arrow(&tmp.display().to_string(), e))?;
                n.get()
            }
            Encoder::Arrow(w, n) => {
                w.write(batch)
                    .map_err(|e| QueryError::arrow(&tmp.display().to_string(), e))?;
                n.get()
            }
        };
        part.rows += batch.num_rows() as u64;
        let full = part.rows > 0 && bytes >= target;
        self.rows += batch.num_rows() as u64;
        if rolls && full {
            self.end()?;
        }
        Ok(())
    }

    /// Closes the open part and renames it into place.
    fn end(&mut self) -> Result<(), QueryError> {
        let Some(part) = self.open.take() else {
            return Ok(());
        };
        match part.encoder {
            Encoder::Parquet(w) => {
                w.close()
                    .map_err(|e| QueryError::write_parquet(&part.tmp, e))?;
            }
            Encoder::Csv(w, _) => {
                let mut inner = w.into_inner();
                inner.flush().map_err(|e| QueryError::io(&part.tmp, e))?;
            }
            Encoder::Arrow(mut w, _) => {
                w.finish()
                    .map_err(|e| QueryError::arrow(&part.tmp.display().to_string(), e))?;
            }
        }
        std::fs::rename(&part.tmp, &part.path).map_err(|e| QueryError::io(&part.path, e))?;
        self.parts.push(part.path);
        Ok(())
    }

    /// Ends the last part (a read of no rows still leaves one file, with
    /// the schema), writes the sidecar and says what was written.
    pub fn close(mut self, report: Report) -> Result<Written, QueryError> {
        if self.open.is_none() && self.parts.is_empty() {
            self.begin()?;
        }
        self.end()?;
        let b = self.b;
        let sidecar_path = match &self.layout {
            Layout::File(p) => PathBuf::from(format!("{}.json", p.display())),
            Layout::Dir(d) => d.join(format!("{}.json", b.table)),
        };
        let files = self
            .parts
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        let sidecar = Sidecar {
            table: &b.table,
            schema_id: b.schema_id,
            resolution: b.query.resolution.name(),
            window: window_text(b),
            known_at: match b.query.resolution {
                Resolution::Latest { known_at: Some(k) } => Some(rfc3339(k.get(), 0)),
                _ => None,
            },
            rows: self.rows,
            rows_read: report.rows_read,
            dropped_window: report.dropped_window,
            dropped_unknown: report.dropped_unknown,
            nan_rows: report.nan_rows,
            sources: &report.sources,
            warnings: report.warnings.iter().map(|d| d.code.as_ref()).collect(),
            files,
            intervals: report.intervals.as_ref(),
            utilake: self.version,
        };
        let text = serde_json::to_string_pretty(&sidecar).expect("a sidecar is JSON");
        std::fs::write(&sidecar_path, text).map_err(|e| QueryError::io(&sidecar_path, e))?;
        Ok(Written {
            parts: self.parts,
            sidecar: sidecar_path,
            rows: self.rows,
            report,
        })
    }
}
