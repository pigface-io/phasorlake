//! Downloading every file of the crawl record into the archive.

use futures::StreamExt as _;
use nemlake_acquire::{FetchError, Fetcher, RemoteFile};
use nemlake_core::{Diagnose as _, Diagnostic};
use serde::Serialize;
use url::Url;

use crate::crawl::{Mirror, RemoteKind, read_record};
use crate::error::MirrorError;

/// The source name the mirror's downloads carry.
pub const SOURCE: &str = "nemweb-mirror";

/// One file dealt with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileDone<'a> {
    /// The file.
    pub url: &'a str,
    /// Whether the archive held it already.
    pub cached: bool,
    /// Its size.
    pub bytes: u64,
    /// Files dealt with so far, failures included.
    pub done: usize,
    /// Files in the crawl record.
    pub total: usize,
}

/// What a download pass did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DownloadSummary {
    /// Files in the crawl record.
    pub files: usize,
    /// Of those, already in the archive.
    pub cached: usize,
    /// Downloaded in this pass.
    pub downloaded: usize,
    /// Bytes downloaded in this pass.
    pub bytes: u64,
    /// Files that could not be downloaded, with why.
    pub failed: Vec<Diagnostic>,
}

/// Downloads every file of the finished crawl, `downloads` at a time,
/// through the fetcher, which skips a file the archive already holds.
///
/// A file that fails is reported and the rest carry on; a refusal by
/// nemweb (`net.blocked`) stops the pass, since every later request would
/// be refused too.
pub async fn download(
    fetcher: &Fetcher,
    mirror: &Mirror,
    downloads: usize,
    mut on_file: impl FnMut(&FileDone<'_>),
) -> Result<DownloadSummary, MirrorError> {
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.crawl_dir().display().to_string(),
        });
    }
    let files: Vec<RemoteFile> = read_record(&mirror.record_path())?
        .into_iter()
        .enumerate()
        .filter(|(_, r)| r.kind == RemoteKind::File)
        .map(|(i, r)| {
            Ok(RemoteFile {
                url: Url::parse(&r.url).map_err(|e| MirrorError::Record {
                    path: mirror.record_path().display().to_string(),
                    line: i as u64 + 1,
                    message: format!("{}: {e}", r.url),
                })?,
                source: SOURCE,
                table: None,
                period: None,
                size: r.size,
                modified: Some(r.modified),
            })
        })
        .collect::<Result<_, MirrorError>>()?;
    let mut summary = DownloadSummary {
        files: files.len(),
        ..DownloadSummary::default()
    };
    let mut results = futures::stream::iter(&files)
        .map(|remote| async move { (remote, fetcher.fetch(remote).await) })
        .buffer_unordered(downloads.max(1));
    let mut done = 0;
    while let Some((remote, result)) = results.next().await {
        done += 1;
        match result {
            Ok(local) => {
                if local.cached {
                    summary.cached += 1;
                } else {
                    summary.downloaded += 1;
                    summary.bytes += local.size;
                }
                on_file(&FileDone {
                    url: remote.url.as_str(),
                    cached: local.cached,
                    bytes: local.size,
                    done,
                    total: summary.files,
                });
            }
            Err(e @ FetchError::Blocked { .. }) => return Err(e.into()),
            Err(e) => summary.failed.push(e.diagnostic()),
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use nemlake_acquire::transport::{Canned, StaticTransport};
    use nemlake_acquire::{Archive, FetcherConfig};

    use super::*;
    use crate::crawl::{crawl, root};

    const PAGE: &str = "<pre> Monday, October 12, 2026 10:45 AM 3 <A HREF=\"/A/x.zip\">x</A><br> Monday, October 12, 2026 10:45 AM 4 <A HREF=\"/A/y.zip\">y</A><br> Monday, October 12, 2026 10:45 AM 5 <A HREF=\"/A/gone.zip\">g</A><br></pre>";

    fn fetcher(t: &Arc<StaticTransport>, archive: &Path) -> Fetcher {
        Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(archive),
            FetcherConfig {
                attempts: 1,
                backoff: Duration::ZERO,
                max_backoff: Duration::ZERO,
                min_interval: Duration::ZERO,
            },
        )
    }

    #[tokio::test]
    async fn downloads_every_crawled_file_once() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/A/", Canned::ok(PAGE.as_bytes()));
        t.add("https://h/A/x.zip", Canned::ok(b"xxx"));
        t.add("https://h/A/y.zip", Canned::ok(b"yyyy"));
        let f = fetcher(&t, &archive);

        let err = download(&f, &mirror, 2, |_| {}).await.unwrap_err();
        assert_eq!(err.diagnostic().code, "mirror.crawl_incomplete");

        crawl(&f, &mirror, &[root("https://h/A/").unwrap()], false, |_| {})
            .await
            .unwrap();
        let mut seen = Vec::new();
        let s = download(&f, &mirror, 2, |d| {
            seen.push((d.url.to_owned(), d.cached, d.total));
        })
        .await
        .unwrap();
        assert_eq!((s.files, s.cached, s.downloaded, s.bytes), (3, 0, 2, 7));
        assert_eq!(s.failed.len(), 1);
        assert_eq!(s.failed[0].code, "net.http_status");
        seen.sort();
        assert_eq!(seen[0], ("https://h/A/x.zip".to_owned(), false, 3));
        assert_eq!(std::fs::read(archive.join("h/A/y.zip")).unwrap(), b"yyyy");

        let s = download(&f, &mirror, 2, |_| {}).await.unwrap();
        assert_eq!((s.cached, s.downloaded), (2, 0));
        assert_eq!(t.request_count(), 1 + 3 + 1);
    }

    #[tokio::test]
    async fn a_refusal_stops_the_pass() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/A/", Canned::ok(PAGE.as_bytes()));
        t.add("https://h/A/x.zip", Canned::status(403));
        let f = fetcher(&t, &home.path().join("archive"));
        crawl(&f, &mirror, &[root("https://h/A/").unwrap()], false, |_| {})
            .await
            .unwrap();
        let err = download(&f, &mirror, 1, |_| {}).await.unwrap_err();
        assert_eq!(err.diagnostic().code, "net.blocked");
    }

    #[test]
    fn a_bad_url_in_the_record_names_its_line() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path());
        std::fs::create_dir_all(mirror.crawl_dir()).unwrap();
        std::fs::write(mirror.done_path(), "").unwrap();
        std::fs::write(
            mirror.record_path(),
            "{\"url\":\"no url\",\"kind\":\"file\",\"modified\":\"m\",\"listed_on\":\"x\"}\n",
        )
        .unwrap();
        let t = Arc::new(StaticTransport::default());
        let f = fetcher(&t, home.path());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let d = rt
            .block_on(download(&f, &mirror, 1, |_| {}))
            .unwrap_err()
            .diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.line),
            ("mirror.record_corrupt", Some(1))
        );
    }

    #[test]
    fn one_run_at_a_time() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        assert!(!mirror.running());
        let held = mirror.lock().unwrap();
        assert!(mirror.running());
        assert_eq!(
            mirror.lock().unwrap_err().diagnostic().code,
            "mirror.locked"
        );
        drop(held);
        assert!(!mirror.running());
        mirror.lock().unwrap();

        let blocker = home.path().join("file");
        std::fs::write(&blocker, "").unwrap();
        let bad = Mirror::new(&blocker);
        assert_eq!(bad.lock().unwrap_err().diagnostic().code, "mirror.io");
        assert!(!bad.running());
    }
}
