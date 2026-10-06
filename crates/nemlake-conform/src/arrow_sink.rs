//! The sink that turns blocks into typed `RecordBatch`es.

use std::sync::Arc;

use std::collections::HashMap;
use std::fmt::Write as _;

use arrow_array::builder::{ArrayBuilder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use nemlake_core::{Category, Diagnose as _, Diagnostic, Fingerprint, IRow};
use nemlake_registry::{Cast, ConformancePlan, Mode, Registry, ResolveOptions, Resolved, TableId};

use nemlake_decode::PREFIX_FIELDS;
use nemlake_decode::error::{RowError, RowFailure, SinkError};
use nemlake_decode::fields::Row;
use nemlake_decode::scan::{Flow, Pos, Sink};
use nemlake_registry::arrow::RAW_OF_KEY;

use crate::builders::{ColumnBuilder, Fault, Parsed, parse_field};

/// A batch of rows from one block.
#[derive(Debug)]
pub struct BlockBatch {
    /// The table.
    pub table: TableId,
    /// The table's increment the plan was based on.
    pub version: u16,
    /// The plan the rows were conformed with.
    pub plan: Arc<ConformancePlan>,
    /// The observed fingerprint of the I row.
    pub fingerprint: Fingerprint,
    /// The rows, in the table's unified lake schema.
    pub batch: RecordBatch,
}

/// Receives batches and anything else the pipeline needs to know.
pub trait BatchSink {
    /// A batch is ready.
    fn batch(&mut self, b: BlockBatch) -> Result<(), SinkError>;
    /// A warning raised while resolving (a deviation, an unseen version).
    fn warning(&mut self, d: Diagnostic);
    /// A block began; `resolved` describes how it matched.
    fn block_started(&mut self, _irow: &IRow<'_>, _resolved: &Resolved, _pos: Pos) {}
    /// A quarantined row.
    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> Flow;
}

/// One route of the plan as the row loop reads it: the file field, the
/// output column, the cast. Kept small so a row's routes share cache lines.
#[derive(Clone, Copy)]
struct Step {
    /// Position of the field in the D row.
    field: u16,
    /// Output column.
    target: u16,
    cast: Cast,
    nullable: bool,
}

/// Values of one output column that did not fit its type since the last
/// flush, kept as text in a column of their own.
struct RawColumn {
    /// The output column whose values these are.
    target: u16,
    /// The raw column's name.
    name: String,
    /// One entry per row of the batch: the text, or null where the value
    /// fit.
    text: StringBuilder,
    /// How many values were kept so.
    count: u64,
    /// The first of them: its text, its line and the fault.
    first: (String, u64, Fault),
}

/// What is noted of a column per flush: how often, and the first instance.
#[derive(Default)]
struct Tally {
    count: u64,
    first: Option<(String, u64)>,
}

impl Tally {
    fn note(&mut self, sample: impl FnOnce() -> String, line: u64) {
        self.count += 1;
        if self.first.is_none() {
            self.first = Some((sample(), line));
        }
    }
}

struct Block {
    resolved: Resolved,
    /// The plan's routes that feed a column, in file order.
    steps: Vec<Step>,
    /// Output columns that no file column feeds and that do not allow
    /// nulls: every row leaves them null all the same.
    missing_required: Vec<u16>,
    fingerprint: Fingerprint,
    builders: Vec<ColumnBuilder>,
    report_type: String,
    report_subtype: String,
    version_text: String,
    rows: usize,
    /// Per output column: timestamps clamped since the last flush.
    clamped: Vec<Tally>,
    /// Per output column: nulls kept in a column that does not allow
    /// them since the last flush. Such a column is nullable in the batch.
    nulls_kept: Vec<Tally>,
    /// Columns with values kept as text since the last flush, in the order
    /// they first had one.
    raws: Vec<RawColumn>,
}

/// The allocations of a [`Block`], carried to the next block of a file.
#[derive(Default)]
struct BlockBuffers {
    steps: Vec<Step>,
    missing_required: Vec<u16>,
    builders: Vec<ColumnBuilder>,
    report_type: String,
    report_subtype: String,
    version_text: String,
    clamped: Vec<Tally>,
    nulls_kept: Vec<Tally>,
    raws: Vec<RawColumn>,
}

impl From<Block> for BlockBuffers {
    fn from(b: Block) -> Self {
        Self {
            steps: b.steps,
            missing_required: b.missing_required,
            builders: b.builders,
            report_type: b.report_type,
            report_subtype: b.report_subtype,
            version_text: b.version_text,
            clamped: b.clamped,
            nulls_kept: b.nulls_kept,
            raws: b.raws,
        }
    }
}

/// Builds typed batches from MMS rows through the registry.
///
/// A row is never refused for one of its values. A value that does not fit
/// its column's type leaves the column null in that row, and its text is
/// kept in a column of its own, `{column}__raw`, which exists only in the
/// batches that need it; an empty value in a column that does not allow
/// nulls is kept as a null. Each is reported once per column per batch
/// (`parse.value_kept_raw`, `parse.null_kept`). In strict mode either
/// refuses the file instead. Only a row whose structure is wrong for its
/// block (too many fields, another block's prefix) is quarantined.
pub struct ArrowSink<'r, B: BatchSink> {
    registry: &'r Registry,
    opts: ResolveOptions,
    /// The month of the source being read, when it is known: it decides
    /// the increment a block is read against.
    period: Option<nemlake_core::YearMonth>,
    batch_rows: usize,
    out: B,
    block: Option<Block>,
    parsed: Vec<Parsed>,
    /// The values of the current row that did not fit: output column,
    /// field position, fault.
    misfits: Vec<(u16, u16, Fault)>,
    /// The shapes (plan and I-row version) whose resolution warnings were
    /// passed on: a file of many blocks of one shape says them once.
    warned: Vec<(nemlake_registry::PlanHash, u16)>,
}

/// Rows a new column builder makes room for. Builders grow as rows come;
/// a block of a few rows (most blocks of a file of many tables) must not
/// pay for touching room for thousands.
const FIRST_ROWS: usize = 256;

/// Rows per batch by default.
pub const DEFAULT_BATCH_ROWS: usize = 65_536;

/// What a column of `ty` expects, in words.
fn expected(ty: &DataType) -> String {
    match ty {
        DataType::Float64 => "a number (Float64)".into(),
        DataType::Int64 => "a whole number (Int64)".into(),
        DataType::Timestamp(..) => "a timestamp yyyy/mm/dd hh24:mi:ss".into(),
        DataType::Date32 => "a date yyyy/mm/dd".into(),
        DataType::Boolean => "a boolean".into(),
        _ => "UTF-8 text".into(),
    }
}

impl<'r, B: BatchSink> ArrowSink<'r, B> {
    /// A sink resolving against `registry`, flushing every `batch_rows` rows.
    pub fn new(registry: &'r Registry, opts: ResolveOptions, batch_rows: usize, out: B) -> Self {
        Self {
            registry,
            opts,
            period: None,
            batch_rows: batch_rows.max(1),
            out,
            block: None,
            parsed: Vec::new(),
            misfits: Vec::new(),
            warned: Vec::new(),
        }
    }

    /// Says which month the source being read is of, so that each block
    /// is read against the increment in force then.
    #[must_use]
    pub fn with_period(mut self, period: Option<nemlake_core::YearMonth>) -> Self {
        self.period = period;
        self
    }

    /// Takes the inner sink back.
    #[cfg(test)]
    pub(crate) fn into_inner(mut self) -> Result<B, SinkError> {
        self.flush()?;
        Ok(self.out)
    }

    /// Flushes and takes the inner sink back, keeping the sink even when the
    /// final flush failed so the caller can abort what it had opened.
    pub fn finish(mut self) -> (B, Result<(), SinkError>) {
        let r = self.flush();
        (self.out, r)
    }

    /// The inner sink, mutably.
    #[cfg(test)]
    pub(crate) fn inner_mut(&mut self) -> &mut B {
        &mut self.out
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        let Some(block) = self.block.as_mut() else {
            return Ok(());
        };
        if block.rows == 0 {
            return Ok(());
        }
        let plan = &block.resolved.plan;
        let table = self.registry.table(block.resolved.table);
        let warn = |code: &'static str, column: &str, message: String| {
            Diagnostic::warning(code, Category::Parse, message)
                .with_table(table.name.clone())
                .with_version(block.resolved.version)
                .with_column(column.to_owned())
        };
        // The plan's schema serves every batch that needs no change to it;
        // a copy is made only for one that does (a column made nullable, a
        // raw column added).
        let lake = &plan.arrow;
        let mut changed: Option<Vec<Field>> = None;
        fn own<'c>(c: &'c mut Option<Vec<Field>>, lake: &Schema) -> &'c mut Vec<Field> {
            c.get_or_insert_with(|| lake.fields().iter().map(|f| f.as_ref().clone()).collect())
        }
        let mut arrays: Vec<ArrayRef> = block
            .builders
            .iter_mut()
            .map(ColumnBuilder::finish)
            .collect();
        // Timestamps beyond the nanosecond range were stored at its nearest
        // end; say so once per column, with how many and an example.
        for (i, tally) in block.clamped.iter_mut().enumerate() {
            let Some((sample, _)) = tally.first.take() else {
                continue;
            };
            let column = lake.field(i).name();
            let mut d = warn(
                "parse.timestamp_clamped",
                column,
                format!(
                    "{} value(s) of column {column} in table {} lie beyond what nanoseconds hold (1677-09-21 to 2262-04-11) and were stored as the nearest end of that range. AEMO writes such dates to mean no end or no start; the downloaded file keeps the original.",
                    tally.count, table.name
                ),
            );
            d.observed = Some(sample);
            d.expected = Some("1677-09-21 to 2262-04-11".into());
            self.out.warning(d);
            tally.count = 0;
        }
        // A column that does not allow nulls, with nulls kept in it, allows
        // them in this batch.
        for (i, tally) in block.nulls_kept.iter_mut().enumerate() {
            let Some((_, line)) = tally.first.take() else {
                continue;
            };
            let column = lake.field(i).name();
            let mut d = warn(
                "parse.null_kept",
                column,
                format!(
                    "{} row(s) of table {} have no value for column {column}, which does not allow nulls; the rows are kept with the column null. The first is line {line}.",
                    tally.count, table.name
                ),
            );
            d.line = Some(line);
            d.expected = Some("a value".into());
            d.observed = Some(String::new());
            self.out.warning(d);
            let f = &mut own(&mut changed, lake)[i];
            *f = f.clone().with_nullable(true);
            tally.count = 0;
        }
        // Values that did not fit their column's type: the text beside it.
        for raw in block.raws.drain(..) {
            // The column is null where its value did not fit.
            let i = raw.target as usize;
            let f = &mut own(&mut changed, lake)[i];
            *f = f.clone().with_nullable(true);
            let target = lake.field(i);
            let column = target.name();
            let (sample, line, fault) = raw.first;
            let fault_text = fault.row_error(column, sample.as_bytes()).to_string();
            let mut d = warn(
                "parse.value_kept_raw",
                column,
                format!(
                    "{} value(s) of column {column} in table {} do not fit its type; each row is kept with the column null and the text in column {}. The first is line {line}: {fault_text}",
                    raw.count, table.name, raw.name
                ),
            );
            d.line = Some(line);
            d.expected = Some(expected(target.data_type()));
            d.observed = Some(sample);
            self.out.warning(d);
            let mut text = raw.text;
            let of = target
                .metadata()
                .get(nemlake_registry::arrow::FIELD_ID_KEY)
                .cloned()
                .unwrap_or_else(|| column.clone());
            let md: HashMap<String, String> = [(RAW_OF_KEY.to_owned(), of)].into();
            own(&mut changed, lake)
                .push(Field::new(raw.name, DataType::Utf8, true).with_metadata(md));
            arrays.push(Arc::new(text.finish()));
        }
        let schema = match changed {
            Some(fields) => Arc::new(Schema::new_with_metadata(fields, lake.metadata().clone())),
            None => Arc::clone(lake),
        };
        let batch = RecordBatch::try_new(schema, arrays).map_err(|e| SinkError::Arrow {
            table: table.name.clone(),
            message: e.to_string(),
        })?;
        let out = BlockBatch {
            table: block.resolved.table,
            version: block.resolved.version,
            plan: block.resolved.plan.clone(),
            fingerprint: block.fingerprint,
            batch,
        };
        block.rows = 0;
        self.out.batch(out)
    }
}

impl<B: BatchSink> Sink for ArrowSink<'_, B> {
    fn comment(&mut self, _row: &Row<'_>, _pos: Pos) -> Result<(), SinkError> {
        Ok(())
    }

    fn header(&mut self, irow: &IRow<'_>, pos: Pos) -> Result<(), SinkError> {
        self.flush()?;
        let resolved = self
            .registry
            .resolve_in(irow, self.opts, self.period)
            .map_err(|e| SinkError::Refused(Box::new(e.diagnostic())))?;
        let shape = (resolved.plan_hash(), irow.version);
        if !resolved.warnings.is_empty() && !self.warned.contains(&shape) {
            self.warned.push(shape);
            for w in &resolved.warnings {
                self.out.warning(w.clone());
            }
        }
        self.out.block_started(irow, &resolved, pos);
        // A file of many blocks reuses the buffers of the block before:
        // only the first I row of a file allocates them, and a block of the
        // same shape keeps its column builders as well.
        let old = self.block.take();
        let same_shape = old
            .as_ref()
            .is_some_and(|b| Arc::ptr_eq(&b.resolved.plan, &resolved.plan));
        let mut b = old.map(BlockBuffers::from).unwrap_or_default();
        let schema = &resolved.plan.arrow;
        let width = schema.fields().len();
        if !same_shape {
            b.builders.clear();
            b.builders.extend(
                schema
                    .fields()
                    .iter()
                    .map(|f| ColumnBuilder::new(f.data_type(), self.batch_rows.min(FIRST_ROWS))),
            );
        }
        self.parsed.clear();
        self.parsed.resize(width, Parsed::Null);
        b.steps.clear();
        b.steps.extend(resolved.plan.routes.iter().filter_map(|r| {
            Some(Step {
                field: (PREFIX_FIELDS + usize::from(r.source_index)) as u16,
                target: r.target?,
                cast: r.cast,
                nullable: r.nullable,
            })
        }));
        b.missing_required.clear();
        b.missing_required.extend(
            resolved
                .plan
                .missing
                .iter()
                .copied()
                .filter(|&m| !schema.field(m as usize).is_nullable()),
        );
        for tallies in [&mut b.clamped, &mut b.nulls_kept] {
            tallies.clear();
            tallies.resize_with(width, Tally::default);
        }
        b.raws.clear();
        for (to, from) in [
            (&mut b.report_type, irow.report_type),
            (&mut b.report_subtype, irow.report_subtype),
        ] {
            to.clear();
            to.push_str(from);
        }
        b.version_text.clear();
        write!(b.version_text, "{}", irow.version).expect("writing to a String");
        self.block = Some(Block {
            steps: b.steps,
            missing_required: b.missing_required,
            fingerprint: irow.fingerprint(),
            builders: b.builders,
            report_type: b.report_type,
            report_subtype: b.report_subtype,
            version_text: b.version_text,
            rows: 0,
            clamped: b.clamped,
            nulls_kept: b.nulls_kept,
            raws: b.raws,
            resolved,
        });
        Ok(())
    }

    #[inline]
    fn row(&mut self, row: &Row<'_>, pos: Pos) -> Result<(), RowFailure> {
        let block = self
            .block
            .as_mut()
            .expect("the scanner delivers rows only after a header");
        if row.len() < PREFIX_FIELDS {
            return Err(RowError::TooShort(row.len()).into());
        }
        // The prefix must match the block: a cheap byte compare.
        if row.get(1) != Some(block.report_type.as_bytes())
            || row.get(2) != Some(block.report_subtype.as_bytes())
            || row.get(3).map(<[u8]>::trim_ascii) != Some(block.version_text.as_bytes())
        {
            return Err(RowError::BlockMismatch {
                report_type: String::from_utf8_lossy(row.get(1).unwrap_or(b"")).into_owned(),
                report_subtype: String::from_utf8_lossy(row.get(2).unwrap_or(b"")).into_owned(),
                version: String::from_utf8_lossy(row.get(3).unwrap_or(b"")).into_owned(),
                block_type: block.report_type.clone(),
                block_subtype: block.report_subtype.clone(),
                block_version: block.resolved.version,
            }
            .into());
        }
        let declared = block.resolved.plan.routes.len();
        let observed = row.len_trimmed().saturating_sub(PREFIX_FIELDS);
        if observed > declared {
            return Err(RowError::FieldCount {
                declared,
                observed: row.len() - PREFIX_FIELDS,
            }
            .into());
        }
        let strict = self.opts.mode == Mode::Strict;
        // A field that does not fit is a value fault, not a row fault: the
        // row is kept. Strict mode refuses the file at the first one.
        let refuse = |block: &Block, target: u16, field: u16, fault: Fault| {
            let column = block.resolved.plan.arrow.field(target as usize).name();
            let bytes = row.get(usize::from(field)).unwrap_or(b"");
            let d = fault
                .row_error(column, bytes)
                .diagnostic()
                .at(pos.line, pos.byte_offset)
                .with_sample(bytes)
                .with_table(block.resolved.plan.table.clone())
                .with_version(block.resolved.version);
            RowFailure::from(SinkError::Refused(Box::new(d)))
        };
        // Pass 1: parse every field; nothing is appended until all are read.
        self.parsed.fill(Parsed::Null);
        self.misfits.clear();
        for c in &block.steps {
            let target = c.target;
            let bytes = row.get(usize::from(c.field)).unwrap_or(b"");
            let parsed = match parse_field(c.cast, c.nullable, c.field, bytes) {
                Ok(p) => p,
                Err(fault) if strict => return Err(refuse(block, target, c.field, fault)),
                Err(fault) => {
                    self.misfits.push((target, c.field, fault));
                    Parsed::Null
                }
            };
            if matches!(parsed, Parsed::Clamped(_)) {
                block.clamped[target as usize]
                    .note(|| String::from_utf8_lossy(bytes).into_owned(), pos.line);
            }
            self.parsed[target as usize] = parsed;
        }
        // A column the file does not carry is null, also where the lake
        // does not allow it.
        if let Some(&m) = block.missing_required.first() {
            if strict {
                let column = block.resolved.plan.arrow.field(m as usize).name();
                let d = RowError::NullNotAllowed {
                    column: column.clone(),
                }
                .diagnostic()
                .at(pos.line, pos.byte_offset)
                .with_table(block.resolved.plan.table.clone())
                .with_version(block.resolved.version);
                return Err(SinkError::Refused(Box::new(d)).into());
            }
            for &m in &block.missing_required {
                block.nulls_kept[m as usize].note(String::new, pos.line);
            }
        }
        // Pass 2: append.
        for (b, v) in block.builders.iter_mut().zip(&self.parsed) {
            match v {
                Parsed::Str(i) => {
                    // SAFETY-free: the bytes live in `row`, which outlives this call.
                    let bytes = row.get(usize::from(*i)).unwrap_or(b"");
                    match b {
                        ColumnBuilder::Utf8(sb) => {
                            sb.append_value(std::str::from_utf8(bytes).unwrap_or(""));
                        }
                        _ => unreachable!("Str is only parsed for Utf8 columns"),
                    }
                }
                other => b.append(*other, |_| b""),
            }
        }
        if !self.misfits.is_empty() || !block.raws.is_empty() {
            keep_misfits(block, &self.misfits, row, pos.line);
        }
        block.rows += 1;
        if block.rows >= self.batch_rows {
            self.flush()?;
        }
        Ok(())
    }

    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> Flow {
        self.out.bad_row(raw, pos, err)
    }

    fn footer(
        &mut self,
        _declared: Option<u64>,
        _lines_read: u64,
        _pos: Pos,
    ) -> Result<(), SinkError> {
        self.flush()
    }
}

/// Records the current row's misfits: an empty value where nulls are not
/// allowed is a kept null; any other value goes, as text, to its column's
/// raw column, which is started (null for the rows before) on its first
/// value. Every raw column gets one entry for the row.
fn keep_misfits(block: &mut Block, misfits: &[(u16, u16, Fault)], row: &Row<'_>, line: u64) {
    let rows_before = block.rows;
    for &(target, field, fault) in misfits {
        let bytes = row.get(usize::from(field)).unwrap_or(b"");
        if fault == Fault::Null {
            block.nulls_kept[target as usize].note(String::new, line);
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        let raw = match block.raws.iter().position(|r| r.target == target) {
            Some(i) => &mut block.raws[i],
            None => {
                let plan = &block.resolved.plan;
                let column = plan.column_name(target as usize);
                let mut name = format!("{column}__raw");
                // A column of the file may already have the name.
                if plan.fields.iter().any(|f| f.name == name) {
                    name.push_str("__2");
                }
                let mut text_column = StringBuilder::new();
                text_column.append_nulls(rows_before);
                block.raws.push(RawColumn {
                    target,
                    name,
                    text: text_column,
                    count: 0,
                    first: (text.clone().into_owned(), line, fault),
                });
                block.raws.last_mut().expect("just pushed")
            }
        };
        raw.text.append_value(&text);
        raw.count += 1;
    }
    // Raw columns of the batch without a value in this row are null for it.
    for raw in &mut block.raws {
        if raw.text.len() == rows_before {
            raw.text.append_null();
        }
    }
}

/// A `BatchSink` that keeps everything in memory, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct CollectBatches {
    /// Batches in order.
    pub batches: Vec<BlockBatch>,
    /// Warnings in order.
    pub warnings: Vec<Diagnostic>,
    /// Quarantined rows: line, raw, diagnostic.
    pub bad: Vec<(u64, Vec<u8>, Diagnostic)>,
    /// Blocks started: table name, version, status label.
    pub blocks: Vec<(String, u16, &'static str)>,
    /// Stop at the first bad row.
    pub fail_fast: bool,
}

#[cfg(test)]
impl BatchSink for CollectBatches {
    fn batch(&mut self, b: BlockBatch) -> Result<(), SinkError> {
        self.batches.push(b);
        Ok(())
    }
    fn warning(&mut self, d: Diagnostic) {
        self.warnings.push(d);
    }
    fn block_started(&mut self, _irow: &IRow<'_>, resolved: &Resolved, _pos: Pos) {
        self.blocks
            .push((String::new(), resolved.version, resolved.status.label()));
    }
    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> Flow {
        self.bad.push((
            pos.line,
            raw.to_vec(),
            err.diagnostic()
                .at(pos.line, pos.byte_offset)
                .with_sample(raw),
        ));
        if self.fail_fast {
            Flow::Stop
        } else {
            Flow::Continue
        }
    }
}

/// Scans `reader` through an [`ArrowSink`] into [`CollectBatches`].
#[cfg(test)]
pub(crate) fn collect<R: std::io::Read>(
    registry: &Registry,
    reader: R,
    opts: ResolveOptions,
    scan_opts: &nemlake_decode::scan::ScanOptions,
) -> Result<(CollectBatches, nemlake_decode::scan::ScanStats), nemlake_decode::error::ScanError> {
    let mut sink = ArrowSink::new(
        registry,
        opts,
        DEFAULT_BATCH_ROWS,
        CollectBatches::default(),
    );
    let stats = nemlake_decode::scan::scan(reader, &mut sink, scan_opts)?;
    let out = sink.into_inner()?;
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use arrow_array::{Array, Float64Array, StringArray, TimestampNanosecondArray};
    use nemlake_registry::Mode;

    use super::*;
    use nemlake_decode::scan::ScanOptions;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/csv/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn reg() -> Arc<Registry> {
        Registry::load_embedded()
    }

    #[test]
    fn builds_typed_batch_from_unit_scada() {
        let r = reg();
        let (out, stats) = collect(
            &r,
            fixture("DISPATCH_UNIT_SCADA_v1_head.csv").as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 20);
        assert_eq!(out.batches.len(), 1);
        let b = &out.batches[0].batch;
        assert_eq!(b.num_rows(), 20);
        // The file's three columns, and the one a later month added, null.
        assert_eq!(b.num_columns(), 4);
        assert_eq!(b.column(3).null_count(), 20);
        let ts = b
            .column(0)
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!(ts.timezone(), Some("+10:00"));
        let expected = nemlake_core::nemtime::parse_nanos(b"2024/01/01 00:05:00", 36_000)
            .unwrap()
            .0;
        assert_eq!(ts.value(0), expected);
        let duid = b.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(duid.value(0), "BUTLERSG");
        let mw = b.column(2).as_any().downcast_ref::<Float64Array>().unwrap();
        assert!((mw.value(0) - 9.199_999).abs() < 1e-9);
        assert_eq!(out.blocks, vec![(String::new(), 1, "conforms")]);
        assert!(out.warnings.is_empty());
        assert_eq!(
            b.schema().metadata()["nemlake:table"],
            "DISPATCH_UNIT_SCADA"
        );
    }

    #[test]
    fn every_fixture_parses_without_bad_rows() {
        let r = reg();
        let dir = format!("{}/../../testdata/csv", env!("CARGO_MANIFEST_DIR"));
        let mut tables = 0;
        for e in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "csv")
                || p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("multi_block"))
            {
                continue;
            }
            let (out, stats) = collect(
                &r,
                std::fs::read(&p).unwrap().as_slice(),
                ResolveOptions::default(),
                &ScanOptions::default(),
            )
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert_eq!(stats.bad_rows, 0, "{}: {:?}", p.display(), out.bad.first());
            assert_eq!(
                out.batches
                    .iter()
                    .map(|b| b.batch.num_rows() as u64)
                    .sum::<u64>(),
                stats.rows,
                "{}",
                p.display()
            );
            tables += 1;
        }
        assert!(tables >= 20);
    }

    /// The multi-block fixture with one more block put in first, right
    /// after the file header: a block for a report pair no table claims.
    fn with_unknown_block() -> Vec<u8> {
        let text = String::from_utf8(fixture("multi_block_dispatchis.csv")).unwrap();
        let after_header = text.find('\n').unwrap() + 1;
        format!(
            "{}I,NEMLAKE_TEST,NEW_THING,1,WHEN,AMOUNT\nD,NEMLAKE_TEST,NEW_THING,1,\"2026/10/01 13:40:00\",1.5\nD,NEMLAKE_TEST,NEW_THING,1,\"2026/10/01 13:45:00\",2.5\n{}",
            &text[..after_header],
            &text[after_header..]
        )
        .into_bytes()
    }

    #[test]
    fn a_block_no_table_claims_is_kept_as_text_unless_strict() {
        let r = reg();
        // Strict mode refuses what the registry does not declare.
        let e = collect(
            &r,
            with_unknown_block().as_slice(),
            ResolveOptions { mode: Mode::Strict },
            &ScanOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(
                &e,
                nemlake_decode::error::ScanError::Sink(SinkError::Refused(d))
                    if d.code == "schema.unknown_table"
            ),
            "{e}"
        );
        // Otherwise nothing is skipped: the block becomes a table derived
        // from its own header, every column text, and a warning says so.
        let (out, stats) = collect(
            &r,
            with_unknown_block().as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.bad_rows, 0);
        let derived: Vec<_> = out
            .batches
            .iter()
            .filter(|b| r.table(b.table).name == "NEMLAKE_TEST_NEW_THING")
            .collect();
        assert_eq!(derived.len(), 1);
        let b = derived[0];
        assert!(r.is_derived(b.table));
        assert_eq!(r.table_dir(b.table), "_unregistered/nemlake_test.new_thing");
        assert_eq!(b.batch.num_rows(), 2);
        assert_eq!(b.batch.num_columns(), 2);
        let text = |col: usize, row: usize| {
            b.batch
                .column(col)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(row)
                .to_owned()
        };
        assert_eq!(text(0, 0), "2026/10/01 13:40:00");
        assert_eq!(text(1, 1), "2.5");
        assert!(
            out.warnings.iter().any(|w| w.code == "schema.unknown_table"
                && w.severity == nemlake_core::Severity::Warning
                && w.message.contains("_unregistered/nemlake_test.new_thing")),
            "{:?}",
            out.warnings
        );
        // The blocks after it are read as they always were.
        let price_rows: usize = out
            .batches
            .iter()
            .filter(|b| r.table(b.table).name == "DISPATCHPRICE")
            .map(|b| b.batch.num_rows())
            .sum();
        assert_eq!(price_rows, 3);
        assert_eq!(
            stats.rows as usize,
            out.batches
                .iter()
                .map(|b| b.batch.num_rows())
                .sum::<usize>()
        );
    }

    #[test]
    fn timestamps_keep_their_fraction_and_far_dates_are_clamped_and_reported() {
        let r = reg();
        let csv = "I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
                   D,DISPATCH,UNIT_SCADA,1,\"9999/12/31 00:00:00\",A,1\n\
                   D,DISPATCH,UNIT_SCADA,1,\"2999/12/31 00:00:00\",B,2\n\
                   D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00.250\",C,3\n";
        let (out, stats) = collect(
            &r,
            csv.as_bytes(),
            ResolveOptions { mode: Mode::Strict },
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 3);
        assert_eq!(stats.bad_rows, 0);
        let ts = out.batches[0]
            .batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!(ts.value(0), i64::MAX);
        assert_eq!(ts.value(1), i64::MAX);
        let whole = nemlake_core::nemtime::parse_nanos(b"2024/01/01 00:05:00", 36_000)
            .unwrap()
            .0;
        assert_eq!(ts.value(2), whole + 250_000_000);
        // One warning for the column, with how many and the first of them.
        let clamped: Vec<_> = out
            .warnings
            .iter()
            .filter(|w| w.code == "parse.timestamp_clamped")
            .collect();
        assert_eq!(clamped.len(), 1, "{:?}", out.warnings);
        let w = clamped[0];
        assert!(
            w.message.starts_with("2 value(s) of column SETTLEMENTDATE"),
            "{}",
            w.message
        );
        assert_eq!(w.observed.as_deref(), Some("9999/12/31 00:00:00"));
        assert_eq!(w.column.as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(w.table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
    }

    #[test]
    fn deviations_keep_what_the_file_carries_unless_strict() {
        // DISPATCHPRICE in the live feed is version 5.
        let r = reg();
        let text = fixture("multi_block_dispatchis.csv");
        let lines: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
        let price: Vec<u8> = lines
            .iter()
            .filter(|l| l.starts_with(b"I,DISPATCH,PRICE") || l.starts_with(b"D,DISPATCH,PRICE"))
            .flat_map(|l| l.iter().copied().chain(std::iter::once(b'\n')))
            .collect();
        let (out, stats) = collect(
            &r,
            price.as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 3);
        // The live feed's DISPATCHPRICE lays the archive's columns out in
        // another order: kept, and flagged.
        assert_eq!(out.blocks[0].2, "deviates");
        assert!(out.warnings.iter().any(|w| w.code == "schema.deviates"));
        // Strict mode with a hand-made unknown version.
        let rows = b"I,DISPATCH,UNIT_SCADA,7,SETTLEMENTDATE,DUID,SCADAVALUE,EXTRA\nD,DISPATCH,UNIT_SCADA,7,\"2024/01/01 00:05:00\",X,1,z\n";
        let e = collect(
            &r,
            rows.as_slice(),
            ResolveOptions { mode: Mode::Strict },
            &ScanOptions::default(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("not listed [\"EXTRA\"]"), "{e}");
        let (out, stats) = collect(
            &r,
            rows.as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 1);
        assert_eq!(out.blocks[0].2, "deviates");
        assert!(out.warnings.iter().any(|w| w.code == "schema.deviates"));
        assert!(
            out.warnings
                .iter()
                .any(|w| w.code == "schema.unseen_version")
        );
        // The column the table does not have is kept, as text.
        let batch = &out.batches[0].batch;
        assert_eq!(batch.num_columns(), 5);
        assert_eq!(batch.schema().field(4).name(), "EXTRA");
        let extra = batch
            .column(4)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(extra.value(0), "z");
        // A file that lacks a key column is kept too, the key null.
        let keyless = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,SCADAVALUE\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",1\n";
        let (out, stats) = collect(
            &r,
            keyless.as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 1);
        assert_eq!(stats.bad_rows, 0);
        assert_eq!(out.batches[0].batch.column(1).null_count(), 1);
        assert!(out.warnings.iter().any(|w| w.code == "schema.missing_key"));
    }

    #[test]
    fn only_rows_of_the_wrong_structure_are_quarantined() {
        let r = reg();
        let rows = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",D,1,extra\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",E,1,,\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",F\n\
D,DISPATCH,PRICE,1,\"2024/01/01 00:05:00\",G,1\n\
D,X\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",H,2\n";
        let (out, stats) = collect(
            &r,
            rows.as_slice(),
            ResolveOptions::default(),
            &ScanOptions {
                max_bad_rows: 100,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            stats.rows, 4,
            "A, E (trailing empties), F (short row → null), H"
        );
        assert_eq!(stats.bad_rows, 3);
        let codes: Vec<_> = out.bad.iter().map(|(_, _, d)| d.code.to_string()).collect();
        assert_eq!(
            codes,
            [
                "csv.field_count_mismatch",
                "csv.block_mismatch",
                "csv.row_too_short"
            ]
        );
        assert_eq!(out.bad[0].0, 3);
        assert!(out.bad[0].2.sample.as_ref().unwrap().text.contains("extra"));
        let b = &out.batches[0].batch;
        assert_eq!(b.num_rows(), 4);
        assert_eq!(
            b.column(2).null_count(),
            1,
            "F's missing SCADAVALUE is null"
        );
        // Fail-fast stops at the first bad row.
        let mut sink = ArrowSink::new(
            &r,
            ResolveOptions::default(),
            8,
            CollectBatches {
                fail_fast: true,
                ..Default::default()
            },
        );
        let e = nemlake_decode::scan::scan(
            rows.as_slice(),
            &mut sink,
            &ScanOptions {
                max_bad_rows: 100,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            e,
            nemlake_decode::error::ScanError::Stopped { line: 3 }
        ));
        assert_eq!(sink.inner_mut().bad.len(), 1);
        sink.inner_mut().bad.clear();
    }

    const MISFITS: &[u8] = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/13/01 00:05:00\",B,1\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",C,notanumber\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",,1\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",D,also bad\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",E,2\n";

    #[test]
    fn a_value_that_does_not_fit_keeps_its_row_and_its_text() {
        let r = reg();
        let (out, stats) = collect(
            &r,
            MISFITS,
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 6);
        assert_eq!(stats.bad_rows, 0);
        assert!(out.bad.is_empty());
        let b = &out.batches[0].batch;
        assert_eq!(b.num_rows(), 6);
        // The four declared columns, then the raw text of the two whose
        // values did not fit, in the order they first failed.
        let names: Vec<_> = b
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert_eq!(
            names,
            [
                "SETTLEMENTDATE",
                "DUID",
                "SCADAVALUE",
                "LASTCHANGED",
                "SETTLEMENTDATE__raw",
                "SCADAVALUE__raw"
            ]
        );
        let text = |col: usize| {
            let a = b
                .column(col)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            (0..a.len())
                .map(|i| (!a.is_null(i)).then(|| a.value(i).to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            text(4),
            [
                None,
                Some("2024/13/01 00:05:00".into()),
                None,
                None,
                None,
                None
            ]
        );
        assert_eq!(
            text(5),
            [
                None,
                None,
                Some("notanumber".into()),
                None,
                Some("also bad".into()),
                None
            ]
        );
        assert_eq!(
            b.schema().field(5).metadata()[RAW_OF_KEY],
            b.schema().field(2).metadata()[nemlake_registry::arrow::FIELD_ID_KEY]
        );
        // The typed columns are null where the value did not fit, and the
        // key column allows it in this batch.
        assert!(b.column(0).is_null(1));
        assert!(b.schema().field(0).is_nullable());
        assert!(b.column(1).is_null(3));
        assert!(b.schema().field(1).is_nullable());
        assert_eq!(b.column(2).null_count(), 2);
        // One warning per column, with how many and the first.
        let codes: Vec<_> = out.warnings.iter().map(|w| w.code.to_string()).collect();
        assert_eq!(
            codes,
            [
                "parse.null_kept",
                "parse.value_kept_raw",
                "parse.value_kept_raw"
            ]
        );
        let scada = &out.warnings[2];
        assert_eq!(scada.column.as_deref(), Some("SCADAVALUE"));
        assert_eq!(scada.line, Some(4));
        assert_eq!(scada.observed.as_deref(), Some("notanumber"));
        assert_eq!(scada.expected.as_deref(), Some("a number (Float64)"));
        assert!(
            scada.message.starts_with("2 value(s) of column SCADAVALUE"),
            "{}",
            scada.message
        );
        assert_eq!(out.warnings[0].column.as_deref(), Some("DUID"));
        assert_eq!(out.warnings[0].line, Some(5));
    }

    /// A table that declares `{column}__raw` gets the text there, under
    /// the column's own id, and no column of the batch alone.
    #[test]
    fn raw_columns_exist_only_in_the_batches_that_need_them() {
        let r = reg();
        let mut sink = ArrowSink::new(&r, ResolveOptions::default(), 2, CollectBatches::default());
        nemlake_decode::scan::scan(MISFITS, &mut sink, &ScanOptions::default()).unwrap();
        let out = sink.into_inner().unwrap();
        let widths: Vec<_> = out.batches.iter().map(|b| b.batch.num_columns()).collect();
        // Rows A,B: a bad date. C,D: a bad number and an empty key. Bad
        // number, then E: a bad number only.
        assert_eq!(widths, [5, 5, 5]);
        assert_eq!(
            out.batches[0].batch.schema().field(4).name(),
            "SETTLEMENTDATE__raw"
        );
        assert_eq!(
            out.batches[1].batch.schema().field(4).name(),
            "SCADAVALUE__raw"
        );
        assert!(out.batches[1].batch.schema().field(1).is_nullable());
        assert!(!out.batches[2].batch.schema().field(1).is_nullable());
        let tail = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1\n";
        let (out, _) = collect(
            &r,
            tail.as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(out.batches[0].batch.num_columns(), 4);
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn strict_mode_refuses_a_value_that_does_not_fit() {
        let r = reg();
        let strict = ResolveOptions { mode: Mode::Strict };
        let e = collect(&r, MISFITS, strict, &ScanOptions::default()).unwrap_err();
        let d = e.diagnostic();
        assert_eq!(d.code, "parse.timestamp");
        assert_eq!(d.column.as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(d.line, Some(3));
        assert_eq!(d.observed.as_deref(), Some("2024/13/01 00:05:00"));
        // An empty key is refused too.
        let empty = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",,1\n";
        let d = collect(&r, empty.as_slice(), strict, &ScanOptions::default())
            .unwrap_err()
            .diagnostic();
        assert_eq!(d.code, "parse.null_not_allowed");
        assert_eq!(d.column.as_deref(), Some("DUID"));
        assert_eq!(d.line, Some(2));
    }

    #[test]
    fn small_batches_flush_and_rows_total() {
        let r = reg();
        let text = fixture("DISPATCHPRICE_v4_head.csv");
        let mut sink = ArrowSink::new(&r, ResolveOptions::default(), 7, CollectBatches::default());
        let stats = nemlake_decode::scan::scan(text.as_slice(), &mut sink, &ScanOptions::default())
            .unwrap();
        let out = sink.into_inner().unwrap();
        assert_eq!(stats.rows, 20);
        assert_eq!(out.batches.len(), 3);
        assert_eq!(
            out.batches
                .iter()
                .map(|b| b.batch.num_rows())
                .collect::<Vec<_>>(),
            [7, 7, 6]
        );
        assert_eq!(out.batches[0].version, 1);
    }
}
