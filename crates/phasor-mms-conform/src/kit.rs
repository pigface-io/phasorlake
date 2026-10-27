//! The MMS format as a kit sees it: a [`Decoder`] that reads C/I/D rows
//! through the registry into blocks.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use phasor_core::{Diagnose, Diagnostic, Period, YearMonth};

use crate::error::{RowError, RowFailure, SinkError};
use phasor_kit::{
    Block, BlockSink, DecodeCtx, DecodeStats, Decoder, Flow, RowAt, Strictness, TableRef, Transform,
};
use phasor_mms_decode::IRow;
use phasor_mms_decode::scan::{Flow as ScanFlow, Pos, ScanOptions, ScanStats, scan_with};
use phasor_registry::catalog::{load_mode, table_ref};
use phasor_registry::file_model::Market;
use phasor_registry::{Mode, Registry, ResolveOptions, Resolved, TableId};
use phasor_zip::has_extension;

use crate::arrow_sink::{ArrowSink, BatchSink, BlockBatch};

/// The MMS decoder: C/I/D rows, resolved through the registry within one
/// market.
pub struct MmsDecoder {
    registry: Arc<Registry>,
    market: Market,
}

impl MmsDecoder {
    /// A decoder over `registry` for the files of `market`.
    pub fn new(registry: Arc<Registry>, market: Market) -> Self {
        Self { registry, market }
    }
}

/// The census's id of a block's shape, from its header: the same
/// canonical bytes the census gives every block it sees.
fn shape_id(irow: &IRow<'_>) -> String {
    phasor_registry::CanonicalShape::of(irow.report_type, irow.report_subtype, irow.columns)
        .id()
        .to_string()
}

/// Carries what the registry resolved across to a [`BlockSink`].
pub(crate) struct Bridge<'a> {
    pub(crate) out: &'a mut dyn BlockSink,
    pub(crate) registry: &'a Registry,
    pub(crate) period: Option<YearMonth>,
    pub(crate) tables: HashMap<TableId, Arc<TableRef>>,
    pub(crate) current: Option<(Arc<TableRef>, Arc<Transform>)>,
}

impl BatchSink for Bridge<'_> {
    fn block_started(&mut self, irow: &IRow<'_>, resolved: &Resolved, _pos: Pos) {
        let registry = self.registry;
        let table = self
            .tables
            .entry(resolved.table)
            .or_insert_with(|| {
                Arc::new(table_ref(
                    registry,
                    resolved.table,
                    load_mode(resolved.plan.load_mode),
                    resolved.plan.time_column.map(usize::from),
                ))
            })
            .clone();
        let transform = Arc::new(Transform {
            block_family: irow.report_type.to_owned(),
            block_name: irow.report_subtype.to_owned(),
            source_version: irow.version,
            source_columns: irow.columns.iter().map(|c| (*c).to_owned()).collect(),
            source_period: self.period.map(|p| p.to_string()),
            plan_hash: resolved.plan_hash().to_string(),
            schema_source: resolved.source.label().to_owned(),
            resolution: resolved.label().to_owned(),
            shape_id: Some(shape_id(irow)),
            schema_id: resolved.schema,
        });
        self.out.block_started(
            &table,
            resolved.version,
            irow.fingerprint(),
            &transform,
            resolved.label(),
        );
        self.current = Some((table, transform));
    }

    fn batch(&mut self, b: BlockBatch) -> Result<(), SinkError> {
        let Some((table, transform)) = self.current.clone() else {
            return Ok(());
        };
        self.out
            .batch(Block {
                table,
                version: b.version,
                fingerprint: b.fingerprint,
                transform,
                batch: b.batch,
            })
            .map_err(SinkError::Refused)
    }

    fn warning(&mut self, d: Diagnostic) {
        self.out.warning(d);
    }

    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> ScanFlow {
        let d = err
            .diagnostic()
            .at(pos.line, pos.byte_offset)
            .with_sample(raw);
        let at = RowAt {
            line: pos.line,
            byte_offset: pos.byte_offset,
        };
        match self.out.bad_row(raw, at, d) {
            Flow::Continue => ScanFlow::Continue,
            Flow::Stop => ScanFlow::Stop,
        }
    }
}

fn stats(s: &ScanStats) -> DecodeStats {
    DecodeStats {
        lines: s.lines,
        bytes: s.bytes,
        blocks: s.blocks,
        rows: s.rows,
        bad_rows: s.bad_rows,
        rows_repeated: 0,
        footer_declared: s.footer_declared,
    }
}

impl Decoder for MmsDecoder {
    fn id(&self) -> &'static str {
        "mms"
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
        let period = ctx.period.map(Period::month);
        let mode = match ctx.options.strictness {
            Strictness::Permissive => Mode::Permissive,
            Strictness::Strict => Mode::Strict,
        };
        let bridge = Bridge {
            out: sink,
            registry: &self.registry,
            period,
            tables: HashMap::new(),
            current: None,
        };
        let mut arrow = ArrowSink::new(
            &self.registry,
            ResolveOptions {
                mode,
                market: self.market,
            },
            ctx.options.batch_rows,
            bridge,
        )
        .with_period(period);
        let opts = ScanOptions {
            max_bad_rows: ctx.options.max_bad_rows,
            ..Default::default()
        };
        let scanned = scan_with(reader, &mut arrow, &opts, scratch);
        let (_, flushed) = arrow.finish();
        match (scanned, flushed) {
            (Ok(s), Ok(())) => Ok(stats(&s)),
            (Ok(_), Err(e)) => Err(Box::new(e.diagnostic())),
            (Err(e), _) => Err(Box::new(e.diagnostic())),
        }
    }
}

/// A row failure as the bridge reports it; kept for the row loop's use.
#[allow(dead_code)]
fn _row_failure(_: RowFailure) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decoder_reads_csv_entries() {
        let registry = Registry::load_embedded();
        let dec = MmsDecoder::new(registry, Market::Nem);
        assert!(dec.accepts("A.CSV") && !dec.accepts("A.ctl"));
        assert_eq!(dec.id(), "mms");
    }
}
