//! The real data.wa.aemo.com.au, opt-in: `UTILAKE_NET_TESTS=1 cargo test -p utilake-wem --test net`.
//! Lists this month under every source of the wem kit and downloads the
//! smallest file listed twice, the second time from the archive.

use std::sync::Arc;
use std::time::Duration;

use utilake_acquire::{Archive, Fetcher, FetcherConfig, ReqwestTransport};
use utilake_core::{TimeRange, YearMonth};
use utilake_kit::Query;
use utilake_mms_registry::Registry;

#[tokio::test]
async fn lists_every_source_and_downloads_the_smallest_file() {
    if std::env::var("UTILAKE_NET_TESTS").is_err() {
        eprintln!("skipped: set UTILAKE_NET_TESTS=1 to hit data.wa.aemo.com.au");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let transport = ReqwestTransport::new(
        "utilake-net-test",
        Duration::from_secs(30),
        Duration::from_secs(120),
    )
    .unwrap();
    let fetcher = Fetcher::new(
        Arc::new(transport),
        Archive::new(dir.path()),
        FetcherConfig::default(),
    );
    let kit = utilake_wem::kit(
        &utilake_wem::WemConfig::default(),
        Registry::load_embedded(),
    )
    .unwrap();
    let now = utilake_core::clock::now_utc_rfc3339();
    let ym = YearMonth::new(now[..4].parse().unwrap(), now[5..7].parse().unwrap()).unwrap();
    let query = Query {
        exclude: Vec::new(),
        tables: vec![],
        range: TimeRange::months(ym, ym),
        instants: None,
    };
    let mut all = Vec::new();
    for source in &kit.sources {
        let files = source.list(&query, &fetcher).await.unwrap();
        eprintln!("{}: {} files listed for {ym}", source.id(), files.len());
        all.extend(files);
    }
    let smallest = all
        .iter()
        .filter(|f| f.size.is_some_and(|s| s > 0))
        .min_by_key(|f| f.size)
        .expect("a file with a size is listed this month");
    eprintln!(
        "smallest: {} ({} bytes)",
        smallest.url,
        smallest.size.unwrap()
    );
    let local = fetcher.fetch(smallest).await.unwrap();
    assert!(!local.cached);
    assert_eq!(Some(local.size), smallest.size);
    let again = fetcher.fetch(smallest).await.unwrap();
    assert!(again.cached);
    assert_eq!(again.sha256, local.sha256);
}
