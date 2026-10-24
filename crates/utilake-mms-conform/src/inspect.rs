//! Summarising a file without building batches: what `utilake inspect` shows.

use utilake_core::Fingerprint;

use utilake_mms_decode::IRow;
use utilake_mms_registry::{Registry, ResolveOptions};

use crate::error::{RowError, RowFailure, SinkError, ValueError};
use utilake_mms_decode::fields::Row;
use utilake_mms_decode::scan::{Flow, Pos, Sink};

/// One block as seen in the file.
#[derive(Debug, Clone)]
pub struct BlockSummary {
    /// Line of the I row.
    pub line: u64,
    /// Report type.
    pub report_type: String,
    /// Report subtype.
    pub report_subtype: String,
    /// Version.
    pub version: u16,
    /// Columns.
    pub columns: Vec<String>,
    /// Fingerprint.
    pub fingerprint: Fingerprint,
    /// D rows.
    pub rows: u64,
    /// `conforms`, `deviates`, `derived`, or the resolve error's code.
    pub status: String,
    /// The table that claims the report pair, whether or not this shape
    /// of it resolved; `None` when no table does.
    pub table: Option<String>,
}

/// Collects block summaries.
pub struct InspectSink<'r> {
    registry: &'r Registry,
    opts: ResolveOptions,
    /// Blocks in order.
    pub blocks: Vec<BlockSummary>,
    /// The first C row's fields.
    pub header: Option<Vec<String>>,
}

impl<'r> InspectSink<'r> {
    /// A sink resolving against `registry`.
    pub fn new(registry: &'r Registry, opts: ResolveOptions) -> Self {
        Self {
            registry,
            opts,
            blocks: Vec::new(),
            header: None,
        }
    }
}

impl Sink for InspectSink<'_> {
    type Reject = ValueError;
    type Fatal = SinkError;
    fn comment(&mut self, row: &Row<'_>, _: Pos) -> Result<(), SinkError> {
        if self.header.is_none() {
            self.header = Some(
                row.iter()
                    .map(|f| String::from_utf8_lossy(f).into_owned())
                    .collect(),
            );
        }
        Ok(())
    }

    fn header(&mut self, irow: &IRow<'_>, pos: Pos) -> Result<(), SinkError> {
        use utilake_core::Diagnose as _;
        // The table is known whenever its report pair is, also when this
        // shape of it does not resolve.
        let table = self
            .registry
            .declared_by_pair(irow.report_type, irow.report_subtype)
            .map(|id| self.registry.table(id).name.clone());
        let status = match self.registry.resolve(irow, self.opts) {
            Ok(r) => r.status.label().to_owned(),
            Err(e) => e.code().to_string(),
        };
        self.blocks.push(BlockSummary {
            line: pos.line,
            report_type: irow.report_type.to_owned(),
            report_subtype: irow.report_subtype.to_owned(),
            version: irow.version,
            columns: irow.columns.iter().map(|c| (*c).to_owned()).collect(),
            fingerprint: irow.fingerprint(),
            rows: 0,
            status,
            table,
        });
        Ok(())
    }

    fn row(&mut self, _: &Row<'_>, _: Pos) -> Result<(), RowFailure> {
        let Some(b) = self.blocks.last_mut() else {
            return Err(RowError::TooShort(0).into());
        };
        b.rows += 1;
        Ok(())
    }

    fn bad_row(&mut self, _: &[u8], _: Pos, _: &RowError) -> Flow {
        Flow::Continue
    }

    fn footer(&mut self, _: Option<u64>, _: u64, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use utilake_mms_registry::Mode;

    use super::*;
    use utilake_mms_decode::scan::{ScanOptions, scan};

    #[test]
    fn summarises_multi_block_file() {
        let r = Registry::load_embedded();
        let fixture = std::fs::read_to_string(format!(
            "{}/../../testdata/csv/multi_block_dispatchis.csv",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        // The fixture's blocks, plus one for a report pair no table claims
        // and one laid out as no run of its table is, put in before the
        // footer.
        let footer = fixture.rfind("C,\"END OF REPORT\"").unwrap();
        let text = format!(
            "{}I,UTILAKE_TEST,NEW_THING,1,WHEN,AMOUNT\nD,UTILAKE_TEST,NEW_THING,1,\"2026/10/01 13:40:00\",1.5\nD,UTILAKE_TEST,NEW_THING,1,\"2026/10/01 13:45:00\",2.5\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,SCADAVALUE,DUID\nD,DISPATCH,UNIT_SCADA,1,\"2026/10/01 13:40:00\",1.5,X\n{}",
            &fixture[..footer],
            &fixture[footer..]
        );
        let text = text.into_bytes();
        let mut s = InspectSink::new(&r, ResolveOptions { mode: Mode::Strict });
        let stats = scan(
            text.as_slice(),
            &mut s,
            &ScanOptions {
                max_bad_rows: 1000,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.blocks, 9);
        assert_eq!(s.blocks.len(), 9);
        assert_eq!(s.header.as_ref().unwrap()[1], "NEMP.WORLD");
        let price = s
            .blocks
            .iter()
            .find(|b| b.report_subtype == "PRICE")
            .unwrap();
        assert_eq!(price.table.as_deref(), Some("DISPATCHPRICE"));
        // The live feed's DISPATCHPRICE is a run of the report folders'
        // history; the swapped UNIT_SCADA is no run's.
        assert_eq!(price.status, "conforms");
        assert_eq!(price.rows, 3);
        let swapped = s
            .blocks
            .iter()
            .find(|b| b.report_subtype == "UNIT_SCADA")
            .unwrap();
        assert_eq!(swapped.table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
        assert_eq!(swapped.status, "schema.deviates");
        let unknown = s
            .blocks
            .iter()
            .find(|b| b.report_subtype == "NEW_THING")
            .unwrap();
        assert_eq!(unknown.status, "schema.unknown_table");
        assert_eq!(unknown.table, None);
        // A D row before any I row is an error from the scanner, not the sink.
        let mut s = InspectSink::new(&r, ResolveOptions::default());
        assert!(scan(b"D,a,b,1,x\n".as_slice(), &mut s, &ScanOptions::default()).is_err());
        let r = s.row(
            &utilake_mms_decode::fields::Splitter::default().split(b"D,a"),
            Pos::default(),
        );
        assert!(matches!(r, Err(RowFailure::Row(e)) if *e == RowError::TooShort(0)));
    }
}
