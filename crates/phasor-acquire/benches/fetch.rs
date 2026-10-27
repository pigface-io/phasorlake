//! The fetch path without a network: parsing a big directory listing, and
//! downloading one large file through the `Fetcher` into the archive.

use std::fmt::Write as _;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use phasor_acquire::transport::{Canned, StaticTransport};
use phasor_acquire::{Archive, Fetcher, FetcherConfig};
use phasor_kit::RemoteFile;
use url::Url;

const LISTING_ROWS: usize = 10_000;
const DOWNLOAD_BYTES: usize = 64 << 20;
const CHUNK: usize = 64 << 10;

/// An IIS directory listing of `LISTING_ROWS` zips, as nemweb serves one.
fn listing() -> Vec<u8> {
    let mut out = String::from(
        "<html><body><pre><A HREF=\"/Data_Archive/\">[To Parent Directory]</A><br><br>",
    );
    for i in 0..LISTING_ROWS {
        let _ = write!(
            out,
            " 2/13/2024  8:27 AM {size} <A HREF=\"/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_TABLE{i}_202401010000.zip\">PUBLIC_DVD_TABLE{i}_202401010000.zip</A><br>",
            size = 1000 + i
        );
    }
    out.push_str("</pre><hr></body></html>");
    out.into_bytes()
}

fn bench(c: &mut Criterion) {
    let html = listing();
    let mut g = c.benchmark_group("fetch");
    g.sample_size(10);
    g.throughput(Throughput::Bytes(html.len() as u64));
    g.bench_function("listing_parse", |b| {
        b.iter(|| black_box(phasor_acquire::listing::parse(black_box(&html)).len()));
    });

    let body: Vec<u8> = (0..DOWNLOAD_BYTES).map(|i| (i * 31 % 251) as u8).collect();
    let url = "https://h/Data_Archive/PUBLIC_DVD_BIG_202401010000.zip";
    let transport = Arc::new(StaticTransport::default());
    let mut canned = Canned::ok(&body);
    canned.chunk = CHUNK;
    transport.add(url, canned);
    let remote = RemoteFile {
        url: Url::parse(url).unwrap(),
        source: "bench",
        table: None,
        period: None,
        size: Some(DOWNLOAD_BYTES as u64),
        modified: None,
        modified_at: None,
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    g.throughput(Throughput::Bytes(DOWNLOAD_BYTES as u64));
    g.bench_function("download", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let fetcher = Fetcher::new(
                    Arc::new(Arc::clone(&transport)),
                    Archive::new(dir.path()),
                    FetcherConfig {
                        min_interval: Duration::ZERO,
                        ..Default::default()
                    },
                );
                (dir, fetcher)
            },
            |(dir, fetcher)| {
                let local = rt.block_on(fetcher.fetch(&remote)).unwrap();
                black_box(local.size);
                drop(dir);
            },
            BatchSize::PerIteration,
        );
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
