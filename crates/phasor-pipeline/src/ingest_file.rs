//! One source file, end to end, on a blocking worker: walk the entries its
//! kit's decoders read, decode each into typed batches, write Parquet per
//! partition, quarantine bad rows. Nothing here touches the manifest or the
//! network; the result is handed back for the async commit step.

use std::path::PathBuf;
use std::sync::Arc;

use crate::entry_times::EntryTimes;
use phasor_core::event::Event;
use phasor_core::source_times;
use phasor_core::{Category, Diagnose, Diagnostic, Fingerprint};
use phasor_kit::LocalFile;
use phasor_kit::{
    Block, BlockSink, DecodeCtx, DecodeOptions, Flow, KitRegistry, RowAt, SourceKit, TableRef,
};
use phasor_provenance::layout::listing_offset;
use phasor_writer::manifest::EntryShare;
use phasor_writer::{
    IngestSession, Shape, Source, Stamp, StoreError, Times, Transform, WrittenFile,
};
use phasor_zip::{Accepts, ArchiveError, WalkError, Walked, has_extension, walk_entries};
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

/// Hands a source file's blocks to its write into the lake, and keeps
/// what is said of each entry.
struct LakeSink<'a> {
    /// The source file's write: every entry's rows go to the same files,
    /// each row naming its entry, and the files are committed together
    /// once the whole file is read.
    session: IngestSession,
    results: Vec<IngestResult>,
    current: Option<usize>,
    local: &'a LocalFile,
    /// When each entry first went public, from the records beside the file.
    entry_times: EntryTimes,
    warnings: Vec<Diagnostic>,
    row_errors: Vec<Diagnostic>,
    fail_fast: bool,
    events: Option<&'a mpsc::Sender<Event>>,
    /// The entry being read.
    entry: String,
}

impl LakeSink<'_> {
    /// Entry `entry`'s source, `url!entry` (the URL alone for a file read
    /// whole), and its publish times. `zip_entry_modified`: its modified
    /// time in its zip's directory, in microseconds since 1970 (UTC), when
    /// it has one.
    fn stamp(&mut self, entry: &str, zip_entry_modified: Option<i64>) -> Stamp {
        let local = self.local;
        let url = local.remote.url.as_str();
        let name = if entry.is_empty() {
            url.to_owned()
        } else {
            format!("{url}!{entry}")
        };
        // Each time as found, none chosen over another: the server's
        // word for the entry's own bytes where the mirror holds them,
        // else for the file; the zip's record of the entry; the stamp
        // in the entry's name (or the file's, read whole), in the
        // clock of the host that named it.
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

/// Reads one entry through the kit's decoder into the file's sink.
fn ingest_entry(
    input: &IngestInput,
    kit: &SourceKit,
    entry: &str,
    zip_entry_modified: Option<i64>,
    reader: &mut dyn std::io::Read,
    scan_buf: &mut Vec<u8>,
    sink: &mut LakeSink<'_>,
) -> EntryReport {
    zone!(ENTRY);
    let ctx = phasor_core::diag::Context {
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
            Category::CONFIG,
            format!(
                "The entry {entry} is not one the kit {} reads; no decoder accepts its name.",
                kit.id
            ),
        );
        d.fill_from(&ctx);
        report.error = Some(d);
        return report;
    };
    sink.begin_entry(entry, zip_entry_modified);
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
                Category::CSV,
                if input.options.strict_footer {
                    phasor_core::Severity::Error
                } else {
                    phasor_core::Severity::Warning
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
            d.severity = phasor_core::Severity::Warning;
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
    // One read buffer and one sink for every entry of the file.
    let mut scan_buf = Vec::new();
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
        events: input.events.as_ref(),
        entry: String::new(),
    };
    if kit.decoder_for(&name).is_none() && !has_extension(&name, ".zip") {
        file_error = Some(
            Diagnostic::error(
                "config.no_decoder",
                Category::CONFIG,
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
                None,
                &mut f,
                &mut scan_buf,
                &mut sink,
            )),
            Err(e) => {
                file_error = Some(
                    Diagnostic::error(
                        "csv.read_failed",
                        Category::CSV,
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
                // The packer's clock: AEST for AEMO's zips, AWST for the WA host's.
                let written = e
                    .modified_at(listing_offset(input.local.remote.url.as_str()))
                    .map(|s| s.unix() * 1_000_000);
                let rep = ingest_entry(
                    &input,
                    kit,
                    &e.name,
                    written,
                    e.reader,
                    &mut scan_buf,
                    &mut sink,
                );
                let stop = fail_fast && rep.error.is_some();
                entries.push(rep);
                if stop { Err(()) } else { Ok(()) }
            },
            &mut |_| {},
        );
        let place = |e: &ArchiveError| {
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

/// Closes the file's writes and quarantine and gives what it read, with
/// whether its files were put in place. When the file or any of its
/// entries failed, its Parquet files are discarded, its results kept
/// without them, and every entry that did not fail itself is told why. A
/// file goes into the lake whole or not at all.
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
