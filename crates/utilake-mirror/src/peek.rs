//! What every zip holds, before it is downloaded: its central directory,
//! read by ranged requests ([`crate::zipdir`]).
//!
//! Each zip the crawl lists and the archive does not hold is peeked once;
//! the entries go to `{home}/mirror/peek.ndjson`, one line per zip, which is
//! also how a peek that was cut off carries on. Zips beside a folder of the
//! same name (the whole-month zips) come first, so `compare` can rule on
//! them before the download reaches them.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compare::pairs;
use crate::crawl::{Mirror, RemoteKind, read_record};
use crate::diag::Diagnose as _;
use crate::error::MirrorError;
use crate::net::{FetchError, Fetcher, Span};
use crate::zipdir::{self, DirError, Entry};

/// What one zip holds, or why that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peeked {
    /// The zip's URL.
    pub url: String,
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
    /// Zips to peek: listed, not archived, not removed, not peeked before.
    pub zips: usize,
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
        self.dir().join("peek.ndjson")
    }
}

/// Every zip peeked so far, in the order peeked.
pub fn read_peeks(path: &Path) -> Result<Vec<Peeked>, MirrorError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(MirrorError::io(path)(e)),
    };
    text.lines()
        .enumerate()
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| MirrorError::Record {
                path: path.display().to_string(),
                line: i as u64 + 1,
                message: e.to_string(),
            })
        })
        .collect()
}

/// A file's spans read by ranged requests.
struct Ranged<'a> {
    fetcher: &'a Fetcher,
    url: &'a str,
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
        self.fetcher
            .fetch_range(self.url, Span { start, end })
            .await
            .map_err(ReadError::Fetch)
    }
}

/// Peeks every zip of the finished crawl that the archive does not hold,
/// the paired zips first, one at a time (the fetcher paces them).
///
/// A zip that is not one, or whose directory is damaged, is recorded with
/// why; a network failure is reported and asked again next pass; a refusal
/// or a full disk stops the pass.
pub async fn peek(
    fetcher: &Fetcher,
    mirror: &Mirror,
    mut on_zip: impl FnMut(&ZipPeeked<'_>),
) -> Result<PeekSummary, MirrorError> {
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.crawl_dir().display().to_string(),
        });
    }
    let path = mirror.peek_path();
    let peeked: HashSet<String> = read_peeks(&path)?.into_iter().map(|p| p.url).collect();
    let removed = mirror.removed()?;
    let paired: HashSet<String> = pairs(mirror)?.into_iter().map(|p| p.zip).collect();
    let mut zips: Vec<(String, u64)> = read_record(&mirror.record_path())?
        .into_iter()
        .filter(|r| r.kind == RemoteKind::File && is_zip_name(&r.url))
        .filter(|r| !peeked.contains(&r.url) && !removed.contains_key(&r.url))
        .filter(|r| !fetcher.archive().path_for(&r.url).is_file())
        .map(|r| (r.url, r.size.unwrap_or(0)))
        .collect();
    // Stable: the paired zips first, each group in URL order.
    zips.sort_by_key(|(url, _)| !paired.contains(url));
    let mut summary = PeekSummary {
        zips: zips.len(),
        ..PeekSummary::default()
    };
    for (done, (url, len)) in zips.iter().enumerate() {
        let mut src = Ranged { fetcher, url };
        let record = match zipdir::read_directory(&mut src, *len).await {
            Ok(entries) => {
                summary.read += 1;
                Peeked {
                    url: url.clone(),
                    entries: Some(entries),
                    error: None,
                }
            }
            Err(ReadError::Dir(e)) => {
                summary.unreadable += 1;
                Peeked {
                    url: url.clone(),
                    entries: None,
                    error: Some(peek_error(&e)),
                }
            }
            Err(ReadError::Fetch(e)) if matches!(e, FetchError::Blocked { .. }) => {
                return Err(e.into());
            }
            Err(ReadError::Fetch(e)) => {
                summary.failed.push(e.diagnostic());
                continue;
            }
        };
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

/// A file on disk, read by span; every span lies inside it, as the
/// directory reader checks, so a read fails only with the disk.
struct OnDisk(std::fs::File);

impl zipdir::Spans for OnDisk {
    type Error = DirError;
    async fn read(&mut self, start: u64, end: u64) -> Result<Vec<u8>, DirError> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let mut buf = vec![0u8; usize::try_from(end - start).unwrap_or(0)];
        self.0
            .seek(SeekFrom::Start(start))
            .and_then(|_| self.0.read_exact(&mut buf))
            .map_err(|_| DirError::Damaged("the file could not be read"))?;
        Ok(buf)
    }
}

/// Records the listing of the zip `url` archived at `path`, read from its
/// central directory on disk, unless the record has it already: kept when
/// a downloaded zip is removed, so what it held stays known.
pub fn keep_listing(mirror: &Mirror, url: &str, path: &Path) -> Result<(), MirrorError> {
    let record = mirror.peek_path();
    if read_peeks(&record)?.iter().any(|p| p.url == url) {
        return Ok(());
    }
    let file = std::fs::File::open(path).map_err(MirrorError::io(path))?;
    let len = file.metadata().map_err(MirrorError::io(path))?.len();
    let read = futures::executor::block_on(zipdir::read_directory(&mut OnDisk(file), len));
    let peeked = match read {
        Ok(entries) => Peeked {
            url: url.to_owned(),
            entries: Some(entries),
            error: None,
        },
        Err(e) => Peeked {
            url: url.to_owned(),
            entries: None,
            error: Some(peek_error(&e)),
        },
    };
    append(&record, &peeked)
}

/// Whether a URL names a zip.
fn is_zip_name(url: &str) -> bool {
    url.len() >= 4 && url[url.len() - 4..].eq_ignore_ascii_case(".zip")
}

fn peek_error(e: &DirError) -> PeekError {
    let code = match e {
        DirError::NotAZip(_) => "zip.not_a_zip",
        DirError::Damaged(_) => "zip.bad_directory",
    };
    PeekError {
        code: code.to_owned(),
        message: e.to_string(),
    }
}

/// Appends one line and syncs it, so a cut-off pass keeps what it read.
fn append(path: &Path, record: &Peeked) -> Result<(), MirrorError> {
    let mut line = serde_json::to_vec(record).expect("a Peeked always serialises");
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(MirrorError::io(path))?;
    f.write_all(&line).map_err(MirrorError::io(path))?;
    f.sync_data().map_err(MirrorError::io(path))
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
    async fn peeks_each_zip_once_the_paired_ones_first() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let a = testzip::stored("a.csv", b"1,2\n");
        let m = testzip::deflated("M/t.zip", b"x");
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
            t.add(&format!("https://h{href}"), Canned::ok(bytes));
        }
        t.add("https://h/A/flaky.zip", Canned::status(500));
        let f = fetcher(&t, &archive);
        crawl(&f, &mirror, &[root("https://h/A/").unwrap()], false, |_| {})
            .await
            .unwrap();
        std::fs::create_dir_all(archive.join("h/A")).unwrap();
        std::fs::write(archive.join("h/A/kept.zip"), b"archived").unwrap();

        let mut order = Vec::new();
        let s = peek(&f, &mirror, |z| order.push(z.url.to_owned()))
            .await
            .unwrap();
        assert_eq!((s.zips, s.read, s.unreadable, s.failed.len()), (4, 2, 1, 1));
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

        // Again: only the zip that failed is asked for.
        let before = t.request_count();
        let s = peek(&f, &mirror, |_| {}).await.unwrap();
        assert_eq!((s.zips, s.failed.len()), (1, 1));
        assert_eq!(t.request_count(), before + 1);

        // Refused: the pass stops.
        let refusing = Arc::new(StaticTransport::default());
        refusing.add("https://h/A/flaky.zip", Canned::status(403));
        let f = fetcher(&refusing, &archive);
        let e = peek(&f, &mirror, |_| {}).await.unwrap_err();
        assert_eq!(e.diagnostic().code, "net.blocked");
    }

    #[tokio::test]
    async fn needs_a_crawl_and_readable_records() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let t = Arc::new(StaticTransport::default());
        let f = fetcher(&t, home.path());
        let e = peek(&f, &mirror, |_| {}).await.unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.crawl_incomplete");
        std::fs::create_dir_all(mirror.crawl_dir()).unwrap();
        std::fs::write(mirror.done_path(), "").unwrap();
        std::fs::write(mirror.record_path(), "").unwrap();
        std::fs::write(mirror.peek_path(), "{\n").unwrap();
        let e = peek(&f, &mirror, |_| {}).await.unwrap_err();
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
    }
}
