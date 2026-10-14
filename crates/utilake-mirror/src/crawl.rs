//! Reading every listing page under the roots into one record of every
//! folder and file.
//!
//! The crawl follows every folder link under a root and records every file
//! link, with no filter of names, extensions or known folders. Each page is
//! kept as served, so a crawl that was cut off picks up from the pages it
//! already has, without asking nemweb for them again.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::net::{Archive, Fetcher};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::MirrorError;
use crate::listing::{self, LinkKind, Unparsed};

/// The folders nemweb keeps for good, under `base`: the long-term archive,
/// and the dated archive of the last year or so of reports.
pub fn default_roots(base: &str) -> Result<Vec<Url>, MirrorError> {
    let base = base.trim_end_matches('/');
    ["Data_Archive/", "Reports/ARCHIVE/"]
        .iter()
        .map(|p| root(&format!("{base}/{p}")))
        .collect()
}

/// A root as given: an http(s) URL of a folder.
pub fn root(s: &str) -> Result<Url, MirrorError> {
    Url::parse(s)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.path().ends_with('/'))
        .filter(|u| u.query().is_none() && u.fragment().is_none())
        .ok_or_else(|| MirrorError::Root { url: s.to_owned() })
}

/// Where the mirror keeps its own files: `{home}/mirror`.
#[derive(Debug, Clone)]
pub struct Mirror {
    dir: PathBuf,
}

impl Mirror {
    /// The mirror at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The mirror's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The current crawl.
    pub fn crawl_dir(&self) -> PathBuf {
        self.dir.join("crawl")
    }

    /// The crawl before the current one.
    pub fn previous_crawl_dir(&self) -> PathBuf {
        self.dir.join("crawl.prev")
    }

    /// The listing pages of the current crawl, kept as served, at their
    /// URL's path.
    pub fn pages(&self) -> Archive {
        Archive::new(self.crawl_dir().join("listings"))
    }

    /// The record of every link of the current crawl.
    pub fn record_path(&self) -> PathBuf {
        self.crawl_dir().join("remote.ndjson")
    }

    /// Written when the current crawl has read every page.
    pub fn done_path(&self) -> PathBuf {
        self.crawl_dir().join("done")
    }

    /// Where the page of a folder URL is kept.
    pub fn page_path(&self, folder: &Url) -> PathBuf {
        let mut path = self.pages().path_for(folder.as_str());
        path.push("index.html");
        path
    }
}

/// What a recorded link is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteKind {
    /// A folder under a root, whose page was read.
    Dir,
    /// A file, to be downloaded.
    File,
    /// A link that leaves the roots: recorded, not followed.
    External,
}

/// One link of the crawl record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// The absolute URL.
    pub url: String,
    /// Folder, file or a link out.
    pub kind: RemoteKind,
    /// Size in bytes, as listed; files only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The date and time, as listed.
    pub modified: String,
    /// The folder whose page listed it.
    pub listed_on: String,
}

/// What a crawl found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CrawlSummary {
    /// Pages read in all.
    pub pages: usize,
    /// Of those, pages asked of nemweb in this run.
    pub pages_fetched: usize,
    /// Folders.
    pub dirs: usize,
    /// Files.
    pub files: usize,
    /// Their listed bytes.
    pub bytes: u64,
    /// Links out of the roots.
    pub external: usize,
    /// Whether the crawl was already complete, so nothing was read.
    pub already_done: bool,
}

/// One page read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageRead<'a> {
    /// The folder.
    pub url: &'a str,
    /// Whether the page came from the kept copy rather than nemweb.
    pub kept: bool,
    /// Folders and files it lists.
    pub links: usize,
    /// Pages read so far.
    pub pages: usize,
    /// Pages still to read.
    pub queued: usize,
}

/// Reads every listing page under `roots` and writes the crawl record.
///
/// With `fresh`, the current crawl is first moved aside so that every page
/// is asked of nemweb again. Without it, a finished crawl is left as it is
/// and an unfinished one carries on from the pages it kept.
pub async fn crawl(
    fetcher: &Fetcher,
    mirror: &Mirror,
    roots: &[Url],
    fresh: bool,
    mut on_page: impl FnMut(&PageRead<'_>),
) -> Result<CrawlSummary, MirrorError> {
    let crawl_dir = mirror.crawl_dir();
    if fresh && crawl_dir.exists() {
        let prev = mirror.previous_crawl_dir();
        if prev.exists() {
            std::fs::remove_dir_all(&prev).map_err(MirrorError::io(&prev))?;
        }
        std::fs::rename(&crawl_dir, &prev).map_err(MirrorError::io(&crawl_dir))?;
    }
    if mirror.done_path().exists() {
        let mut summary = summarise(&read_record(&mirror.record_path())?);
        summary.already_done = true;
        return Ok(summary);
    }
    let mut queue: VecDeque<Url> = roots.iter().cloned().collect();
    let mut seen: HashSet<String> = roots.iter().map(|r| r.as_str().to_owned()).collect();
    let mut record: BTreeMap<String, Remote> = BTreeMap::new();
    let mut summary = CrawlSummary::default();
    while let Some(folder) = queue.pop_front() {
        let page_path = mirror.page_path(&folder);
        let (html, kept) = match std::fs::read(&page_path) {
            Ok(html) => (html, true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let html = fetcher.fetch_listing(folder.as_str()).await?;
                write_atomic(&page_path, &html)?;
                summary.pages_fetched += 1;
                (html, false)
            }
            Err(e) => return Err(MirrorError::io(&page_path)(e)),
        };
        summary.pages += 1;
        let links = listing::parse(&html).map_err(|source| MirrorError::Listing {
            url: folder.to_string(),
            source,
        })?;
        for link in &links {
            let remote = resolve(&folder, link, roots).map_err(|source| MirrorError::Listing {
                url: folder.to_string(),
                source,
            })?;
            if remote.kind == RemoteKind::Dir && seen.insert(remote.url.clone()) {
                queue.push_back(Url::parse(&remote.url).expect("resolved from a URL"));
            }
            record.entry(remote.url.clone()).or_insert(remote);
        }
        on_page(&PageRead {
            url: folder.as_str(),
            kept,
            links: links.len(),
            pages: summary.pages,
            queued: queue.len(),
        });
    }
    write_record(&mirror.record_path(), record.values())?;
    write_atomic(&mirror.done_path(), b"")?;
    let found = summarise(&record.into_values().collect::<Vec<_>>());
    Ok(CrawlSummary {
        pages: summary.pages,
        pages_fetched: summary.pages_fetched,
        ..found
    })
}

/// A listed link as a record: its absolute URL, and whether it is a folder
/// to read, a file to download or a link out of the roots.
fn resolve(folder: &Url, link: &listing::Link, roots: &[Url]) -> Result<Remote, Unparsed> {
    let bad = |reason| Unparsed {
        reason,
        text: link.href.clone(),
    };
    let mut url = folder
        .join(&link.href)
        .map_err(|_| bad("an href that is not a URL"))?;
    if url.fragment().is_some() {
        return Err(bad(
            "an href with a fragment, which would cut the file name",
        ));
    }
    let same_host = url.host_str() == folder.host_str() && url.scheme() == folder.scheme();
    let kind = match link.kind {
        LinkKind::Dir => {
            if !url.path().ends_with('/') {
                let path = format!("{}/", url.path());
                url.set_path(&path);
            }
            if roots.iter().any(|r| url.as_str().starts_with(r.as_str())) {
                RemoteKind::Dir
            } else {
                RemoteKind::External
            }
        }
        LinkKind::File if same_host => RemoteKind::File,
        LinkKind::File => RemoteKind::External,
    };
    Ok(Remote {
        url: url.into(),
        kind,
        size: link.size,
        modified: link.modified.clone(),
        listed_on: folder.to_string(),
    })
}

fn summarise(record: &[Remote]) -> CrawlSummary {
    let mut s = CrawlSummary::default();
    for r in record {
        match r.kind {
            RemoteKind::Dir => s.dirs += 1,
            RemoteKind::File => {
                s.files += 1;
                s.bytes += r.size.unwrap_or(0);
            }
            RemoteKind::External => s.external += 1,
        }
    }
    s
}

/// The crawl record at `path`, in URL order.
pub fn read_record(path: &Path) -> Result<Vec<Remote>, MirrorError> {
    let text = std::fs::read_to_string(path).map_err(MirrorError::io(path))?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| MirrorError::Record {
                path: path.display().to_string(),
                line: i as u64 + 1,
                message: e.to_string(),
            })
        })
        .collect()
}

fn write_record<'r>(
    path: &Path,
    record: impl Iterator<Item = &'r Remote>,
) -> Result<(), MirrorError> {
    let mut out = Vec::with_capacity(1 << 20);
    for r in record {
        serde_json::to_writer(&mut out, r).expect("a Remote always serialises");
        out.push(b'\n');
    }
    write_atomic(path, &out)
}

/// Writes `bytes` beside `path` and renames it into place, so a cut-off
/// write never leaves a partial file under the final name.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MirrorError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(MirrorError::io(parent))?;
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut f = std::fs::File::create(&part).map_err(MirrorError::io(&part))?;
    f.write_all(bytes).map_err(MirrorError::io(&part))?;
    f.sync_all().map_err(MirrorError::io(&part))?;
    std::fs::rename(&part, path).map_err(MirrorError::io(path))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::diag::Diagnose as _;
    use crate::net::transport::{Canned, StaticTransport};
    use crate::net::{FetchError, FetcherConfig};

    use super::*;

    fn page(lines: &[&str]) -> Vec<u8> {
        let mut body =
            String::from("<html><body><pre><A HREF=\"/\">[To Parent Directory]</A><br><br>");
        for l in lines {
            body.push_str(l);
            body.push_str("<br>");
        }
        body.push_str("</pre></body></html>");
        body.into_bytes()
    }

    fn dir(href: &str) -> String {
        format!(" Monday, October 12, 2026 10:45 AM &lt;dir&gt; <A HREF=\"{href}\">d</A>")
    }

    fn file(href: &str, size: u64) -> String {
        format!(" Monday, October 12, 2026 10:45 AM {size} <A HREF=\"{href}\">f</A>")
    }

    /// `https://h/A/` holds a folder, a file, a folder outside the root and
    /// a file on another host; `b/` links back to the root and to `c`
    /// without its slash; `c/` is empty.
    fn site() -> Arc<StaticTransport> {
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/A/",
            Canned::ok(&page(&[
                &dir("/A/b/"),
                &file("/A/x.zip", 3),
                &dir("/Z/"),
                &file("https://o/f.zip", 9),
            ])),
        );
        t.add(
            "https://h/A/b/",
            Canned::ok(&page(&[&file("y%23z.csv", 5), &dir("/A/"), &dir("/A/b/c")])),
        );
        t.add("https://h/A/b/c/", Canned::ok(&page(&[])));
        t
    }

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

    fn roots() -> Vec<Url> {
        vec![root("https://h/A/").unwrap()]
    }

    #[tokio::test]
    async fn follows_every_folder_and_records_every_link() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let t = site();
        let f = fetcher(&t, &home.path().join("archive"));
        let mut seen = Vec::new();
        let s = crawl(&f, &mirror, &roots(), false, |p| {
            seen.push((p.url.to_owned(), p.kept, p.links, p.pages, p.queued));
        })
        .await
        .unwrap();
        assert_eq!(
            s,
            CrawlSummary {
                pages: 3,
                pages_fetched: 3,
                dirs: 3,
                files: 2,
                bytes: 8,
                external: 2,
                already_done: false,
            }
        );
        assert_eq!(seen[0], ("https://h/A/".to_owned(), false, 4, 1, 1));
        assert_eq!(seen[2].0, "https://h/A/b/c/");
        let record = read_record(&mirror.record_path()).unwrap();
        let urls: Vec<_> = record.iter().map(|r| (r.url.as_str(), r.kind)).collect();
        assert_eq!(
            urls,
            [
                ("https://h/A/", RemoteKind::Dir),
                ("https://h/A/b/", RemoteKind::Dir),
                ("https://h/A/b/c/", RemoteKind::Dir),
                ("https://h/A/b/y%23z.csv", RemoteKind::File),
                ("https://h/A/x.zip", RemoteKind::File),
                ("https://h/Z/", RemoteKind::External),
                ("https://o/f.zip", RemoteKind::External),
            ]
        );
        assert_eq!(record[4].size, Some(3));
        assert_eq!(record[4].listed_on, "https://h/A/");
        assert!(
            mirror
                .page_path(&roots()[0])
                .ends_with("listings/h/A/index.html")
        );
        assert!(mirror.done_path().exists());

        // Done: nothing is asked again.
        let again = crawl(&f, &mirror, &roots(), false, |_| {}).await.unwrap();
        assert!(again.already_done);
        assert_eq!((again.files, again.pages), (2, 0));
        assert_eq!(t.request_count(), 3);

        // Fresh: the crawl moves aside and every page is asked again.
        let fresh = crawl(&f, &mirror, &roots(), true, |_| {}).await.unwrap();
        assert_eq!(fresh.pages_fetched, 3);
        assert!(mirror.previous_crawl_dir().join("done").exists());
        let fresh = crawl(&f, &mirror, &roots(), true, |_| {}).await.unwrap();
        assert_eq!(fresh.pages_fetched, 3);
        assert_eq!(t.request_count(), 9);
    }

    #[tokio::test]
    async fn a_cut_off_crawl_carries_on_from_its_kept_pages() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        // The first two pages answer; the third is refused.
        let blocked = Arc::new(StaticTransport::default());
        let full = site();
        let f = fetcher(&full, &home.path().join("archive"));
        for url in ["https://h/A/", "https://h/A/b/"] {
            blocked.add(url, Canned::ok(&f.fetch_listing(url).await.unwrap()));
        }
        blocked.add("https://h/A/b/c/", Canned::status(403));
        let f = fetcher(&blocked, &home.path().join("archive"));
        let err = crawl(&f, &mirror, &roots(), false, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            MirrorError::Fetch(FetchError::Blocked { .. })
        ));
        assert_eq!(err.diagnostic().code, "net.blocked");
        assert!(!mirror.done_path().exists());

        let f = fetcher(&full, &home.path().join("archive"));
        let s = crawl(&f, &mirror, &roots(), false, |_| {}).await.unwrap();
        assert_eq!((s.pages, s.pages_fetched, s.files), (3, 1, 2));
    }

    #[tokio::test]
    async fn an_unreadable_line_stops_the_crawl() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/A/", Canned::ok(&page(&["a new kind of line"])));
        let f = fetcher(&t, &home.path().join("archive"));
        let err = crawl(&f, &mirror, &roots(), false, |_| {})
            .await
            .unwrap_err();
        let d = err.diagnostic();
        assert_eq!(d.code, "listing.unparsed_line");
        assert_eq!(d.url.as_deref(), Some("https://h/A/"));
        assert!(err.to_string().contains("a listing line without a link"));

        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/A/",
            Canned::ok(&page(&[&file("/A/a.zip#cut", 1)])),
        );
        let f = fetcher(&t, &home.path().join("archive"));
        crawl(&f, &mirror, &roots(), true, |_| {})
            .await
            .unwrap_err();
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/A/",
            Canned::ok(&page(&[&file("http://[x/a.zip", 1)])),
        );
        let f = fetcher(&t, &home.path().join("archive"));
        let err = crawl(&f, &mirror, &roots(), true, |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("an href that is not a URL"));
    }

    #[test]
    fn roots_are_folder_urls() {
        let r = default_roots("https://www.nemweb.com.au/").unwrap();
        assert_eq!(r[0].as_str(), "https://www.nemweb.com.au/Data_Archive/");
        assert_eq!(r[1].as_str(), "https://www.nemweb.com.au/Reports/ARCHIVE/");
        for bad in ["https://h/A", "ftp://h/A/", "https://h/A/?q", "not a url"] {
            let e = root(bad).unwrap_err();
            assert_eq!(e.diagnostic().code, "config.mirror_root");
        }
    }

    #[test]
    fn record_and_io_errors_say_where() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("remote.ndjson");
        std::fs::write(&p, "\n{\"url\":1}\n").unwrap();
        let d = read_record(&p).unwrap_err().diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.line),
            ("mirror.record_corrupt", Some(2))
        );
        let d = read_record(&dir.path().join("none"))
            .unwrap_err()
            .diagnostic();
        assert_eq!(d.code, "mirror.io");
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "").unwrap();
        assert!(write_atomic(&blocker.join("x"), b"").is_err());
        assert!(write_atomic(&blocker.join("x/y"), b"").is_err());
    }

    #[tokio::test]
    async fn a_kept_page_that_cannot_be_read_stops_the_crawl() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        std::fs::create_dir_all(mirror.page_path(&roots()[0])).unwrap();
        let t = site();
        let f = fetcher(&t, &home.path().join("archive"));
        let e = crawl(&f, &mirror, &roots(), false, |_| {})
            .await
            .unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.io");
        assert_eq!(t.request_count(), 0);
    }
}
