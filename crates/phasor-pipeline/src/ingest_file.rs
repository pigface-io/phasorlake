//! One source file, end to end, on a blocking worker: decode its entries,
//! write Parquet, quarantine bad rows. No manifest or network access here.

use std::path::PathBuf;
use std::sync::Arc;

use crate::decode::{DecodeFile, FileSink, decode_file, footer_mismatch};
use crate::entry_times::EntryTimes;
use phasor_core::event::Event;
use phasor_core::source_times;
use phasor_core::{Category, Diagnose, Diagnostic, Fingerprint};
use phasor_kit::LocalFile;
use phasor_kit::{
    Block, BlockSink, DecodeOptions, DecodeStats, Flow, KitRegistry, RowAt, TableRef,
};
use phasor_provenance::layout::listing_offset;
use phasor_writer::manifest::EntryShare;
use phasor_writer::{
    IngestSession, Shape, Source, Stamp, StoreError, Times, Transform, WrittenFile,
};
use phasor_zip::ArchiveError;
use tokio::sync::mpsc;

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
    /// The file is being read again because its rules have changed.
    pub rules_changed: bool,
    /// Entries (nested paths joined with `!`) already in the lake from a fetch of each alone.
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
    /// The first [`ROW_ERRORS_KEPT`] unreadable rows: errors if the entry was refused, else warnings.
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
    /// What the file read, per shape; no Parquet files when an entry failed (all or nothing).
    pub results: Vec<IngestResult>,
    /// An error that prevented reading the file at all.
    pub file_error: Option<Diagnostic>,
    /// The file was read again because its rules have changed.
    pub rules_changed: bool,
    /// Entries left to an earlier read of each alone.
    pub left: Vec<String>,
}

/// Parquet writers all workers of a run may hold open, under the default open-file limit.
const OPEN_WRITERS_PER_RUN: usize = 128;

/// How many writers one of `jobs` workers may hold open at once; never fewer than eight.
pub fn open_writers(jobs: usize) -> usize {
    (OPEN_WRITERS_PER_RUN / jobs.max(1)).max(8)
}

/// How many row errors of one entry are reported one by one.
pub const ROW_ERRORS_KEPT: usize = 20;

/// Hands a source file's blocks to its lake write, and keeps what is said of each entry.
struct LakeSink<'a> {
    /// The file's write; all entries' rows commit together once the file is read.
    session: IngestSession,
    results: Vec<IngestResult>,
    current: Option<usize>,
    local: &'a LocalFile,
    /// When each entry first went public, from the records beside the file.
    entry_times: EntryTimes,
    warnings: Vec<Diagnostic>,
    row_errors: Vec<Diagnostic>,
    fail_fast: bool,
    /// A footer count mismatch fails the entry.
    strict_footer: bool,
    /// The request, for diagnostics.
    request: &'a str,
    events: Option<&'a mpsc::Sender<Event>>,
    /// The entry being read.
    entry: String,
    /// What was said of each entry read so far, in order.
    entries: Vec<EntryReport>,
}

impl LakeSink<'_> {
    /// Entry `entry`'s source name and times; `zip_entry_modified` is microseconds UTC.
    fn stamp(&mut self, entry: &str, zip_entry_modified: Option<i64>) -> Stamp {
        let local = self.local;
        let url = local.remote.url.as_str();
        let name = if entry.is_empty() {
            url.to_owned()
        } else {
            format!("{url}!{entry}")
        };
        // Every time as found, none chosen over another.
        let modified = local.last_modified_at.map(|s| s.rfc3339());
        let server_last_modified = self
            .entry_times
            .server_last_modified(entry)
            .or_else(|| source_times::server_last_modified(modified.as_deref()));
        let named = if entry.is_empty() { url } else { entry };
        let times = Times {
            server_last_modified,
            zip_entry_modified,
            filename_timestamp: source_times::filename_timestamp(named, listing_offset(url)),
        };
        Stamp::new(&name, times)
    }

    /// Begins entry `entry`: what is said from here on is said of it.
    fn begin_entry(&mut self, entry: &str, zip_entry_modified: Option<i64>) {
        let stamp = self.stamp(entry, zip_entry_modified);
        self.session.begin_entry(entry, stamp);
        entry.clone_into(&mut self.entry);
        self.current = None;
    }

    /// The shape of result `idx`.
    fn shape(&self, idx: usize) -> Shape {
        let r = &self.results[idx];
        Shape {
            table: r.table.clone(),
            version: r.version,
            fingerprint: r.fingerprint.0,
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
        // Two column lists against one schema are two shapes, two results.
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
        let Some(idx) = self.current else {
            return Ok(());
        };
        let rows = b.batch.num_rows() as u64;
        let table = b.table.name.clone();
        self.session.write(b).map_err(|e| {
            Box::new(
                Diagnostic::error(
                    "store.write_failed",
                    Category::STORE,
                    format!("Writing a batch of table {table} failed: {e}"),
                )
                .with_table(table.clone())
                .caused_by(&e),
            )
        })?;
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
        let shape = self.current.map(|i| {
            self.results[i].bad_rows += 1;
            self.shape(i)
        });
        if self.row_errors.len() < ROW_ERRORS_KEPT {
            self.row_errors.push(d.clone());
        }
        if let Err(e) = self.session.quarantine(&table, shape.as_ref(), raw, &d) {
            let mut w = e.diagnostic();
            w.severity = phasor_core::Severity::Warning;
            self.warnings.push(w);
        }
        if self.fail_fast {
            Flow::Stop
        } else {
            Flow::Continue
        }
    }
}

impl FileSink for LakeSink<'_> {
    fn entry_started(&mut self, entry: &str, zip_entry_modified: Option<i64>) {
        self.begin_entry(entry, zip_entry_modified);
    }

    fn entry_done(&mut self, entry: &str, result: Result<DecodeStats, Box<Diagnostic>>) -> Flow {
        let ctx = phasor_core::diag::Context {
            request: Some(self.request.to_owned()),
            url: Some(self.local.remote.url.to_string()),
            file: Some(self.local.path.display().to_string()),
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
        let severity = if self.strict_footer {
            phasor_core::Severity::Error
        } else {
            phasor_core::Severity::Warning
        };
        match result {
            Ok(stats) => {
                if let Some(d) = footer_mismatch(&stats, severity) {
                    if self.strict_footer {
                        report.error = Some(d);
                    } else {
                        self.warnings.push(d);
                    }
                }
            }
            Err(d) => report.error = Some(*d),
        }
        for w in &mut self.warnings {
            w.fill_from(&ctx);
        }
        if let Some(e) = &mut report.error {
            e.fill_from(&ctx);
        }
        report.row_errors = std::mem::take(&mut self.row_errors);
        for d in &mut report.row_errors {
            d.fill_from(&ctx);
            // Accepted without these rows; they are in the quarantine file.
            if report.error.is_none() {
                d.severity = phasor_core::Severity::Warning;
            }
        }
        report.warnings = std::mem::take(&mut self.warnings);
        let stop = self.fail_fast && report.error.is_some();
        self.entries.push(report);
        if stop { Flow::Stop } else { Flow::Continue }
    }
}

/// Ingests one local file, or the entries of a zip (nested included) its kit's decoders read.
pub fn ingest_file(input: IngestInput) -> FileReport {
    zone!(FILE);
    let path = input.local.path.clone();
    let mut entries = Vec::new();
    let mut file_error = None;
    let Some(kit) = input.kits.kit(input.kit) else {
        return FileReport {
            file_error: Some(
                Diagnostic::error(
                    "config.no_decoder",
                    Category::CONFIG,
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
    let source = Source {
        url: input.local.remote.url.to_string(),
        sha256: input.local.sha256.clone(),
        stem: input.local.remote.stem(),
        period: input.local.remote.period.map(phasor_core::Period::month),
        day: match input.local.remote.period {
            Some(phasor_core::Period::Day(d)) => Some(d),
            _ => None,
        },
    };
    let mut sink = LakeSink {
        session: IngestSession::new(
            &input.lake_root,
            &input.quarantine_root,
            input.options.layout,
            open_writers(input.options.jobs),
            source,
        ),
        results: Vec::new(),
        current: None,
        local: &input.local,
        entry_times: EntryTimes::new(&input.local),
        warnings: Vec::new(),
        row_errors: Vec::new(),
        fail_fast: input.options.fail_fast,
        strict_footer: input.options.strict_footer,
        request: &input.request,
        events: input.events.as_ref(),
        entry: String::new(),
        entries: Vec::new(),
    };
    let options = DecodeOptions {
        strictness: input.options.mode,
        max_bad_rows: input.options.max_bad_rows,
        batch_rows: input.options.batch_rows,
    };
    let decode = DecodeFile {
        kit,
        local: &input.local,
        options: &options,
        tmp_dir: &input.tmp_dir,
        skip: &input.skip,
    };
    let place = |mut d: Diagnostic| {
        d.request = Some(input.request.clone());
        d.url = Some(input.local.remote.url.to_string());
        d.file = Some(path.display().to_string());
        d
    };
    let walked = decode_file(&decode, &mut sink);
    entries.append(&mut sink.entries);
    match walked {
        Ok(failed) => entries.extend(failed.iter().map(|a| EntryReport {
            entry: match a {
                ArchiveError::Entry { entry, .. }
                | ArchiveError::TooDeep { entry, .. }
                | ArchiveError::Spool { entry, .. } => entry.clone(),
                ArchiveError::Open { .. } | ArchiveError::NoEntry { .. } => String::new(),
            },
            warnings: Vec::new(),
            row_errors: Vec::new(),
            error: Some(place(a.diagnostic())),
        })),
        Err(d) => file_error = Some(place(*d)),
    }
    let (results, committed) = finish(sink, &mut entries, file_error.is_some(), &input.request);
    if let Err(e) = committed {
        // Every entry's files at once: none is in place if one is not.
        let mut d = e.diagnostic();
        d.file = Some(path.display().to_string());
        d.request = Some(input.request.clone());
        file_error.get_or_insert(d);
    }
    FileReport {
        rules_changed: input.rules_changed,
        left: input.skip,
        local: input.local,
        entries,
        results,
        file_error,
    }
}

/// Closes the file's writes and quarantine; a failed file or entry discards all its Parquet files.
fn finish(
    mut sink: LakeSink<'_>,
    entries: &mut [EntryReport],
    file_failed: bool,
    request: &str,
) -> (Vec<IngestResult>, Result<(), StoreError>) {
    let failed = entries
        .iter()
        .find_map(|e| e.error.as_ref().map(|_| e.entry.clone()));
    let closed = if !file_failed && failed.is_none() {
        match sink.session.close() {
            Ok(()) => true,
            Err(e) => {
                let d = e.diagnostic();
                if let Some(first) = entries.first_mut() {
                    first.error = Some(d);
                }
                false
            }
        }
    } else {
        false
    };
    let (mut written, committed) = if closed {
        sink.session.commit()
    } else {
        (sink.session.abort(), Ok(()))
    };
    let mut results = sink.results;
    for r in &mut results {
        let shape = Shape {
            table: r.table.clone(),
            version: r.version,
            fingerprint: r.fingerprint.0,
        };
        let o = written.take(&shape);
        r.outputs = o.outputs;
        r.entries = o.entries;
        if let Some(p) = written.take_quarantine(&r.table) {
            r.quarantine_path = Some(p);
        }
    }
    if closed {
        return (results, committed);
    }
    let url = sink.local.remote.url.to_string();
    for e in entries.iter_mut().filter(|e| e.error.is_none()) {
        let because = failed.as_deref().map_or_else(
            || "the file could not be read whole".to_owned(),
            |f| format!("the entry {f} of the same file failed"),
        );
        e.error = Some(
            Diagnostic::error(
                "store.file_discarded",
                Category::STORE,
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
    (results, Ok(()))
}
