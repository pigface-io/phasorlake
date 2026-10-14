//! Downloading every file of the crawl record into the archive.

use std::collections::HashSet;

use futures::StreamExt as _;
use serde::Serialize;
use url::Url;

use crate::compare::pairs;
use crate::crawl::{Mirror, RemoteKind, read_record};
use crate::diag::{Diagnose as _, Diagnostic};
use crate::error::MirrorError;
use crate::net::{FetchError, Fetcher, RemoteFile};

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
    /// Of those, removed as the same as their folders, so not fetched.
    pub removed: usize,
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
/// A zip published beside a folder of the same name (a whole-month zip)
/// comes last, after its folder, and is skipped when `compare --delete`
/// has found it the same as the folder by the time its turn comes.
///
/// A file that fails is reported and the rest carry on. Two failures stop
/// the pass, since every later file would fail the same way: a refusal by
/// nemweb (`net.blocked`), and a full disk.
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
    let removed = mirror.removed()?;
    let paired: HashSet<String> = pairs(mirror)?.into_iter().map(|p| p.zip).collect();
    let mut record = read_record(&mirror.record_path())?;
    let listed = record.iter().filter(|r| r.kind == RemoteKind::File).count();
    // Stable: every other file in URL order, then the paired zips.
    record.sort_by_key(|r| paired.contains(&r.url));
    let files: Vec<RemoteFile> = record
        .into_iter()
        .enumerate()
        .filter(|(_, r)| r.kind == RemoteKind::File && !removed.contains_key(&r.url))
        .map(|(i, r)| {
            Ok(RemoteFile {
                url: Url::parse(&r.url).map_err(|e| MirrorError::Record {
                    path: mirror.record_path().display().to_string(),
                    line: i as u64 + 1,
                    message: format!("{}: {e}", r.url),
                })?,
                size: r.size,
                modified: Some(r.modified),
            })
        })
        .collect::<Result<_, MirrorError>>()?;
    let mut summary = DownloadSummary {
        files: listed,
        removed: listed - files.len(),
        ..DownloadSummary::default()
    };
    let paired = &paired;
    let mut results = futures::stream::iter(&files)
        .map(|remote| async move {
            // Found the same as its folder since the pass began?
            if paired.contains(remote.url.as_str())
                && mirror.removed()?.contains_key(remote.url.as_str())
            {
                return Ok((remote, None));
            }
            Ok::<_, MirrorError>((remote, Some(fetcher.fetch(remote).await)))
        })
        .buffer_unordered(downloads.max(1));
    let mut done = 0;
    while let Some(next) = results.next().await {
        let (remote, result) = next?;
        done += 1;
        let Some(result) = result else {
            summary.removed += 1;
            continue;
        };
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
                    total: files.len(),
                });
            }
            Err(e) if stops_the_pass(&e) => return Err(e.into()),
            Err(e) => summary.failed.push(e.diagnostic()),
        }
    }
    Ok(summary)
}

/// Whether every later download would fail as this one did.
fn stops_the_pass(e: &FetchError) -> bool {
    match e {
        FetchError::Blocked { .. } => true,
        FetchError::CacheIo { source, .. } => source.kind() == std::io::ErrorKind::StorageFull,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::net::transport::{Canned, StaticTransport};
    use crate::net::{Archive, FetcherConfig};

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

    #[tokio::test]
    async fn a_whole_month_zip_comes_after_its_folder_and_is_skipped_once_removed() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/A/",
            Canned::ok(
                b"<pre> Monday, October 12, 2026 10:45 AM &lt;dir&gt; <A HREF=\"/A/M/\">M</A><br> Monday, October 12, 2026 10:45 AM 1 <A HREF=\"/A/M.zip\">M.zip</A><br> Monday, October 12, 2026 10:45 AM 1 <A HREF=\"/A/N.zip\">N.zip</A><br> Monday, October 12, 2026 10:45 AM &lt;dir&gt; <A HREF=\"/A/N/\">N</A><br></pre>",
            ),
        );
        for folder in ["M", "N"] {
            let page = format!(
                "<pre> Monday, October 12, 2026 10:45 AM 1 <A HREF=\"/A/{folder}/a.zip\">a</A><br></pre>"
            );
            t.add(
                &format!("https://h/A/{folder}/"),
                Canned::ok(page.as_bytes()),
            );
        }
        for url in [
            "https://h/A/M.zip",
            "https://h/A/N.zip",
            "https://h/A/M/a.zip",
            "https://h/A/N/a.zip",
        ] {
            t.add(url, Canned::ok(b"z"));
        }
        let f = fetcher(&t, &archive);
        crawl(&f, &mirror, &[root("https://h/A/").unwrap()], false, |_| {})
            .await
            .unwrap();
        // Once N's folder is down, N.zip is found the same as it.
        let removed = mirror.removed_path();
        let s = download(&f, &mirror, 1, |d| {
            if d.url == "https://h/A/N/a.zip" {
                std::fs::write(
                    &removed,
                    "{\"url\":\"https://h/A/N.zip\",\"size\":1,\"sha256\":\"x\",\"same_as\":\"https://h/A/N/\",\"removed_at\":\"t\"}\n",
                )
                .unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!((s.files, s.downloaded, s.removed), (4, 3, 1));
        let fetched: Vec<String> = t.requests.lock().unwrap()[3..].to_vec();
        assert_eq!(
            fetched,
            [
                "https://h/A/M/a.zip",
                "https://h/A/N/a.zip",
                "https://h/A/M.zip"
            ]
        );
        // A record of removals that cannot be read stops the pass.
        std::fs::write(&removed, "{").unwrap();
        std::fs::remove_file(f.archive().path_for("https://h/A/M.zip")).unwrap();
        let e = download(&f, &mirror, 1, |_| {}).await.unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.record_corrupt");
    }

    #[test]
    fn a_full_disk_stops_the_pass_and_other_disk_faults_do_not() {
        let io = |kind| FetchError::CacheIo {
            path: "/a.zip.part".into(),
            source: std::io::Error::from(kind),
        };
        assert!(stops_the_pass(&io(std::io::ErrorKind::StorageFull)));
        assert!(!stops_the_pass(&io(std::io::ErrorKind::PermissionDenied)));
        assert!(!stops_the_pass(&FetchError::Offline { url: "u".into() }));
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
