//! The fetch path without a network: downloading one large file through the
//! `Fetcher` into the archive.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use phasor_mirror::Url;
use phasor_mirror::net::transport::{Canned, StaticTransport};
use phasor_mirror::net::{Archive, Fetcher, FetcherConfig, RemoteFile};

const DOWNLOAD_BYTES: usize = 64 << 20;
const CHUNK: usize = 64 << 10;

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("fetch");
    g.sample_size(10);

    let body: Vec<u8> = (0..DOWNLOAD_BYTES).map(|i| (i * 31 % 251) as u8).collect();
    let url = "https://h/Data_Archive/PUBLIC_DVD_BIG_202401010000.zip";
    let transport = Arc::new(StaticTransport::default());
    let mut canned = Canned::ok(&body);
    canned.chunk = CHUNK;
    transport.add(url, canned);
    let remote = RemoteFile {
        url: Url::parse(url).unwrap(),
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
                        attempts: 1,
                        backoff: Duration::ZERO,
                        max_backoff: Duration::ZERO,
                        min_interval: Duration::ZERO,
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
