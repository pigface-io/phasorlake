//! The sink that turns blocks into typed `RecordBatch`es.

use std::sync::Arc;

use std::collections::HashMap;
use std::fmt::Write as _;

use arrow_array::builder::{ArrayBuilder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use phasor_core::{Category, Diagnose as _, Diagnostic, Fingerprint};
use phasor_mms_decode::IRow;
use phasor_registry::{Cast, Mode, Registry, ResolveOptions, Resolved, TableId};

use crate::error::{RowError, RowFailure, SinkError, ValueError};
use phasor_mms_decode::PREFIX_FIELDS;
use phasor_mms_decode::fields::Row;
use phasor_mms_decode::scan::{Flow, Pos, Sink};
use phasor_registry::arrow::RAW_OF_KEY;

use crate::builders::{ColumnBuilder, Fault, Parsed, append_field, parse_field};

/// A batch of rows from one block.
#[derive(Debug)]
pub struct BlockBatch {
    /// The table.
    pub table: TableId,
    /// The schema of the table's history the plan was based on.
    pub version: u16,
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

/// One route of the plan; kept small so a row's routes share cache lines.
#[derive(Clone, Copy)]
struct Step {
    field: u16,
    target: u16,
    cast: Cast,
    nullable: bool,
}

/// Values of one output column that did not fit its type since the last flush.
struct RawColumn {
    target: u16,
    name: String,
    /// One entry per row of the batch: the text, or null where the value fit.
    text: StringBuilder,
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
    /// Non-nullable output columns no file column feeds; every row leaves them null.
    missing_required: Vec<u16>,
    fingerprint: Fingerprint,
    builders: Vec<ColumnBuilder>,
    report_type: String,
    report_subtype: String,
    version_text: String,
    rows: usize,
    /// Per output column: timestamps clamped since the last flush.
    clamped: Vec<Tally>,
    /// Per output column: nulls kept in a non-nullable column; it is nullable in the batch.
    nulls_kept: Vec<Tally>,
    /// Columns with values kept as text since the last flush, in first-seen order.
    raws: Vec<RawColumn>,
    /// Rows per batch for this block's width (see [`BATCH_CELLS`]).
    batch_rows: usize,
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
/// A value that does not fit its type is null, its text kept in `{column}__raw`;
/// an empty non-nullable value is kept as null. Each is reported once per column
/// per batch (`parse.value_kept_raw`, `parse.null_kept`); strict mode refuses
/// the file instead. Only a structurally wrong row is quarantined.
pub struct ArrowSink<'r, B: BatchSink> {
    registry: &'r Registry,
    opts: ResolveOptions,
    /// Fields of a row before its data: [`PREFIX_FIELDS`] for MMS, 0 for plain.
    prefix: usize,
    period: Option<phasor_core::YearMonth>,
    batch_rows: usize,
    out: B,
    block: Option<Block>,
    parsed: Vec<Parsed>,
    /// The current row's misfits: output column, field position, fault.
    misfits: Vec<(u16, u16, Fault)>,
    /// Shapes whose resolution warnings were already passed on, so each is said once.
    warned: Vec<(phasor_registry::PlanHash, u16)>,
}

/// Rows a new column builder makes room for, small so tiny blocks stay cheap.
const FIRST_ROWS: usize = 256;

/// Rows per batch by default.
pub const DEFAULT_BATCH_ROWS: usize = 65_536;

/// Values (rows times columns) a batch holds at most, so wide tables get fewer rows per batch.
pub const BATCH_CELLS: usize = 1 << 20;

/// Rows the Parquet writer takes at a time; batches of a multiple of it leave page ends to the data alone.
const WRITE_BATCH_ROWS: usize = 1024;

/// Rows per batch for a table `width` columns wide, in whole multiples of [`WRITE_BATCH_ROWS`].
fn rows_for(width: usize, batch_rows: usize) -> usize {
    let rows = (BATCH_CELLS / width.max(1)).min(batch_rows);
    if batch_rows < WRITE_BATCH_ROWS {
        return rows.max(1);
    }
    (rows / WRITE_BATCH_ROWS).max(1) * WRITE_BATCH_ROWS
}

/// What a column of `ty` expects, in words.
fn expected(ty: &DataType) -> String {
    match ty {
        DataType::Float64 => "a number (Float64)".into(),
        DataType::Decimal128(p, s) => format!("a number of {p} digits, {s} after the point"),
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
            prefix: PREFIX_FIELDS,
            period: None,
            batch_rows: batch_rows.max(1),
            out,
            block: None,
            parsed: Vec::new(),
            misfits: Vec::new(),
            warned: Vec::new(),
        }
    }

    /// Sets the source's month, which picks the schema run each block is read with.
    #[must_use]
    pub fn with_period(mut self, period: Option<phasor_core::YearMonth>) -> Self {
        self.period = period;
        self
    }

    /// Reads rows of `prefix` fields before their data; 0 for a plain file.
    #[must_use]
    pub fn with_prefix(mut self, prefix: usize) -> Self {
        self.prefix = prefix;
        self
    }

    /// Takes the inner sink back.
    #[cfg(test)]
    pub(crate) fn into_inner(mut self) -> Result<B, SinkError> {
        self.flush()?;
        Ok(self.out)
    }

    /// Flushes and takes the inner sink back, even when the final flush failed.
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
        zone!(FLUSH);
        let Some(block) = self.block.as_mut() else {
            return Ok(());
        };
        let batch_rows = block.batch_rows;
        if block.rows == 0 {
            return Ok(());
        }
        let plan = &block.resolved.plan;
        let table = self.registry.table(block.resolved.table);
        let warn = |code: &'static str, column: &str, message: String| {
            Diagnostic::warning(code, Category::PARSE, message)
                .with_table(table.name.clone())
                .with_version(block.resolved.version)
                .with_column(column.to_owned())
        };
        // The plan's schema is copied only for a batch that changes it.
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
        for raw in block.raws.drain(..) {
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
                .get(phasor_registry::arrow::FIELD_ID_KEY)
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
            fingerprint: block.fingerprint,
            batch,
        };
        let full = block.rows >= batch_rows;
        block.rows = 0;
        self.out.batch(out)?;
        // A full batch is likely followed by another: size the builders for it.
        if full && let Some(block) = self.block.as_mut() {
            let lake = &block.resolved.plan.arrow;
            for (b, f) in block.builders.iter_mut().zip(lake.fields()) {
                *b = ColumnBuilder::new(f.data_type(), batch_rows);
            }
        }
        Ok(())
    }
}

impl<B: BatchSink> Sink for ArrowSink<'_, B> {
    type Reject = ValueError;
    type Fatal = SinkError;
    fn comment(&mut self, _row: &Row<'_>, _pos: Pos) -> Result<(), SinkError> {
        Ok(())
    }

    fn header(&mut self, irow: &IRow<'_>, pos: Pos) -> Result<(), SinkError> {
        self.flush()?;
        zone!(BLOCK);
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
        // Reuse the previous block's buffers, and its builders when the shape is the same.
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
                field: (self.prefix + usize::from(r.source_index)) as u16,
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
            fingerprint: resolved.fingerprint,
            builders: b.builders,
            report_type: b.report_type,
            report_subtype: b.report_subtype,
            version_text: b.version_text,
            rows: 0,
            clamped: b.clamped,
            nulls_kept: b.nulls_kept,
            raws: b.raws,
            batch_rows: rows_for(width, self.batch_rows),
            resolved,
        });
        Ok(())
    }

    #[inline]
    fn row(&mut self, row: &Row<'_>, pos: Pos) -> Result<(), RowFailure> {
        zone!(ROW);
        let block = self
            .block
            .as_mut()
            .expect("the scanner delivers rows only after a header");
        if row.len() < self.prefix {
            return Err(RowError::TooShort(row.len()).into());
        }
        if self.prefix > 0
            && (row.get(1) != Some(block.report_type.as_bytes())
                || row.get(2) != Some(block.report_subtype.as_bytes())
                || row.get(3).map(<[u8]>::trim_ascii) != Some(block.version_text.as_bytes()))
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
        let observed = row.len_trimmed().saturating_sub(self.prefix);
        if observed > declared {
            return Err(RowError::FieldCount {
                declared,
                observed: row.len() - self.prefix,
            }
            .into());
        }
        let strict = self.opts.mode == Mode::Strict;
        // A misfit keeps the row; strict mode refuses the file at the first one.
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
            RowFailure::fatal(SinkError::Refused(Box::new(d)))
        };
        self.misfits.clear();
        if strict {
            // Pass 1: parse every field; nothing is appended until all are read,
            // so a refused row leaves the builders as they were.
            {
                zone!(PARSE);
                self.parsed.fill(Parsed::Null);
                for c in &block.steps {
                    let target = c.target;
                    let bytes = row.get(usize::from(c.field)).unwrap_or(b"");
                    let parsed = parse_field(c.cast, c.nullable, c.field, bytes)
                        .map_err(|fault| refuse(block, target, c.field, fault))?;
                    if matches!(parsed, Parsed::Clamped(_)) {
                        block.clamped[target as usize]
                            .note(|| String::from_utf8_lossy(bytes).into_owned(), pos.line);
                    }
                    self.parsed[target as usize] = parsed;
                }
            }
            if let Some(&m) = block.missing_required.first() {
                let column = block.resolved.plan.arrow.field(m as usize).name();
                let d = ValueError::NullNotAllowed {
                    column: column.clone(),
                }
                .diagnostic()
                .at(pos.line, pos.byte_offset)
                .with_table(block.resolved.plan.table.clone())
                .with_version(block.resolved.version);
                return Err(RowFailure::fatal(SinkError::Refused(Box::new(d))));
            }
            // Pass 2: append.
            zone!(APPEND);
            for (b, v) in block.builders.iter_mut().zip(&self.parsed) {
                b.append(*v, |i| row.get(usize::from(i)).unwrap_or(b""));
            }
        } else {
            // Every row past the checks above is kept, so each field goes
            // straight to its builder; a misfit is null.
            zone!(PARSE);
            for c in &block.steps {
                let target = c.target as usize;
                let bytes = row.get(usize::from(c.field)).unwrap_or(b"");
                let b = &mut block.builders[target];
                match append_field(b, c.cast, c.nullable, bytes) {
                    Ok(false) => {}
                    Ok(true) => block.clamped[target]
                        .note(|| String::from_utf8_lossy(bytes).into_owned(), pos.line),
                    Err(fault) => {
                        b.append_null();
                        self.misfits.push((c.target, c.field, fault));
                    }
                }
            }
            for &m in &block.resolved.plan.missing {
                block.builders[m as usize].append_null();
            }
            for &m in &block.missing_required {
                block.nulls_kept[m as usize].note(String::new, pos.line);
            }
        }
        if !self.misfits.is_empty() || !block.raws.is_empty() {
            keep_misfits(block, &self.misfits, row, pos.line);
        }
        block.rows += 1;
        if block.rows >= block.batch_rows {
            self.flush().map_err(RowFailure::fatal)?;
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

/// Records the current row's misfits; every raw column gets one entry for the row.
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
    scan_opts: &phasor_mms_decode::scan::ScanOptions,
) -> Result<(CollectBatches, phasor_mms_decode::scan::ScanStats), crate::error::ScanError> {
    let mut sink = ArrowSink::new(
        registry,
        opts,
        DEFAULT_BATCH_ROWS,
        CollectBatches::default(),
    );
    let stats = phasor_mms_decode::scan::scan(reader, &mut sink, scan_opts)?;
    let out = sink.into_inner()?;
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use arrow_array::{Array, Decimal128Array, StringArray, TimestampMicrosecondArray};
    use phasor_registry::Mode;

    use super::*;
    use phasor_mms_decode::scan::ScanOptions;

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
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(ts.timezone(), Some("+10:00"));
        let expected = crate::mmstime::parse_nanos(b"2024/01/01 00:05:00", 36_000)
            .unwrap()
            .0;
        assert_eq!(ts.value(0), expected / 1_000, "microseconds");
        let duid = b.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(duid.value(0), "BUTLERSG");
        // SCADAVALUE is decimal(16, 6) in the table's history: exact.
        let mw = b
            .column(2)
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .unwrap();
        assert_eq!(
            (mw.value(0), mw.precision(), mw.scale()),
            (9_199_999, 16, 6)
        );
        assert_eq!(out.blocks, vec![(String::new(), 0, "conforms")]);
        assert!(out.warnings.is_empty());
        assert_eq!(b.schema().metadata()["phasor:table"], "DISPATCH_UNIT_SCADA");
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

    /// The multi-block fixture led by a block for a report pair no table claims.
    fn with_unknown_block() -> Vec<u8> {
        let text = String::from_utf8(fixture("multi_block_dispatchis.csv")).unwrap();
        let after_header = text.find('\n').unwrap() + 1;
        format!(
            "{}I,PHASOR_TEST,NEW_THING,1,WHEN,AMOUNT\nD,PHASOR_TEST,NEW_THING,1,\"2026/10/01 13:40:00\",1.5\nD,PHASOR_TEST,NEW_THING,1,\"2026/10/01 13:45:00\",2.5\n{}",
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
            ResolveOptions {
                mode: Mode::Strict,
                ..Default::default()
            },
            &ScanOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(
                &e,
                crate::error::ScanError::Sink(SinkError::Refused(d))
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
            .filter(|b| r.table(b.table).name == "PHASOR_TEST_NEW_THING")
            .collect();
        assert_eq!(derived.len(), 1);
        let b = derived[0];
        assert!(r.is_derived(b.table));
        assert_eq!(r.table_dir(b.table), "_unregistered/phasor_test.new_thing");
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
                && w.severity == phasor_core::Severity::Warning
                && w.message.contains("_unregistered/phasor_test.new_thing")),
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
            ResolveOptions {
                mode: Mode::Strict,
                ..Default::default()
            },
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 3);
        assert_eq!(stats.bad_rows, 0);
        let ts = out.batches[0]
            .batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(ts.value(0), i64::MAX / 1_000);
        assert_eq!(ts.value(1), i64::MAX / 1_000);
        let whole = crate::mmstime::parse_nanos(b"2024/01/01 00:05:00", 36_000)
            .unwrap()
            .0;
        assert_eq!(ts.value(2), whole / 1_000 + 250_000);
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
        // DISPATCHPRICE in the live feed is version 5, with two of its
        // columns swapped here: a layout no run of the history has.
        let r = reg();
        let text = fixture("multi_block_dispatchis.csv");
        let lines: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
        let price: Vec<u8> = lines
            .iter()
            .filter(|l| l.starts_with(b"I,DISPATCH,PRICE") || l.starts_with(b"D,DISPATCH,PRICE"))
            .flat_map(|l| l.iter().copied().chain(std::iter::once(b'\n')))
            .collect();
        let price = String::from_utf8(price)
            .unwrap()
            .replacen(",RRP,EEP,", ",EEP,RRP,", 1)
            .into_bytes();
        let (out, stats) = collect(
            &r,
            price.as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 3);
        // The swapped columns are kept as the file lays them out, and
        // flagged.
        assert_eq!(out.blocks[0].2, "deviates");
        assert!(out.warnings.iter().any(|w| w.code == "schema.deviates"));
        // Strict mode with a hand-made unknown version.
        let rows = b"I,DISPATCH,UNIT_SCADA,7,SETTLEMENTDATE,DUID,SCADAVALUE,EXTRA\nD,DISPATCH,UNIT_SCADA,7,\"2024/01/01 00:05:00\",X,1,z\n";
        let e = collect(
            &r,
            rows.as_slice(),
            ResolveOptions {
                mode: Mode::Strict,
                ..Default::default()
            },
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
        // A file that lacks one of the table's columns is kept too, null there.
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
        assert!(out.warnings.iter().all(|w| w.code != "schema.missing_key"));
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
        let e = phasor_mms_decode::scan::scan(
            rows.as_slice(),
            &mut sink,
            &ScanOptions {
                max_bad_rows: 100,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(e, crate::error::ScanError::Stopped { line: 3 }));
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

    /// A decimal too wide for its column is kept as text beside a null, never wrapped.
    #[test]
    fn a_decimal_too_long_for_its_column_is_kept_as_text() {
        let r = reg();
        let long = "9".repeat(45);
        let rows = format!(
            "I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,{long}\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",B,1.5\n"
        );
        let (out, stats) = collect(
            &r,
            rows.as_bytes(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!((stats.rows, stats.bad_rows), (2, 0));
        let b = &out.batches[0].batch;
        let mw = b
            .column(2)
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .unwrap();
        assert!(mw.is_null(0));
        assert_eq!(mw.value(1), 1_500_000);
        let raw = b
            .column_by_name("SCADAVALUE__raw")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(raw.value(0), long);
        assert!(raw.is_null(1));
        assert!(
            out.warnings
                .iter()
                .any(|w| w.code == "parse.value_kept_raw")
        );
    }

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
            b.schema().field(2).metadata()[phasor_registry::arrow::FIELD_ID_KEY]
        );
        // The typed columns are null where the value did not fit; every
        // column allows it.
        assert!(b.column(0).is_null(1));
        assert!(b.schema().field(0).is_nullable());
        assert!(b.column(1).is_null(3));
        assert!(b.schema().field(1).is_nullable());
        assert_eq!(b.column(2).null_count(), 2);
        // One warning per column, with how many and the first.
        let codes: Vec<_> = out.warnings.iter().map(|w| w.code.to_string()).collect();
        assert_eq!(codes, ["parse.value_kept_raw", "parse.value_kept_raw"]);
        let scada = &out.warnings[1];
        assert_eq!(scada.column.as_deref(), Some("SCADAVALUE"));
        assert_eq!(scada.line, Some(4));
        assert_eq!(scada.observed.as_deref(), Some("notanumber"));
        assert_eq!(
            scada.expected.as_deref(),
            Some("a number of 16 digits, 6 after the point")
        );
        assert!(
            scada.message.starts_with("2 value(s) of column SCADAVALUE"),
            "{}",
            scada.message
        );
    }

    /// A table that declares `{column}__raw` gets the text there, under the column's own id.
    #[test]
    fn raw_columns_exist_only_in_the_batches_that_need_them() {
        let r = reg();
        let mut sink = ArrowSink::new(&r, ResolveOptions::default(), 2, CollectBatches::default());
        phasor_mms_decode::scan::scan(MISFITS, &mut sink, &ScanOptions::default()).unwrap();
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
        assert!(out.batches[2].batch.schema().field(1).is_nullable());
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
        let strict = ResolveOptions {
            mode: Mode::Strict,
            ..Default::default()
        };
        let e = collect(&r, MISFITS, strict, &ScanOptions::default()).unwrap_err();
        let d = e.diagnostic();
        assert_eq!(d.code, "parse.timestamp");
        assert_eq!(d.column.as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(d.line, Some(3));
        assert_eq!(d.observed.as_deref(), Some("2024/13/01 00:05:00"));
        // An empty value is kept, even in strict mode: no column refuses a null.
        let empty = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",,1\n";
        let (out, stats) = collect(&r, empty.as_slice(), strict, &ScanOptions::default()).unwrap();
        assert_eq!((stats.rows, stats.bad_rows), (1, 0));
        assert!(out.batches[0].batch.column(1).is_null(0));
    }

    #[test]
    fn a_wide_table_has_fewer_rows_per_batch() {
        assert_eq!(rows_for(6, DEFAULT_BATCH_ROWS), DEFAULT_BATCH_ROWS);
        assert_eq!(rows_for(128, DEFAULT_BATCH_ROWS), BATCH_CELLS / 128);
        assert_eq!(rows_for(123, DEFAULT_BATCH_ROWS), 8 * WRITE_BATCH_ROWS);
        assert_eq!(rows_for(1 << 16, DEFAULT_BATCH_ROWS), WRITE_BATCH_ROWS);
        assert_eq!(rows_for(1 << 16, 16), 16);
        assert_eq!(rows_for(4, 16), 16);
        assert_eq!(rows_for(0, 1024), 1024);
    }

    #[test]
    fn small_batches_flush_and_rows_total() {
        let r = reg();
        let text = fixture("DISPATCHPRICE_v4_head.csv");
        let mut sink = ArrowSink::new(&r, ResolveOptions::default(), 7, CollectBatches::default());
        let stats =
            phasor_mms_decode::scan::scan(text.as_slice(), &mut sink, &ScanOptions::default())
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
        // The head carries the columns of DISPATCHPRICE's schema 1.
        assert_eq!(out.batches[0].version, 1);
    }
}
