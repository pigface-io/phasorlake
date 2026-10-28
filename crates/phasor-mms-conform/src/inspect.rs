//! Summarising a file without building batches: what `phasor inspect` shows.

use std::path::Path;

use phasor_core::{Diagnostic, Fingerprint};
use phasor_mms_decode::IRow;
use phasor_mms_decode::scan::{ScanOptions, scan};
use phasor_registry::{Mode, Registry, ResolveOptions};
use phasor_zip::{ArchiveError, WalkError, walk_csv_entries};

use phasor_core::Diagnose as _;

use crate::error::{RowError, RowFailure, SinkError, ValueError};
use phasor_mms_decode::fields::Row;
use phasor_mms_decode::scan::{Flow, Pos, Sink};

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
    /// The table that claims the report pair, whether or not this shape resolved.
    pub table: Option<String>,
}

/// What a file holds, block by block, and what of it could not be read.
#[derive(Debug, Default)]
pub struct Inspection {
    /// Every block of every CSV, in order, with the entry it came from.
    pub blocks: Vec<(String, BlockSummary)>,
    /// Entries whose footer disagrees: entry, lines declared, lines read.
    pub footers: Vec<(String, u64, u64)>,
    /// The entries of a zip that are not CSVs.
    pub skipped: Vec<String>,
    /// The entries of a zip that could not be read.
    pub failed: Vec<ArchiveError>,
}

/// Why a file could not be inspected at all.
#[derive(Debug)]
pub enum InspectError {
    /// The file could not be opened.
    Open(std::io::Error),
    /// A CSV's rows could not be scanned.
    Scan(Box<Diagnostic>),
    /// The zip could not be walked.
    Archive(ArchiveError),
}

/// Every block of every CSV in `path` (a CSV or zip), each resolved strictly against `registry`.
pub fn file(registry: &Registry, path: &Path, tmp: &Path) -> Result<Inspection, InspectError> {
    let opts = ResolveOptions {
        mode: Mode::Strict,
        ..Default::default()
    };
    let mut out = Inspection::default();
    let mut handle = |entry: &str, reader: &mut dyn std::io::Read| -> Result<(), InspectError> {
        let mut sink = InspectSink::new(registry, opts);
        let stats = scan(
            reader,
            &mut sink,
            &ScanOptions {
                max_bad_rows: u64::MAX,
                ..Default::default()
            },
        )
        .map_err(|e| InspectError::Scan(Box::new(e.diagnostic())))?;
        out.blocks
            .extend(sink.blocks.into_iter().map(|b| (entry.to_owned(), b)));
        if stats.footer_mismatch() {
            out.footers.push((
                entry.to_owned(),
                stats.footer_declared.unwrap_or(0),
                stats.lines,
            ));
        }
        Ok(())
    };
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
    {
        let mut f = std::fs::File::open(path).map_err(InspectError::Open)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        handle(&name, &mut f)?;
        return Ok(out);
    }
    let mut skipped = Vec::new();
    let walked = walk_csv_entries(path, tmp, &mut |e| handle(&e.name, e.reader), &mut |o| {
        skipped.push(o.name);
    })
    .map_err(|e| match e {
        WalkError::Archive(a) => InspectError::Archive(a),
        WalkError::Callback(c) => c,
    })?;
    out.skipped = skipped;
    out.failed = walked.failed;
    Ok(out)
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
        use phasor_core::Diagnose as _;
        let table = self
            .registry
            .declared_by_pair(self.opts.market, irow.report_type, irow.report_subtype)
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
    use phasor_registry::Mode;

    use super::*;
    use phasor_mms_decode::scan::{ScanOptions, scan};

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
            "{}I,PHASOR_TEST,NEW_THING,1,WHEN,AMOUNT\nD,PHASOR_TEST,NEW_THING,1,\"2026/10/01 13:40:00\",1.5\nD,PHASOR_TEST,NEW_THING,1,\"2026/10/01 13:45:00\",2.5\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,SCADAVALUE,DUID\nD,DISPATCH,UNIT_SCADA,1,\"2026/10/01 13:40:00\",1.5,X\n{}",
            &fixture[..footer],
            &fixture[footer..]
        );
        let text = text.into_bytes();
        let mut s = InspectSink::new(
            &r,
            ResolveOptions {
                mode: Mode::Strict,
                ..Default::default()
            },
        );
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
            &phasor_mms_decode::fields::Splitter::default().split(b"D,a"),
            Pos::default(),
        );
        assert!(matches!(r, Err(RowFailure::Row(e)) if *e == RowError::TooShort(0)));
    }
}
