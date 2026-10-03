//! Throughput of the scanner alone and of the typed Arrow path, over a
//! synthetic file generated from the DISPATCH_UNIT_SCADA fixture.

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nemlake_core::IRow;
use nemlake_mms::arrow_sink::{ArrowSink, BatchSink, BlockBatch};
use nemlake_mms::scan::{Flow, Pos, ScanOptions, Sink, scan};
use nemlake_mms::{Row, RowError, RowFailure, SinkError};
use nemlake_schema::{Registry, ResolveOptions, TypeOptions};

const TARGET_BYTES: usize = 64 << 20;

fn synthetic() -> Vec<u8> {
    let fixture = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/csv/DISPATCH_UNIT_SCADA_v1_head.csv"
    ))
    .unwrap();
    let text = String::from_utf8(fixture).unwrap();
    let mut lines = text.lines();
    let header = lines.next().unwrap();
    let irow = lines.next().unwrap();
    let rows: Vec<&str> = lines.filter(|l| l.starts_with("D,")).collect();
    let mut out = String::with_capacity(TARGET_BYTES + 1024);
    out.push_str(header);
    out.push_str("\r\n");
    out.push_str(irow);
    out.push_str("\r\n");
    let mut i = 0usize;
    while out.len() < TARGET_BYTES {
        let r = rows[i % rows.len()];
        // Vary the value so the float parser sees different digits.
        let (prefix, _) = r.rsplit_once(',').unwrap();
        out.push_str(prefix);
        out.push(',');
        let _ = write!(out, "{:.6}", (i as f64) * 0.731);
        out.push_str("\r\n");
        i += 1;
    }
    let _ = write!(out, "C,\"END OF REPORT\",{}\r\n", i + 3);
    out.into_bytes()
}

struct Count(u64);
impl Sink for Count {
    fn comment(&mut self, _: &Row<'_>, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
    fn header(&mut self, _: &IRow<'_>, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
    fn row(&mut self, row: &Row<'_>, _: Pos) -> Result<(), RowFailure> {
        self.0 += row.len() as u64;
        Ok(())
    }
    fn bad_row(&mut self, _: &[u8], _: Pos, _: &RowError) -> Flow {
        Flow::Continue
    }
    fn footer(&mut self, _: Option<u64>, _: u64, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
}

struct Drop;
impl BatchSink for Drop {
    fn batch(&mut self, b: BlockBatch) -> Result<(), SinkError> {
        black_box(b.batch.num_rows());
        Ok(())
    }
    fn warning(&mut self, _: nemlake_core::Diagnostic) {}
    fn bad_row(&mut self, _: &[u8], _: Pos, _: &RowError) -> Flow {
        Flow::Continue
    }
}

fn bench(c: &mut Criterion) {
    let data = synthetic();
    let registry = Registry::load_embedded(TypeOptions::default());
    let mut g = c.benchmark_group("parse");
    g.throughput(Throughput::Bytes(data.len() as u64));
    g.sample_size(10);
    g.bench_function("scan_split_only", |b| {
        b.iter(|| {
            let mut s = Count(0);
            let st = scan(black_box(data.as_slice()), &mut s, &ScanOptions::default()).unwrap();
            black_box((st.rows, s.0));
        });
    });
    g.bench_function("scan_typed_arrow", |b| {
        b.iter(|| {
            let mut s = ArrowSink::new(&registry, ResolveOptions::default(), 65_536, Drop);
            let st = scan(black_box(data.as_slice()), &mut s, &ScanOptions::default()).unwrap();
            black_box(st.rows);
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
