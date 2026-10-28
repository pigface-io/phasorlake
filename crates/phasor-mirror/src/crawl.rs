//! Reading every listing page under the roots into each local folder's
//! `listing.ndjson`, unfiltered. Pages are kept as served, so a cut-off
//! crawl picks up from the pages it already has.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::net::Fetchers;
use serde::{Deserialize, Serialize};
use url::Url;

pub use crate::config::Mirror;
use crate::error::MirrorError;
use crate::listing::{self, LinkKind, Unparsed};
use crate::stamp::Stamp;

/// The folders mirrored under `base`: the long-term archive and the reports.
pub fn default_roots(base: &str) -> Result<Vec<Url>, MirrorError> {
    let base = base.trim_end_matches('/');
    ["Data_Archive/", "Reports/ARCHIVE/", "Reports/CURRENT/"]
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

impl Mirror {
    /// What every listing showed when last read, in URL order.
    pub fn record(&self) -> Result<Vec<Remote>, MirrorError> {
        let mut record = Vec::new();
        for path in listings_under(self.folders().root()) {
            record.extend(read_record(&path)?);
        }
        record.sort_by(|a, b| a.url.cmp(&b.url));
        Ok(record)
    }

    /// The record less what is not kept ([`Mirror::keeping`]).
    pub fn kept(&self) -> Result<Vec<Remote>, MirrorError> {
        Ok(self.kept_in(self.record()?))
    }

    /// `record` less what is not kept.
    pub fn kept_in(&self, mut record: Vec<Remote>) -> Vec<Remote> {
        record.retain(|r| crate::keep::keeps(&r.url, self.keep_from()));
        record
    }

    /// Replaces what the listing `folder` shows with `links`. The other
    /// listing of its local folder keeps its lines.
    pub fn put_folder(&self, folder: &str, links: Vec<Remote>) -> Result<(), MirrorError> {
        let path = self.listing_path(folder);
        let mut lines = read_listing(&path)?;
        lines.retain(|r| r.listed_on != folder);
        lines.extend(links);
        write_listing(&path, lines)
    }

    /// Records `folder` and all under it gone; its own link on the page above stays.
    pub fn folder_gone(&self, folder: &str) -> Result<(), MirrorError> {
        for path in listings_under(&self.folder_dir(folder)) {
            let mut lines = read_listing(&path)?;
            lines.retain(|r| !r.listed_on.starts_with(folder));
            write_listing(&path, lines)?;
        }
        Ok(())
    }

    /// Writes `record` as the listings it came from, one file per local folder.
    pub fn write_record(
        &self,
        record: impl IntoIterator<Item = Remote>,
    ) -> Result<(), MirrorError> {
        let mut by_file: BTreeMap<PathBuf, Vec<Remote>> = BTreeMap::new();
        for r in record {
            by_file
                .entry(self.listing_path(&r.listed_on))
                .or_default()
                .push(r);
        }
        for (path, lines) in by_file {
            write_listing(&path, lines)?;
        }
        Ok(())
    }

    /// Forgets every listing and page a crawl read; the folders' events stay.
    pub fn forget_crawl(&self) -> Result<(), MirrorError> {
        let done = self.done_path();
        if done.exists() {
            std::fs::remove_file(&done).map_err(MirrorError::io(&done))?;
        }
        let mut stack = vec![self.folders().root().to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = e.path();
                let name = e.file_name();
                if name == "pages" {
                    std::fs::remove_dir_all(&path).map_err(MirrorError::io(&path))?;
                } else if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(path);
                } else if name == LISTING {
                    std::fs::remove_file(&path).map_err(MirrorError::io(&path))?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
impl Mirror {
    /// Writes the record from NDJSON text, for tests. Panics on a bad line.
    pub fn set_record_text(&self, text: impl AsRef<str>) {
        let lines: Vec<Remote> = text
            .as_ref()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str::<Remote>(l).expect("a test's record line"))
            .collect();
        std::fs::create_dir_all(self.folders().root()).expect("the folders");
        self.write_record(lines).expect("the record written");
    }

    /// The record as NDJSON, one link a line in URL order.
    pub fn record_text(&self) -> String {
        self.record()
            .expect("the record reads")
            .iter()
            .map(|r| serde_json::to_string(r).expect("a Remote serialises") + "\n")
            .collect()
    }
}

/// A local folder's record of what its listings show.
const LISTING: &str = "listing.ndjson";

fn listings_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(e.path());
            } else if e.file_name() == LISTING {
                out.push(e.path());
            }
        }
    }
    out
}

/// The lines of the listing file at `path`; none when there is none.
fn read_listing(path: &Path) -> Result<Vec<Remote>, MirrorError> {
    crate::ndjson::read(path)?.map_or(Ok(Vec::new()), |text| parse_record(path, &text))
}

/// An empty listing removes the file.
fn write_listing(path: &Path, mut lines: Vec<Remote>) -> Result<(), MirrorError> {
    if lines.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(MirrorError::io(path)(e)),
            _ => Ok(()),
        };
    }
    lines.sort_by(|a, b| a.url.cmp(&b.url));
    let mut out = Vec::with_capacity(lines.len() * 256);
    for r in &lines {
        serde_json::to_writer(&mut out, r).expect("a Remote always serialises");
        out.push(b'\n');
    }
    write_atomic(path, &out)
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
    /// The listed date and time as an instant, in the host's clock; derived
    /// from `modified` as AEST when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Stamp>,
    /// The folder whose page listed it.
    pub listed_on: String,
    /// When that page was read from nemweb (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crawled_at: Option<Stamp>,
}

impl Remote {
    /// The record with `modified_at` derived from `modified` when it lacks it.
    pub fn derived(mut self) -> Self {
        if self.modified_at.is_none() {
            self.modified_at = Stamp::from_listing(&self.modified, crate::stamp::AEST);
        }
        self
    }
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
/// With `fresh`, every page is read again; without it, a finished crawl is
/// left as it is.
pub async fn crawl(
    fetcher: &Fetchers,
    mirror: &Mirror,
    roots: &[Url],
    fresh: bool,
    mut on_page: impl FnMut(&PageRead<'_>),
) -> Result<CrawlSummary, MirrorError> {
    if fresh {
        mirror.forget_crawl()?;
    }
    if mirror.done_path().exists() {
        let mut summary = summarise(&mirror.record()?);
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
        // A kept page was read from nemweb when it was written.
        let crawled_at = std::fs::metadata(&page_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| Stamp::from_unix(i64::try_from(d.as_secs()).unwrap_or(0)));
        let offset = fetcher.offset_of(folder.as_str());
        let links = read_page(&folder, &html, roots, crawled_at, offset)?;
        let count = links.len();
        for remote in links {
            if remote.kind == RemoteKind::Dir && seen.insert(remote.url.clone()) {
                queue.push_back(Url::parse(&remote.url).expect("resolved from a URL"));
            }
            if !record.contains_key(&remote.url) {
                record.insert(remote.url.clone(), remote);
            }
        }
        on_page(&PageRead {
            url: folder.as_str(),
            kept,
            links: count,
            pages: summary.pages,
            queued: queue.len(),
        });
    }
    let found = summarise(record.values());
    mirror.write_record(record.into_values())?;
    let done = serde_json::json!({ "completed_at": Stamp::now(), "pages": summary.pages });
    write_atomic(&mirror.done_path(), done.to_string().as_bytes())?;
    Ok(CrawlSummary {
        pages: summary.pages,
        pages_fetched: summary.pages_fetched,
        ..found
    })
}

/// Every link of `folder`'s listing page as a record (`offset` seconds east of UTC).
pub(crate) fn read_page(
    folder: &Url,
    html: &[u8],
    roots: &[Url],
    crawled_at: Option<Stamp>,
    offset: i32,
) -> Result<Vec<Remote>, MirrorError> {
    let bad = |source| MirrorError::Listing {
        url: folder.to_string(),
        source,
    };
    listing::parse_in(html, offset)
        .map_err(bad)?
        .iter()
        .map(|link| {
            let mut remote = resolve(folder, link, roots).map_err(bad)?;
            remote.crawled_at = crawled_at;
            Ok(remote)
        })
        .collect()
}

fn resolve(folder: &Url, link: &listing::Link, roots: &[Url]) -> Result<Remote, Unparsed> {
    let bad = |reason| Unparsed {
        reason,
        text: link.href.clone(),
    };
    let mut url = folder
        .join(&listing::href_text(&link.href))
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
        modified_at: Some(link.modified_at),
        listed_on: folder.to_string(),
        crawled_at: None,
    })
}

fn summarise<'a>(record: impl IntoIterator<Item = &'a Remote>) -> CrawlSummary {
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

/// The crawl record at `path`, in URL order, each link with its `modified_at`.
pub fn read_record(path: &Path) -> Result<Vec<Remote>, MirrorError> {
    let text = std::fs::read_to_string(path).map_err(MirrorError::io(path))?;
    parse_record(path, &text)
}

fn parse_record(path: &Path, text: &str) -> Result<Vec<Remote>, MirrorError> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(i, l)| {
            serde_json::from_str::<Remote>(l)
                .map(Remote::derived)
                .map_err(MirrorError::record(path, i))
        })
        .collect()
}

/// Writes `bytes` beside `path` and renames it into place.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MirrorError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(MirrorError::io(parent))?;
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut f = std::fs::File::create(&part).map_err(MirrorError::io(&part))?;
    f.write_all(bytes).map_err(MirrorError::io(&part))?;
    crate::net::archive::to_disk(&f).map_err(MirrorError::io(&part))?;
    std::fs::rename(&part, path).map_err(MirrorError::io(path))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_empty_mirror_lists_nothing_and_an_empty_listing_goes() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path());
        assert!(mirror.record().unwrap().is_empty());
        mirror.folder_gone("https://h/none/").unwrap();
        let dir = home.path().join("listing.ndjson");
        std::fs::create_dir_all(dir.join("x")).unwrap();
        assert!(super::write_listing(&dir, Vec::new()).is_err());
        super::write_listing(&home.path().join("none.ndjson"), Vec::new()).unwrap();
    }

    use std::sync::Arc;
    use std::time::Duration;

    use crate::diag::Diagnose as _;
    use crate::net::transport::{Canned, StaticTransport};
    use crate::net::{FetchError, FetcherConfig};

    use super::*;
    use crate::net::Archive;

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

    fn fetcher(t: &Arc<StaticTransport>, archive: &Path) -> Fetchers {
        Fetchers::one(crate::net::Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(archive),
            FetcherConfig {
                attempts: 1,
                backoff: Duration::ZERO,
                max_backoff: Duration::ZERO,
                min_interval: Duration::ZERO,
            },
        ))
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
        let record = mirror.record().unwrap();
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
        assert!(record.iter().all(|r| r.crawled_at.is_some()));
        // The listing's text as written, and the same time as an instant.
        assert_eq!(record[4].modified, "Monday, October 12, 2026 10:45 AM");
        assert_eq!(
            record[4].modified_at.unwrap().to_string(),
            "2026-10-12T10:45:00+10:00"
        );
        crate::stamp::check::assert_aware_lines(&mirror.record_text());
        assert!(
            mirror
                .page_path(&roots()[0])
                .ends_with("folders/h/A/pages/index.html")
        );
        assert!(mirror.done_path().exists());

        // Done: nothing is asked again.
        let again = crawl(&f, &mirror, &roots(), false, |_| {}).await.unwrap();
        assert!(again.already_done);
        assert_eq!((again.files, again.pages), (2, 0));
        assert_eq!(t.request_count(), 3);

        // Fresh: the crawl's pages are forgotten and every page is asked
        // again; a folder's events stay.
        mirror
            .note(
                "https://h/A/",
                &crate::journal::Event::now("https://h/A/x", crate::journal::Change::Same),
            )
            .unwrap();
        let fresh = crawl(&f, &mirror, &roots(), true, |_| {}).await.unwrap();
        assert_eq!(fresh.pages_fetched, 3);
        assert_eq!(mirror.journal("https://h/A/").unwrap().len(), 1);
        let fresh = crawl(&f, &mirror, &roots(), true, |_| {}).await.unwrap();
        assert_eq!(fresh.pages_fetched, 3);
        assert_eq!(t.request_count(), 9);
    }

    #[tokio::test]
    async fn each_host_is_read_through_its_own_fetcher_in_its_own_clock() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        let nem = site();
        let wa = Arc::new(StaticTransport::default());
        wa.add(
            "https://w/public/d/",
            Canned::ok(
                b"<html><body><H1>x</H1><hr><pre><A HREF=\"/public/\">[To Parent Directory]</A><br><br>10/20/2026  3:20 PM          714 <A HREF=\"/public/d/x.zip\">x.zip</A><br></pre><hr></body></html>",
            ),
        );
        let fast = FetcherConfig {
            attempts: 1,
            backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
            min_interval: Duration::ZERO,
        };
        let host = |base: &str, offset: i32, t: &Arc<StaticTransport>| crate::net::HostFetcher {
            base_url: base.to_owned(),
            offset_secs: offset,
            fetcher: crate::net::Fetcher::new(Arc::new(t.clone()), archive.clone(), fast.clone()),
        };
        let f = Fetchers::new(vec![
            host("https://h", 36_000, &nem),
            host("https://w", 28_800, &wa),
        ]);
        let roots = [
            root("https://h/A/").unwrap(),
            root("https://w/public/d/").unwrap(),
        ];
        let s = crawl(&f, &mirror, &roots, false, |_| {}).await.unwrap();
        assert_eq!((s.pages, s.files), (4, 3));
        assert_eq!((nem.request_count(), wa.request_count()), (3, 1));
        let record = mirror.record().unwrap();
        let at = |url: &str| {
            record
                .iter()
                .find(|r| r.url == url)
                .and_then(|r| r.modified_at)
                .unwrap()
                .to_string()
        };
        assert_eq!(at("https://h/A/x.zip"), "2026-10-12T10:45:00+10:00");
        assert_eq!(at("https://w/public/d/x.zip"), "2026-10-20T15:20:00+08:00");
        let wa_file = record
            .iter()
            .find(|r| r.url == "https://w/public/d/x.zip")
            .unwrap();
        assert_eq!(
            (wa_file.size, wa_file.modified.as_str()),
            (Some(714), "10/20/2026  3:20 PM")
        );
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
    fn a_watch_puts_each_folder_it_reads_over_the_crawl() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path());
        let link = |url: &str, kind, on: &str| Remote {
            url: url.to_owned(),
            kind,
            size: Some(1),
            modified: "Monday, October 12, 2026 10:45 AM".to_owned(),
            modified_at: None,
            listed_on: on.to_owned(),
            crawled_at: None,
        };
        let crawled = [
            link("https://h/A/a.csv", RemoteKind::File, "https://h/A/"),
            link("https://h/A/b/", RemoteKind::Dir, "https://h/A/"),
            link("https://h/A/b/c.csv", RemoteKind::File, "https://h/A/b/"),
            link("https://h/A/d/", RemoteKind::Dir, "https://h/A/"),
            link("https://h/A/d/e.csv", RemoteKind::File, "https://h/A/d/"),
        ];
        mirror.write_record(crawled.clone()).unwrap();
        let urls = |m: &Mirror| {
            m.record()
                .unwrap()
                .into_iter()
                .map(|r| r.url)
                .collect::<Vec<_>>()
        };
        assert_eq!(urls(&mirror).len(), 5);

        // A: a.csv gone, n.csv new; b/ gone from nemweb; d/ read again.
        mirror
            .put_folder(
                "https://h/A/",
                vec![
                    link("https://h/A/b/", RemoteKind::Dir, "https://h/A/"),
                    link("https://h/A/d/", RemoteKind::Dir, "https://h/A/"),
                    link("https://h/A/n.csv", RemoteKind::File, "https://h/A/"),
                ],
            )
            .unwrap();
        mirror.put_folder("https://h/A/b/", vec![]).unwrap();
        mirror.folder_gone("https://h/A/b/").unwrap();
        let r = mirror.record().unwrap();
        assert_eq!(
            r.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            [
                "https://h/A/b/",
                "https://h/A/d/",
                "https://h/A/d/e.csv",
                "https://h/A/n.csv"
            ]
        );
        assert!(r[0].modified_at.is_some());
        // b/ back: its gone mark is lifted.
        mirror
            .put_folder(
                "https://h/A/b/",
                vec![link(
                    "https://h/A/b/f.csv",
                    RemoteKind::File,
                    "https://h/A/b/",
                )],
            )
            .unwrap();
        assert!(urls(&mirror).contains(&"https://h/A/b/f.csv".to_owned()));
        // A listing that does not read stops the record.
        let bad = mirror.listing_path("https://h/A/d/");
        std::fs::write(&bad, "{").unwrap();
        let e = mirror.record().unwrap_err();
        assert!(e.to_string().contains("listing.ndjson"), "{e}");
        std::fs::remove_file(&bad).unwrap();
    }

    #[test]
    fn roots_are_folder_urls() {
        let r = default_roots("https://www.nemweb.com.au/").unwrap();
        assert_eq!(r[0].as_str(), "https://www.nemweb.com.au/Data_Archive/");
        assert_eq!(r[1].as_str(), "https://www.nemweb.com.au/Reports/ARCHIVE/");
        assert_eq!(r[2].as_str(), "https://www.nemweb.com.au/Reports/CURRENT/");
        assert_eq!(r.len(), 3);
        for bad in ["https://h/A", "ftp://h/A/", "https://h/A/?q", "not a url"] {
            let e = root(bad).unwrap_err();
            assert_eq!(e.diagnostic().code, "config.mirror_root");
        }
    }

    #[test]
    fn a_record_written_without_the_instant_gets_it_when_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("remote.ndjson");
        std::fs::write(
            &p,
            "{\"url\":\"https://h/A/\",\"kind\":\"dir\",\"modified\":\"Monday, October 12, 2026 10:45 AM\",\"listed_on\":\"https://h/\"}\n\
             {\"url\":\"https://h/A/x\",\"kind\":\"file\",\"size\":1,\"modified\":\"m\",\"listed_on\":\"https://h/\",\"crawled_at\":\"2026-10-12T00:00:00Z\"}\n",
        )
        .unwrap();
        let r = read_record(&p).unwrap();
        assert_eq!(r[0].modified, "Monday, October 12, 2026 10:45 AM");
        assert_eq!(
            r[0].modified_at.unwrap().to_string(),
            "2026-10-12T10:45:00+10:00"
        );
        assert!(r[0].crawled_at.is_none());
        // Text no reader of a listing would have passed: kept, no instant.
        assert_eq!((r[1].modified.as_str(), r[1].modified_at), ("m", None));
        assert_eq!(r[1].crawled_at.unwrap().to_string(), "2026-10-12T00:00:00Z");
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
