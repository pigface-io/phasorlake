//! The MMS format as a kit sees it: a [`Decoder`] that reads C/I/D rows
//! through the registry into blocks, and a [`Catalog`] over the registry.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use utilake_core::{Diagnose, Diagnostic, Period, YearMonth};

use crate::error::{RowError, RowFailure, SinkError};
use utilake_kit::container::has_extension;
use utilake_kit::{
    Block, BlockSink, Catalog, DecodeCtx, DecodeStats, Decoder, Flow, LoadMode, RowAt, Strictness,
    TableRef, Transform,
};
use utilake_mms_decode::IRow;
use utilake_mms_decode::scan::{Flow as ScanFlow, Pos, ScanOptions, ScanStats, scan_with};
use utilake_mms_registry::file_model::Market;
use utilake_mms_registry::{Mode, Registry, ResolveOptions, Resolved, TableId};

use crate::arrow_sink::{ArrowSink, BatchSink, BlockBatch};

/// The MMS decoder: C/I/D rows, resolved through the registry.
pub struct MmsDecoder {
    registry: Arc<Registry>,
}

impl MmsDecoder {
    /// A decoder over `registry`.
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }
}

/// The census's id of a block's shape, from its header: the same
/// canonical bytes the census gives every block it sees.
fn shape_id(irow: &IRow<'_>) -> String {
    utilake_mms_registry::CanonicalShape::of(irow.report_type, irow.report_subtype, irow.columns)
        .id()
        .to_string()
}

fn load_mode(m: utilake_mms_registry::LoadMode) -> LoadMode {
    match m {
        utilake_mms_registry::LoadMode::Append => LoadMode::Append,
        utilake_mms_registry::LoadMode::Snapshot => LoadMode::Snapshot,
    }
}

/// A table of the registry as a block names it.
fn table_ref(
    registry: &Registry,
    id: TableId,
    plan_mode: LoadMode,
    time_column: Option<usize>,
) -> TableRef {
    let t = registry.table(id);
    TableRef {
        market: t.market.as_str().to_owned(),
        name: t.name.clone(),
        dir: registry.table_dir(id),
        utc_offset_secs: registry.market_of(id).utc_offset_secs,
        load_mode: plan_mode,
        time_column,
    }
}

/// Carries what the registry resolved across to a [`BlockSink`].
struct Bridge<'a> {
    out: &'a mut dyn BlockSink,
    registry: &'a Registry,
    period: Option<YearMonth>,
    tables: HashMap<TableId, Arc<TableRef>>,
    current: Option<(Arc<TableRef>, Arc<Transform>)>,
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
            ResolveOptions { mode },
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

/// The registry's NEM tables as a [`Catalog`].
pub struct MmsCatalog {
    registry: Arc<Registry>,
}

impl MmsCatalog {
    /// A catalog over `registry`.
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }

    /// The registry behind the catalog, for the schema commands.
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    fn table_of(&self, id: TableId) -> Arc<TableRef> {
        let t = self.registry.table(id);
        let time_column = t
            .time_column
            .as_deref()
            .and_then(|c| self.registry.lake_schema(id).index_of(c).ok());
        Arc::new(table_ref(
            &self.registry,
            id,
            load_mode(t.load_mode),
            time_column,
        ))
    }
}

impl Catalog for MmsCatalog {
    fn market(&self) -> &str {
        Market::Nem.as_str()
    }

    fn tables(&self) -> Vec<Arc<TableRef>> {
        let mut out: Vec<Arc<TableRef>> = self
            .registry
            .tables()
            .filter(|(_, t)| t.market == Market::Nem)
            .map(|(id, _)| self.table_of(id))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn table(&self, name: &str) -> Option<Arc<TableRef>> {
        self.registry
            .table_by_name(&name.to_ascii_uppercase())
            .map(|id| self.table_of(id))
    }

    /// Whether every shape that was read from a source would be read by
    /// the same rules today. The answer comes from the recorded transforms
    /// alone; the source is not opened.
    ///
    /// Strictness is not part of the question: a shape the registry would
    /// refuse in strict mode is current when the permissive rules that
    /// wrote it have not changed.
    fn plans_current(&self, transforms: &[Transform]) -> bool {
        transforms.iter().all(|t| {
            if t.plan_hash.is_empty() {
                return false;
            }
            let columns: Vec<&str> = t.source_columns.iter().map(String::as_str).collect();
            let irow = IRow {
                report_type: &t.block_family,
                report_subtype: &t.block_name,
                version: t.source_version,
                columns: &columns,
            };
            // The month the source was read as: it picks among the runs
            // of the table's history that fit the block.
            let period = t.source_period.as_deref().and_then(|p| p.parse().ok());
            self.registry
                .resolve_in(
                    &irow,
                    ResolveOptions {
                        mode: Mode::Permissive,
                    },
                    period,
                )
                .is_ok_and(|r| r.plan_hash().to_string() == t.plan_hash)
        })
    }

    fn with_overlay(&self, dir: &Path) -> Result<Arc<dyn Catalog>, Box<Diagnostic>> {
        let registry = self
            .registry
            .with_overlay(dir)
            .map_err(|e| Box::new(e.diagnostic()))?;
        Ok(Arc::new(Self::new(Arc::new(registry))))
    }
}

/// A row failure as the bridge reports it; kept for the row loop's use.
#[allow(dead_code)]
fn _row_failure(_: RowFailure) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_nem_tables_and_finds_one_by_name() {
        let registry = Registry::load_embedded();
        let cat = MmsCatalog::new(registry.clone());
        assert_eq!(cat.market(), "nem");
        let tables = cat.tables();
        assert!(tables.len() > 200);
        assert!(tables.windows(2).all(|w| w[0].name < w[1].name));
        let scada = cat.table("dispatch_unit_scada").unwrap();
        assert_eq!(scada.name, "DISPATCH_UNIT_SCADA");
        assert_eq!(scada.dir, "dispatch_unit_scada");
        assert_eq!(scada.market, "nem");
        assert_eq!(scada.utc_offset_secs, 36_000);
        assert_eq!(scada.load_mode, LoadMode::Append);
        assert_eq!(
            scada.time_column,
            registry
                .lake_schema(registry.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
                .index_of("SETTLEMENTDATE")
                .ok()
        );
        assert!(cat.table("NO_SUCH_TABLE").is_none());
        assert!(cat.plans_current(&[]));
        assert!(!cat.plans_current(&[Transform::default()]));
        assert!(Arc::clone(cat.registry()).table_by_name("REGION").is_some());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(cat.with_overlay(dir.path()).unwrap().market(), "nem");
        let dec = MmsDecoder::new(registry);
        assert!(dec.accepts("A.CSV") && !dec.accepts("A.ctl"));
        assert_eq!(dec.id(), "mms");
    }
}
