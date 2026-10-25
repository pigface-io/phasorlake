//! One source file, end to end, on a blocking worker: walk the entries its
//! kit's decoders read, decode each into typed batches, write Parquet per
//! partition, quarantine bad rows. Nothing here touches the manifest or the
//! network; the result is handed back for the async commit step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::entry_times::EntryTimes;
use arrow_array::RecordBatch;
use arrow_schema::{ArrowError, Field, Schema, SchemaRef};
use tokio::sync::mpsc;
use utilake_acquire::LocalFile;
use utilake_core::event::Event;
use utilake_core::{Category, Diagnose, Diagnostic, Fingerprint, YearMonth};
use utilake_kit::container::{
    Accepts, ArchiveError, WalkError, Walked, has_extension, walk_entries,
};
use utilake_kit::quarantine::QuarantineWriter;
use utilake_kit::{
    Block, BlockSink, DecodeCtx, DecodeOptions, Flow, KitRegistry, LoadMode, RowAt, SourceKit,
    TableRef,
};
use utilake_writer::manifest::EntryShare;
use utilake_writer::partition::split_by_month;
use utilake_writer::provenance::Stamp;
use utilake_writer::{
    FileMeta, LakeWriter, ParquetHive, Partition, PartitionKey, Transform, WriterId, WrittenFile,
};

use crate::plan::RunOptions;

/// What one worker is given.
pub struct IngestInput {
    /// The file.
    pub local: LocalFile,
    /// Every kit.
    pub kits: Arc<KitRegistry>,
    /// The kit that reads this file.
    pub kit: &'static str,
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
    /// Entries of the file (nested paths joined with `!`) that are in the
    /// lake already from a fetch of each alone, left as they are.
    pub skip: Vec<String>,
}

/// One shape (table, version, column list) within one entry.
#[derive(Debug, Clone)]
pub struct IngestResult {
    /// Table.
    pub table: String,
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
    /// What each entry gave, in the order the entries were read.
    pub entries: Vec<EntryShare>,
    /// Quarantine file.
    pub quarantine_path: Option<String>,
}

/// One CSV entry of a source.
#[derive(Debug)]
pub struct EntryReport {
    /// Entry name, nested joined with `!`.
    pub entry: String,
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
    /// Entries in archive order, with what was said of each.
    pub entries: Vec<EntryReport>,
    /// What the file read, per shape, and the Parquet files it put in the
    /// lake: none when an entry failed, since a file goes into the lake
    /// whole or not at all.
    pub results: Vec<IngestResult>,
    /// An error that prevented reading the file at all.
    pub file_error: Option<Diagnostic>,
    /// The file was read again because the rules it had been written by
    /// have changed.
    pub rules_changed: bool,
    /// Entries left to an earlier read of each alone.
    pub left: Vec<String>,
}

/// What tells one open writer from another: table, version, column list
/// and partition.
type WriterKey = (String, u16, u128, Partition);

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
    /// The columns its file takes from batches.
    schema: SchemaRef,
    /// Those, then each row's source and publish time: the file's schema.
    lake: SchemaRef,
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
    zone!(FIT);
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
    /// The source file's writer: every entry's rows go to the same files,
    /// each row naming its entry, and the files are committed together
    /// once the whole file is read.
    writer: ParquetHive,
    /// A file is closed for the next once it holds this many bytes.
    file_bytes: u64,
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
    /// The entry being read.
    entry: String,
    /// Its source's stamp for each market offset its tables were read in.
    stamps: Vec<(i32, Stamp)>,
    /// When each entry first went public, from the records beside the file.
    entry_times: EntryTimes,
    period: Option<YearMonth>,
    /// The day the file covers, when the source cuts its files by day: a
    /// snapshot it carries is the day's.
    day: Option<utilake_core::CivilDay>,
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
            source_stem: self.local.remote.stem(),
            schema_version: version,
            fingerprint,
            source_url: self.local.remote.url.to_string(),
            source_sha256: self.local.sha256.clone(),
            time_column,
        }
    }

    /// The current entry's source, `url!entry` (the URL alone for a file
    /// read whole), and its publish time in the market offset given.
    fn stamp(&mut self, offset_secs: i32) -> &Stamp {
        if let Some(i) = self.stamps.iter().position(|(o, _)| *o == offset_secs) {
            return &self.stamps[i].1;
        }
        let local = self.local;
        let url = local.remote.url.as_str();
        let name = if self.entry.is_empty() {
            url.to_owned()
        } else {
            format!("{url}!{}", self.entry)
        };
        // The entry's own first publication where the mirror holds it; the
        // file's otherwise, which is never earlier than the truth.
        let published = self.entry_times.published(&self.entry).or_else(|| {
            let modified = local.last_modified_at.map(|s| s.rfc3339());
            utilake_core::published::published(url, modified.as_deref(), offset_secs)
        });
        self.stamps
            .push((offset_secs, Stamp::new(&name, published)));
        &self.stamps[self.stamps.len() - 1].1
    }

    /// The current entry's share of result `idx`.
    fn share(&mut self, idx: usize) -> &mut EntryShare {
        let entries = &mut self.results[idx].entries;
        let at = match entries.iter().rposition(|e| e.entry == self.entry) {
            Some(i) => i,
            None => {
                entries.push(EntryShare {
                    entry: self.entry.clone(),
                    ..EntryShare::default()
                });
                entries.len() - 1
            }
        };
        &mut entries[at]
    }

    /// Begins entry `entry`: what is said from here on is said of it.
    fn begin_entry(&mut self, entry: &str) {
        entry.clone_into(&mut self.entry);
        self.stamps.clear();
        self.current = None;
    }

    /// Finishes one writer and files what it wrote under its result.
    fn close_one(
        &mut self,
        key: WriterKey,
        wid: WriterId,
    ) -> Result<(), utilake_writer::StoreError> {
        let written = self.writer.close(wid)?;
        let (table, version, shape, _) = key;
        if let Some(r) = self
            .results
            .iter_mut()
            .find(|r| r.table == table && r.version == version && r.fingerprint.0 == shape)
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
            .min_by_key(|(k, w)| (w.used, (*k).clone()))
            .map(|(k, w)| (k.clone(), w.id))
        else {
            return Ok(());
        };
        self.writers.remove(&key);
        self.close_one(key, wid)
    }

    fn close_all(&mut self) -> Result<(), utilake_writer::StoreError> {
        zone!(CLOSE);
        let mut keys: Vec<_> = self.writers.drain().collect();
        keys.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (key, w) in keys {
            self.close_one(key, w.id)?;
        }
        Ok(())
    }

    /// Discards every Parquet file the file wrote: the writers still
    /// open, and the files closed and not yet committed. A file with a
    /// failed entry leaves nothing in the lake; what it quarantined stays,
    /// to show why.
    fn abort_all(&mut self) {
        let ids: Vec<WriterId> = self.writers.drain().map(|(_, w)| w.id).collect();
        for w in ids {
            self.writer.abort(w);
        }
        self.writer.discard(0);
        for r in &mut self.results {
            r.outputs.clear();
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

impl BlockSink for LakeSink<'_> {
    fn block_started(
        &mut self,
        table: &Arc<TableRef>,
        version: u16,
        fingerprint: Fingerprint,
        transform: &Arc<Transform>,
        status: &str,
    ) {
        // Two column lists read against one schema are two shapes, read by two
        // plans: each is a result of its own.
        let idx = match self.results.iter().position(|r| {
            r.table == table.name && r.version == version && r.fingerprint == fingerprint
        }) {
            Some(i) => i,
            None => {
                self.results.push(IngestResult {
                    table: table.name.clone(),
                    version,
                    fingerprint,
                    transform: (**transform).clone(),
                    rows: 0,
                    bad_rows: 0,
                    outputs: vec![],
                    entries: vec![],
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
                version,
                status: status.to_owned(),
            });
        }
    }

    fn batch(&mut self, b: Block) -> Result<(), Box<Diagnostic>> {
        zone!(BATCH);
        let table = b.table.clone();
        let Some(idx) = self.current else {
            return Ok(());
        };
        let rows = b.batch.num_rows() as u64;
        let fail = |e: &(dyn std::error::Error + 'static)| {
            Box::new(
                Diagnostic::error(
                    "store.write_failed",
                    Category::Store,
                    format!("Writing a batch of table {} failed: {e}", table.name),
                )
                .with_table(table.name.clone())
                .caused_by(e),
            )
        };
        let parts: Vec<(Partition, arrow_array::RecordBatch)> =
            match (table.load_mode, table.time_column) {
                (LoadMode::Snapshot, _) => vec![(
                    match (self.day, self.period) {
                        (Some(d), _) => Partition::SnapshotDay {
                            year: d.month().year(),
                            month: d.month().month(),
                            day: d.day(),
                        },
                        (None, Some(p)) => Partition::Snapshot {
                            year: p.year(),
                            month: p.month(),
                        },
                        (None, None) => Partition::Unknown,
                    },
                    b.batch,
                )],
                (LoadMode::Append, Some(tc)) => {
                    split_by_month(&b.batch, tc, table.utc_offset_secs).map_err(|e| fail(&e))?
                }
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
            let key: WriterKey = (table.name.clone(), b.version, b.fingerprint.0, partition);
            self.tick += 1;
            // A batch with a column, or a null, its file has no room for
            // finishes that file; the partition goes on in a further one
            // whose schema takes both.
            let open = match self.writers.get_mut(&key) {
                Some(w) => match widened(&w.schema, &batch.schema()) {
                    None => {
                        w.used = self.tick;
                        Some((w.id, w.schema.clone(), w.lake.clone()))
                    }
                    Some(wider) => {
                        let id = w.id;
                        self.writers.remove(&key);
                        self.close_one(key.clone(), id).map_err(|e| fail(&e))?;
                        self.next_schema.insert(key.clone(), Arc::new(wider));
                        None
                    }
                },
                None => None,
            };
            let (wid, schema, lake) = match open {
                Some(o) => o,
                None => {
                    // Every open writer holds a file and a row group in
                    // memory. A source whose rows wander over hundreds of
                    // months would hold hundreds of both.
                    if self.writers.len() >= self.max_open {
                        self.close_least_recent().map_err(|e| fail(&e))?;
                    }
                    let part = {
                        let n = self.parts.entry(key.clone()).or_insert(0);
                        *n += 1;
                        *n
                    };
                    let pk = PartitionKey {
                        market: table.market.clone(),
                        table_dir: table.dir.clone(),
                        partition,
                    };
                    let meta = self.meta(
                        &table.name,
                        b.version,
                        b.fingerprint,
                        table.time_column,
                        (*b.transform).clone(),
                        part,
                    );
                    // A further file of a key starts as wide as the one
                    // before it ended.
                    let schema = match self.next_schema.remove(&key) {
                        Some(s) => widened(&s, &batch.schema()).map_or(s, Arc::new),
                        None => batch.schema(),
                    };
                    let lake = utilake_writer::provenance::lake_schema(&schema);
                    let id = self
                        .writer
                        .open(&pk, lake.clone(), &meta)
                        .map_err(|e| fail(&e))?;
                    self.writers.insert(
                        key.clone(),
                        OpenWriter {
                            id,
                            used: self.tick,
                            schema: schema.clone(),
                            lake: lake.clone(),
                        },
                    );
                    (id, schema, lake)
                }
            };
            let batch = fitted(&batch, &schema).map_err(|e| fail(&e))?;
            let bounds = table
                .time_column
                .map(|tc| utilake_writer::partition::time_bounds(&batch, tc));
            let stamp = self.stamp(table.utc_offset_secs).clone();
            let batch =
                utilake_writer::provenance::stamped(&batch, &stamp, &lake).map_err(|e| fail(&e))?;
            self.writer.write(wid, &batch).map_err(|e| fail(&e))?;
            let share = self.share(idx);
            share.published = stamp.published();
            share.add(batch.num_rows() as u64, bounds);
            // A file that has grown to its size is closed; the partition
            // goes on in a further one.
            if self.writer.bytes_in_file(wid) >= self.file_bytes {
                self.writers.remove(&key);
                self.close_one(key, wid).map_err(|e| fail(&e))?;
            }
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

    fn bad_row(&mut self, raw: &[u8], _at: RowAt, mut d: Diagnostic) -> Flow {
        let (table, version) = match self.current {
            Some(i) => (self.results[i].table.clone(), Some(self.results[i].version)),
            None => ("unknown".to_owned(), None),
        };
        d.table = Some(table.clone());
        d.version = version;
        if let Some(i) = self.current {
            self.results[i].bad_rows += 1;
            self.share(i).bad_rows += 1;
        }
        if self.row_errors.len() < ROW_ERRORS_KEPT {
            self.row_errors.push(d.clone());
        }
        let stem = self.local.remote.stem();
        let q = self
            .quarantine
            .entry(table.clone())
            .or_insert_with(|| QuarantineWriter::new(self.quarantine_root, &table, &stem));
        let source = format!("{}!{}", self.local.remote.url, self.entry);
        if let Err(e) = q.write(raw, &source, &d) {
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

/// Reads one entry through the kit's decoder into the file's sink.
fn ingest_entry(
    input: &IngestInput,
    kit: &SourceKit,
    entry: &str,
    reader: &mut dyn std::io::Read,
    scan_buf: &mut Vec<u8>,
    sink: &mut LakeSink<'_>,
) -> EntryReport {
    zone!(ENTRY);
    let ctx = utilake_core::diag::Context {
        request: Some(input.request.clone()),
        url: Some(input.local.remote.url.to_string()),
        file: Some(input.local.path.display().to_string()),
        entry: Some(entry.to_owned()),
        table: None,
        version: None,
    };
    let mut report = EntryReport {
        entry: entry.to_owned(),
        warnings: vec![],
        row_errors: vec![],
        error: None,
    };
    let Some(decoder) = kit.decoder_for(entry) else {
        let mut d = Diagnostic::error(
            "config.no_decoder",
            Category::Config,
            format!(
                "The entry {entry} is not one the kit {} reads; no decoder accepts its name.",
                kit.id
            ),
        );
        d.fill_from(&ctx);
        report.error = Some(d);
        return report;
    };
    sink.begin_entry(entry);
    let options = DecodeOptions {
        strictness: input.options.mode,
        max_bad_rows: input.options.max_bad_rows,
        batch_rows: input.options.batch_rows,
    };
    let dctx = DecodeCtx {
        file: &input.local,
        entry,
        period: input.local.remote.period,
        options: &options,
        previous: input.local.previous.as_deref(),
    };
    let decoded = {
        zone!(DECODE);
        decoder.decode(reader, &dctx, sink, scan_buf)
    };
    match decoded {
        Ok(stats) if stats.footer_mismatch() => {
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
        Ok(_) => {}
        Err(d) => report.error = Some(*d),
    }
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
    report
}

/// Ingests one local file: a file a decoder of the kit reads whole, or a
/// zip whose entries (nested zips included) are walked for the ones a
/// decoder reads.
pub fn ingest_file(input: IngestInput) -> FileReport {
    zone!(FILE);
    let path = input.local.path.clone();
    // The file's name is its URL's: a version the mirror kept beside the
    // first (`{name}@{stamp}`) is read as the file it is a version of.
    let name = input.local.remote.file_name();
    let mut entries = Vec::new();
    let mut file_error = None;
    let Some(kit) = input.kits.kit(input.kit) else {
        return FileReport {
            file_error: Some(
                Diagnostic::error(
                    "config.no_decoder",
                    Category::Config,
                    format!("No kit named {} was assembled into this binary.", input.kit),
                )
                .with_file(path.display().to_string())
                .with_request(input.request.clone()),
            ),
            rules_changed: input.rules_changed,
            left: input.skip,
            local: input.local,
            entries,
            results: Vec::new(),
        };
    };
    // One read buffer and one sink for every entry of the file.
    let mut scan_buf = Vec::new();
    let mut sink = LakeSink {
        writer: ParquetHive::new(&input.lake_root).with_layout(input.options.layout),
        file_bytes: u64::from(input.options.layout.file_size_mb) << 20,
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
        entry: String::new(),
        stamps: Vec::new(),
        entry_times: EntryTimes::new(&input.local),
        period: input.local.remote.period.map(utilake_core::Period::month),
        day: match input.local.remote.period {
            Some(utilake_core::Period::Day(d)) => Some(d),
            _ => None,
        },
        warnings: Vec::new(),
        row_errors: Vec::new(),
        fail_fast: input.options.fail_fast,
        events: input.events.as_ref(),
    };
    if kit.decoder_for(&name).is_none() && !has_extension(&name, ".zip") {
        file_error = Some(
            Diagnostic::error(
                "config.no_decoder",
                Category::Config,
                format!(
                    "The file {name} is not one the kit {} reads; no decoder accepts its name and it is not a zip.",
                    kit.id
                ),
            )
            .with_file(path.display().to_string())
            .with_request(input.request.clone()),
        );
    } else if kit.decoder_for(&name).is_some() {
        match std::fs::File::open(&path) {
            Ok(mut f) => entries.push(ingest_entry(
                &input,
                kit,
                &name,
                &mut f,
                &mut scan_buf,
                &mut sink,
            )),
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
        let accepts = Accepts {
            accepts: &|n: &str| kit.decoder_for(n).is_some() && !input.skip.iter().any(|s| s == n),
            wanted: "readable",
        };
        let result: Result<Walked, WalkError<()>> = walk_entries(
            &path,
            &input.tmp_dir,
            accepts,
            &mut |e| {
                let rep = ingest_entry(&input, kit, &e.name, e.reader, &mut scan_buf, &mut sink);
                let stop = fail_fast && rep.error.is_some();
                entries.push(rep);
                if stop { Err(()) } else { Ok(()) }
            },
            &mut |_| {},
        );
        let place = |e: &dyn Diagnose| {
            let mut d = e.diagnostic();
            d.request = Some(input.request.clone());
            d.url = Some(input.local.remote.url.to_string());
            d.file = Some(path.display().to_string());
            d
        };
        match result {
            // An entry left behind is an entry that failed; the rest were
            // read.
            Ok(w) => entries.extend(w.failed.iter().map(|a| EntryReport {
                entry: match a {
                    ArchiveError::Entry { entry, .. }
                    | ArchiveError::TooDeep { entry, .. }
                    | ArchiveError::Spool { entry, .. } => entry.clone(),
                    ArchiveError::Open { .. } | ArchiveError::NoEntry { .. } => String::new(),
                },
                warnings: Vec::new(),
                row_errors: Vec::new(),
                error: Some(place(a)),
            })),
            Err(WalkError::Archive(e)) => file_error = Some(place(&e)),
            Err(WalkError::Callback(())) => {}
        }
    }
    let results = finish(
        &mut sink,
        &mut entries,
        file_error.is_some(),
        &input.request,
    );
    let (mut results, kept) = results;
    if kept && let Err(e) = sink.writer.commit() {
        // Every entry's files at once: none is in place if one is not.
        for r in &mut results {
            for o in r.outputs.drain(..) {
                let _ = std::fs::remove_file(&o.path);
            }
        }
        let mut d = e.diagnostic();
        d.file = Some(path.display().to_string());
        d.request = Some(input.request.clone());
        file_error.get_or_insert(d);
    }
    drop(sink);
    FileReport {
        rules_changed: input.rules_changed,
        left: input.skip,
        local: input.local,
        entries,
        results,
        file_error,
    }
}

/// Closes the file's writers and quarantine and gives what it read, with
/// whether it is kept. When the file or any of its entries failed, its
/// Parquet files are discarded, its results kept without them, and every
/// entry that did not fail itself is told why. A file goes into the lake
/// whole or not at all.
fn finish(
    sink: &mut LakeSink<'_>,
    entries: &mut [EntryReport],
    file_failed: bool,
    request: &str,
) -> (Vec<IngestResult>, bool) {
    let failed = entries
        .iter()
        .find_map(|e| e.error.as_ref().map(|_| e.entry.clone()));
    if !file_failed && failed.is_none() {
        match sink.close_all() {
            Ok(()) => {
                sink.finish_quarantine();
                return (std::mem::take(&mut sink.results), true);
            }
            Err(e) => {
                let d = e.diagnostic();
                if let Some(first) = entries.first_mut() {
                    first.error = Some(d);
                }
            }
        }
    }
    sink.abort_all();
    sink.finish_quarantine();
    let url = sink.local.remote.url.to_string();
    for e in entries.iter_mut().filter(|e| e.error.is_none()) {
        let because = failed.as_deref().map_or_else(
            || "the file could not be read whole".to_owned(),
            |f| format!("the entry {f} of the same file failed"),
        );
        e.error = Some(
            Diagnostic::error(
                "store.file_discarded",
                Category::Store,
                format!(
                    "The entry {} was read but not kept: {because}, and a file goes into the lake whole or not at all.",
                    e.entry
                ),
            )
            .with_url(url.clone())
            .with_entry(e.entry.clone())
            .with_request(request.to_owned()),
        );
    }
    (std::mem::take(&mut sink.results), false)
}
