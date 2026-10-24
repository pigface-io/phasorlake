//! A feed's folders as a source: the roots the shared feeds table names
//! for it, walked by their directory listings, each file named by the
//! feed's rules.
//!
//! The gas feeds (GBB, GSH, STTM, DWGM under nemweb's report folders) and
//! the WEM's WEMDE documents are all folder trees of files whose names say
//! their block, their version and their release month, and the lake and
//! the census read them by the one table in utilake-plain-decode. This
//! source lists such a tree, finds its files in the archive (each version
//! the mirror kept, in order, so a file that restates the one before is
//! read against it) and claims a URL by the feed's roots in its path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use url::Url;
use utilake_acquire::{
    Archive, Archived, Entry, FetchError, Fetcher, LocalFile, Pick, Picks, Query, RemoteFile,
    Source, listing,
};
use utilake_core::calendar::local_year_month;
use utilake_core::timetext::{Stamp, parse_iis_listing};
use utilake_core::{Period, YearMonth};
use utilake_plain_decode::names::block_name;
use utilake_plain_decode::{BlockName, Feed};

/// Names the table that claims a block, when the registry declares one.
pub type TableOf = Arc<dyn Fn(&BlockName) -> Option<String> + Send + Sync>;

/// A feed's folders under one host.
pub struct FeedFolders {
    id: &'static str,
    base: Url,
    feed: &'static Feed,
    table_of: TableOf,
}

fn parse_error(url: &str, what: &'static str, e: &dyn std::fmt::Display) -> FetchError {
    FetchError::Parse {
        url: url.to_owned(),
        what,
        message: e.to_string(),
    }
}

/// The month of an instant in the feed's clock.
fn month_of(at: Stamp, offset_secs: i32) -> Option<YearMonth> {
    let (year, month) = local_year_month(at.secs_utc.saturating_mul(1_000_000_000), offset_secs);
    YearMonth::new(year, u8::try_from(month).ok()?)
}

impl FeedFolders {
    /// The folders of `feed` under `base_url` (the feed's host, a mirror or
    /// a test server), their files recorded under `id`, their blocks
    /// named by `table_of` where the registry claims them.
    pub fn new(
        id: &'static str,
        base_url: &str,
        feed: &'static Feed,
        table_of: TableOf,
    ) -> Result<Self, FetchError> {
        let mut base = Url::parse(base_url).map_err(|e| parse_error(base_url, "base URL", &e))?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            id,
            base,
            feed,
            table_of,
        })
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
                    .map_err(|e| parse_error(&rel, "feed root", &e))
            })
            .collect()
    }

    /// A file at `url` as the feed sees it: the table its block belongs
    /// to, and the month it was released in, from its name first, else
    /// from the time it was listed with.
    fn remote(
        &self,
        url: Url,
        size: Option<u64>,
        modified: Option<String>,
        modified_at: Option<Stamp>,
    ) -> RemoteFile {
        let path = url.path().to_owned();
        let table = block_name(self.feed, &path).and_then(|b| (self.table_of)(&b));
        let period = self
            .feed
            .release_month(&path)
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

    /// Whether a query asks for this file: its month, when known, is in
    /// the range, and its table is one asked for (every table when none
    /// is named, a block no table claims only then).
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

    /// Whether a path lies in a folder the feed skips; a URL writes a space
    /// in a folder's name as `%20`.
    fn skipped(&self, path: &str) -> bool {
        path.split('/')
            .filter(|s| !s.is_empty())
            .any(|s| self.feed.ignores(&s.replace("%20", " ")))
    }

    /// The version stamp a kept version carries in its file name
    /// (`{name}@{stamp}`), else the time it was listed with, else none.
    fn version_tag(a: &Archived) -> Option<String> {
        let name = a.path.file_name()?.to_str()?;
        if let Some((_, stamp)) = name.rsplit_once('@') {
            return Some(stamp.to_owned());
        }
        a.sidecar
            .listed_modified_at
            .map(|s| s.rfc3339().replace([':', '-'], ""))
    }
}

#[async_trait]
impl Source for FeedFolders {
    fn id(&self) -> &'static str {
        self.id
    }

    /// Every file under the feed's roots the query asks for, by their
    /// listings, folders the feed skips left out.
    async fn list(&self, query: &Query, fetcher: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
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
                    .map_err(|er| parse_error(&e.href, "listing href", &er))?;
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

    /// The archived files under the feed's roots the query asks for, every
    /// version of a file the mirror kept in the order published, each
    /// naming the one before it so a feed that restates its files is read
    /// against it. A later version is told apart in the manifest by the
    /// version's stamp as the URL's fragment.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        let mut out = Vec::new();
        for root in self.roots()? {
            let dir = archive.path_for(root.as_str());
            let mut by_url: BTreeMap<String, Vec<Archived>> = BTreeMap::new();
            for a in archive.entries_under(Some(&dir)) {
                by_url.entry(a.sidecar.url.clone()).or_default().push(a);
            }
            for (url, mut versions) in by_url {
                let Ok(base) = Url::parse(&url) else {
                    continue;
                };
                if self.skipped(base.path()) {
                    continue;
                }
                versions.sort_by(|a, b| {
                    let key = |x: &Archived| {
                        (
                            x.sidecar.listed_modified_at.map(|s| s.secs_utc),
                            x.path.clone(),
                        )
                    };
                    key(a).cmp(&key(b))
                });
                let mut previous: Option<PathBuf> = None;
                for (i, a) in versions.into_iter().enumerate() {
                    let mut url = base.clone();
                    if i > 0 {
                        let tag = Self::version_tag(&a).unwrap_or_else(|| i.to_string());
                        url.set_fragment(Some(&tag));
                    }
                    let remote = self.remote(
                        url,
                        Some(a.sidecar.size),
                        a.sidecar.listed_modified.clone(),
                        a.sidecar.listed_modified_at,
                    );
                    let path = a.path.clone();
                    if self.wanted(&remote, query) {
                        out.push(LocalFile {
                            remote,
                            path: a.path,
                            sha256: a.sidecar.sha256,
                            size: a.sidecar.size,
                            cached: true,
                            etag: a.sidecar.etag,
                            last_modified: a.sidecar.last_modified,
                            last_modified_at: a.sidecar.last_modified_at,
                            previous: previous.clone(),
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

    /// A URL under one of the feed's roots, whatever its host: a mirror's
    /// `file://` path and the host's `https://` URL both claim.
    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        let path = url.path();
        let under_root = self.feed.roots.iter().any(|r| {
            let root = format!("/{}/", r.trim_matches('/'));
            path.contains(&root) && path.len() > path.find(&root).unwrap_or(0) + root.len()
        });
        if !under_root || path.ends_with('/') || self.skipped(path) {
            return None;
        }
        Some(self.remote(url.clone(), None, None, None))
    }

    /// A zip under a feed's folders holds a day's many reports: read by its
    /// directory when the query names tables.
    fn wants_ranged(&self, remote: &RemoteFile) -> bool {
        remote
            .url
            .path()
            .rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    }

    /// The entries whose blocks belong to the tables asked for; the whole
    /// zip when no table is named.
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

    use utilake_acquire::transport::{Canned, StaticTransport};
    use utilake_acquire::{FetcherConfig, Sidecar};
    use utilake_core::TimeRange;
    use utilake_plain_decode::feeds::feed;

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
        Arc::new(|b: &BlockName| match (b.family.as_str(), b.name.as_str()) {
            ("GBB", "ACTUAL_FLOW_STORAGE" | "ACTUAL_FLOW_STORAGE_LAST31") => {
                Some("GBB_ACTUAL_FLOW_STORAGE".to_owned())
            }
            ("STTM", n) if n.starts_with("INT651") => {
                Some("STTM_INT651_EX_ANTE_MARKET_PRICE".to_owned())
            }
            _ => None,
        })
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
        let listed_modified_at = listed.and_then(|l| parse_iis_listing(l, 36_000));
        Archive::write_sidecar(
            &path,
            &Sidecar {
                url: url.to_owned(),
                size: bytes.len() as u64,
                sha256: utilake_acquire::archive::sha256_hex(bytes),
                fetched_at: "2026-10-21T12:00:00Z".into(),
                etag: None,
                last_modified: None,
                last_modified_at: None,
                listed_modified: listed.map(str::to_owned),
                listed_modified_at,
                tool: "test".into(),
                extracted_from: None,
            },
        )
        .unwrap();
        path
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
        let mut tagless = archive.entries_under(Some(bare.parent().unwrap()));
        tagless.retain(|a| a.path == bare);
        assert_eq!(FeedFolders::version_tag(&tagless[0]).as_deref(), Some("x"));
        let plain = archive
            .entries_under(None)
            .into_iter()
            .find(|a| a.sidecar.url == ignored)
            .unwrap();
        assert_eq!(FeedFolders::version_tag(&plain), None);
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
        let mut sidecar = Archive::read_sidecar(&archive.path_for(kept))
            .unwrap()
            .unwrap();
        sidecar.url = "not a url".into();
        Archive::write_sidecar(&odd, &sidecar).unwrap();
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
        assert!(claimed("https://h/Reports/CURRENT/STTM/").is_none());
        assert!(claimed("https://h/Reports/CURRENT/GBB/x.csv").is_none());
        assert!(claimed("https://h/Reports/CURRENT/STTM/MOS Estimates/x.csv").is_none());
        let zip = claimed("https://h/Reports/CURRENT/STTM/Day01.zip").unwrap();
        assert!(s.wants_ranged(&zip));
        assert!(!s.wants_ranged(&r));
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
        let Picks::Entries(p) = s.pick(
            &zip,
            &entries,
            &query(
                &["STTM_INT651_EX_ANTE_MARKET_PRICE"],
                months("2026-10", "2026-10"),
            ),
        ) else {
            panic!("entries")
        };
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].index, 0);
        assert_eq!(
            p[0].remote.url.as_str(),
            "https://h/Reports/CURRENT/STTM/Day01.zip!int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv"
        );
        assert_eq!(p[0].remote.size, Some(10));
        assert_eq!(s.granularity(), utilake_acquire::Granularity::Month);
    }
}
