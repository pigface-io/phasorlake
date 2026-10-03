//! The sink that turns blocks into typed `RecordBatch`es.

use arrow_array::RecordBatch;
use nemlake_core::{Diagnostic, Fingerprint, IRow};
use nemlake_schema::{Registry, ResolveOptions, Resolved, SchemaVersion, TableId};

use crate::builders::{ColumnBuilder, Parsed, parse_field};
use crate::error::{RowError, RowFailure, SinkError};
use crate::fields::Row;
use crate::scan::{Flow, Pos, Sink};

/// Rows AEMO always writes before the data: `D`, report type, subtype, version.
pub const PREFIX_FIELDS: usize = 4;

/// A batch of rows from one block.
#[derive(Debug)]
pub struct BlockBatch {
    /// The table.
    pub table: TableId,
    /// The version the projection was based on.
    pub version: SchemaVersion,
    /// The observed fingerprint of the I row.
    pub fingerprint: Fingerprint,
    /// The rows, in the table's unified lake schema.
    pub batch: RecordBatch,
}

/// Receives batches and anything else the pipeline needs to know.
pub trait BatchSink {
    /// A batch is ready.
    fn batch(&mut self, b: BlockBatch) -> Result<(), SinkError>;
    /// A warning raised while resolving (drift, decimal fallback).
    fn warning(&mut self, d: Diagnostic);
    /// A block began; `resolved` describes how it matched.
    fn block_started(&mut self, _irow: &IRow<'_>, _resolved: &Resolved, _pos: Pos) {}
    /// A quarantined row.
    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> Flow;
}

struct Block {
    resolved: Resolved,
    fingerprint: Fingerprint,
    builders: Vec<ColumnBuilder>,
    report_type: String,
    report_subtype: String,
    version_text: String,
    rows: usize,
}

/// Builds typed batches from MMS rows through the registry.
pub struct ArrowSink<'r, B: BatchSink> {
    registry: &'r Registry,
    opts: ResolveOptions,
    batch_rows: usize,
    out: B,
    block: Option<Block>,
    parsed: Vec<Parsed>,
    /// Skip blocks of unknown tables (with a warning) instead of failing the file.
    skip_unknown: bool,
    /// Rows skipped because their block was unknown.
    pub skipped_rows: u64,
    skipping: bool,
}

/// Rows per batch by default.
pub const DEFAULT_BATCH_ROWS: usize = 65_536;

impl<'r, B: BatchSink> ArrowSink<'r, B> {
    /// A sink resolving against `registry`, flushing every `batch_rows` rows.
    pub fn new(registry: &'r Registry, opts: ResolveOptions, batch_rows: usize, out: B) -> Self {
        Self {
            registry,
            opts,
            batch_rows: batch_rows.max(1),
            out,
            block: None,
            parsed: Vec::new(),
            skip_unknown: false,
            skipped_rows: 0,
            skipping: false,
        }
    }

    /// Takes the inner sink back.
    pub fn into_inner(mut self) -> Result<B, SinkError> {
        self.flush()?;
        Ok(self.out)
    }

    /// The inner sink.
    pub fn inner(&self) -> &B {
        &self.out
    }

    /// The inner sink, mutably.
    pub fn inner_mut(&mut self) -> &mut B {
        &mut self.out
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        let Some(block) = self.block.as_mut() else {
            return Ok(());
        };
        if block.rows == 0 {
            return Ok(());
        }
        let schema = block.resolved.projection.arrow.clone();
        let arrays: Vec<_> = block
            .builders
            .iter_mut()
            .map(ColumnBuilder::finish)
            .collect();
        let table = self.registry.table(block.resolved.table);
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
        self.skipping = false;
        let resolved = match self.registry.resolve(irow, self.opts) {
            Ok(r) => r,
            Err(e @ nemlake_schema::ResolveError::UnknownTable { .. }) if self.skip_unknown => {
                use nemlake_core::Diagnose as _;
                let mut d = e.diagnostic();
                d.severity = nemlake_core::Severity::Warning;
                d.message = format!("{} The block is skipped.", d.message);
                d.line = Some(pos.line);
                self.out.warning(d);
                self.block = None;
                self.skipping = true;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        for w in &resolved.warnings {
            self.out.warning(w.clone());
        }
        self.out.block_started(irow, &resolved, pos);
        let schema = &resolved.projection.arrow;
        let builders = schema
            .fields()
            .iter()
            .map(|f| ColumnBuilder::new(f.data_type(), self.batch_rows.min(8192)))
            .collect();
        self.parsed.clear();
        self.parsed.resize(schema.fields().len(), Parsed::Null);
        self.block = Some(Block {
            fingerprint: irow.fingerprint(),
            builders,
            report_type: irow.report_type.to_owned(),
            report_subtype: irow.report_subtype.to_owned(),
            version_text: irow.version.to_string(),
            rows: 0,
            resolved,
        });
        Ok(())
    }

    #[inline]
    fn row(&mut self, row: &Row<'_>, _pos: Pos) -> Result<(), RowFailure> {
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
                block_version: block.resolved.version.0,
            }
            .into());
        }
        let casts = &block.resolved.projection.casts;
        let declared = casts.len();
        let observed = row.len_trimmed().saturating_sub(PREFIX_FIELDS);
        if observed > declared {
            return Err(RowError::FieldCount {
                declared,
                observed: row.len() - PREFIX_FIELDS,
            }
            .into());
        }
        let table = self.registry.table(block.resolved.table);
        // Pass 1: parse every field; nothing is appended until all succeed.
        self.parsed.fill(Parsed::Null);
        for (i, c) in casts.iter().enumerate() {
            let Some(target) = c.target else { continue };
            let bytes = row.get(PREFIX_FIELDS + i).unwrap_or(b"");
            let column = &table.columns[target as usize].name;
            self.parsed[target as usize] = parse_field(
                c.cast,
                c.nullable,
                (PREFIX_FIELDS + i) as u16,
                column,
                bytes,
            )?;
        }
        for &m in &block.resolved.projection.missing {
            let col = &table.columns[m as usize];
            if !col.nullable {
                return Err(RowError::NullNotAllowed {
                    column: col.name.clone(),
                }
                .into());
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

/// A `BatchSink` that keeps everything in memory, for tests and `inspect`.
#[derive(Debug, Default)]
pub struct CollectBatches {
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
            .push((String::new(), resolved.version.0, resolved.status.label()));
    }
    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> Flow {
        use nemlake_core::Diagnose as _;
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

/// Convenience: scans `reader` through an [`ArrowSink`] into [`CollectBatches`].
pub fn collect<R: std::io::Read>(
    registry: &Registry,
    reader: R,
    opts: ResolveOptions,
    scan_opts: &crate::scan::ScanOptions,
) -> Result<(CollectBatches, crate::scan::ScanStats), crate::error::ScanError> {
    let mut sink = ArrowSink::new(
        registry,
        opts,
        DEFAULT_BATCH_ROWS,
        CollectBatches::default(),
    );
    let stats = crate::scan::scan(reader, &mut sink, scan_opts)?;
    let out = sink.into_inner()?;
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Array, Float64Array, StringArray, TimestampMicrosecondArray};
    use nemlake_schema::{Mode, TypeOptions};

    use super::*;
    use crate::scan::ScanOptions;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/csv/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn reg() -> Arc<Registry> {
        Registry::load_embedded(TypeOptions::default())
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
        assert_eq!(b.num_columns(), 3);
        let ts = b
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(ts.timezone(), Some("+10:00"));
        let expected = nemlake_core::nemtime::parse_micros(b"2024/01/01 00:05:00", 36_000).unwrap();
        assert_eq!(ts.value(0), expected);
        let duid = b.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(duid.value(0), "BUTLERSG");
        let mw = b.column(2).as_any().downcast_ref::<Float64Array>().unwrap();
        assert!((mw.value(0) - 9.199_999).abs() < 1e-9);
        assert_eq!(out.blocks, vec![(String::new(), 1, "exact")]);
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

    #[test]
    fn multi_block_file_routes_blocks_and_tolerates_unknown_tables_only_when_skipped() {
        let r = reg();
        // The live file has three report pairs the registry does not know yet.
        let e = collect(
            &r,
            fixture("multi_block_dispatchis.csv").as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(
                e,
                crate::error::ScanError::Sink(SinkError::Resolve(
                    nemlake_schema::ResolveError::UnknownTable { .. }
                ))
            ),
            "{e}"
        );
    }

    #[test]
    fn drift_version_alias_and_decimal() {
        // DISPATCHPRICE in the live feed is version 5 with more columns than the archive's version 4.
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
        assert_eq!(
            out.blocks[0].2, "exact",
            "the registry sampled both versions"
        );
        // Strict mode with a hand-made unknown version.
        let rows = b"I,DISPATCH,UNIT_SCADA,7,SETTLEMENTDATE,DUID,SCADAVALUE,EXTRA\nD,DISPATCH,UNIT_SCADA,7,\"2024/01/01 00:05:00\",X,1,z\n";
        let e = collect(
            &r,
            rows.as_slice(),
            ResolveOptions { mode: Mode::Strict },
            &ScanOptions::default(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("no declared version 7"), "{e}");
        let (out, stats) = collect(
            &r,
            rows.as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.rows, 1);
        assert_eq!(out.blocks[0].2, "drift");
        assert_eq!(out.warnings[0].code, "schema.unknown_columns");
        assert_eq!(
            out.batches[0].batch.num_columns(),
            3,
            "unknown column dropped"
        );
        // Decimal mode.
        let dec = Registry::load_embedded(TypeOptions { decimal: true });
        let (out, _) = collect(
            &dec,
            fixture("DISPATCH_UNIT_SCADA_v1_head.csv").as_slice(),
            ResolveOptions::default(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            out.batches[0].batch.column(2).data_type(),
            arrow_schema::DataType::Decimal128(16, 6)
        ));
    }

    #[test]
    fn bad_rows_are_quarantined_not_appended() {
        let r = reg();
        let rows = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/13/01 00:05:00\",B,1\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",C,notanumber\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",,1\n\
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
        assert_eq!(stats.bad_rows, 6);
        let codes: Vec<_> = out.bad.iter().map(|(_, _, d)| d.code.to_string()).collect();
        assert_eq!(
            codes,
            [
                "parse.timestamp",
                "parse.float",
                "parse.null_not_allowed",
                "csv.field_count_mismatch",
                "csv.block_mismatch",
                "csv.row_too_short"
            ]
        );
        assert_eq!(out.bad[0].0, 3);
        assert!(
            out.bad[0]
                .2
                .sample
                .as_ref()
                .unwrap()
                .text
                .contains("2024/13/01")
        );
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
        let e = crate::scan::scan(
            rows.as_slice(),
            &mut sink,
            &ScanOptions {
                max_bad_rows: 100,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(e, crate::error::ScanError::Stopped { line: 3 }));
        assert_eq!(sink.inner().bad.len(), 1);
        sink.inner_mut().bad.clear();
    }

    #[test]
    fn small_batches_flush_and_rows_total() {
        let r = reg();
        let text = fixture("DISPATCHPRICE_v4_head.csv");
        let mut sink = ArrowSink::new(&r, ResolveOptions::default(), 7, CollectBatches::default());
        let stats = crate::scan::scan(text.as_slice(), &mut sink, &ScanOptions::default()).unwrap();
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
        assert_eq!(out.batches[0].version, SchemaVersion(4));
    }
}
