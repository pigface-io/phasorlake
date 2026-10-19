//! One source file, end to end, on a blocking worker: walk its CSV entries,
//! scan each through the registry into typed batches, write Parquet per
//! partition, quarantine bad rows. Nothing here touches the manifest or the
//! network; the result is handed back for the async commit step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::{ArrowError, Field, Schema, SchemaRef};
use tokio::sync::mpsc;
use utilake_acquire::LocalFile;
use utilake_conform::arrow_sink::{ArrowSink, BatchSink, BlockBatch};
use utilake_core::event::Event;
use utilake_core::{Category, Diagnose, Diagnostic, Fingerprint, IRow, YearMonth};
use utilake_decode::archive::{WalkError, walk_csv_entries};
use utilake_decode::quarantine::QuarantineWriter;
use utilake_decode::scan::{Flow, Pos, ScanOptions, scan_with};
use utilake_decode::{RowError, SinkError};
use utilake_registry::{LoadMode, Registry, ResolveOptions, Resolved, TableId};
use utilake_writer::partition::split_by_month;
use utilake_writer::{
    FileMeta, LakeWriter, ParquetHive, Partition, PartitionKey, Transform, WriterId, WrittenFile,
};

use crate::plan::RunOptions;

/// What one worker is given.
pub struct IngestInput {
    /// The file.
    pub local: LocalFile,
    /// The registry.
    pub registry: Arc<Registry>,
    /// `lake/` root.
    pub lake_root: PathBuf,
    /// `quarantine/` root.
    pub quarantine_root: PathBuf,
    /// Scratch for spooled nested zips.
    pub tmp_dir: PathBuf,
    /// Options.
    pub options: RunOptions,
    /// The request, for diagnostics.
    pub request: String,
    /// Events.
    pub events: Option<mpsc::Sender<Event>>,
    /// The file is in the lake already, written by rules that have changed
    /// since; it is being read again.
    pub rules_changed: bool,
}

/// One shape (table, version, column list) within one entry.
#[derive(Debug, Clone)]
pub struct IngestResult {
    /// Table.
    pub table: String,
    /// Table handle.
    pub table_id: TableId,
    /// Version the plan used.
    pub version: u16,
    /// Observed I-row fingerprint.
    pub fingerprint: Fingerprint,
    /// The shape that was read and the rules it was read with.
    pub transform: Transform,
    /// Rows written.
    pub rows: u64,
    /// Rows quarantined.
    pub bad_rows: u64,
    /// Files written.
    pub outputs: Vec<WrittenFile>,
    /// Quarantine file.
    pub quarantine_path: Option<String>,
}

/// One CSV entry of a source.
#[derive(Debug)]
pub struct EntryReport {
    /// Entry name, nested joined with `!`.
    pub entry: String,
    /// Per-table results.
    pub results: Vec<IngestResult>,
    /// Warnings raised.
    pub warnings: Vec<Diagnostic>,
    /// What was wrong with each row that could not be read, the first
    /// [`ROW_ERRORS_KEPT`] of them: errors when the entry was refused for
    /// them, warnings when it was accepted without those rows.
    pub row_errors: Vec<Diagnostic>,
    /// The error that failed the entry, if any.
    pub error: Option<Diagnostic>,
}

/// One source file.
#[derive(Debug)]
pub struct FileReport {
    /// The file.
    pub local: LocalFile,
    /// Entries in archive order.
    pub entries: Vec<EntryReport>,
    /// An error that prevented reading the file at all.
    pub file_error: Option<Diagnostic>,
    /// The file was read again because the rules it had been written by
    /// have changed.
    pub rules_changed: bool,
}

/// What tells one open writer from another: table, version, column list
/// and partition.
type WriterKey = (TableId, u16, u128, Partition);

/// Parquet writers all the workers of a run may hold open together. An
/// operating system gives a process a few hundred open files by default
/// (256 on macOS), and downloads and the manifest need some.
const OPEN_WRITERS_PER_RUN: usize = 128;

/// How many writers one worker may hold open at once, given how many
/// workers run: the run's share, and never fewer than eight.
pub fn open_writers(jobs: usize) -> usize {
    (OPEN_WRITERS_PER_RUN / jobs.max(1)).max(8)
}

/// How many row errors of one entry are reported one by one. Every bad
/// row is in the quarantine file; this bounds what a run prints.
pub const ROW_ERRORS_KEPT: usize = 20;

/// A writer that is open: its id, when it was last written to (in
/// batches), and the schema of its file.
struct OpenWriter {
    id: WriterId,
    used: u64,
    schema: SchemaRef,
}

/// The schema a file must have to take `batch` as well as what it took so
/// far, when `file` is not it: `file`'s columns, nullable where either is,
/// then the columns only `batch` has (a column of values kept as text
/// appears in the batches that need it). `None` when `file` takes `batch`.
fn widened(file: &Schema, batch: &Schema) -> Option<Schema> {
    let mut fields: Vec<Field> = file.fields().iter().map(|f| f.as_ref().clone()).collect();
    let mut wider = false;
    for b in batch.fields() {
        match fields.iter_mut().find(|f| f.name() == b.name()) {
            Some(f) if b.is_nullable() && !f.is_nullable() => {
                *f = f.clone().with_nullable(true);
                wider = true;
            }
            Some(_) => {}
            None => {
                fields.push(b.as_ref().clone());
                wider = true;
            }
        }
    }
    wider.then(|| Schema::new_with_metadata(fields, file.metadata().clone()))
}

/// `batch` in `schema`, which has every column of `batch` and maybe more:
/// those are null.
fn fitted(batch: &RecordBatch, schema: &SchemaRef) -> Result<RecordBatch, ArrowError> {
    if batch.schema().fields() == schema.fields() {
        return Ok(batch.clone());
    }
    let columns = schema
        .fields()
        .iter()
        .map(|f| match batch.schema().index_of(f.name()) {
            Ok(i) => batch.column(i).clone(),
            Err(_) => arrow_array::new_null_array(f.data_type(), batch.num_rows()),
        })
        .collect();
    RecordBatch::try_new(schema.clone(), columns)
}

/// Routes batches to Parquet writers and quarantines bad rows.
struct LakeSink<'a> {
    registry: &'a Registry,
    writer: ParquetHive,
    /// Open writers by table, version, shape and partition. The shape is
    /// part of the key because two column lists of one version may differ
    /// in the columns they are written with.
    writers: HashMap<WriterKey, OpenWriter>,
    /// How many files each key has had opened, so a further file of a
    /// partition gets a name of its own.
    parts: HashMap<WriterKey, u32>,
    /// The schema a key's next file starts with, when its last file was
    /// finished for being too narrow.
    next_schema: HashMap<WriterKey, SchemaRef>,
    /// At most this many writers are open at once; see [`open_writers`].
    max_open: usize,
    /// Counts batches, to tell which writer was used longest ago.
    tick: u64,
    results: Vec<IngestResult>,
    current: Option<usize>,
    quarantine: HashMap<String, QuarantineWriter>,
    quarantine_root: &'a Path,
    local: &'a LocalFile,
    entry: &'a str,
    period: Option<YearMonth>,
    warnings: Vec<Diagnostic>,
    row_errors: Vec<Diagnostic>,
    fail_fast: bool,
    events: Option<&'a mpsc::Sender<Event>>,
}

impl LakeSink<'_> {
    fn meta(
        &self,
        table: &str,
        version: u16,
        fingerprint: Fingerprint,
        time_column: Option<usize>,
        transform: Transform,
        part: u32,
    ) -> FileMeta {
        FileMeta {
            transform,
            part,
            table: table.to_owned(),
            source_stem: entry_stem(self.entry, &self.local.remote.stem()),
            schema_version: version,
            fingerprint,
            source_url: self.local.remote.url.to_string(),
            source_sha256: self.local.sha256.clone(),
            entry: self.entry.to_owned(),
            time_column,
        }
    }

    /// Finishes one writer and files what it wrote under its result.
    fn close_one(
        &mut self,
        key: WriterKey,
        wid: WriterId,
    ) -> Result<(), utilake_writer::StoreError> {
        let written = self.writer.close(wid)?;
        let (table_id, version, shape, _) = key;
        if let Some(r) = self
            .results
            .iter_mut()
            .find(|r| r.table_id == table_id && r.version == version && r.fingerprint.0 == shape)
        {
            r.outputs.push(written);
        }
        Ok(())
    }

    /// Finishes the writer that was written to longest ago, to make room
    /// for another. Its file is complete; rows that come for its partition
    /// later go to a further file.
    fn close_least_recent(&mut self) -> Result<(), utilake_writer::StoreError> {
        let Some((key, wid)) = self
            .writers
            .iter()
            .min_by_key(|(k, w)| (w.used, **k))
            .map(|(k, w)| (*k, w.id))
        else {
            return Ok(());
        };
        self.writers.remove(&key);
        self.close_one(key, wid)
    }

    fn close_all(&mut self) -> Result<(), utilake_writer::StoreError> {
        let mut keys: Vec<_> = self.writers.drain().collect();
        keys.sort_by_key(|(k, _)| *k);
        for (key, w) in keys {
            self.close_one(key, w.id)?;
        }
        Ok(())
    }

    /// Discards everything the entry wrote: the writers still open, and
    /// the files of writers closed early to make room. A failed entry
    /// leaves nothing in the lake.
    fn abort_all(&mut self) {
        let ids: Vec<WriterId> = self.writers.drain().map(|(_, w)| w.id).collect();
        for w in ids {
            self.writer.abort(w);
        }
        for r in &mut self.results {
            for o in r.outputs.drain(..) {
                let _ = std::fs::remove_file(&o.path);
            }
        }
    }

    fn finish_quarantine(&mut self) {
        let qs: Vec<(String, QuarantineWriter)> = self.quarantine.drain().collect();
        for (table, q) in qs {
            if let Ok(Some(p)) = q.finish()
                && let Some(r) = self.results.iter_mut().find(|r| r.table == table)
            {
                r.quarantine_path = Some(p.display().to_string());
            }
        }
    }
}

impl BatchSink for LakeSink<'_> {
    fn block_started(&mut self, irow: &IRow<'_>, resolved: &Resolved, _pos: Pos) {
        let table = self.registry.table(resolved.table);
        let fingerprint = irow.fingerprint();
        // Two column lists of one increment are two shapes, read by two
        // plans: each is a result of its own.
        let idx = match self.results.iter().position(|r| {
            r.table_id == resolved.table
                && r.version == resolved.version
                && r.fingerprint == fingerprint
        }) {
            Some(i) => i,
            None => {
                self.results.push(IngestResult {
                    table: table.name.clone(),
                    table_id: resolved.table,
                    version: resolved.version,
                    fingerprint,
                    transform: Transform {
                        report_type: irow.report_type.to_owned(),
                        report_subtype: irow.report_subtype.to_owned(),
                        source_version: irow.version,
                        source_columns: irow.columns.iter().map(|c| (*c).to_owned()).collect(),
                        source_period: self.period.map(|p| p.to_string()),
                        plan_hash: resolved.plan_hash().to_string(),
                        schema_source: resolved.source.label().to_owned(),
                        resolution: resolved.label().to_owned(),
                        shape_id: shape_id(&self.registry.shape_ids(irow, self.period)),
                    },
                    rows: 0,
                    bad_rows: 0,
                    outputs: vec![],
                    quarantine_path: None,
                });
                self.results.len() - 1
            }
        };
        self.current = Some(idx);
        if let Some(tx) = self.events {
            let _ = tx.try_send(Event::BlockResolved {
                source: format!("{}!{}", self.local.remote.url, self.entry),
                table: table.name.clone(),
                version: resolved.version,
                status: resolved.label().to_owned(),
            });
        }
    }

    fn batch(&mut self, b: BlockBatch) -> Result<(), SinkError> {
        let table = self.registry.table(b.table);
        let market = self.registry.market_of(b.table);
        let Some(idx) = self.current else {
            return Ok(());
        };
        let rows = b.batch.num_rows() as u64;
        let time_col = b.plan.time_column.map(usize::from);
        let parts: Vec<(Partition, arrow_array::RecordBatch)> = match (b.plan.load_mode, time_col) {
            (LoadMode::Snapshot, _) => vec![(
                self.period
                    .map_or(Partition::Unknown, |p| Partition::Snapshot {
                        year: p.year(),
                        month: p.month(),
                    }),
                b.batch,
            )],
            (LoadMode::Append, Some(tc)) => split_by_month(&b.batch, tc, market.utc_offset_secs)
                .map_err(|e| SinkError::write(table.name.clone(), &e))?,
            // No time of its own: the month of the file is all there is.
            (LoadMode::Append, None) => vec![(
                self.period
                    .map_or(Partition::Unknown, |p| Partition::Month {
                        year: p.year(),
                        month: p.month(),
                    }),
                b.batch,
            )],
        };
        for (partition, batch) in parts {
            let key: WriterKey = (b.table, b.version, b.fingerprint.0, partition);
            self.tick += 1;
            let fail =
                |e: &(dyn std::error::Error + 'static)| SinkError::write(table.name.clone(), e);
            // A batch with a column, or a null, its file has no room for
            // finishes that file; the partition goes on in a further one
            // whose schema takes both.
            let open = match self.writers.get_mut(&key) {
                Some(w) => match widened(&w.schema, &batch.schema()) {
                    None => {
                        w.used = self.tick;
                        Some((w.id, w.schema.clone()))
                    }
                    Some(wider) => {
                        let id = w.id;
                        self.writers.remove(&key);
                        self.close_one(key, id).map_err(|e| fail(&e))?;
                        self.next_schema.insert(key, Arc::new(wider));
                        None
                    }
                },
                None => None,
            };
            let (wid, schema) = match open {
                Some(o) => o,
                None => {
                    // Every open writer holds a file and a row group in
                    // memory. A source whose rows wander over hundreds of
                    // months would hold hundreds of both.
                    if self.writers.len() >= self.max_open {
                        self.close_least_recent().map_err(|e| fail(&e))?;
                    }
                    let part = {
                        let n = self.parts.entry(key).or_insert(0);
                        *n += 1;
                        *n
                    };
                    let pk = PartitionKey {
                        market: table.market.to_string(),
                        table_dir: self.registry.table_dir(b.table),
                        partition,
                    };
                    let meta = self.meta(
                        &table.name,
                        b.version,
                        b.fingerprint,
                        time_col,
                        self.results[idx].transform.clone(),
                        part,
                    );
                    // A further file of a key starts as wide as the one
                    // before it ended.
                    let schema = match self.next_schema.remove(&key) {
                        Some(s) => widened(&s, &batch.schema()).map_or(s, Arc::new),
                        None => batch.schema(),
                    };
                    let id = self
                        .writer
                        .open(&pk, schema.clone(), &meta)
                        .map_err(|e| fail(&e))?;
                    self.writers.insert(
                        key,
                        OpenWriter {
                            id,
                            used: self.tick,
                            schema: schema.clone(),
                        },
                    );
                    (id, schema)
                }
            };
            let batch = fitted(&batch, &schema).map_err(|e| fail(&e))?;
            self.writer.write(wid, &batch).map_err(|e| fail(&e))?;
        }
        self.results[idx].rows += rows;
        Ok(())
    }

    fn warning(&mut self, mut d: Diagnostic) {
        if d.table.is_none()
            && let Some(idx) = self.current
        {
            d.table = Some(self.results[idx].table.clone());
        }
        self.warnings.push(d);
    }

    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> Flow {
        let (table, version) = match self.current {
            Some(i) => (self.results[i].table.clone(), Some(self.results[i].version)),
            None => ("unknown".to_owned(), None),
        };
        let mut d = err
            .diagnostic()
            .at(pos.line, pos.byte_offset)
            .with_sample(raw)
            .with_table(table.clone());
        d.version = version;
        if let Some(i) = self.current {
            self.results[i].bad_rows += 1;
        }
        if self.row_errors.len() < ROW_ERRORS_KEPT {
            self.row_errors.push(d.clone());
        }
        let stem = self.local.remote.stem();
        let q = self
            .quarantine
            .entry(table.clone())
            .or_insert_with(|| QuarantineWriter::new(self.quarantine_root, &table, &stem));
        if let Err(e) = q.write(raw, &d) {
            self.warnings.push(
                Diagnostic::warning(
                    "store.quarantine_write",
                    Category::Store,
                    format!(
                        "The quarantine file {} could not be written.",
                        q.path().display()
                    ),
                )
                .caused_by(&e),
            );
        }
        if self.fail_fast {
            Flow::Stop
        } else {
            Flow::Continue
        }
    }
}

/// The census ids of a shape as recorded with an ingest: comma-separated,
/// `None` when the census has not seen it.
fn shape_id(ids: &[utilake_core::ShapeId]) -> Option<String> {
    (!ids.is_empty()).then(|| {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    })
}

/// The stem that names the Parquet files written from one entry of a
/// source: the entry's own file name without its extension, lower-case and
/// made safe for a path, so two CSVs of one zip never write the same file.
/// A source that is itself the entry gives the source's stem.
fn entry_stem(entry: &str, source_stem: &str) -> String {
    let name = entry.rsplit(['!', '/']).next().unwrap_or(entry);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let stem: String = stem
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() {
        source_stem.to_owned()
    } else {
        stem
    }
}

/// The archive month in the name of an archive file, in either naming
/// scheme (see [`utilake_core::archive_name`]).
pub fn period_from_stem(stem: &str) -> Option<YearMonth> {
    utilake_core::archive_name::parse(stem).map(|n| n.month)
}

fn ingest_entry(
    input: &IngestInput,
    entry: &str,
    reader: &mut dyn std::io::Read,
    scan_buf: &mut Vec<u8>,
) -> EntryReport {
    let period = input
        .local
        .remote
        .period
        .or_else(|| period_from_stem(&input.local.remote.stem().to_ascii_uppercase()));
    let initial = LakeSink {
        registry: &input.registry,
        writer: ParquetHive::new(&input.lake_root),
        writers: HashMap::new(),
        parts: HashMap::new(),
        next_schema: HashMap::new(),
        max_open: open_writers(input.options.jobs),
        tick: 0,
        results: Vec::new(),
        current: None,
        quarantine: HashMap::new(),
        quarantine_root: &input.quarantine_root,
        local: &input.local,
        entry,
        period,
        warnings: Vec::new(),
        row_errors: Vec::new(),
        fail_fast: input.options.fail_fast,
        events: input.events.as_ref(),
    };
    let resolve = ResolveOptions {
        mode: input.options.mode,
    };
    let mut arrow = ArrowSink::new(&input.registry, resolve, input.options.batch_rows, initial)
        .with_period(period);
    let mut sink: LakeSink<'_>;
    let scan_opts = ScanOptions {
        max_bad_rows: input.options.max_bad_rows,
        ..Default::default()
    };
    let ctx = utilake_core::diag::Context {
        request: Some(input.request.clone()),
        url: Some(input.local.remote.url.to_string()),
        file: Some(input.local.path.display().to_string()),
        entry: Some(entry.to_owned()),
        table: None,
        version: None,
    };
    let scanned = scan_with(reader, &mut arrow, &scan_opts, scan_buf);
    let (recovered, flushed) = arrow.finish();
    sink = recovered;
    let scanned = match (scanned, flushed) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Ok(_), Err(e)) => Err(e.diagnostic()),
        (Err(e), _) => Err(e.diagnostic()),
    };
    let mut report = EntryReport {
        entry: entry.to_owned(),
        results: vec![],
        warnings: vec![],
        row_errors: vec![],
        error: None,
    };
    match scanned {
        Ok(stats) => {
            if let Err(e) = sink.close_all() {
                sink.abort_all();
                let mut d = e.diagnostic();
                d.fill_from(&ctx);
                report.error = Some(d);
            } else if stats.footer_mismatch() {
                let d = Diagnostic::new(
                    "csv.footer_count_mismatch",
                    Category::Csv,
                    if input.options.strict_footer {
                        utilake_core::Severity::Error
                    } else {
                        utilake_core::Severity::Warning
                    },
                    format!(
                        "The footer says {} lines but {} were read.",
                        stats.footer_declared.unwrap_or(0),
                        stats.lines
                    ),
                )
                .expected_observed(
                    stats.footer_declared.unwrap_or(0).to_string(),
                    stats.lines.to_string(),
                );
                if input.options.strict_footer {
                    report.error = Some(d);
                } else {
                    sink.warnings.push(d);
                }
            }
        }
        Err(mut d) => {
            sink.abort_all();
            d.fill_from(&ctx);
            report.error = Some(d);
        }
    }
    sink.finish_quarantine();
    for w in &mut sink.warnings {
        w.fill_from(&ctx);
    }
    if let Some(e) = &mut report.error {
        e.fill_from(&ctx);
    }
    report.row_errors = std::mem::take(&mut sink.row_errors);
    for d in &mut report.row_errors {
        d.fill_from(&ctx);
        // The entry went into the lake without these rows: they are in
        // the quarantine file, and the run goes on.
        if report.error.is_none() {
            d.severity = utilake_core::Severity::Warning;
        }
    }
    report.warnings = std::mem::take(&mut sink.warnings);
    report.results = std::mem::take(&mut sink.results);
    report
}

/// Ingests one local file: a zip (entries walked, nested zips included) or
/// a bare CSV.
pub fn ingest_file(input: IngestInput) -> FileReport {
    // One read buffer for every entry of the file.
    let mut scan_buf = Vec::new();
    let path = input.local.path.clone();
    let is_csv = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    let mut entries = Vec::new();
    let mut file_error = None;
    if is_csv {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::File::open(&path) {
            Ok(mut f) => entries.push(ingest_entry(&input, &name, &mut f, &mut scan_buf)),
            Err(e) => {
                file_error = Some(
                    Diagnostic::error(
                        "csv.read_failed",
                        Category::Csv,
                        format!("The file {} could not be opened.", path.display()),
                    )
                    .with_file(path.display().to_string())
                    .caused_by(&e),
                );
            }
        }
    } else {
        let fail_fast = input.options.fail_fast;
        let result: Result<usize, WalkError<()>> = walk_csv_entries(
            &path,
            &input.tmp_dir,
            &mut |e| {
                let rep = ingest_entry(&input, &e.name, e.reader, &mut scan_buf);
                let stop = fail_fast && rep.error.is_some();
                entries.push(rep);
                if stop { Err(()) } else { Ok(()) }
            },
            &mut |_| {},
        );
        if let Err(WalkError::Archive(e)) = result {
            let mut d = e.diagnostic();
            d.request = Some(input.request.clone());
            d.url = Some(input.local.remote.url.to_string());
            d.file = Some(path.display().to_string());
            file_error = Some(d);
        }
    }
    FileReport {
        rules_changed: input.rules_changed,
        local: input.local,
        entries,
        file_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods() {
        assert_eq!(
            period_from_stem("PUBLIC_DVD_REGION_202401010000"),
            YearMonth::new(2024, 1)
        );
        assert_eq!(period_from_stem("REGION_v1_head"), None);
        assert_eq!(
            period_from_stem("PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202608010000"),
            YearMonth::new(2026, 8)
        );
        assert_eq!(period_from_stem("PUBLIC_DVD_REGION_2024010100001"), None);
    }
}
