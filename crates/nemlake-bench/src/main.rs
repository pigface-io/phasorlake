//! End-to-end benchmark: ingest a synthetic month-sized zip built from the
//! DISPATCH_UNIT_SCADA fixture through the real pipeline into a temp home,
//! and print wall time and peak heap as JSON for `xtask bench`. Peak RSS is
//! measured from outside by `/usr/bin/time`. The allocator is `peak_alloc`,
//! an external crate, so this crate stays free of `unsafe`.

use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nemlake_acquire::cache::DiskCache;
use nemlake_acquire::{Fetcher, FetcherConfig, OfflineTransport};
use nemlake_core::paths::Home;
use nemlake_pipeline::{Deps, Input, Plan, RunOptions, run};
use nemlake_registry::{Registry, TypeOptions};
use nemlake_writer::{Manifest, ManifestHandle};
use peak_alloc::PeakAlloc;
use tokio_util::sync::CancellationToken;

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

/// Rows in the synthetic file: about 1/8 of a real month of DISPATCH_UNIT_SCADA,
/// enough to measure, small enough for a pre-push hook.
const ROWS: usize = 524_288;

fn synthetic_zip(dir: &Path) -> std::path::PathBuf {
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/csv/DISPATCH_UNIT_SCADA_v1_head.csv"
    ))
    .expect("fixture");
    let mut lines = fixture.lines();
    let header = lines.next().expect("C row");
    let irow = lines.next().expect("I row");
    let rows: Vec<&str> = lines.filter(|l| l.starts_with("D,")).collect();
    let mut csv = String::with_capacity(ROWS * 64);
    csv.push_str(header);
    csv.push_str("\r\n");
    csv.push_str(irow);
    csv.push_str("\r\n");
    for i in 0..ROWS {
        let r = rows[i % rows.len()];
        let (prefix, _) = r.rsplit_once(',').expect("a value");
        // Advance the timestamp every 5 minutes so partitions and sort order behave as in a real file.
        let minute = (i / rows.len()) * 5;
        let (day, rem) = (minute / 1440, minute % 1440);
        let prefix = prefix.replacen(
            "2024/01/01 00:05:00",
            &format!(
                "2024/01/{:02} {:02}:{:02}:00",
                (day % 28) + 1,
                rem / 60,
                rem % 60
            ),
            1,
        );
        let _ = writeln!(csv, "{prefix},{:.6}\r", (i as f64) * 0.731);
    }
    let _ = write!(csv, "C,\"END OF REPORT\",{}\r\n", ROWS + 3);
    let path = dir.join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip");
    let f = std::fs::File::create(&path).expect("zip");
    let mut w = zip::ZipWriter::new(f);
    w.start_file(
        "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV",
        zip::write::SimpleFileOptions::default(),
    )
    .expect("entry");
    w.write_all(csv.as_bytes()).expect("write");
    w.finish().expect("finish");
    path
}

fn main() {
    let dir = tempfile::tempdir().expect("tempdir");
    let zip_path = synthetic_zip(dir.path());
    let zip_bytes = std::fs::metadata(&zip_path).map_or(0, |m| m.len());
    let home = Home::at(dir.path().join("home"));
    home.ensure_dirs().expect("home");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let registry = Registry::load_embedded(TypeOptions::default());
    PEAK.reset_peak_usage();
    let start = Instant::now();
    let (summary, exit) = runtime.block_on(async {
        let (manifest, _thread) =
            ManifestHandle::spawn_with(Manifest::open(&home.manifest()).expect("manifest"));
        let fetcher = Arc::new(Fetcher::new(
            Arc::new(OfflineTransport),
            DiskCache::new(home.cache()),
            FetcherConfig::default(),
        ));
        let deps = Deps {
            registry: registry.clone(),
            fetcher,
            manifest,
            home: home.clone(),
        };
        let plan = Plan {
            request: "bench".into(),
            input: Input::Local(vec![zip_path.clone()]),
            options: RunOptions {
                jobs: 1,
                ..RunOptions::default()
            },
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let rep = run(plan, deps, tx, CancellationToken::new()).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), drain).await;
        (rep.summary, rep.exit_code)
    });
    let elapsed = start.elapsed();
    let peak_mb = f64::from(PEAK.peak_usage_as_mb());
    assert_eq!(exit, 0, "the benchmark ingest must succeed: {summary:?}");
    assert_eq!(summary.rows, ROWS as u64, "every row must land");
    let mut out = serde_json::Map::new();
    let mut m = serde_json::Map::new();
    m.insert(
        "max_ms".into(),
        serde_json::json!((elapsed.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0),
    );
    m.insert(
        "max_peak_heap_mb".into(),
        serde_json::json!((peak_mb * 1000.0).round() / 1000.0),
    );
    m.insert(
        "min_rows_per_s".into(),
        serde_json::json!((ROWS as f64 / elapsed.as_secs_f64()).round()),
    );
    out.insert(
        "e2e/ingest_synthetic_zip".into(),
        serde_json::Value::Object(m),
    );
    eprintln!(
        "ingested {} rows from a {:.1} MB zip in {:.0} ms; peak heap {:.1} MB",
        summary.rows,
        zip_bytes as f64 / 1e6,
        elapsed.as_secs_f64() * 1000.0,
        peak_mb
    );
    println!("{}", serde_json::Value::Object(out));
}
