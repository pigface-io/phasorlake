//! A feed's folder tree as a [`Source`]: the roots the shared feeds table names
//! for it, walked by their listings, each file named by the feed's rules. Used by
//! the gas feeds and the WEM's WEMDE documents.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use phasor_kit::{
    Entry, Listings, LocalFile, Pick, Picks, Query, RemoteFile, Serves, Source, TableOf,
};
use url::Url;

use crate::archive::{Archive, Held};
use crate::error::FetchError;
use crate::listing;
use phasor_core::calendar::local_year_month;
use phasor_core::timetext::{Stamp, parse_iis_listing};
use phasor_core::{Period, YearMonth};
use phasor_plain_decode::Feed;
use phasor_plain_decode::names::block_name;

/// A feed's folders under one host.
pub struct FeedFolders {
    id: &'static str,
    base: Url,
    feed: &'static Feed,
    table_of: TableOf,
    serves: Serves,
}

/// `url` as the base of a folder: with a `/` at the end of its path.
pub fn base_url(url: &str) -> Result<Url, FetchError> {
    let mut base = Url::parse(url).map_err(FetchError::parse(url, "base URL"))?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    Ok(base)
}

/// The month of an instant in the feed's clock.
fn month_of(at: Stamp, offset_secs: i32) -> Option<YearMonth> {
    let (year, month) = local_year_month(at.secs_utc.saturating_mul(1_000_000_000), offset_secs);
    YearMonth::new(year, u8::try_from(month).ok()?)
}

impl FeedFolders {
    /// The folders of `feed` under `base_url`, recorded under `id`, blocks named by `table_of`.
    pub fn new(
        id: &'static str,
        base_url: &str,
        feed: &'static Feed,
        table_of: TableOf,
    ) -> Result<Self, FetchError> {
        Ok(Self {
            id,
            base: self::base_url(base_url)?,
            feed,
            table_of,
            serves: Arc::new(|_| true),
        })
    }

    /// The same source, serving only the tables `serves` says are the feed's.
    #[must_use]
    pub fn with_tables_served(mut self, serves: Serves) -> Self {
        self.serves = serves;
        self
    }

    /// The feed.
    pub fn feed(&self) -> &'static Feed {
        self.feed
    }

    /// The feed's roots as folder URLs under the base.
    pub fn roots(&self) -> Result<Vec<Url>, FetchError> {
        self.feed
            .roots
            .iter()
            .map(|r| {
                let rel = format!("{}/", r.trim_matches('/'));
                self.base
                    .join(&rel)
                    .map_err(FetchError::parse(&rel, "feed root"))
            })
            .collect()
    }

    /// A file at `url`: its block's table and release month, from its name, else its listed time.
    fn remote(
        &self,
        url: Url,
        size: Option<u64>,
        modified: Option<String>,
        modified_at: Option<Stamp>,
    ) -> RemoteFile {
        let path = url.path();
        let table = block_name(self.feed, path).and_then(|b| (self.table_of)(&b));
        let period = self
            .feed
            .release_month(path)
            .and_then(|m| YearMonth::new(m.year(), m.month()))
            .or_else(|| modified_at.and_then(|at| month_of(at, self.feed.offset_secs)))
            .map(Period::Month);
        RemoteFile {
            url,
            source: self.id,
            table,
            period,
            size,
            modified,
            modified_at,
        }
    }

    /// Whether a query asks for this file by month and table.
    fn wanted(&self, remote: &RemoteFile, query: &Query) -> bool {
        if remote.period.is_some_and(|p| !query.range.contains(p)) {
            return false;
        }
        let excluded = |t: &str| query.exclude.iter().any(|x| x.eq_ignore_ascii_case(t));
        match &remote.table {
            Some(t) if excluded(t) => false,
            _ if query.tables.is_empty() => true,
            Some(t) => query.tables.iter().any(|q| q.eq_ignore_ascii_case(t)),
            None => false,
        }
    }

    /// Whether a path lies in a folder the feed skips (a space is `%20` in a URL).
    fn skipped(&self, path: &str) -> bool {
        path.split('/')
            .filter(|s| !s.is_empty())
            .any(|s| self.feed.ignores(&s.replace("%20", " ")))
    }

    /// The version stamp in a kept version's name (`{name}@{stamp}`), else its listed time.
    fn version_tag(a: &Held) -> Option<String> {
        let name = a.path.file_name()?.to_str()?;
        if let Some((_, stamp)) = name.rsplit_once('@') {
            return Some(stamp.to_owned());
        }
        let p = &a.provenance;
        p.listed(p.url())?
            .at
            .map(|s| s.rfc3339().replace([':', '-'], ""))
    }
}

#[async_trait]
impl Source for FeedFolders {
    fn id(&self) -> &'static str {
        self.id
    }

    fn serves(&self, table: &str) -> bool {
        (self.serves)(table)
    }

    /// Lists every file under the feed's roots the query asks for.
    async fn list(
        &self,
        query: &Query,
        fetcher: &dyn Listings,
    ) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        let mut queue: Vec<Url> = self.roots()?;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(folder) = queue.pop() {
            if !seen.insert(folder.to_string()) {
                continue;
            }
            let body = match fetcher.fetch_listing(folder.as_str()).await {
                Ok(b) => b,
                // A root the feed names but the host no longer lists.
                Err(FetchError::HttpStatus { status: 404, .. }) => continue,
                Err(e) => return Err(e),
            };
            for e in listing::parse(&body) {
                let url = folder
                    .join(&e.href)
                    .map_err(FetchError::parse(&e.href, "listing href"))?;
                if e.is_dir {
                    // Down the tree only, never up or out of it.
                    if url.as_str().starts_with(folder.as_str())
                        && url.as_str() != folder.as_str()
                        && !self.feed.ignores(e.name.trim_end_matches('/'))
                    {
                        queue.push(url);
                    }
                    continue;
                }
                let modified_at = e
                    .modified
                    .as_ref()
                    .and_then(|m| parse_iis_listing(m.text(), self.feed.offset_secs));
                let r = self.remote(
                    url,
                    e.size,
                    e.modified.as_ref().map(|m| m.text().to_owned()),
                    modified_at,
                );
                if self.wanted(&r, query) {
                    out.push(r);
                }
            }
        }
        out.sort_by(|a, b| (a.period, a.url.as_str()).cmp(&(b.period, b.url.as_str())));
        out.dedup_by(|a, b| a.url == b.url);
        Ok(out)
    }

    /// Every archived version, in published order, each naming the one before it;
    /// a later version carries its stamp as the URL's fragment.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        let mut out = Vec::new();
        // CURRENT and ARCHIVE share one local folder: walk each folder once.
        let mut walked: Vec<PathBuf> = Vec::new();
        for root in self.roots()? {
            let dir = archive.path_for(root.as_str());
            if walked.contains(&dir) {
                continue;
            }
            walked.push(dir.clone());
            let mut by_url: BTreeMap<String, Vec<Held>> = BTreeMap::new();
            for a in crate::archive::entries_under(archive, Some(&dir)) {
                by_url
                    .entry(a.provenance.url().to_owned())
                    .or_default()
                    .push(a);
            }
            for (url, mut versions) in by_url {
                let Ok(base) = Url::parse(&url) else {
                    continue;
                };
                if self.skipped(base.path()) {
                    continue;
                }
                versions.sort_by_cached_key(|x| {
                    let p = &x.provenance;
                    (
                        p.listed(p.url()).and_then(|l| l.at).map(|s| s.secs_utc),
                        x.path.clone(),
                    )
                });
                let mut previous: Option<PathBuf> = None;
                for (i, a) in versions.into_iter().enumerate() {
                    let mut url = base.clone();
                    if i > 0 {
                        let tag = Self::version_tag(&a).unwrap_or_else(|| i.to_string());
                        url.set_fragment(Some(&tag));
                    }
                    let p = &a.provenance;
                    let listed_at = p.listed(p.url()).and_then(|l| l.at);
                    let remote = self.remote(url, Some(p.content.size), None, listed_at);
                    let path = a.path.clone();
                    if self.wanted(&remote, query) {
                        out.push(LocalFile {
                            remote,
                            sha256: p.content.sha256.hex(),
                            size: p.content.size,
                            cached: true,
                            etag: p.etag().map(str::to_owned),
                            last_modified: p.modified().map(Stamp::http_date),
                            last_modified_at: p.modified(),
                            fetched_at: Some(p.fetched_at()),
                            previous: previous.clone(),
                            path: a.path,
                        });
                    }
                    previous = Some(path);
                }
            }
        }
        out.sort_by(|a, b| {
            (a.remote.period, a.remote.url.as_str()).cmp(&(b.remote.period, b.remote.url.as_str()))
        });
        Ok(out)
    }

    /// Claims a URL under the feed's roots, `file://` mirror or `https://` host.
    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        let path = url.path();
        // A root as AEMO lists it, or as the archive keeps it.
        let roots = self
            .feed
            .roots
            .iter()
            .map(|r| (*r).to_owned())
            .chain(self.feed.local_roots());
        let under_root = roots.into_iter().any(|r| {
            let root = format!("/{}/", r.trim_matches('/'));
            path.contains(&root) && path.len() > path.find(&root).unwrap_or(0) + root.len()
        });
        if !under_root || path.ends_with('/') || self.skipped(path) {
            return None;
        }
        Some(self.remote(url.clone(), None, None, None))
    }

    /// A zip under a feed's folders is read by its directory when the query names tables.
    fn wants_ranged(&self, remote: &RemoteFile, query: &Query) -> bool {
        !query.tables.is_empty()
            && remote
                .url
                .path()
                .rsplit('.')
                .next()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    }

    /// Picks the entries whose blocks belong to the tables asked for.
    fn pick(&self, remote: &RemoteFile, entries: &[Entry], query: &Query) -> Picks {
        if query.tables.is_empty() {
            return Picks::Whole;
        }
        let picks = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_dir())
            .filter_map(|(index, e)| {
                let url = Url::parse(&format!("{}!{}", remote.url, e.name)).ok()?;
                let r = self.remote(
                    url,
                    Some(e.size),
                    remote.modified.clone(),
                    remote.modified_at,
                );
                self.wanted(&r, query).then_some(Pick { index, remote: r })
            })
            .collect();
        Picks::Entries(picks)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::transport::{Canned, StaticTransport};
    use crate::{Fetcher, FetcherConfig};
    use phasor_core::TimeRange;
    use phasor_plain_decode::feeds::feed;

    use super::*;

    fn fetcher(t: &Arc<StaticTransport>, archive: &std::path::Path) -> Fetcher {
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

    fn listing(folder: &str, entries: &[(&str, Option<u64>)]) -> Vec<u8> {
        let mut html = String::from("<html><body><pre>");
        for (name, size) in entries {
            let href = format!("{folder}{name}");
            let what = size.map_or("&lt;dir&gt;".to_owned(), |s| s.to_string());
            let _ = write!(
                html,
                "Tuesday, October 21, 2026 10:11 PM {what} <A HREF=\"{href}\">{name}</A><br>"
            );
        }
        html.push_str("</pre></body></html>");
        html.into_bytes()
    }

    fn tables() -> TableOf {
        Arc::new(
            |b: &phasor_plain_decode::BlockName| match (b.family.as_str(), b.name.as_str()) {
                ("GBB", "ACTUAL_FLOW_STORAGE" | "ACTUAL_FLOW_STORAGE_LAST31") => {
                    Some("GBB_ACTUAL_FLOW_STORAGE".to_owned())
                }
                ("STTM", n) if n.starts_with("INT651") => {
                    Some("STTM_INT651_EX_ANTE_MARKET_PRICE".to_owned())
                }
                _ => None,
            },
        )
    }

    fn months(a: &str, b: &str) -> TimeRange {
        TimeRange::months(a.parse().unwrap(), b.parse().unwrap())
    }

    fn query(tables: &[&str], range: TimeRange) -> Query {
        Query {
            tables: tables.iter().map(|t| (*t).to_owned()).collect(),
            exclude: Vec::new(),
            range,
            instants: None,
        }
    }

    #[tokio::test]
    async fn lists_the_feeds_tree_naming_each_files_table_and_month() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/Reports/CURRENT/GBB/",
            Canned::ok(&listing(
                "/Reports/CURRENT/GBB/",
                &[
                    ("GasBBActualFlowStorage_20261001.CSV", Some(10)),
                    ("GasBBActualFlowStorageLast31.CSV", Some(20)),
                    ("GasBBNominationAndForecast.CSV", Some(30)),
                    ("DUPLICATE/", None),
                    ("notes.txt", Some(1)),
                ],
            )),
        );
        t.add(
            "https://h/Reports/CURRENT/GBB/DUPLICATE/",
            Canned::ok(&listing(
                "/Reports/CURRENT/GBB/DUPLICATE/",
                &[("GasBBActualFlowStorage_20260901.CSV", Some(5))],
            )),
        );
        let gbb = feed("gbb").unwrap();
        let s = FeedFolders::new("gas-gbb", "https://h", gbb, tables()).unwrap();
        assert_eq!(s.id(), "gas-gbb");
        assert_eq!(s.feed().id, "gbb");
        assert_eq!(
            s.roots().unwrap()[0].as_str(),
            "https://h/Reports/CURRENT/GBB/"
        );
        let f = fetcher(&t, dir.path());
        // Every file of the tree for every table.
        let all = s
            .list(&query(&[], months("2026-09", "2026-10")), &f)
            .await
            .unwrap();
        let names: Vec<String> = all.iter().map(RemoteFile::file_name).collect();
        assert_eq!(
            names,
            [
                "GasBBActualFlowStorage_20260901.CSV",
                "GasBBActualFlowStorageLast31.CSV",
                "GasBBActualFlowStorage_20261001.CSV",
                "GasBBNominationAndForecast.CSV",
                "notes.txt"
            ],
            "by month, then URL"
        );
        // A name with a date: its month; without: the month it was listed
        // in, read in the feed's clock.
        assert_eq!(all[0].period.unwrap().to_string(), "2026-09");
        assert_eq!(all[1].period.unwrap().to_string(), "2026-10");
        assert_eq!(all[1].table.as_deref(), Some("GBB_ACTUAL_FLOW_STORAGE"));
        assert_eq!(all[3].table, None, "no table claims the block");
        assert_eq!(all[1].size, Some(20));
        assert_eq!(
            all[1].modified_at.unwrap().rfc3339(),
            "2026-10-21T22:11:00+10:00"
        );
        assert_eq!(t.request_count(), 2);
        // One table, one month: the files of its blocks in that month.
        let some = s
            .list(
                &query(&["gbb_actual_flow_storage"], months("2026-10", "2026-10")),
                &f,
            )
            .await
            .unwrap();
        let names: Vec<String> = some.iter().map(RemoteFile::file_name).collect();
        assert_eq!(
            names,
            [
                "GasBBActualFlowStorageLast31.CSV",
                "GasBBActualFlowStorage_20261001.CSV"
            ]
        );
        // Excluded, or a block no table claims, when tables are named.
        let mut q = query(&["GBB_ACTUAL_FLOW_STORAGE"], months("2026-10", "2026-10"));
        q.exclude.push("gbb_actual_flow_storage".into());
        assert!(s.list(&q, &f).await.unwrap().is_empty());
        // A root the host does not list is passed over; another failure is
        // the error.
        let t2 = Arc::new(StaticTransport::default());
        t2.add("https://h/Reports/CURRENT/GBB/", Canned::status(404));
        assert!(
            s.list(
                &query(&[], months("2026-10", "2026-10")),
                &fetcher(&t2, dir.path())
            )
            .await
            .unwrap()
            .is_empty()
        );
        let t3 = Arc::new(StaticTransport::default());
        t3.add("https://h/Reports/CURRENT/GBB/", Canned::status(500));
        assert!(
            s.list(
                &query(&[], months("2026-10", "2026-10")),
                &fetcher(&t3, dir.path())
            )
            .await
            .is_err()
        );
        assert!(FeedFolders::new("x", "not a url", gbb, tables()).is_err());
        // A base under a path gets its slash; a listing with a link that is
        // not a URL is the error.
        let under = FeedFolders::new("gas-gbb", "https://h/mirror", gbb, tables()).unwrap();
        assert_eq!(
            under.roots().unwrap()[0].as_str(),
            "https://h/mirror/Reports/CURRENT/GBB/"
        );
        let t4 = Arc::new(StaticTransport::default());
        t4.add(
            "https://h/mirror/Reports/CURRENT/GBB/",
            Canned::ok(b"<pre>Tuesday, October 21, 2026 10:11 PM 1 <A HREF=\"http://[::1\">x</A><br></pre>"),
        );
        assert!(matches!(
            under
                .list(
                    &query(&[], months("2026-10", "2026-10")),
                    &fetcher(&t4, dir.path())
                )
                .await
                .unwrap_err(),
            FetchError::Parse {
                what: "listing href",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn skips_the_folders_the_feed_ignores_and_links_up_or_out() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/Reports/CURRENT/STTM/",
            Canned::ok(
                b"<pre><A HREF=\"/Reports/CURRENT/\">[To Parent Directory]</A><br>\
                  Tuesday, October 21, 2026 10:11 PM &lt;dir&gt; <A HREF=\"/Reports/CURRENT/STTM/MOS Estimates/\">MOS Estimates</A><br>\
                  Tuesday, October 21, 2026 10:11 PM &lt;dir&gt; <A HREF=\"/Reports/CURRENT/STTM/\">STTM</A><br>\
                  Tuesday, October 21, 2026 10:11 PM 9 <A HREF=\"/Reports/CURRENT/STTM/int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv\">int651</A><br></pre>",
            ),
        );
        let sttm = feed("sttm").unwrap();
        let s = FeedFolders::new("gas-sttm", "https://h/", sttm, tables()).unwrap();
        let f = fetcher(&t, dir.path());
        let all = s
            .list(&query(&[], months("2026-10", "2026-10")), &f)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(
            all[0].table.as_deref(),
            Some("STTM_INT651_EX_ANTE_MARKET_PRICE")
        );
        assert_eq!(
            all[0].period.unwrap().to_string(),
            "2026-10",
            "the ~stamp's month"
        );
        assert_eq!(
            t.request_count(),
            1,
            "the parent and the ignored folder are not read"
        );
    }

    /// Writes an archived file with its sidecar, as the mirror does.
    fn archived(
        archive: &Archive,
        url: &str,
        version: Option<&str>,
        bytes: &[u8],
        listed: Option<&str>,
    ) -> PathBuf {
        let base = archive.path_for(url);
        let path = match version {
            Some(v) => {
                let mut name = base.file_name().unwrap().to_os_string();
                name.push(format!("@{v}"));
                base.with_file_name(name)
            }
            None => base,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let v1 = serde_json::json!({
            "url": url,
            "size": bytes.len(),
            "sha256": crate::archive::sha256_hex(bytes),
            "fetched_at": "2026-10-21T12:00:00Z",
            "listed_modified": listed,
            "tool": "test",
        });
        let record = crate::Provenance::from_json(v1.to_string().as_bytes()).unwrap();
        Archive::write(&path, &record).unwrap();
        path
    }

    /// A feed's CURRENT and ARCHIVE sides share one local folder, read once.
    #[test]
    fn a_folder_both_sides_share_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let gsh = feed("gsh").unwrap();
        assert!(gsh.roots.len() > 1, "{:?}", gsh.roots);
        let s = FeedFolders::new("gas-gsh", "https://www.nemweb.com.au", gsh, tables()).unwrap();
        let roots = s.roots().unwrap();
        assert_eq!(
            archive.path_for(roots[0].as_str()),
            archive.path_for(roots[1].as_str())
        );
        let url = roots[0].join("GSH_FILE.CSV").unwrap();
        archived(&archive, url.as_str(), None, b"a,b\n1,2\n", None);
        let all = s
            .archived(&archive, &query(&[], months("2026-09", "2026-10")))
            .unwrap();
        let urls: Vec<&str> = all.iter().map(|l| l.remote.url.as_str()).collect();
        assert_eq!(urls, [url.as_str()]);
    }

    #[test]
    fn archived_versions_come_in_order_each_naming_the_one_before() {
        let dir = tempfile::tempdir().unwrap();
        let archive = Archive::new(dir.path());
        let url = "https://h/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV";
        let v1 = archived(
            &archive,
            url,
            None,
            b"a,b\n1,2\n",
            Some("Monday, October 20, 2026 10:00 AM"),
        );
        let v2 = archived(
            &archive,
            url,
            Some("20261021T221100+1000"),
            b"a,b\n1,2\n3,4\n",
            Some("Tuesday, October 21, 2026 10:11 PM"),
        );
        let other = "https://h/Reports/CURRENT/GBB/GasBBNominationAndForecast.CSV";
        archived(
            &archive,
            other,
            None,
            b"x\n",
            Some("Tuesday, October 21, 2026 10:11 PM"),
        );
        let ignored = "https://h/Reports/CURRENT/GBB/DUPLICATE/GasBBActualFlowStorage_20260901.CSV";
        archived(&archive, ignored, None, b"x\n", None);
        let gbb = feed("gbb").unwrap();
        let s = FeedFolders::new("gas-gbb", "https://h", gbb, tables()).unwrap();
        let all = s
            .archived(&archive, &query(&[], months("2026-09", "2026-10")))
            .unwrap();
        let urls: Vec<&str> = all.iter().map(|l| l.remote.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://h/Reports/CURRENT/GBB/DUPLICATE/GasBBActualFlowStorage_20260901.CSV",
                "https://h/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV",
                "https://h/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV#20261021T221100+1000",
                "https://h/Reports/CURRENT/GBB/GasBBNominationAndForecast.CSV",
            ]
        );
        assert_eq!(all[1].path, v1);
        assert_eq!(all[1].previous, None);
        assert_eq!(all[2].path, v2);
        assert_eq!(all[2].previous.as_deref(), Some(v1.as_path()));
        assert_eq!(
            all[2].remote.table.as_deref(),
            Some("GBB_ACTUAL_FLOW_STORAGE")
        );
        assert_eq!(all[2].remote.period.unwrap().to_string(), "2026-10");
        assert!(all.iter().all(|l| l.cached && l.remote.source == "gas-gbb"));
        // One table: its files alone, every version.
        let some = s
            .archived(
                &archive,
                &query(&["GBB_ACTUAL_FLOW_STORAGE"], months("2026-10", "2026-10")),
            )
            .unwrap();
        assert_eq!(some.len(), 2);
        // A month the files are not of.
        assert!(
            s.archived(&archive, &query(&[], months("2025-01", "2025-01")))
                .unwrap()
                .is_empty()
        );
        // A version without a stamp in its name or listing is numbered.
        let bare = archived(&archive, other, Some("x"), b"y\n", None);
        let mut tagless = crate::archive::entries_under(&archive, Some(bare.parent().unwrap()));
        tagless.retain(|a| a.path == bare);
        assert_eq!(FeedFolders::version_tag(&tagless[0]).as_deref(), Some("x"));
        let plain = crate::archive::entries_under(&archive, None)
            .into_iter()
            .find(|a| a.provenance.url() == ignored)
            .unwrap();
        assert_eq!(FeedFolders::version_tag(&plain), None);
        // One with no stamp in its name is tagged by when it was listed.
        let listed = crate::archive::entries_under(&archive, None)
            .into_iter()
            .find(|a| a.provenance.url() == other && !a.path.to_string_lossy().contains('@'))
            .unwrap();
        assert_eq!(
            FeedFolders::version_tag(&listed).as_deref(),
            Some("20261021T221100+1000")
        );
        // A sidecar whose URL does not parse, and a file in a folder the
        // feed skips, are left out.
        let sttm = feed("sttm").unwrap();
        let s2 = FeedFolders::new("gas-sttm", "https://h", sttm, tables()).unwrap();
        let kept = "https://h/Reports/CURRENT/STTM/int651_v1_x~20261001113227.csv";
        archived(&archive, kept, None, b"a\n", None);
        archived(
            &archive,
            "https://h/Reports/CURRENT/STTM/MOS Estimates/int651_v1_y~20261001113227.csv",
            None,
            b"a\n",
            None,
        );
        let odd = archive.path_for("https://h/Reports/CURRENT/STTM/odd.csv");
        std::fs::write(&odd, b"a\n").unwrap();
        let mut sidecar = Archive::read(&archive.path_for(kept)).unwrap().unwrap();
        sidecar.addresses[0].url = "not a url".into();
        Archive::write(&odd, &sidecar).unwrap();
        let got = s2
            .archived(&archive, &query(&[], months("2026-10", "2026-10")))
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].remote.url.as_str(), kept);
    }

    #[test]
    fn claims_a_url_under_its_roots_and_picks_a_zips_entries_by_table() {
        let sttm = feed("sttm").unwrap();
        let s = FeedFolders::new("gas-sttm", "https://h", sttm, tables()).unwrap();
        let claimed = |u: &str| s.describe(&Url::parse(u).unwrap());
        let r = claimed("file:///data/x/archive/www.nemweb.com.au/Reports/CURRENT/STTM/int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv").unwrap();
        assert_eq!(r.source, "gas-sttm");
        assert_eq!(r.table.as_deref(), Some("STTM_INT651_EX_ANTE_MARKET_PRICE"));
        assert_eq!(r.period.unwrap().to_string(), "2026-10");
        assert!(claimed("https://h/Reports/CURRENT/STTM/Day01.zip").is_some());
        // As the archive keeps it: one folder whichever listing.
        assert_eq!(
            claimed("file:///v/archive/www.nemweb.com.au/Reports/STTM/int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv")
                .unwrap()
                .table
                .as_deref(),
            Some("STTM_INT651_EX_ANTE_MARKET_PRICE")
        );
        assert!(claimed("https://h/Reports/CURRENT/STTM/").is_none());
        assert!(claimed("https://h/Reports/CURRENT/GBB/x.csv").is_none());
        assert!(claimed("https://h/Reports/CURRENT/STTM/MOS Estimates/x.csv").is_none());
        let zip = claimed("https://h/Reports/CURRENT/STTM/Day01.zip").unwrap();
        let named = query(
            &["STTM_INT651_EX_ANTE_MARKET_PRICE"],
            TimeRange::months(r.period.unwrap().month(), r.period.unwrap().month()),
        );
        assert!(s.wants_ranged(&zip, &named));
        assert!(!s.wants_ranged(&r, &named));
        // No table named: the whole zip, its directory unread.
        assert!(!s.wants_ranged(&zip, &query(&[], named.range)));
        let entry = |name: &str, size: u64| Entry {
            name: name.to_owned(),
            size,
            compressed: size,
            crc32: 0,
            method: 0,
            dos_date: 0,
            dos_time: 0,
            local_header_offset: 0,
        };
        let entries = [
            entry(
                "int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv",
                10,
            ),
            entry("int652_v1_x~20261001113227.csv", 20),
            entry("sub/", 0),
        ];
        assert_eq!(
            s.pick(&zip, &entries, &query(&[], months("2026-10", "2026-10"))),
            Picks::Whole
        );
        let picked = s.pick(
            &zip,
            &entries,
            &query(
                &["STTM_INT651_EX_ANTE_MARKET_PRICE"],
                months("2026-10", "2026-10"),
            ),
        );
        let one = "https://h/Reports/CURRENT/STTM/Day01.zip!int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv";
        assert!(
            matches!(&picked, Picks::Entries(p) if p.len() == 1
                && p[0].index == 0
                && p[0].remote.url.as_str() == one
                && p[0].remote.size == Some(10)),
            "{picked:?}"
        );
        assert_eq!(s.granularity(), phasor_kit::Granularity::Month);
        assert!(s.serves("ANYTHING"), "every table until told otherwise");
        let s = s.with_tables_served(Arc::new(|t: &str| t.starts_with("STTM_")));
        assert!(s.serves("STTM_INT651_EX_ANTE_MARKET_PRICE") && !s.serves("GBB_FACILITIES"));
    }
}
