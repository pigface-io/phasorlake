//! Throughput of the plain decoders through to Arrow batches: a header-row
//! CSV of the gas bulletin board, and a WEMDE JSON document, each built
//! from its fixture to a size where the per-file cost does not dominate.

use std::fmt::Write as _;
use std::hint::black_box;
use std::io::Cursor;
use std::sync::Arc;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use url::Url;
use utilake_acquire::{LocalFile, RemoteFile};
use utilake_core::Diagnostic;
use utilake_kit::{
    Block, BlockSink, DecodeCtx, DecodeOptions, Decoder, Flow, RowAt, Strictness, TableRef,
    Transform,
};
use utilake_mms_conform::plain::{PlainCsvDecoder, WemdeDecoder};
use utilake_mms_decode::Fingerprint;
use utilake_mms_registry::Registry;
use utilake_plain_decode::feeds::feed;

const CSV_BYTES: usize = 16 << 20;
const JSON_BYTES: usize = 8 << 20;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../testdata/plain/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The GBB fixture's rows repeated, a value varied so the number parser
/// sees different digits.
fn gbb() -> Vec<u8> {
    let text = fixture("gbb_actual_flow_storage_last31.csv");
    let mut lines = text.lines();
    let header = lines.next().unwrap();
    let rows: Vec<&str> = lines.collect();
    let mut out = String::with_capacity(CSV_BYTES + 1024);
    out.push_str(header);
    out.push_str("\r\n");
    let mut i = 0usize;
    while out.len() < CSV_BYTES {
        let fields: Vec<&str> = rows[i % rows.len()].split(',').collect();
        for (j, f) in fields.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            if j == 4 {
                let _ = write!(out, "{:.3}", (i as f64) * 0.731);
            } else {
                out.push_str(f);
            }
        }
        out.push_str("\r\n");
        i += 1;
    }
    out.into_bytes()
}

/// A WEMDE reference trading price document with as many intervals as
/// fill `JSON_BYTES`.
fn wemde() -> Vec<u8> {
    let mut out = String::with_capacity(JSON_BYTES + 1024);
    out.push_str(
        r#"{"transactionId":"4a1b8c2e-0000-4000-8000-000000000001","data":{"tradingDay":"2026-10-21","referenceTradingPrices":["#,
    );
    let mut i = 0u64;
    while out.len() < JSON_BYTES {
        if i > 0 {
            out.push(',');
        }
        let (h, m) = ((i / 2) % 24, (i % 2) * 30);
        let _ = write!(
            out,
            r#"{{"tradingInterval":"2026-10-21T{h:02}:{m:02}:00+08:00","referenceTradingPrice":{:.2},"isPublished":true}}"#,
            (i as f64) * 0.37
        );
        i += 1;
    }
    out.push_str("]}}");
    out.into_bytes()
}

struct Drop;
impl BlockSink for Drop {
    fn block_started(
        &mut self,
        _: &Arc<TableRef>,
        _: u16,
        _: Fingerprint,
        _: &Arc<Transform>,
        _: &str,
    ) {
    }
    fn batch(&mut self, b: Block) -> Result<(), Box<Diagnostic>> {
        black_box(b.batch.num_rows());
        Ok(())
    }
    fn warning(&mut self, _: Diagnostic) {}
    fn bad_row(&mut self, _: &[u8], _: RowAt, _: Diagnostic) -> Flow {
        Flow::Continue
    }
}

fn run(d: &dyn Decoder, url: &str, entry: &str, bytes: &[u8]) -> u64 {
    let file = LocalFile {
        remote: RemoteFile {
            url: Url::parse(url).unwrap(),
            source: "bench",
            table: None,
            period: None,
            size: None,
            modified: None,
            modified_at: None,
        },
        path: std::path::PathBuf::from("/none"),
        sha256: String::new(),
        size: bytes.len() as u64,
        cached: true,
        etag: None,
        last_modified: None,
        last_modified_at: None,
        fetched_at: None,
        previous: None,
    };
    let options = DecodeOptions {
        strictness: Strictness::Permissive,
        max_bad_rows: 0,
        batch_rows: 65_536,
    };
    let ctx = DecodeCtx {
        file: &file,
        entry,
        period: None,
        options: &options,
        previous: None,
    };
    d.decode(&mut Cursor::new(bytes), &ctx, &mut Drop, &mut Vec::new())
        .unwrap()
        .rows
}

fn bench(c: &mut Criterion) {
    let registry = Registry::load_embedded();
    let mut g = c.benchmark_group("plain");
    g.sample_size(10);

    let csv = gbb();
    let d = PlainCsvDecoder::new(registry.clone(), feed("gbb").unwrap());
    let url = "https://www.nemweb.com.au/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV";
    assert!(run(&d, url, "GasBBActualFlowStorageLast31.CSV", &csv) > 0);
    g.throughput(Throughput::Bytes(csv.len() as u64));
    g.bench_function("gbb_csv", |b| {
        b.iter(|| {
            black_box(run(
                &d,
                url,
                "GasBBActualFlowStorageLast31.CSV",
                black_box(&csv),
            ))
        });
    });

    let json = wemde();
    let d = WemdeDecoder::new(registry, feed("wemde").unwrap());
    let url = "https://data.wa.aemo.com.au/public/market-data/wemde/referenceTradingPrice/previous/ReferenceTradingPrice_20261021.zip";
    assert!(run(&d, url, "ReferenceTradingPrice_20261021.json", &json) > 0);
    g.throughput(Throughput::Bytes(json.len() as u64));
    g.bench_function("wemde_json", |b| {
        b.iter(|| {
            black_box(run(
                &d,
                url,
                "ReferenceTradingPrice_20261021.json",
                black_box(&json),
            ))
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
