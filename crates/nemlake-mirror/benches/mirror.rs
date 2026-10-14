//! The mirror without a network: reading a big directory listing strictly.

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

const LISTING_ROWS: usize = 10_000;

/// An IIS directory listing of `LISTING_ROWS` zips, as nemweb serves one.
fn listing() -> Vec<u8> {
    let mut out = String::from(
        "<html><body><pre><A HREF=\"/Data_Archive/\">[To Parent Directory]</A><br><br>",
    );
    for i in 0..LISTING_ROWS {
        let _ = write!(
            out,
            "    Tuesday, December 9, 2025 09:05 AM   {size} <A HREF=\"/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_TABLE{i}_202401010000.zip\">PUBLIC_DVD_TABLE{i}_202401010000.zip</A><br>",
            size = 1000 + i
        );
    }
    out.push_str("</pre><hr></body></html>");
    out.into_bytes()
}

fn bench(c: &mut Criterion) {
    let html = listing();
    let mut g = c.benchmark_group("mirror");
    g.sample_size(10);
    g.throughput(Throughput::Bytes(html.len() as u64));
    g.bench_function("listing_parse", |b| {
        b.iter(|| {
            black_box(
                nemlake_mirror::listing::parse(black_box(&html))
                    .unwrap()
                    .len(),
            )
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
