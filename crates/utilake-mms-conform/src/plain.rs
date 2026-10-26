//! The files that are not MMS rows, through the same registry and sink: a
//! header-row CSV, whose name names the block it carries, and a WEMDE JSON
//! document, whose arrays under `data` each carry one.
//!
//! Both are read by the plain decoder of utilake-schemas, the one the
//! census reads them with, so a block's name, its columns and every date
//! are exactly what the census recorded, and each block resolves through
//! the registry like an MMS block: a shape the history has conforms, a
//! new one is derived. A row has no prefix: its block is the header that
//! opened it ([`ArrowSink::with_prefix`]).
//!
//! A feed whose files restate the one before ([`Dedupe::VersionDiff`]) is
//! read against the previous version the source names
//! ([`DecodeCtx::previous`]): a row the previous version already had is
//! not ingested again, and [`DecodeStats::rows_repeated`] counts it.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use utilake_core::{Category, Diagnose, Diagnostic, Period};
use utilake_kit::{BlockSink, DecodeCtx, DecodeStats, Decoder, Strictness};
use utilake_mms_decode::{Flow, IRow, Pos, Row, RowError, RowFailure, Sink as _};
use utilake_mms_registry::{Market, Mode, Registry, ResolveOptions};
use utilake_plain_decode::names::block_name;
use utilake_plain_decode::{
    BlockName, Dedupe, Feed, Format, Header, JsonError, PlainError, PlainOptions, PlainRowError,
    PlainSink, RowSet, peek, sniff,
};
use utilake_zip::has_extension;

use crate::arrow_sink::ArrowSink;
use crate::error::SinkError;
use crate::kit::{Bridge, MmsDecoder};

/// A header-row CSV of one feed, resolved through the registry.
pub struct PlainCsvDecoder {
    registry: Arc<Registry>,
    market: Market,
    feed: &'static Feed,
}

impl PlainCsvDecoder {
    /// A decoder over `registry` for the CSV files of `feed`.
    pub fn new(registry: Arc<Registry>, feed: &'static Feed) -> Self {
        Self {
            registry,
            market: market_of(feed),
            feed,
        }
    }

    /// The feed.
    pub fn feed(&self) -> &'static Feed {
        self.feed
    }
}

/// A WEMDE JSON document of one feed: each array under `data` a block,
/// resolved through the registry.
pub struct WemdeDecoder {
    registry: Arc<Registry>,
    market: Market,
    feed: &'static Feed,
}

impl WemdeDecoder {
    /// A decoder over `registry` for the documents of `feed`.
    pub fn new(registry: Arc<Registry>, feed: &'static Feed) -> Self {
        Self {
            registry,
            market: market_of(feed),
            feed,
        }
    }
}

/// A `.csv` of a feed whose folders hold more than one kind, read by what
/// its first bytes say it is: MMS rows (the GSH files among the GBB
/// reports) through the MMS decoder, a header row through the plain one,
/// and markup (a page saved as `.csv`) skipped with a warning.
pub struct SniffedCsv {
    mms: MmsDecoder,
    plain: PlainCsvDecoder,
}

impl SniffedCsv {
    /// A decoder over `registry` for the `.csv` files of `feed`.
    pub fn new(registry: Arc<Registry>, feed: &'static Feed) -> Self {
        Self {
            mms: MmsDecoder::new(registry.clone(), market_of(feed)),
            plain: PlainCsvDecoder::new(registry, feed),
        }
    }
}

/// The market a feed belongs to.
fn market_of(feed: &Feed) -> Market {
    Market::parse(feed.market).unwrap_or_default()
}

/// The path the block-naming rule reads: the file's path on its host, with
/// a zip entry's name after `!` when the entry is not the file itself.
fn path_of(ctx: &DecodeCtx<'_>) -> String {
    let path = ctx.file.remote.url.path();
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if ctx.entry.is_empty() || ctx.entry == file_name {
        path.to_owned()
    } else {
        format!("{path}!{}", ctx.entry)
    }
}

/// The options a feed's files are read with.
fn options<'a>(feed: &'a Feed, ctx: &DecodeCtx<'_>, name: &str) -> PlainOptions<'a> {
    PlainOptions {
        offset: feed.offset_text(),
        max_bad_rows: ctx.options.max_bad_rows,
        unpivot: feed.unpivot_of(name),
        ..PlainOptions::default()
    }
}

/// The block a file carries, or the diagnostic saying its name reads as
/// none under its feed.
fn name_of(
    feed: &Feed,
    ctx: &DecodeCtx<'_>,
    code: &'static str,
    category: Category,
) -> Result<BlockName, Box<Diagnostic>> {
    let path = path_of(ctx);
    block_name(feed, &path).ok_or_else(|| {
        Box::new(
            Diagnostic::error(
                code,
                category,
                format!(
                    "The file {path} names no block under the {} feed of the {} market: its name follows none of the feed's forms, so no table can claim it.",
                    feed.id, feed.market
                ),
            )
            .with_entry(ctx.entry.to_owned()),
        )
    })
}

/// A plain scan's failure as a diagnostic; `sink` says what the sink's
/// own failure is.
fn plain_diag<F: std::fmt::Display>(
    e: PlainError<F>,
    sink: impl FnOnce(F) -> Diagnostic,
) -> Diagnostic {
    let msg = e.to_string();
    let (code, line) = match &e {
        PlainError::Io(_) => ("csv.read_failed", None),
        PlainError::RecordTooLong { line, .. } => ("csv.line_too_long", Some(*line)),
        PlainError::HeaderMissing => ("csv.header_missing", None),
        PlainError::BadHeader { line, .. } => ("csv.header_malformed", Some(*line)),
        PlainError::TooManyBadRows { line, .. } => ("csv.too_many_bad_rows", Some(*line)),
        PlainError::Stopped { line } => ("csv.stopped_fail_fast", Some(*line)),
        PlainError::Sink(_) => ("", None),
    };
    let mut d = match e {
        PlainError::Sink(f) => return sink(f),
        PlainError::Io(source) => Diagnostic::error(code, Category::Csv, msg).caused_by(&source),
        _ => Diagnostic::error(code, Category::Csv, msg),
    };
    d.line = line;
    d
}

/// A document scan's failure as a diagnostic.
fn json_diag(e: JsonError<SinkError>) -> Diagnostic {
    let msg = e.to_string();
    match e {
        JsonError::Syntax(source) => {
            Diagnostic::error("json.syntax", Category::Json, msg).caused_by(&source)
        }
        JsonError::Shape(_) => Diagnostic::error("json.not_wemde", Category::Json, msg),
        JsonError::Sink(f) => f.diagnostic(),
    }
}

/// Hands a plain file's blocks and rows to an [`ArrowSink`] as the MMS
/// scanner would: each header as an I row named by the file, each row
/// with no prefix.
struct PlainBridge<'a> {
    arrow: ArrowSink<'a, Bridge<'a>>,
    /// The block the file carries; a document's arrays append to its name.
    name: BlockName,
    /// The name of the block the current header opened.
    current: String,
    /// The previous version's rows, when the feed restates its files.
    previous: Option<RowSet>,
    /// Blocks opened.
    blocks: u64,
    /// Rows the previous version already had.
    repeated: u64,
    /// Rows the sink refused.
    bad: u64,
    /// A refused row's text, rebuilt from its fields for the quarantine.
    raw: Vec<u8>,
}

impl<'a> PlainBridge<'a> {
    fn new(
        registry: &'a Registry,
        market: Market,
        ctx: &DecodeCtx<'_>,
        sink: &'a mut dyn BlockSink,
        name: BlockName,
        previous: Option<RowSet>,
    ) -> Self {
        let period = ctx.period.map(Period::month);
        let mode = match ctx.options.strictness {
            Strictness::Permissive => Mode::Permissive,
            Strictness::Strict => Mode::Strict,
        };
        let bridge = Bridge {
            out: sink,
            registry,
            period,
            tables: HashMap::new(),
            current: None,
        };
        let arrow = ArrowSink::new(
            registry,
            ResolveOptions { mode, market },
            ctx.options.batch_rows,
            bridge,
        )
        .with_period(period)
        .with_prefix(0);
        Self {
            arrow,
            name,
            current: String::new(),
            previous,
            blocks: 0,
            repeated: 0,
            bad: 0,
            raw: Vec::new(),
        }
    }

    /// Flushes, and the counts the scan's own do not hold.
    fn finish(self) -> (Result<(), SinkError>, u64, u64, u64) {
        let (blocks, repeated, bad) = (self.blocks, self.repeated, self.bad);
        let (_, flushed) = self.arrow.finish();
        (flushed, blocks, repeated, bad)
    }
}

impl PlainSink for PlainBridge<'_> {
    type Fatal = SinkError;

    fn header(&mut self, array: Option<&str>, header: &Header, pos: Pos) -> Result<(), SinkError> {
        self.current = match array {
            Some(a) => self.name.with_array(a).name,
            None => self.name.name.clone(),
        };
        let columns: Vec<&str> = header.names.iter().map(String::as_str).collect();
        let irow = IRow {
            report_type: &self.name.family,
            report_subtype: &self.current,
            version: self.name.version,
            columns: &columns,
        };
        self.blocks += 1;
        self.arrow.header(&irow, pos)
    }

    fn row(&mut self, row: &Row<'_>, pos: Pos) -> Result<(), SinkError> {
        if self.previous.as_ref().is_some_and(|p| p.contains(row)) {
            self.repeated += 1;
            return Ok(());
        }
        match self.arrow.row(row, pos) {
            Ok(()) => Ok(()),
            Err(RowFailure::Sink(f)) => Err(*f),
            Err(RowFailure::Row(e)) => {
                self.bad += 1;
                self.raw.clear();
                for (i, f) in row.iter().enumerate() {
                    if i > 0 {
                        self.raw.push(b',');
                    }
                    self.raw.extend_from_slice(f);
                }
                match self.arrow.bad_row(&self.raw, pos, &e) {
                    Flow::Continue => Ok(()),
                    Flow::Stop => Err(SinkError::Refused(Box::new(
                        Diagnostic::error(
                            "csv.stopped_fail_fast",
                            Category::Csv,
                            format!("Stopped at line {}: {e}", pos.line),
                        )
                        .at(pos.line, pos.byte_offset),
                    ))),
                }
            }
        }
    }

    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &PlainRowError) -> Flow {
        let PlainRowError::TooManyFields { declared, observed } = err;
        self.arrow.bad_row(
            raw,
            pos,
            &RowError::FieldCount {
                declared: *declared,
                observed: *observed,
            },
        )
    }
}

/// The rows of the previous version of the file, when the feed restates
/// its files and the source named one.
fn previous_rows(
    feed: &Feed,
    ctx: &DecodeCtx<'_>,
    opts: &PlainOptions<'_>,
) -> Result<Option<RowSet>, Box<Diagnostic>> {
    let Some(path) = ctx.previous.filter(|_| feed.dedupe == Dedupe::VersionDiff) else {
        return Ok(None);
    };
    let shown = path.display().to_string();
    let file = std::fs::File::open(path).map_err(|e| {
        Box::new(
            Diagnostic::error(
                "csv.read_failed",
                Category::Csv,
                format!("The previous version of the file, {shown}, could not be read: {e}"),
            )
            .with_file(shown.clone())
            .caused_by(&e),
        )
    })?;
    let (_, set) = RowSet::collect_csv(file, opts, feed.time_columns).map_err(|e| {
        let mut d = plain_diag(e, |never| match never {});
        d.file = Some(shown);
        Box::new(d)
    })?;
    Ok(Some(set))
}

impl Decoder for PlainCsvDecoder {
    fn id(&self) -> &'static str {
        "csv"
    }

    fn accepts(&self, entry_name: &str) -> bool {
        has_extension(entry_name, ".csv")
    }

    fn decode(
        &self,
        reader: &mut dyn Read,
        ctx: &DecodeCtx<'_>,
        sink: &mut dyn BlockSink,
        _scratch: &mut Vec<u8>,
    ) -> Result<DecodeStats, Box<Diagnostic>> {
        let name = name_of(self.feed, ctx, "csv.unnamed_block", Category::Csv)?;
        let opts = options(self.feed, ctx, &name.name);
        let previous = previous_rows(self.feed, ctx, &opts)?;
        let mut bridge = PlainBridge::new(&self.registry, self.market, ctx, sink, name, previous);
        let scanned = utilake_plain_decode::csv::scan(reader, &mut bridge, &opts);
        let (flushed, blocks, repeated, bad) = bridge.finish();
        match (scanned, flushed) {
            (Ok(s), Ok(())) => Ok(DecodeStats {
                lines: s.lines,
                bytes: s.bytes,
                blocks,
                rows: s.rows - repeated - bad,
                bad_rows: s.bad_rows + bad,
                rows_repeated: repeated,
                footer_declared: None,
            }),
            (Ok(_), Err(e)) => Err(Box::new(e.diagnostic())),
            (Err(e), _) => Err(Box::new(plain_diag(e, |f| f.diagnostic()))),
        }
    }
}

impl Decoder for WemdeDecoder {
    fn id(&self) -> &'static str {
        "json"
    }

    fn accepts(&self, entry_name: &str) -> bool {
        has_extension(entry_name, ".json")
    }

    fn decode(
        &self,
        reader: &mut dyn Read,
        ctx: &DecodeCtx<'_>,
        sink: &mut dyn BlockSink,
        _scratch: &mut Vec<u8>,
    ) -> Result<DecodeStats, Box<Diagnostic>> {
        let name = name_of(self.feed, ctx, "json.unnamed_block", Category::Json)?;
        let opts = options(self.feed, ctx, &name.name);
        let mut bridge = PlainBridge::new(&self.registry, self.market, ctx, sink, name, None);
        let scanned = utilake_plain_decode::json::scan(reader, &mut bridge, &opts);
        let (flushed, blocks, _, bad) = bridge.finish();
        match (scanned, flushed) {
            (Ok(s), Ok(())) => Ok(DecodeStats {
                lines: 0,
                bytes: ctx.file.size,
                blocks,
                rows: s.rows - bad,
                bad_rows: bad,
                rows_repeated: 0,
                footer_declared: None,
            }),
            (Ok(_), Err(e)) => Err(Box::new(e.diagnostic())),
            (Err(e), _) => Err(Box::new(json_diag(e))),
        }
    }
}

impl Decoder for SniffedCsv {
    fn id(&self) -> &'static str {
        "csv"
    }

    fn accepts(&self, entry_name: &str) -> bool {
        has_extension(entry_name, ".csv")
    }

    fn decode(
        &self,
        reader: &mut dyn Read,
        ctx: &DecodeCtx<'_>,
        sink: &mut dyn BlockSink,
        scratch: &mut Vec<u8>,
    ) -> Result<DecodeStats, Box<Diagnostic>> {
        let (head, mut rest) = peek(&mut *reader).map_err(|e| {
            Box::new(
                Diagnostic::error(
                    "csv.read_failed",
                    Category::Csv,
                    format!("Reading the first bytes of the entry failed: {e}"),
                )
                .with_entry(ctx.entry.to_owned())
                .caused_by(&e),
            )
        })?;
        match sniff(&head) {
            Format::Mms => self.mms.decode(&mut rest, ctx, sink, scratch),
            Format::Csv => self.plain.decode(&mut rest, ctx, sink, scratch),
            Format::Markup => {
                sink.warning(
                    Diagnostic::warning(
                        "csv.markup_skipped",
                        Category::Csv,
                        format!(
                            "The entry {} is a page (HTML or XML) saved under a .csv name; it carries no rows and is skipped.",
                            ctx.entry
                        ),
                    )
                    .with_entry(ctx.entry.to_owned()),
                );
                Ok(DecodeStats::default())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use arrow_array::Array as _;
    use url::Url;
    use utilake_kit::{Block, DecodeOptions, Flow as KitFlow, RowAt, TableRef, Transform};
    use utilake_kit::{LocalFile, RemoteFile};
    use utilake_mms_decode::Fingerprint;
    use utilake_plain_decode::feeds::feed;

    use super::*;

    /// Keeps what a decoder hands on.
    #[derive(Default)]
    struct Collect {
        blocks: Vec<(String, String, u16, String)>,
        batches: Vec<Block>,
        warnings: Vec<Diagnostic>,
        bad: Vec<(u64, String, String)>,
        stop: bool,
    }

    impl BlockSink for Collect {
        fn block_started(
            &mut self,
            table: &Arc<TableRef>,
            version: u16,
            _: Fingerprint,
            transform: &Arc<Transform>,
            status: &str,
        ) {
            self.blocks.push((
                table.name.clone(),
                table.market.clone(),
                version,
                format!(
                    "{status}:{}:{}",
                    transform.block_family, transform.block_name
                ),
            ));
        }
        fn batch(&mut self, b: Block) -> Result<(), Box<Diagnostic>> {
            self.batches.push(b);
            Ok(())
        }
        fn warning(&mut self, d: Diagnostic) {
            self.warnings.push(d);
        }
        fn bad_row(&mut self, raw: &[u8], at: RowAt, d: Diagnostic) -> KitFlow {
            self.bad.push((
                at.line,
                String::from_utf8_lossy(raw).into_owned(),
                d.code.to_string(),
            ));
            if self.stop {
                KitFlow::Stop
            } else {
                KitFlow::Continue
            }
        }
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/plain/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn local(url: &str, size: u64) -> LocalFile {
        LocalFile {
            remote: RemoteFile {
                url: Url::parse(url).unwrap(),
                source: "test",
                table: None,
                period: None,
                size: None,
                modified: None,
                modified_at: None,
            },
            path: std::path::PathBuf::from("/none"),
            sha256: String::new(),
            size,
            cached: true,
            etag: None,
            last_modified: None,
            last_modified_at: None,
            fetched_at: None,
            previous: None,
        }
    }

    fn options(strict: bool, max_bad_rows: u64) -> DecodeOptions {
        DecodeOptions {
            strictness: if strict {
                Strictness::Strict
            } else {
                Strictness::Permissive
            },
            max_bad_rows,
            batch_rows: 1024,
        }
    }

    fn decode(
        d: &dyn Decoder,
        url: &str,
        entry: &str,
        bytes: &[u8],
        opts: &DecodeOptions,
        previous: Option<&std::path::Path>,
        sink: &mut Collect,
    ) -> Result<DecodeStats, Box<Diagnostic>> {
        let file = local(url, bytes.len() as u64);
        let ctx = DecodeCtx {
            file: &file,
            entry,
            period: None,
            options: opts,
            previous,
        };
        d.decode(&mut Cursor::new(bytes), &ctx, sink, &mut Vec::new())
    }

    const GBB: &str =
        "https://www.nemweb.com.au/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV";

    #[test]
    fn a_gbb_file_is_a_block_named_by_its_file_read_as_the_census_does() {
        let registry = Registry::load_embedded();
        let d = PlainCsvDecoder::new(registry, feed("gbb").unwrap());
        assert_eq!(d.id(), "csv");
        assert!(d.accepts("GasBBActualFlowStorage.CSV") && !d.accepts("x.json"));
        assert_eq!(d.feed().id, "gbb");
        let bytes = fixture("gbb_actual_flow_storage_last31.csv");
        let mut sink = Collect::default();
        let stats = decode(
            &d,
            GBB,
            "GasBBActualFlowStorageLast31.CSV",
            &bytes,
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!(
            (
                stats.blocks,
                stats.rows,
                stats.bad_rows,
                stats.rows_repeated
            ),
            (1, 2, 0, 0)
        );
        assert_eq!(stats.lines, 3);
        // The registry's gas table that claims the pair, under the gas
        // market, the block named by family and name.
        let (table, market, _, how) = &sink.blocks[0];
        assert_eq!(table, "GBB_ACTUAL_FLOW_STORAGE");
        assert_eq!(market, "gas");
        assert!(how.ends_with(":GBB:ACTUAL_FLOW_STORAGE_LAST31"), "{how}");
        assert!(!how.starts_with("derived"), "{how}");
        let batch = &sink.batches[0].batch;
        assert_eq!(batch.num_rows(), 2);
        let schema = batch.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        // Column names are the header's, normalised to `[A-Z0-9_]`, as the
        // census records them.
        assert_eq!(names[..3], ["GASDATE", "FACILITYNAME", "FACILITYID"]);
        assert_eq!(names[13], "LASTUPDATED");
        let transform = &sink.batches[0].transform;
        assert_eq!(transform.block_family, "GBB");
        assert_eq!(transform.source_columns[0], "GASDATE");
        // The date, in the MMS form whatever the file wrote, lands in the
        // declared table's typed column.
        assert_eq!(
            *schema.field(0).data_type(),
            arrow_schema::DataType::Date32,
            "{schema:?}"
        );
        assert_eq!(batch.column(0).null_count(), 0);
        // A block no table claims is derived, named family and name; strict
        // mode refuses it.
        let mut sink = Collect::default();
        let unknown = "https://www.nemweb.com.au/Reports/CURRENT/GBB/GasBBNoSuchThing.CSV";
        decode(
            &d,
            unknown,
            "GasBBNoSuchThing.CSV",
            &bytes,
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.blocks[0].0, "GBB_NO_SUCH_THING");
        assert_eq!(sink.blocks[0].3, "derived:GBB:NO_SUCH_THING");
        let mut strict = Collect::default();
        let e = decode(
            &d,
            unknown,
            "GasBBNoSuchThing.CSV",
            &bytes,
            &options(true, 0),
            None,
            &mut strict,
        )
        .unwrap_err();
        assert_eq!(e.code, "schema.unknown_table");
    }

    #[test]
    fn a_later_version_adds_only_the_rows_the_previous_one_lacked() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = dir.path().join("v1.csv");
        let bytes = fixture("gbb_actual_flow_storage_last31.csv");
        std::fs::write(&v1, &bytes).unwrap();
        // Version 2: the first row again, the second row's Demand revised
        // (GBB's LastUpdated is its revision marker, so it is compared like
        // every other column), and a new row.
        let text = String::from_utf8(bytes).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        let revised = lines[2].replace("17.687", "17.700");
        let added = lines[1].replace("2026/09/20", "2026/09/21");
        lines[2] = &revised;
        lines.push(&added);
        let v2 = lines.join("\n");
        let registry = Registry::load_embedded();
        let d = PlainCsvDecoder::new(registry, feed("gbb").unwrap());
        let mut sink = Collect::default();
        let stats = decode(
            &d,
            GBB,
            "GasBBActualFlowStorageLast31.CSV",
            v2.as_bytes(),
            &options(false, 0),
            Some(&v1),
            &mut sink,
        )
        .unwrap();
        assert_eq!((stats.rows, stats.rows_repeated, stats.bad_rows), (2, 1, 0));
        assert_eq!(sink.batches[0].batch.num_rows(), 2);
        // A previous version that cannot be read, or is not a CSV, fails
        // the entry and names the file.
        let mut sink = Collect::default();
        let e = decode(
            &d,
            GBB,
            "x.CSV",
            v2.as_bytes(),
            &options(false, 0),
            Some(&dir.path().join("none.csv")),
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.read_failed");
        assert!(e.file.as_deref().unwrap().ends_with("none.csv"));
        let empty = dir.path().join("empty.csv");
        std::fs::write(&empty, b"").unwrap();
        let e = decode(
            &d,
            GBB,
            "x.CSV",
            v2.as_bytes(),
            &options(false, 0),
            Some(&empty),
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.header_missing");
        assert!(e.file.as_deref().unwrap().ends_with("empty.csv"));
        // No previous version named: every row is new.
        let wem = PlainCsvDecoder::new(Registry::load_embedded(), feed("datafiles").unwrap());
        let stats = decode(
            &wem,
            "https://data.wa.aemo.com.au/public/public-data/datafiles/balancing-summary/balancing-summary-2022.csv",
            "balancing-summary-2022.csv",
            &fixture("wem_balancing_summary_2022.csv"),
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!(stats.rows_repeated, 0);
        assert!(stats.rows > 0);
        assert_eq!(sink.blocks.last().unwrap().0, "WEM_BALANCING_SUMMARY");
        assert_eq!(sink.blocks.last().unwrap().1, "wem");
    }

    #[test]
    fn rows_the_scanner_or_the_sink_refuse_are_quarantined_or_stop_the_file() {
        let registry = Registry::load_embedded();
        let d = PlainCsvDecoder::new(registry, feed("sttm").unwrap());
        let url = "https://www.nemweb.com.au/Reports/CURRENT/STTM/int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv";
        // A row with more fields than the header: the scanner refuses it.
        let text = "gas_date,hub,price\n01 Oct 2026,ADL,10.98\n01 Oct 2026,BRI,10.95,extra,more\n02 Oct 2026,SYD,9\n";
        let mut sink = Collect::default();
        let stats = decode(
            &d,
            url,
            "int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv",
            text.as_bytes(),
            &options(false, 5),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!((stats.rows, stats.bad_rows), (2, 1));
        assert_eq!(sink.bad.len(), 1);
        assert_eq!(sink.bad[0].2, "csv.field_count_mismatch");
        assert_eq!(sink.blocks[0].0, "STTM_INT651_EX_ANTE_MARKET_PRICE");
        // More than tolerated: the file fails.
        let mut sink = Collect::default();
        let e = decode(
            &d,
            url,
            "int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv",
            text.as_bytes(),
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.too_many_bad_rows");
        assert_eq!(e.line, Some(3));
        // An empty file has no header.
        let e = decode(
            &d,
            url,
            "int651_x~1.csv",
            b"\n\n",
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.header_missing");
        // A header field that names no column.
        let e = decode(
            &d,
            url,
            "int651_x.csv",
            b"a,***\n1,2\n",
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.header_malformed");
        assert_eq!(e.line, Some(1));
        // A name the feed's rule cannot read.
        let e = decode(
            &d,
            url,
            "notes.csv",
            b"a,b\n1,2\n",
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.unnamed_block");
        assert!(
            e.message.contains("sttm feed of the gas market"),
            "{}",
            e.message
        );
        // A record longer than the limit.
        let long = format!("a,b\n1,\"{}\n", "x".repeat(100));
        let mut c = Collect::default();
        let file = local(url, 0);
        let opts = options(false, 0);
        let ctx = DecodeCtx {
            file: &file,
            entry: "int651_x.csv",
            period: None,
            options: &opts,
            previous: None,
        };
        let name = name_of(d.feed, &ctx, "csv.unnamed_block", Category::Csv).unwrap();
        let small = PlainOptions {
            max_record_bytes: 16,
            ..super::options(d.feed, &ctx, &name.name)
        };
        let mut bridge = PlainBridge::new(&d.registry, d.market, &ctx, &mut c, name, None);
        let e = utilake_plain_decode::csv::scan(Cursor::new(long.as_bytes()), &mut bridge, &small)
            .unwrap_err();
        let diag = plain_diag(e, |f: SinkError| f.diagnostic());
        assert_eq!(diag.code, "csv.line_too_long");
        assert_eq!(diag.line, Some(2));
        // The sink stopping at a bad row ends the scan.
        let mut stopping = Collect {
            stop: true,
            ..Collect::default()
        };
        let e = decode(
            &d,
            url,
            "int651_x.csv",
            text.as_bytes(),
            &options(false, 5),
            None,
            &mut stopping,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.stopped_fail_fast");
        // A read that fails mid-file.
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("gone"))
            }
        }
        let mut sink = Collect::default();
        let file = local(url, 0);
        let ctx = DecodeCtx {
            file: &file,
            entry: "int651_x.csv",
            period: None,
            options: &opts,
            previous: None,
        };
        let e = d
            .decode(&mut Broken, &ctx, &mut sink, &mut Vec::new())
            .unwrap_err();
        assert_eq!(e.code, "csv.read_failed");
    }

    #[test]
    fn a_wemde_document_is_one_block_per_array() {
        let registry = Registry::load_embedded();
        let d = WemdeDecoder::new(registry, feed("wemde").unwrap());
        assert_eq!(d.id(), "json");
        assert!(d.accepts("ReferenceTradingPrice_20231001.json") && !d.accepts("a.csv"));
        let url = "https://data.wa.aemo.com.au/public/market-data/wemde/referenceTradingPrice/previous/ReferenceTradingPrice_20261021.zip";
        let bytes = fixture("wemde_reference_trading_price_2026-10-21.json");
        let mut sink = Collect::default();
        let stats = decode(
            &d,
            url,
            "ReferenceTradingPrice_20261021.json",
            &bytes,
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!((stats.blocks, stats.rows, stats.bad_rows), (1, 2, 0));
        assert_eq!(stats.bytes, bytes.len() as u64);
        let (table, market, _, how) = &sink.blocks[0];
        assert_eq!(
            table,
            "WEMDE_REFERENCE_TRADING_PRICE_REFERENCE_TRADING_PRICES"
        );
        assert_eq!(market, "wem");
        assert_eq!(
            how,
            "derived:WEMDE:REFERENCE_TRADING_PRICE_REFERENCE_TRADING_PRICES"
        );
        let batch = &sink.batches[0].batch;
        let schema = batch.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            names.contains(&"TRADING_INTERVAL") && names.contains(&"TRANSACTION_ID"),
            "{names:?}"
        );
        let i = names.iter().position(|n| *n == "TRADING_INTERVAL").unwrap();
        let col = batch
            .column(i)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(col.value(0), "2026/10/21 08:00:00");
        // Not JSON; not a WEMDE document; a name no rule reads.
        let mut sink = Collect::default();
        let e = decode(&d, url, "x.json", b"{", &options(false, 0), None, &mut sink).unwrap_err();
        assert_eq!(e.code, "json.syntax");
        let e = decode(
            &d,
            url,
            "x.json",
            b"[1,2]",
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "json.not_wemde");
        let e = decode(
            &d,
            "https://data.wa.aemo.com.au/other/x.json",
            "x.json",
            &bytes,
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "json.unnamed_block");
    }

    #[test]
    fn a_sniffed_csv_goes_to_the_decoder_its_bytes_name() {
        let registry = Registry::load_embedded();
        let d = SniffedCsv::new(registry, feed("gbb").unwrap());
        assert_eq!(d.id(), "csv");
        assert!(d.accepts("GasBBGSHGasTrades.CSV") && !d.accepts("a.zip"));
        let url = "https://www.nemweb.com.au/Reports/CURRENT/GBB/";
        // MMS rows: the I row names the block, in the gas market.
        let mms = b"C,GSH,TEST,1\nI,GSH,GAS_TRADES,1,TRADE_ID,PRICE\nD,GSH,GAS_TRADES,1,7,10.5\nC,\"END OF REPORT\",4\n";
        let mut sink = Collect::default();
        let stats = decode(
            &d,
            &format!("{url}GasBBGSHGasTrades.CSV"),
            "GasBBGSHGasTrades.CSV",
            mms,
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!((stats.blocks, stats.rows), (1, 1));
        assert_eq!(sink.blocks[0].0, "GAS_GSH_TRADES");
        assert_eq!(sink.blocks[0].1, "gas");
        // A header row: the file name names the block.
        let plain = fixture("gbb_actual_flow_storage_last31.csv");
        let mut sink = Collect::default();
        let stats = decode(
            &d,
            GBB,
            "GasBBActualFlowStorageLast31.CSV",
            &plain,
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!((stats.blocks, stats.rows), (1, 2));
        assert_eq!(sink.blocks[0].0, "GBB_ACTUAL_FLOW_STORAGE");
        // Markup: skipped with a warning, nothing read.
        let mut sink = Collect::default();
        let stats = decode(
            &d,
            &format!("{url}page.csv"),
            "page.csv",
            b"<html><body>no</body></html>",
            &options(false, 0),
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!(stats, DecodeStats::default());
        assert_eq!(sink.warnings.len(), 1);
        assert_eq!(sink.warnings[0].code, "csv.markup_skipped");
        assert!(sink.blocks.is_empty());
        // A reader that fails before the first bytes.
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("gone"))
            }
        }
        let file = local(GBB, 0);
        let opts = options(false, 0);
        let ctx = DecodeCtx {
            file: &file,
            entry: "x.csv",
            period: None,
            options: &opts,
            previous: None,
        };
        let e = d
            .decode(&mut Broken, &ctx, &mut sink, &mut Vec::new())
            .unwrap_err();
        assert_eq!(e.code, "csv.read_failed");
    }

    #[test]
    fn the_path_the_naming_rule_reads_joins_a_zip_entry() {
        let file = local("https://h/Reports/CURRENT/STTM/Day01.zip", 0);
        let opts = options(false, 0);
        let entry = |e: &'static str| DecodeCtx {
            file: &file,
            entry: e,
            period: None,
            options: &opts,
            previous: None,
        };
        assert_eq!(
            path_of(&entry("int651_v1_x~20261001.csv")),
            "/Reports/CURRENT/STTM/Day01.zip!int651_v1_x~20261001.csv"
        );
        assert_eq!(
            path_of(&entry("Day01.zip")),
            "/Reports/CURRENT/STTM/Day01.zip"
        );
        assert_eq!(path_of(&entry("")), "/Reports/CURRENT/STTM/Day01.zip");
        assert_eq!(market_of(feed("wemde").unwrap()), Market::Wem);
        assert_eq!(market_of(feed("gsh").unwrap()), Market::Gas);
    }
}
