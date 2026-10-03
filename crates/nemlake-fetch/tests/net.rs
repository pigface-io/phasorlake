//! The real nemweb, opt-in: `NEMLAKE_NET_TESTS=1 cargo test -p nemlake-fetch --test net`.
//! Lists January 2024 and downloads the smallest zip in it twice, the second
//! time from the cache.

use std::sync::Arc;
use std::time::Duration;

use nemlake_core::YearMonth;
use nemlake_fetch::cache::DiskCache;
use nemlake_fetch::{Fetcher, FetcherConfig, MmsdmArchive, Query, ReqwestTransport, Source};

#[tokio::test]
async fn lists_and_downloads_the_smallest_zip() {
    if std::env::var("NEMLAKE_NET_TESTS").is_err() {
        eprintln!("skipped: set NEMLAKE_NET_TESTS=1 to hit nemweb");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let transport = ReqwestTransport::new(
        "nemlake-net-test",
        Duration::from_secs(30),
        Duration::from_secs(120),
    )
    .unwrap();
    let fetcher = Fetcher::new(
        Arc::new(transport),
        DiskCache::new(dir.path()),
        FetcherConfig::default(),
    );
    let source = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
    let ym = YearMonth::new(2024, 1).unwrap();
    let files = source
        .list(
            &Query {
                exclude: Vec::new(),
                tables: vec![],
                from: ym,
                to: ym,
            },
            &fetcher,
        )
        .await
        .unwrap();
    assert!(files.len() > 100, "{} files listed", files.len());
    let smallest = files
        .iter()
        .filter(|f| f.size.is_some())
        .min_by_key(|f| f.size)
        .unwrap();
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
