//! What every zip holds before it is downloaded: its central directory, read
//! by ranged requests ([`crate::zipdir`]) into `{home}/mirror/peek.ndjson`,
//! one line per zip.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compare::pairs_in;
use crate::crawl::{Mirror, Remote, RemoteKind};
use crate::diag::Diagnose as _;
use crate::error::MirrorError;
use crate::net::{FetchError, Fetchers, RangeRead, Span};
use crate::stamp::Stamp;
use crate::zipdir::{self, DirError, Entry};

/// What one zip holds, or why that could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peeked {
    /// The zip's URL.
    pub url: String,
    /// When its directory was read (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peeked_at: Option<Stamp>,
    /// The size the crawl listed it at when it was peeked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_size: Option<u64>,
    /// The listed date as text, read only to derive its instant; never written.
    #[serde(default, skip_serializing)]
    pub listed_modified: Option<String>,
    /// `listed_modified` as an instant (AEST).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified_at: Option<Stamp>,
    /// Its size as the server gave it (`Content-Range`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The server's `ETag` for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The server's `Last-Modified` as text, read only to derive its instant; never written.
    #[serde(default, skip_serializing)]
    pub last_modified: Option<String>,
    /// `last_modified` as an instant (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_at: Option<Stamp>,
    /// Its entries, when its directory was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<Entry>>,
    /// Why not, when it was not: it is not a zip, or its directory is damaged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PeekError>,
}

/// Why a zip's directory could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeekError {
    /// `zip.not_a_zip` or `zip.bad_directory`.
    pub code: String,
    /// What happened.
    pub message: String,
}

impl Peeked {
    /// The record with any missing `_at` instants, its entries' too, derived from the text.
    pub fn derived(mut self) -> Self {
        if self.listed_modified_at.is_none() {
            self.listed_modified_at = self
                .listed_modified
                .as_deref()
                .and_then(|t| Stamp::from_listing(t, crate::stamp::AEST));
        }
        if self.last_modified_at.is_none() {
            self.last_modified_at = self
                .last_modified
                .as_deref()
                .and_then(Stamp::from_http_date);
        }
        if let Some(entries) = self.entries.take() {
            self.entries = Some(entries.into_iter().map(Entry::derived).collect());
        }
        self
    }
}

/// One zip peeked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ZipPeeked<'a> {
    /// The zip.
    pub url: &'a str,
    /// Its entries, when read.
    pub entries: Option<usize>,
    /// Zips peeked so far in this pass.
    pub done: usize,
    /// Zips to peek in this pass.
    pub total: usize,
}

/// What a peek pass did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PeekSummary {
    /// Zips to peek: listed, not removed, not peeked before.
    pub zips: usize,
    /// Of those, archived when their turn came, so not read.
    pub archived: usize,
    /// Of those, read.
    pub read: usize,
    /// Of those, not zips or damaged.
    pub unreadable: usize,
    /// Of those, failed on the network; the next pass asks again.
    pub failed: Vec<crate::diag::Diagnostic>,
}

impl Mirror {
    /// The record of peeked zips.
    pub fn peek_path(&self) -> PathBuf {
        self.dir().join("peeks.ndjson")
    }
}

/// Every zip peeked so far, in the order peeked, each with its `_at` instants.
pub fn read_peeks(path: &Path) -> Result<Vec<Peeked>, MirrorError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(MirrorError::io(path)(e)),
    };
    text.lines()
        .enumerate()
        .map(|(i, l)| {
            serde_json::from_str::<Peeked>(l)
                .map(Peeked::derived)
                .map_err(|e| MirrorError::Record {
                    path: path.display().to_string(),
                    line: i as u64 + 1,
                    message: e.to_string(),
                })
        })
        .collect()
}

/// A file's spans read by ranged requests, keeping the first response's headers.
struct Ranged<'a> {
    fetcher: &'a Fetchers,
    url: &'a str,
    seen: Option<RangeRead>,
}

/// A read that failed: on the network, or in the directory.
enum ReadError {
    Fetch(FetchError),
    Dir(DirError),
}

impl From<DirError> for ReadError {
    fn from(e: DirError) -> Self {
        Self::Dir(e)
    }
}

impl zipdir::Spans for Ranged<'_> {
    type Error = ReadError;
    async fn read(&mut self, start: u64, end: u64) -> Result<Vec<u8>, ReadError> {
        let mut r = self
            .fetcher
            .fetch_range(self.url, Span::Bytes { start, end })
            .await
            .map_err(ReadError::Fetch)?;
        let bytes = std::mem::take(&mut r.bytes);
        self.seen.get_or_insert(r);
        Ok(bytes)
    }
}

/// Peeks every zip of the finished crawl that the archive does not hold, the
/// paired zips first; with `only_paired`, just those.
///
/// A zip that is not one, or whose directory is damaged, is recorded with
/// why; a refusal or a full disk stops the pass.
pub async fn peek(
    fetcher: &Fetchers,
    mirror: &Mirror,
    only_paired: bool,
    mut on_zip: impl FnMut(&ZipPeeked<'_>),
) -> Result<PeekSummary, MirrorError> {
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.done_path().display().to_string(),
        });
    }
    let path = mirror.peek_path();
    let record = mirror.record()?;
    let peeked = current_in(mirror, &record)?;
    let removed = mirror.removed()?;
    let paired: HashSet<String> = pairs_in(&record, &removed, fetcher.archive())
        .into_iter()
        .map(|p| p.zip)
        .collect();
    let mut zips: Vec<Remote> = mirror
        .kept_in(record)
        .into_iter()
        .filter(|r| r.kind == RemoteKind::File && is_zip_name(&r.url))
        .filter(|r| !peeked.contains_key(&r.url) && !removed.contains_key(&r.url))
        .filter(|r| !only_paired || paired.contains(&r.url))
        .collect();
    // Stable: the paired zips first, each group in URL order.
    zips.sort_by_key(|r| !paired.contains(&r.url));
    let mut summary = PeekSummary {
        zips: zips.len(),
        ..PeekSummary::default()
    };
    for (done, remote) in zips.iter().enumerate() {
        let url = &remote.url;
        // Asked here, not up front, since the download beside it may have
        // archived the zip since the pass began.
        if fetcher.archive().path_for(url).is_file() {
            summary.archived += 1;
            continue;
        }
        let mut src = Ranged {
            fetcher,
            url,
            seen: None,
        };
        let offset = fetcher.offset_of(url);
        let read = zipdir::read_directory_in(&mut src, remote.size.unwrap_or(0), offset).await;
        let seen = src.seen.take();
        let last_modified = seen.as_ref().and_then(|s| s.last_modified.clone());
        let mut record = Peeked {
            url: url.clone(),
            peeked_at: Some(Stamp::now()),
            listed_size: remote.size,
            listed_modified: None,
            listed_modified_at: remote.modified_at,
            size: seen.as_ref().and_then(|s| s.total_size),
            etag: seen.and_then(|s| s.etag),
            last_modified_at: last_modified.as_deref().and_then(Stamp::from_http_date),
            last_modified: None,
            entries: None,
            error: None,
        };
        match read {
            Ok(entries) => {
                summary.read += 1;
                record.entries = Some(entries);
            }
            Err(ReadError::Dir(e)) => {
                summary.unreadable += 1;
                record.error = Some(peek_error(&e));
            }
            Err(ReadError::Fetch(e)) if matches!(e, FetchError::Blocked { .. }) => {
                return Err(e.into());
            }
            Err(ReadError::Fetch(e)) => {
                summary.failed.push(e.diagnostic());
                continue;
            }
        }
        append(&path, &record)?;
        on_zip(&ZipPeeked {
            url,
            entries: record.entries.as_ref().map(Vec::len),
            done: done + 1,
            total: summary.zips,
        });
    }
    Ok(summary)
}

/// The crawl record and the zips whose listing holds against it, read once
/// for many [`keep_listing`]s.
#[derive(Debug, Clone, Default)]
pub struct Known {
    /// The crawl record, in URL order.
    pub record: Vec<Remote>,
    /// The zips [`current`] holds a listing of.
    pub peeked: HashSet<String>,
}

impl Known {
    /// The record and the current listings as they are now.
    pub fn read(mirror: &Mirror) -> Result<Self, MirrorError> {
        let record = mirror.record()?;
        let peeked = current_in(mirror, &record)?.into_keys().collect();
        Ok(Self { record, peeked })
    }
}

/// Records the listing of the zip `url` archived at `path`, unless the record has it already.
pub fn keep_listing(
    mirror: &Mirror,
    known: &mut Known,
    url: &str,
    path: &Path,
    offset: i32,
) -> Result<(), MirrorError> {
    if known.peeked.contains(url) {
        return Ok(());
    }
    let at = known.record.partition_point(|r| r.url.as_str() < url);
    let listed = known.record.get(at).filter(|r| r.url == url);
    let sidecar = crate::net::Archive::read(path)?;
    let file = std::fs::File::open(path).map_err(MirrorError::io(path))?;
    let len = file.metadata().map_err(MirrorError::io(path))?.len();
    let read = futures::executor::block_on(zipdir::read_directory_in(
        &mut zipdir::OnDisk(file),
        len,
        offset,
    ));
    let (entries, error) = match read {
        Ok(entries) => (Some(entries), None),
        Err(e) => (None, Some(peek_error(&e))),
    };
    let peeked = Peeked {
        url: url.to_owned(),
        peeked_at: Some(Stamp::now()),
        listed_size: listed.and_then(|r| r.size),
        listed_modified_at: listed.and_then(|r| r.modified_at),
        listed_modified: None,
        size: Some(len),
        etag: sidecar.as_ref().and_then(|s| s.etag().map(str::to_owned)),
        last_modified: None,
        last_modified_at: sidecar.and_then(|s| s.modified()),
        entries,
        error,
    };
    append(&mirror.peek_path(), &peeked)?;
    known.peeked.insert(url.to_owned());
    Ok(())
}

/// The latest record of each zip peeked at the size and date the crawl lists it at.
pub fn current(mirror: &Mirror) -> Result<HashMap<String, Peeked>, MirrorError> {
    current_in(mirror, &mirror.record()?)
}

/// [`current`] against the crawl `record` given.
pub fn current_in(
    mirror: &Mirror,
    record: &[Remote],
) -> Result<HashMap<String, Peeked>, MirrorError> {
    let listed: HashMap<&str, (Option<u64>, Option<Stamp>)> = record
        .iter()
        .map(|r| (r.url.as_str(), (r.size, r.modified_at)))
        .collect();
    let mut out = HashMap::new();
    for p in read_peeks(&mirror.peek_path())? {
        let holds = listed.get(p.url.as_str()).is_some_and(|(size, at)| {
            p.listed_size == *size
                && match (p.listed_modified_at, at) {
                    (Some(a), Some(b)) => a.same_instant(*b),
                    (a, b) => a.is_none() && b.is_none(),
                }
        });
        if holds {
            out.insert(p.url.clone(), p);
        } else {
            out.remove(&p.url);
        }
    }
    Ok(out)
}

/// The latest record of each zip peeked, current or not.
pub fn latest(mirror: &Mirror) -> Result<HashMap<String, Peeked>, MirrorError> {
    Ok(read_peeks(&mirror.peek_path())?
        .into_iter()
        .map(|p| (p.url.clone(), p))
        .collect())
}

/// Whether a URL names a zip.
fn is_zip_name(url: &str) -> bool {
    url.len() >= 4 && url[url.len() - 4..].eq_ignore_ascii_case(".zip")
}

fn peek_error(e: &DirError) -> PeekError {
    let d = e.diagnostic();
    PeekError {
        code: d.code.into_owned(),
        message: d.message,
    }
}

/// Appends one line and syncs it, so a cut-off pass keeps what it read.
fn append(path: &Path, record: &Peeked) -> Result<(), MirrorError> {
    // A time given as text is written as its instant only.
    let record = record.clone().derived();
    let mut line = serde_json::to_vec(&record).expect("a Peeked always serialises");
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(MirrorError::io(path))?;
    f.write_all(&line).map_err(MirrorError::io(path))?;
    crate::net::archive::to_disk(&f).map_err(MirrorError::io(path))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::crawl::{crawl, root};
    use crate::net::transport::{Canned, StaticTransport};
    use crate::net::{Archive, FetcherConfig};
    use crate::testzip;

    fn line(href: &str, size: Option<usize>) -> String {
        let what = size.map_or("&lt;dir&gt;".to_owned(), |s| s.to_string());
        format!(" Monday, October 12, 2026 10:45 AM {what} <A HREF=\"{href}\">x</A><br>")
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

    #[tokio::test]
    async fn peeks_each_zip_once_the_paired_ones_first() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let a = testzip::stored("a.csv", b"1,2\n");
        let m = testzip::deflated("M/t.zip", b"x");
        let m_len = m.len() as u64;
        let files: [(&str, &[u8]); 5] = [
            ("/A/a.zip", &a),
            ("/A/M.zip", &m),
            ("/A/text.ZIP", b"not a zip"),
            ("/A/kept.zip", b"archived"),
            ("/A/b.txt", b"b"),
        ];
        let mut page = String::from("<pre>");
        page.push_str(&line("/A/M/", None));
        for (href, bytes) in files {
            page.push_str(&line(href, Some(bytes.len())));
        }
        page.push_str(&line("/A/flaky.zip", Some(9)));
        page.push_str("</pre>");
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/A/", Canned::ok(page.as_bytes()));
        t.add("https://h/A/M/", Canned::ok(b"<pre></pre>"));
        for (href, bytes) in files {
            t.add(&format!("https://h{href}"), Canned::ranged(bytes));
        }
        t.add("https://h/A/flaky.zip", Canned::status(500));
        let f = fetcher(&t, &archive);
        crawl(&f, &mirror, &[root("https://h/A/").unwrap()], false, |_| {})
            .await
            .unwrap();
        std::fs::create_dir_all(archive.join("h/A")).unwrap();
        std::fs::write(archive.join("h/A/kept.zip"), b"archived").unwrap();

        let mut order = Vec::new();
        let s = peek(&f, &mirror, false, |z| order.push(z.url.to_owned()))
            .await
            .unwrap();
        assert_eq!(
            (s.zips, s.archived, s.read, s.unreadable, s.failed.len()),
            (5, 1, 2, 1, 1)
        );
        assert_eq!(s.failed[0].code, "net.retries_exhausted");
        assert_eq!(
            order,
            [
                "https://h/A/M.zip",
                "https://h/A/a.zip",
                "https://h/A/text.ZIP"
            ]
        );
        let peeks = read_peeks(&mirror.peek_path()).unwrap();
        let entries = peeks[0].entries.as_ref().unwrap();
        assert_eq!(
            (entries[0].name.as_str(), entries[0].method),
            ("M/t.zip", 8)
        );
        assert_eq!(peeks[1].entries.as_ref().unwrap()[0].size, 4);
        assert_eq!(peeks[2].error.as_ref().unwrap().code, "zip.not_a_zip");
        // Each says when, against which listing, and what the server said.
        let m = &peeks[0];
        assert!(m.peeked_at.is_some());
        assert_eq!((m.listed_size, m.size), (Some(m_len), Some(m_len)));
        // Instants only: the text is not written.
        assert_eq!((&m.listed_modified, &m.last_modified), (&None, &None));
        assert_eq!(m.etag.as_deref(), Some("\"etag\""));
        // Each text time has its instant beside it: the listing's in AEST,
        // the server's in UTC, the zip's entries' in AEST.
        assert_eq!(
            m.listed_modified_at.unwrap().to_string(),
            "2026-10-12T10:45:00+10:00"
        );
        assert_eq!(
            m.last_modified_at.unwrap().to_string(),
            "2024-02-13T08:27:00Z"
        );
        assert!(entries[0].modified_at.is_some());
        let text = std::fs::read_to_string(mirror.peek_path()).unwrap();
        crate::stamp::check::assert_aware_lines(&text);
        // A record written without the instants reads back with them.
        let stripped: String = text
            .lines()
            .map(|l| {
                let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
                // As an older mirror wrote it: the times as text only.
                let o = v.as_object_mut().unwrap();
                let listed = o.remove("listed_modified_at").unwrap();
                let at = Stamp::parse(listed.as_str().unwrap()).unwrap();
                assert_eq!(at.to_string(), "2026-10-12T10:45:00+10:00");
                o.insert(
                    "listed_modified".into(),
                    "Monday, October 12, 2026 10:45 AM".into(),
                );
                o.remove("last_modified_at");
                o.insert(
                    "last_modified".into(),
                    "Tue, 13 Feb 2024 08:27:00 GMT".into(),
                );
                if let Some(es) = o.get_mut("entries").and_then(|e| e.as_array_mut()) {
                    for e in es {
                        let e = e.as_object_mut().unwrap();
                        let at = Stamp::parse(e.remove("modified_at").unwrap().as_str().unwrap());
                        let text = at.unwrap().to_string()[..19].replace('T', " ");
                        e.insert("modified".into(), text.into());
                    }
                }
                v.to_string() + "\n"
            })
            .collect();
        std::fs::write(mirror.peek_path(), &stripped).unwrap();
        let instants = |p: &Peeked| {
            let entries: Vec<_> = p.entries.iter().flatten().map(|e| e.modified_at).collect();
            (p.listed_modified_at, p.last_modified_at, entries)
        };
        let old: Vec<_> = read_peeks(&mirror.peek_path())
            .unwrap()
            .iter()
            .map(instants)
            .collect();
        assert_eq!(old, peeks.iter().map(instants).collect::<Vec<_>>());
        std::fs::write(mirror.peek_path(), &text).unwrap();
        assert_eq!(current(&mirror).unwrap().len(), 3);

        // AEMO publishes M.zip again: the crawl lists another size, its
        // listing no longer holds, and it is peeked again.
        let record = mirror.record_text();
        let republished = record.replace(
            &format!("\"url\":\"https://h/A/M.zip\",\"kind\":\"file\",\"size\":{m_len}"),
            &format!(
                "\"url\":\"https://h/A/M.zip\",\"kind\":\"file\",\"size\":{}",
                m_len + 1
            ),
        );
        assert_ne!(record, republished);
        mirror.set_record_text(&republished);
        assert!(!current(&mirror).unwrap().contains_key("https://h/A/M.zip"));
        let mut again = Vec::new();
        peek(&f, &mirror, false, |z| again.push(z.url.to_owned()))
            .await
            .unwrap();
        assert!(again.contains(&"https://h/A/M.zip".to_owned()), "{again:?}");
        assert!(current(&mirror).unwrap().contains_key("https://h/A/M.zip"));

        // Again: only the zip that failed is asked for; the archived one is
        // passed over without a request.
        let before = t.request_count();
        let s = peek(&f, &mirror, false, |_| {}).await.unwrap();
        assert_eq!((s.zips, s.archived, s.failed.len()), (2, 1, 1));
        assert_eq!(t.request_count(), before + 1);

        // Refused: the pass stops.
        let refusing = Arc::new(StaticTransport::default());
        refusing.add("https://h/A/flaky.zip", Canned::status(403));
        let f = fetcher(&refusing, &archive);
        let e = peek(&f, &mirror, false, |_| {}).await.unwrap_err();
        assert_eq!(e.diagnostic().code, "net.blocked");
    }

    #[tokio::test]
    async fn needs_a_crawl_and_readable_records() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let t = Arc::new(StaticTransport::default());
        let f = fetcher(&t, home.path());
        let e = peek(&f, &mirror, false, |_| {}).await.unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.crawl_incomplete");
        std::fs::create_dir_all(mirror.dir()).unwrap();
        std::fs::write(mirror.done_path(), "").unwrap();
        mirror.set_record_text("");
        std::fs::write(mirror.peek_path(), "{\n").unwrap();
        let e = peek(&f, &mirror, false, |_| {}).await.unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.record_corrupt");
        std::fs::remove_file(mirror.peek_path()).unwrap();
        std::fs::create_dir_all(mirror.peek_path()).unwrap();
        let e = read_peeks(&mirror.peek_path()).unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.io");
        // A record that cannot be appended to stops the pass.
        std::fs::remove_dir(mirror.peek_path()).unwrap();
        assert_eq!(
            append(
                &home.path().join("no/such/dir/p"),
                &Peeked {
                    url: "u".into(),
                    entries: None,
                    error: None,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .diagnostic()
            .code,
            "mirror.io"
        );
        assert_eq!(
            peek_error(&DirError::Damaged("x")).code,
            "zip.bad_directory"
        );
        assert_eq!(
            peek_error(&DirError::Short { wanted: 2, got: 1 }).code,
            "zip.bad_directory"
        );
    }
}
