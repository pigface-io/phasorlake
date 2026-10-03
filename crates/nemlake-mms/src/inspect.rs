//! Summarising a file without building batches: what `nemlake inspect` shows.

use nemlake_core::{Fingerprint, IRow};
use nemlake_schema::infer::Sampler;
use nemlake_schema::{Registry, ResolveOptions};

use crate::arrow_sink::PREFIX_FIELDS;
use crate::error::{RowError, RowFailure, SinkError};
use crate::fields::Row;
use crate::scan::{Flow, Pos, Sink};

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
    /// `exact`, `aliased`, `drift`, or the resolve error's code.
    pub status: String,
    /// The table the registry mapped it to.
    pub table: Option<String>,
    /// Type evidence from the rows, for `--infer`.
    pub sampler: Sampler,
}

/// Collects block summaries.
pub struct InspectSink<'r> {
    registry: &'r Registry,
    opts: ResolveOptions,
    /// Blocks in order.
    pub blocks: Vec<BlockSummary>,
    /// The first C row's fields.
    pub header: Option<Vec<String>>,
    /// Rows to sample per block for type evidence.
    pub sample_rows: u64,
}

impl<'r> InspectSink<'r> {
    /// A sink resolving against `registry`.
    pub fn new(registry: &'r Registry, opts: ResolveOptions, sample_rows: u64) -> Self {
        Self {
            registry,
            opts,
            blocks: Vec::new(),
            header: None,
            sample_rows,
        }
    }
}

impl Sink for InspectSink<'_> {
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
        use nemlake_core::Diagnose as _;
        let (status, table) = match self.registry.resolve(irow, self.opts) {
            Ok(r) => (
                r.status.label().to_owned(),
                Some(self.registry.table(r.table).name.clone()),
            ),
            Err(e) => (e.code().to_string(), None),
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
            sampler: Sampler::new(irow.columns.len()),
        });
        Ok(())
    }

    fn row(&mut self, row: &Row<'_>, _: Pos) -> Result<(), RowFailure> {
        let Some(b) = self.blocks.last_mut() else {
            return Err(RowError::TooShort(0).into());
        };
        b.rows += 1;
        if b.rows <= self.sample_rows {
            b.sampler
                .observe((PREFIX_FIELDS..row.len()).filter_map(|i| row.get(i)));
        }
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
    use nemlake_schema::{Mode, TypeOptions};

    use super::*;
    use crate::scan::{ScanOptions, scan};

    #[test]
    fn summarises_multi_block_file() {
        let r = Registry::load_embedded(TypeOptions::default());
        let text = std::fs::read(format!(
            "{}/../../testdata/csv/multi_block_dispatchis.csv",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let mut s = InspectSink::new(&r, ResolveOptions { mode: Mode::Strict }, 100);
        let stats = scan(
            text.as_slice(),
            &mut s,
            &ScanOptions {
                max_bad_rows: 1000,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.blocks, 7);
        assert_eq!(s.blocks.len(), 7);
        assert_eq!(s.header.as_ref().unwrap()[1], "NEMP.WORLD");
        let price = s
            .blocks
            .iter()
            .find(|b| b.report_subtype == "PRICE")
            .unwrap();
        assert_eq!(price.table.as_deref(), Some("DISPATCHPRICE"));
        assert_eq!(price.status, "exact");
        assert_eq!(price.rows, 3);
        let unknown = s
            .blocks
            .iter()
            .find(|b| b.report_subtype == "LOCAL_PRICE")
            .unwrap();
        assert_eq!(unknown.status, "schema.unknown_table");
        assert_eq!(unknown.table, None);
        assert_eq!(unknown.sampler.rows, 3);
        assert_eq!(
            unknown.sampler.column_type(0),
            nemlake_schema::ColumnType::Timestamp
        );
        let draft = nemlake_schema::infer::draft_table(
            nemlake_schema::Market::Nem,
            &IRow {
                report_type: &unknown.report_type,
                report_subtype: &unknown.report_subtype,
                version: unknown.version,
                columns: &unknown
                    .columns
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            },
            &unknown.sampler,
        );
        assert_eq!(draft.name, "DISPATCH_LOCAL_PRICE");
        // A D row before any I row is an error from the scanner, not the sink.
        let mut s = InspectSink::new(&r, ResolveOptions::default(), 1);
        assert!(scan(b"D,a,b,1,x\n".as_slice(), &mut s, &ScanOptions::default()).is_err());
        let r = s.row(
            &crate::fields::Splitter::default().split(b"D,a"),
            Pos::default(),
        );
        assert!(matches!(r, Err(RowFailure::Row(e)) if *e == RowError::TooShort(0)));
    }
}
