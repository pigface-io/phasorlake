//! Downloading: paced, retried, stopped for good when refused, and kept in
//! the archive.
//!
//! A file is downloaded whole, once. Its bytes go from the transport to
//! disk without a copy: each chunk is handed as it arrived to a writer on a
//! blocking thread, which hashes it and writes it through a 1 MiB buffer,
//! so no hashing or file I/O runs on the async runtime.
//!
//! A zip that holds more than a request needs (a whole-month archive of
//! every table, a day's zip of five-minute files) is read by its central
//! directory first, through ranged requests, and only the entries the
//! source picks are fetched, each as the file it is: a nested zip stays a
//! zip, a deflated CSV is inflated. The archive keeps each at the zip's
//! path with `!{entry}` appended, its sidecar saying where in the zip it
//! was. When the picks are most of the zip, or the server ignores ranges,
//! the zip is downloaded whole instead.

use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use phasor_core::event::Event;
use phasor_core::timetext::{self, Stamp};

use phasor_zip::zipdir::{DirectoryReader, Entry, LOCAL_HEADER, Step, TAIL, local_header_len};

use crate::archive::{
    Address, Archive, Content, Held, Listed, Provenance, Served, Span as ZipSpan, Via, add_address,
};
use crate::error::FetchError;
use crate::transport::{Span, Transport};
use phasor_kit::{Listings, LocalFile, Pick, Picks, RemoteFile};
pub use phasor_net::fetch::{FetcherConfig, RangeRead};
use phasor_net::fetch::{Requests, stream_to_file};

/// The listing line a remote file was listed with: its time as an
/// instant, read from the listing's text in the host's clock when the
/// source gave only the text.
fn listed_of(remote: &RemoteFile) -> Listed {
    let url = remote.url.as_str();
    Listed {
        size: remote.size,
        at: remote.modified_at.or_else(|| {
            let offset = phasor_provenance::layout::listing_offset(url);
            timetext::parse(remote.modified.as_deref()?, offset)
        }),
    }
}

/// What [`Fetcher::fetch_picks`] fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    /// The zip, whole: the source wanted it all, the picks were most of
    /// it, the server ignored the range, or the archive had it already.
    Whole(Box<LocalFile>),
    /// The picked entries, in the order picked, each as its own file.
    Entries(Vec<LocalFile>),
}

impl Picked {
    /// The files fetched: the one zip, or each picked entry.
    pub fn files(self) -> Vec<LocalFile> {
        match self {
            Self::Whole(l) => vec![*l],
            Self::Entries(e) => e,
        }
    }
}

/// Bytes read past an entry's compressed data in one request, so its local
/// header's extra field, and the next picked entry when it follows, come
/// in the same request.
const SLACK: u64 = 256;

/// Fetches remote files into the archive.
pub struct Fetcher {
    requests: Requests,
    archive: Archive,
    events: Option<tokio::sync::mpsc::Sender<Event>>,
}

#[async_trait::async_trait]
impl Listings for Fetcher {
    async fn fetch_listing(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        self.requests.fetch_listing(url).await
    }
}

impl Fetcher {
    /// A fetcher over a transport and an archive.
    pub fn new(transport: Arc<dyn Transport>, archive: Archive, config: FetcherConfig) -> Self {
        Self {
            requests: Requests::new(transport, config),
            archive,
            events: None,
        }
    }

    /// Sends progress events to `tx`.
    #[must_use]
    pub fn with_events(mut self, tx: tokio::sync::mpsc::Sender<Event>) -> Self {
        self.events = Some(tx);
        self
    }

    /// The archive.
    pub fn archive(&self) -> &Archive {
        &self.archive
    }

    /// Whether the server has refused this client during this run.
    pub fn is_blocked(&self) -> bool {
        self.requests.is_blocked()
    }

    fn ensure_online(&self, url: &str) -> Result<(), FetchError> {
        self.requests.ensure_online(url)
    }

    /// GETs a page (a directory listing, a report page) into memory.
    pub async fn fetch_listing(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        self.requests.fetch_listing(url).await
    }

    /// GETs one span of `url` into memory, with what the server said of
    /// the whole file. A server that answers with the whole file instead
    /// is [`FetchError::RangeIgnored`].
    pub async fn fetch_range(&self, url: &str, span: Span) -> Result<RangeRead, FetchError> {
        self.requests.fetch_range(url, span).await
    }

    /// The entries of the zip `remote` that `pick` wants, each fetched
    /// alone by ranged requests and archived as the file it is, or the zip
    /// whole when the picks are most of it, the server ignores the range,
    /// or the archive holds it already. `pick` sees the zip's directory.
    pub async fn fetch_picks(
        &self,
        remote: &RemoteFile,
        pick: &(dyn Fn(&[Entry]) -> Picks + Sync),
    ) -> Result<Picked, FetchError> {
        let url = remote.url.as_str();
        if self.archive.lookup(url)?.is_some() {
            return self.fetch(remote).await.map(|l| Picked::Whole(Box::new(l)));
        }
        self.ensure_online(url)?;
        let corrupt = |entry: &str, message: String| FetchError::EntryCorrupt {
            url: url.to_owned(),
            entry: entry.to_owned(),
            message,
        };
        // The directory: its tail first, which also says how long the
        // file is; the rest as the reader asks.
        let tail = match self.fetch_range(url, Span::Last(TAIL)).await {
            Ok(t) => t,
            Err(FetchError::RangeIgnored { .. }) => {
                return self.fetch(remote).await.map(|l| Picked::Whole(Box::new(l)));
            }
            Err(e) => return Err(e),
        };
        let Some(total) = tail.total_size else {
            return self.fetch(remote).await.map(|l| Picked::Whole(Box::new(l)));
        };
        let (etag, last_modified) = (tail.etag, tail.last_modified);
        let mut reader = DirectoryReader::new(total);
        let mut step = reader
            .feed(tail.bytes)
            .map_err(|e| corrupt("", e.to_string()))?;
        let entries = loop {
            match step {
                Step::Read { start, end } => {
                    let r = self.fetch_range(url, Span::Bytes { start, end }).await?;
                    step = reader
                        .feed(r.bytes)
                        .map_err(|e| corrupt("", e.to_string()))?;
                }
                Step::Done(entries) => break entries,
            }
        };
        let picks = match pick(&entries) {
            Picks::Whole => return self.fetch(remote).await.map(|l| Picked::Whole(Box::new(l))),
            Picks::Entries(p) => p,
        };
        let entry_of = |p: &Pick| {
            entries
                .get(p.index)
                .ok_or_else(|| corrupt("", format!("no entry {} in the directory", p.index)))
        };
        let mut wanted = 0u64;
        for p in &picks {
            wanted += entry_of(p)?.compressed;
        }
        // Most of the zip: one request for the whole beats many for parts.
        if wanted.saturating_mul(2) > total {
            return self.fetch(remote).await.map(|l| Picked::Whole(Box::new(l)));
        }
        // Each pick keeps its place in the answer, whatever order the
        // requests take.
        let mut out: Vec<(usize, LocalFile)> = Vec::with_capacity(picks.len());
        let mut todo: Vec<(usize, Pick)> = Vec::new();
        for (place, p) in picks.into_iter().enumerate() {
            match self.archive.lookup(p.remote.url.as_str())? {
                Some(found) => out.push((place, LocalFile::from_held(&p.remote, found, true))),
                None => todo.push((place, p)),
            }
        }
        if !todo.is_empty()
            && let Some(tx) = &self.events
        {
            let _ = tx.try_send(Event::DownloadStarted {
                url: url.to_owned(),
                size: Some(wanted),
            });
        }
        // Entries that lie together in the zip come in one request.
        todo.sort_by_key(|(_, p)| entries[p.index].local_header_offset);
        let span_end = |e: &Entry| {
            (e.local_header_offset
                + LOCAL_HEADER as u64
                + e.name.len() as u64
                + SLACK
                + e.compressed)
                .min(total)
        };
        let mut i = 0;
        while i < todo.len() {
            let start = entries[todo[i].1.index].local_header_offset;
            let mut end = span_end(&entries[todo[i].1.index]);
            let mut j = i + 1;
            while j < todo.len() && entries[todo[j].1.index].local_header_offset <= end {
                end = end.max(span_end(&entries[todo[j].1.index]));
                j += 1;
            }
            let mut buf = self
                .fetch_range(url, Span::Bytes { start, end })
                .await?
                .bytes;
            for (place, p) in &todo[i..j] {
                let e = &entries[p.index];
                let short = || corrupt(&e.name, "shorter than the directory says".into());
                // Offsets within the request's bytes; the request spans at
                // most the file, so they fit the address space.
                let at = (e.local_header_offset - start) as usize;
                let header = buf.get(at..at + LOCAL_HEADER).ok_or_else(short)?;
                let data_start =
                    at + local_header_len(header).map_err(|x| corrupt(&e.name, x.to_string()))?;
                let data_end = data_start as u64 + e.compressed;
                // An extra field longer than the slack, or a directory that
                // overstates: the rest of the data, as far as the file goes.
                if data_end > buf.len() as u64 {
                    let more = self
                        .fetch_range(
                            url,
                            Span::Bytes {
                                start: start + buf.len() as u64,
                                end: (start + data_end).min(total),
                            },
                        )
                        .await?;
                    buf.extend_from_slice(&more.bytes);
                }
                let data = buf
                    .get(data_start..usize::try_from(data_end).unwrap_or(usize::MAX))
                    .ok_or_else(short)?;
                let bytes = unpack(e, data).map_err(|m| corrupt(&e.name, m))?;
                let local =
                    self.keep_entry(remote, e, &p.remote, &bytes, total, (&etag, &last_modified))?;
                out.push((*place, local));
            }
            i = j;
        }
        out.sort_by_key(|(place, _)| *place);
        Ok(Picked::Entries(out.into_iter().map(|(_, l)| l).collect()))
    }

    /// Archives the bytes of one entry of the zip `zip` as the file
    /// `remote`, with a sidecar that says where in the zip it was.
    fn keep_entry(
        &self,
        zip: &RemoteFile,
        e: &Entry,
        remote: &RemoteFile,
        bytes: &[u8],
        zip_size: u64,
        (etag, last_modified): (&Option<String>, &Option<String>),
    ) -> Result<LocalFile, FetchError> {
        let path = self.archive.path_for(remote.url.as_str());
        make_room(&path)?;
        let part = Archive::part_path(&path);
        std::fs::write(&part, bytes)
            .and_then(|()| std::fs::rename(&part, &path))
            .map_err(|source| FetchError::CacheIo {
                path: path.display().to_string(),
                source,
            })?;
        let provenance = Provenance::new(
            Content {
                crc32: Some(e.crc32),
                ..Content::of(bytes)
            },
            format!("phasor/{}", env!("CARGO_PKG_VERSION")),
            Address {
                url: remote.url.to_string(),
                seen: Stamp::now(),
                gone: None,
                via: Via::Range {
                    span: ZipSpan {
                        zip: zip.url.to_string(),
                        entry: e.name.clone(),
                        zip_size,
                        offset: e.local_header_offset,
                        compressed: e.compressed,
                        method: e.method,
                    },
                    listed: Some(listed_of(zip)),
                    served: Some(Served {
                        etag: etag.clone(),
                        modified: last_modified.as_deref().and_then(timetext::parse_http_date),
                    }),
                },
            },
        );
        Archive::write(&path, &provenance)?;
        Ok(LocalFile::from_held(
            remote,
            Held { path, provenance },
            false,
        ))
    }

    /// The archived copy of a remote file, or the file downloaded into the
    /// archive whole, with a sidecar that says where it came from.
    pub async fn fetch(&self, remote: &RemoteFile) -> Result<LocalFile, FetchError> {
        if let Some(found) = self.archive.lookup(remote.url.as_str())? {
            return Ok(LocalFile::from_held(remote, found, true));
        }
        self.download(remote, false).await
    }

    /// Fetches a document AEMO changes in place at one URL (a workbook
    /// it republishes): downloaded every time, kept as a new version
    /// beside the earlier ones when its bytes are new, else noted as seen
    /// again on the version it is (`cached`). No version is written over.
    pub async fn fetch_document(&self, remote: &RemoteFile) -> Result<LocalFile, FetchError> {
        self.download(remote, true).await
    }

    async fn download(&self, remote: &RemoteFile, again: bool) -> Result<LocalFile, FetchError> {
        let url = remote.url.as_str();
        self.ensure_online(url)?;
        let base = self.archive.path_for(url);
        // Files other URLs keep at the path: both listings of a report
        // folder are one folder; and, fetched again, the URL's own earlier
        // versions. Never written over.
        let others: Vec<Held> = Archive::held_at(&base)?
            .into_iter()
            .filter(|a| again || !a.provenance.names(url))
            .collect();
        make_room(&base)?;
        let part = Archive::part_path(&base);
        let streamed = self
            .requests
            .get_with_retries(url, None, |resp| {
                stream_to_file(url, resp, &part, 0, false, self.events.clone())
            })
            .await?;
        if let Some(same) = others
            .iter()
            .find(|a| a.provenance.content.sha256 == streamed.sha256)
        {
            let _ = std::fs::remove_file(&part);
            let also = Address {
                url: url.to_owned(),
                seen: Stamp::now(),
                gone: None,
                via: Via::Same {
                    listed: Some(listed_of(remote)),
                },
            };
            add_address(&same.path, also, true)?;
            if again {
                return Ok(LocalFile::from_held(remote, same.clone(), true));
            }
            let found = self.archive.lookup(url)?.unwrap_or_else(|| same.clone());
            return Ok(LocalFile::from_held(remote, found, false));
        }
        let path = match others.iter().any(|a| a.path == base) {
            false => base,
            true => {
                let v = Archive::version_path(&base, remote.modified_at.unwrap_or_else(Stamp::now));
                match v.exists() {
                    false => v,
                    true => Archive::version_path(&base, Stamp::now()),
                }
            }
        };
        std::fs::rename(&part, &path).map_err(|source| FetchError::CacheIo {
            path: path.display().to_string(),
            source,
        })?;
        let provenance = Provenance::new(
            Content {
                size: streamed.size,
                sha256: streamed.sha256,
                crc32: Some(streamed.crc32),
            },
            format!("phasor/{}", env!("CARGO_PKG_VERSION")),
            Address {
                url: url.to_owned(),
                seen: Stamp::now(),
                gone: None,
                via: Via::Fetched {
                    listed: Some(listed_of(remote)),
                    served: Some(Served {
                        etag: streamed.etag,
                        modified: streamed
                            .last_modified
                            .as_deref()
                            .and_then(timetext::parse_http_date),
                    }),
                },
            },
        );
        Archive::write(&path, &provenance)?;
        Ok(LocalFile::from_held(
            remote,
            Held { path, provenance },
            false,
        ))
    }
}

/// Creates the folders an archived file goes under.
fn make_room(path: &Path) -> Result<(), FetchError> {
    let parent = path
        .parent()
        .expect("an archived path lies under the archive's root");
    std::fs::create_dir_all(parent).map_err(|source| FetchError::CacheIo {
        path: parent.display().to_string(),
        source,
    })
}

/// The bytes of an entry as the file it is: a stored entry's as they are, a
/// deflated entry's inflated; checked against the directory's size and
/// CRC-32.
fn unpack(e: &Entry, data: &[u8]) -> Result<Vec<u8>, String> {
    let bytes = match e.method {
        0 => data.to_vec(),
        8 => {
            let mut out = Vec::with_capacity(usize::try_from(e.size.min(1 << 20)).unwrap_or(0));
            flate2::read::DeflateDecoder::new(data)
                .take(e.size + 1)
                .read_to_end(&mut out)
                .map_err(|x| format!("it does not inflate: {x}"))?;
            out
        }
        m => {
            return Err(format!(
                "compression method {m} is neither stored nor deflated"
            ));
        }
    };
    if bytes.len() as u64 != e.size {
        return Err(format!(
            "{} bytes where the directory says {}",
            bytes.len(),
            e.size
        ));
    }
    let crc = crc32fast::hash(&bytes);
    if crc != e.crc32 {
        return Err(format!(
            "CRC-32 {crc:08x} where the directory says {:08x}",
            e.crc32
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bytes::Bytes;
    use phasor_core::YearMonth;
    use url::Url;

    use super::*;
    use crate::archive::sha256_hex;
    use crate::transport::{Canned, OfflineTransport, StaticTransport};

    fn remote(url: &str) -> RemoteFile {
        RemoteFile {
            url: Url::parse(url).unwrap(),
            source: "nem-mmsdm",
            table: Some("REGION".into()),
            period: Some(YearMonth::new(2024, 1).unwrap().into()),
            size: None,
            modified: None,
            modified_at: None,
        }
    }

    fn fast() -> FetcherConfig {
        FetcherConfig {
            attempts: 3,
            backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            min_interval: Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn downloads_once_into_the_archive_with_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://h/Data_Archive/DATA/a.zip";
        let t = Arc::new(StaticTransport::default());
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        t.add(url, Canned::ok(&body));
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast()).with_events(tx);
        let local = f.fetch(&remote(url)).await.unwrap();
        assert!(!local.cached);
        assert_eq!(local.size, body.len() as u64);
        assert_eq!(local.sha256, sha256_hex(&body));
        assert_eq!(local.path, dir.path().join("h/Data_Archive/DATA/a.zip"));
        assert_eq!(std::fs::read(&local.path).unwrap(), body);
        assert_eq!(local.etag.as_deref(), Some("\"etag\""));
        assert!(!Archive::part_path(&local.path).exists());
        let sc = Archive::read(&local.path).unwrap().unwrap();
        assert_eq!(sc.url(), url);
        assert_eq!(sc.content.sha256.hex(), local.sha256);
        assert_eq!(sc.tool, format!("phasor/{}", env!("CARGO_PKG_VERSION")));
        assert_eq!(
            sc.modified().map(|s| s.rfc3339()).as_deref(),
            Some("2024-02-13T08:27:00Z"),
            "the server's Last-Modified, as an instant"
        );
        assert_eq!(
            local.last_modified.as_deref(),
            Some("Tue, 13 Feb 2024 08:27:00 GMT"),
            "and as the header was sent"
        );
        assert_eq!(local.last_modified_at, sc.modified());
        // Once archived, it is never asked for again.
        let again = f.fetch(&remote(url)).await.unwrap();
        assert!(again.cached);
        assert_eq!((again.path, again.sha256), (local.path, local.sha256));
        assert_eq!(t.request_count(), 1);
        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the fetcher reports a download start");
        assert!(matches!(got, Some(Event::DownloadStarted { .. })));
        assert_eq!(f.archive().root(), dir.path());
    }

    #[tokio::test]
    async fn another_url_at_the_same_path_is_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let cur = "https://www.nemweb.com.au/Reports/CURRENT/GSH/Benchmark_Price/B.zip";
        let arc = "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/Benchmark_Price/B.zip";
        let t = Arc::new(StaticTransport::default());
        t.add(cur, Canned::ok(b"partial"))
            .add(arc, Canned::ok(b"final"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let first = f.fetch(&remote(cur)).await.unwrap();
        let listed = RemoteFile {
            modified_at: timetext::parse_rfc3339("2026-10-13T11:53:00+10:00"),
            ..remote(arc)
        };
        let second = f.fetch(&listed).await.unwrap();
        assert_ne!(first.path, second.path);
        assert!(
            second
                .path
                .to_string_lossy()
                .ends_with("B.zip@20261013T115300+1000")
        );
        assert_eq!(std::fs::read(&first.path).unwrap(), b"partial");
        assert_eq!(f.archive().lookup(arc).unwrap().unwrap().path, second.path);
        assert_eq!(f.archive().lookup(cur).unwrap().unwrap().path, first.path);
        // Found again with no request.
        assert!(f.fetch(&listed).await.unwrap().cached);
        assert_eq!(t.request_count(), 2);
        // A version under that name already: kept under the download's time.
        let (c, a) = (
            "https://www.nemweb.com.au/Reports/CURRENT/GSH/X/c.csv",
            "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/X/c.csv",
        );
        t.add(c, Canned::ok(b"one")).add(a, Canned::ok(b"two"));
        let one = f.fetch(&remote(c)).await.unwrap();
        let taken = Archive::version_path(
            &one.path,
            timetext::parse_rfc3339("2026-10-13T11:53:00+10:00").unwrap(),
        );
        std::fs::write(&taken, b"x").unwrap();
        let two = f
            .fetch(&RemoteFile {
                modified_at: timetext::parse_rfc3339("2026-10-13T11:53:00+10:00"),
                ..remote(a)
            })
            .await
            .unwrap();
        assert!(two.path != taken && two.path != one.path);
        // The same bytes: one file, both URLs on its sidecar, the other
        // fields as written.
        let (s1, s2) = (
            "https://www.nemweb.com.au/Reports/CURRENT/GSH/X/s.csv",
            "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/X/s.csv",
        );
        t.add(s1, Canned::ok(b"same")).add(s2, Canned::ok(b"same"));
        let k = f.fetch(&remote(s1)).await.unwrap();
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(Archive::record_path(&k.path)).unwrap())
                .unwrap();
        v["crc32"] = serde_json::json!(7);
        std::fs::write(Archive::record_path(&k.path), v.to_string()).unwrap();
        let k2 = f.fetch(&remote(s2)).await.unwrap();
        assert_eq!(k2.path, k.path);
        let sc = Archive::read(&k.path).unwrap().unwrap();
        assert!(sc.names(s2) && sc.names(s1));
        assert!(matches!(sc.addresses[1].via, Via::Same { .. }));
        // What else the record held is kept.
        assert_eq!(sc.content.crc32, Some(7));
        assert!(f.fetch(&remote(s2)).await.unwrap().cached);
    }

    #[tokio::test]
    async fn a_document_changed_in_place_keeps_every_version() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://h/files/List.xlsx";
        let t = Arc::new(StaticTransport::default());
        t.add(url, Canned::ok(b"week one"))
            .add(url, Canned::ok(b"week two"))
            .add(url, Canned::ok(b"week one"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let first = f.fetch_document(&remote(url)).await.unwrap();
        let second = f.fetch_document(&remote(url)).await.unwrap();
        assert!(!first.cached && !second.cached);
        assert_eq!(first.path, dir.path().join("h/files/List.xlsx"));
        assert_ne!(first.path, second.path, "a new version beside the first");
        assert_eq!(std::fs::read(&first.path).unwrap(), b"week one");
        assert_eq!(std::fs::read(&second.path).unwrap(), b"week two");
        // Bytes held already: seen again, nothing written.
        let third = f.fetch_document(&remote(url)).await.unwrap();
        assert!(third.cached);
        assert_eq!(third.path, first.path);
        assert_eq!(Archive::held_at(&first.path).unwrap().len(), 2);
        assert_eq!(t.request_count(), 3);
    }

    #[tokio::test]
    async fn retries_then_gives_up_and_404_is_final() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/flaky.zip", Canned::status(503))
            .add("https://h/flaky.zip", Canned::ok(b"ok"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let local = f.fetch(&remote("https://h/flaky.zip")).await.unwrap();
        assert_eq!(local.size, 2);
        assert_eq!(t.request_count(), 2);
        t.add("https://h/down.zip", Canned::status(503));
        let e = f.fetch(&remote("https://h/down.zip")).await.unwrap_err();
        assert!(
            matches!(e, FetchError::RetriesExhausted { attempts: 3, .. }),
            "{e}"
        );
        assert_eq!(t.request_count(), 5);
        t.add("https://h/missing.zip", Canned::status(404));
        let e = f.fetch(&remote("https://h/missing.zip")).await.unwrap_err();
        assert!(
            matches!(e, FetchError::HttpStatus { status: 404, .. }),
            "{e}"
        );
        assert_eq!(t.request_count(), 6, "no retry on 404");
        let mut c = Canned::ok(b"x");
        c.connect_error = Some("refused".into());
        t.add("https://h/refused.zip", c);
        assert!(matches!(
            f.fetch(&remote("https://h/refused.zip")).await.unwrap_err(),
            FetchError::RetriesExhausted { .. }
        ));
    }

    #[tokio::test]
    async fn a_refusal_stops_every_later_request() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut refusal = Canned::status(403);
        refusal.body = Bytes::from_static(b"Website is unavailable");
        t.add("https://h/a.zip", refusal);
        t.add("https://h/b.zip", Canned::ok(b"fine"));
        t.add("https://h/listing/", Canned::ok(b"<pre></pre>"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        assert!(!f.is_blocked());
        // A 403 is not retried: one request, and the breaker trips.
        let e = f.fetch(&remote("https://h/a.zip")).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::Blocked { status: 403, first_url, .. } if first_url == "https://h/a.zip"),
            "{e}"
        );
        assert_eq!(t.request_count(), 1);
        assert!(f.is_blocked());
        let d = phasor_core::Diagnose::diagnostic(&e);
        assert_eq!(d.code, "net.blocked");
        assert!(d.sample.is_some());
        assert!(e.to_string().contains("request_interval_ms"), "{e}");
        // Everything after it fails at once and sends nothing.
        let e = f.fetch(&remote("https://h/b.zip")).await.unwrap_err();
        assert!(
            matches!(&e, FetchError::Blocked { url, first_url, .. } if url == "https://h/b.zip" && first_url == "https://h/a.zip"),
            "{e}"
        );
        assert!(matches!(
            f.fetch_listing("https://h/listing/").await,
            Err(FetchError::Blocked { .. })
        ));
        assert_eq!(t.request_count(), 1, "no request after the refusal");

        // 429 is retried with backoff; when every attempt is refused the breaker trips too.
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/busy.zip", Canned::status(429));
        t.add("https://h/b.zip", Canned::ok(b"fine"));
        let f = Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(dir.path().join("two")),
            fast(),
        );
        let e = f.fetch(&remote("https://h/busy.zip")).await.unwrap_err();
        assert!(matches!(&e, FetchError::Blocked { status: 429, .. }), "{e}");
        assert_eq!(t.request_count(), 3);
        assert!(f.fetch(&remote("https://h/b.zip")).await.is_err());
        assert_eq!(t.request_count(), 3);
    }

    #[tokio::test]
    async fn truncated_and_broken_bodies_are_retried_and_leave_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut short = Canned::ok(b"abcdef");
        short.claimed_length = Some(10);
        let mut broken = Canned::ok(b"abcdef");
        broken.fail_after = Some(2);
        t.add("https://h/t.zip", short)
            .add("https://h/t.zip", broken)
            .add("https://h/t.zip", Canned::ok(b"abcdef"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let local = f.fetch(&remote("https://h/t.zip")).await.unwrap();
        assert_eq!(local.size, 6);
        assert_eq!(t.request_count(), 3);
        assert!(!Archive::part_path(&local.path).exists());
        // Only truncation, every time: no file, no part, no sidecar.
        let mut short = Canned::ok(b"abc");
        short.claimed_length = Some(10);
        t.add("https://h/short.zip", short);
        let e = f.fetch(&remote("https://h/short.zip")).await.unwrap_err();
        assert!(
            matches!(
                &e,
                FetchError::RetriesExhausted { last, .. } if matches!(
                    **last,
                    FetchError::ContentLength {
                        expected: 10,
                        received: 3,
                        ..
                    }
                )
            ),
            "{e}"
        );
        let path = f.archive().path_for("https://h/short.zip");
        assert!(!path.exists() && !Archive::part_path(&path).exists());
        assert!(f.archive().lookup("https://h/short.zip").unwrap().is_none());
    }

    #[tokio::test]
    async fn listings_offline_and_pacing() {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/dir/", Canned::ok(b"<html>listing</html>"));
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        assert_eq!(
            f.fetch_listing("https://h/dir/").await.unwrap(),
            b"<html>listing</html>"
        );
        let mut short = Canned::ok(b"abc");
        short.claimed_length = Some(10);
        t.add("https://h/bad/", short);
        assert!(matches!(
            f.fetch_listing("https://h/bad/").await.unwrap_err(),
            FetchError::RetriesExhausted { .. }
        ));
        // Offline: an archived file serves, nothing else does.
        t.add("https://h/a.zip", Canned::ok(b"zip"));
        f.fetch(&remote("https://h/a.zip")).await.unwrap();
        let off = Fetcher::new(Arc::new(OfflineTransport), Archive::new(dir.path()), fast());
        assert!(off.fetch(&remote("https://h/a.zip")).await.unwrap().cached);
        assert!(matches!(
            off.fetch_listing("https://h/dir/").await,
            Err(FetchError::Offline { .. })
        ));
        assert!(matches!(
            off.fetch(&remote("https://h/b.zip")).await,
            Err(FetchError::Offline { .. })
        ));
        // Pacing: two requests respect the minimum interval.
        let mut cfg = fast();
        cfg.min_interval = Duration::from_millis(30);
        let paced = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), cfg);
        let start = Instant::now();
        paced.fetch_listing("https://h/dir/").await.unwrap();
        paced.fetch_listing("https://h/dir/").await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(30));
    }
}

#[cfg(test)]
mod picks_tests {
    use std::io::{Cursor, Write as _};
    use std::sync::Arc;

    use phasor_core::YearMonth;
    use url::Url;

    use std::time::Duration;

    use super::*;
    use crate::transport::{Canned, HttpResponse, StaticTransport};
    use phasor_kit::Query;

    const ZIP: &str = "https://h/A/MMSDM_2014_01.zip";

    fn remote(url: &str) -> RemoteFile {
        RemoteFile {
            url: Url::parse(url).unwrap(),
            source: "nem-mmsdm",
            table: None,
            period: Some(YearMonth::new(2014, 1).unwrap().into()),
            size: None,
            modified: Some("1/02/2014 10:00 AM".into()),
            modified_at: None,
        }
    }

    fn fast() -> FetcherConfig {
        FetcherConfig {
            attempts: 2,
            backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            min_interval: Duration::ZERO,
        }
    }

    /// A zip of one stored entry.
    fn inner(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file(name, opts).unwrap();
        w.write_all(bytes).unwrap();
        w.finish().unwrap().into_inner()
    }

    /// A whole-month zip: two small nested zips side by side, a large one,
    /// a deflated text file and a folder.
    fn month_zip() -> (Vec<u8>, Vec<Vec<u8>>, Vec<u8>) {
        let a = inner("A.CSV", b"a,b\n1,2\n");
        let b = inner("B.CSV", b"c\n3\n");
        let big = inner("BIG.CSV", &b"x,y\n".repeat(600));
        let text = b"line,of,text\n".repeat(20);
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in [
            ("PUBLIC_DVD_A_201401010000.zip", &a),
            ("PUBLIC_DVD_B_201401010000.zip", &b),
            ("PUBLIC_DVD_BIG_201401010000.zip", &big),
        ] {
            w.start_file(name, stored).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.add_directory("d/", stored).unwrap();
        w.start_file("d/notes.csv", deflated).unwrap();
        w.write_all(&text).unwrap();
        (w.finish().unwrap().into_inner(), vec![a, b, big], text)
    }

    fn pick_of(indexes: &[usize]) -> impl Fn(&[Entry]) -> Picks + Sync {
        let indexes = indexes.to_vec();
        move |entries: &[Entry]| {
            Picks::Entries(
                indexes
                    .iter()
                    .map(|&i| Pick {
                        index: i,
                        remote: remote(&format!(
                            "{ZIP}!{}",
                            entries.get(i).map_or("none", |e| e.name.as_str())
                        )),
                    })
                    .collect(),
            )
        }
    }

    fn setup(body: Canned) -> (tempfile::TempDir, Arc<StaticTransport>, Fetcher) {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add(ZIP, body);
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        (dir, t, f)
    }

    fn entries(p: Picked) -> Vec<LocalFile> {
        assert!(matches!(&p, Picked::Entries(_)), "{p:?}");
        p.files()
    }

    fn whole(p: Picked) -> LocalFile {
        assert!(matches!(&p, Picked::Whole(_)), "{p:?}");
        p.files().remove(0)
    }

    #[tokio::test]
    async fn picked_entries_come_alone_as_the_files_they_are_and_are_kept() {
        let (zip, inners, text) = month_zip();
        let (_dir, t, f) = setup(Canned::ranged(&zip));
        let r = remote(ZIP);
        // The first nested zip and the deflated text: apart in the zip, so
        // the directory's tail and one span each.
        let got = entries(f.fetch_picks(&r, &pick_of(&[0, 4])).await.unwrap());
        assert_eq!(t.request_count(), 3, "{:?}", t.requests.lock().unwrap());
        assert_eq!(got.len(), 2);
        assert_eq!(std::fs::read(&got[0].path).unwrap(), inners[0]);
        assert!(
            got[0]
                .path
                .ends_with("MMSDM_2014_01.zip!PUBLIC_DVD_A_201401010000.zip")
        );
        assert_eq!(std::fs::read(&got[1].path).unwrap(), text);
        assert!(got[1].path.ends_with("MMSDM_2014_01.zip!d/notes.csv"));
        assert!(!got[0].cached && got[0].etag.as_deref() == Some("\"etag\""));
        let side = Archive::read(&got[1].path).unwrap().unwrap();
        let via = serde_json::to_value(&side.origin().via).unwrap();
        assert_eq!(
            (&via["via"], &via["span"]["zip"], &via["span"]["entry"]),
            (
                &serde_json::json!("range"),
                &serde_json::json!(ZIP),
                &serde_json::json!("d/notes.csv")
            )
        );
        assert_eq!(via["span"]["method"], 8);
        assert_eq!(via["span"]["zip_size"], zip.len());
        assert!(via["span"]["compressed"].as_u64().unwrap() > 0);
        assert!(via["listed"]["at"].is_string(), "the zip's listing time");
        assert_eq!(side.content.size, text.len() as u64);
        assert_eq!(side.content.sha256.hex(), crate::archive::sha256_hex(&text));
        assert!(side.content.crc32.is_some());
        assert!(
            f.archive()
                .lookup(&format!("{ZIP}!d/notes.csv"))
                .unwrap()
                .is_some()
        );
        // Asked again: the directory is read again (the picks are named
        // from it), then the archive answers for every entry.
        let again = entries(f.fetch_picks(&r, &pick_of(&[0, 4])).await.unwrap());
        assert_eq!(t.request_count(), 4);
        assert!(again.iter().all(|l| l.cached));
        // Two entries side by side: the tail and one span.
        let (_dir, t, f) = setup(Canned::ranged(&zip));
        let got = entries(f.fetch_picks(&r, &pick_of(&[1, 0])).await.unwrap());
        assert_eq!(t.request_count(), 2, "{:?}", t.requests.lock().unwrap());
        assert_eq!(
            std::fs::read(&got[0].path).unwrap(),
            inners[1],
            "picked order"
        );
        assert_eq!(std::fs::read(&got[1].path).unwrap(), inners[0]);
        // Nothing picked: nothing fetched beyond the directory.
        assert_eq!(
            entries(f.fetch_picks(&r, &pick_of(&[])).await.unwrap()),
            Vec::<LocalFile>::new()
        );
        assert!(!f.archive().lookup(ZIP).unwrap().is_some());
    }

    #[tokio::test]
    async fn the_zip_comes_whole_when_that_is_cheaper_or_the_only_way() {
        let (zip, _, _) = month_zip();
        // The source wants it all.
        let (_dir, t, f) = setup(Canned::ranged(&zip));
        let r = remote(ZIP);
        let w = whole(f.fetch_picks(&r, &|_| Picks::Whole).await.unwrap());
        assert_eq!((w.cached, w.size), (false, zip.len() as u64));
        assert_eq!(t.request_count(), 2, "the tail, then the file");
        // Archived whole already: no request, whatever the picks.
        let w = whole(f.fetch_picks(&r, &pick_of(&[0])).await.unwrap());
        assert!(w.cached);
        assert_eq!(t.request_count(), 2);
        // The picks are most of the zip.
        let (_dir, t, f) = setup(Canned::ranged(&zip));
        assert!(matches!(
            f.fetch_picks(&r, &pick_of(&[2])).await.unwrap(),
            Picked::Whole(_)
        ));
        assert_eq!(t.request_count(), 2);
        // The server ignores ranges.
        let (_dir, t, f) = setup(Canned::ok(&zip));
        assert!(matches!(
            f.fetch_picks(&r, &pick_of(&[0])).await.unwrap(),
            Picked::Whole(_)
        ));
        assert_eq!(
            t.request_count(),
            2,
            "the range once, not retried, then the file"
        );
        // A ranged read on its own says what the server said.
        let (_dir, _t, f) = setup(Canned::ranged(&zip));
        let read = f
            .fetch_range(ZIP, Span::Bytes { start: 0, end: 4 })
            .await
            .unwrap();
        assert_eq!(read.bytes, b"PK\x03\x04");
        assert_eq!(read.total_size, Some(zip.len() as u64));
        assert_eq!(read.etag.as_deref(), Some("\"etag\""));
        assert!(read.last_modified.is_some());
        assert!(matches!(
            f.fetch_range("https://h/none.zip", Span::Last(1)).await,
            Err(FetchError::HttpStatus { status: 404, .. })
        ));
    }

    /// A server that honours a range but says nothing of the whole file.
    struct NoTotal(Arc<StaticTransport>);

    #[async_trait::async_trait]
    impl Transport for NoTotal {
        async fn get(&self, url: &str) -> Result<HttpResponse, FetchError> {
            self.0.get(url).await
        }
        async fn get_range(&self, url: &str, span: Span) -> Result<HttpResponse, FetchError> {
            let mut r = self.0.get_range(url, span).await?;
            r.total_size = None;
            Ok(r)
        }
    }

    #[tokio::test]
    async fn a_server_that_hides_the_size_gives_the_zip_whole_and_events_say_what_is_fetched() {
        let (zip, _, _) = month_zip();
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        t.add(ZIP, Canned::ranged(&zip));
        let f = Fetcher::new(
            Arc::new(NoTotal(t.clone())),
            Archive::new(dir.path()),
            fast(),
        );
        assert!(matches!(
            f.fetch_picks(&remote(ZIP), &pick_of(&[0])).await.unwrap(),
            Picked::Whole(_)
        ));
        // Events: one start for the picks, sized by their compressed bytes.
        let (_dir, _t, f) = setup(Canned::ranged(&zip));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let f = f.with_events(tx);
        entries(
            f.fetch_picks(&remote(ZIP), &pick_of(&[0, 1]))
                .await
                .unwrap(),
        );
        let started = rx.recv().await;
        assert!(
            matches!(&started, Some(Event::DownloadStarted { .. })),
            "{started:?}"
        );
        if let Some(Event::DownloadStarted { url, size }) = started {
            assert_eq!(url, ZIP);
            assert!(size.is_some_and(|s| s > 0 && s < zip.len() as u64));
        }
    }

    /// The zip with `f` applied to the central directory record of the
    /// entry at `index`.
    fn tampered(zip: &[u8], index: usize, f: impl Fn(&mut [u8])) -> Vec<u8> {
        let mut z = zip.to_vec();
        // The outer directory, through the end record (the nested zips
        // carry directories of their own).
        let eocd = z.len() - 22;
        let mut at = u32::from_le_bytes(z[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        for _ in 0..index {
            let n = u16::from_le_bytes([z[at + 28], z[at + 29]]) as usize;
            let m = u16::from_le_bytes([z[at + 30], z[at + 31]]) as usize;
            let k = u16::from_le_bytes([z[at + 32], z[at + 33]]) as usize;
            at += 46 + n + m + k;
        }
        assert_eq!(&z[at..at + 4], b"PK\x01\x02");
        f(&mut z[at..]);
        z
    }

    #[tokio::test]
    async fn an_entry_that_is_not_what_the_directory_says_is_refused() {
        let (zip, _, _) = month_zip();
        let r = remote(ZIP);
        let err = |p: Picked| -> String { panic!("{p:?}") };
        // Not a zip at all.
        let (_dir, _t, f) = setup(Canned::ranged(b"just text, not a zip"));
        let e = f.fetch_picks(&r, &pick_of(&[0])).await.err().unwrap();
        assert!(
            matches!(&e, FetchError::EntryCorrupt { entry, .. } if entry.is_empty()),
            "{e}"
        );
        assert!(e.to_string().contains("not a zip"));
        // A pick the directory does not have.
        let (_dir, _t, f) = setup(Canned::ranged(&zip));
        let e = f.fetch_picks(&r, &pick_of(&[9])).await.err().unwrap();
        assert!(e.to_string().contains("no entry 9"), "{e}");
        // The directory's CRC-32 disagrees with the bytes.
        let z = tampered(&zip, 0, |rec| rec[16] ^= 0xff);
        let (dir, _t, f) = setup(Canned::ranged(&z));
        let e = f
            .fetch_picks(&r, &pick_of(&[0]))
            .await
            .map(err)
            .unwrap_err();
        assert!(
            matches!(&e, FetchError::EntryCorrupt { entry, .. } if entry == "PUBLIC_DVD_A_201401010000.zip")
        );
        assert!(e.to_string().contains("CRC-32"), "{e}");
        assert!(crate::archive::entries_under(&crate::Archive::new(dir.path()), None).is_empty());
        // A method that is neither stored nor deflated.
        let z = tampered(&zip, 0, |rec| rec[10] = 99);
        let (_dir, _t, f) = setup(Canned::ranged(&z));
        let e = f
            .fetch_picks(&r, &pick_of(&[0]))
            .await
            .map(err)
            .unwrap_err();
        assert!(e.to_string().contains("method 99"), "{e}");
        // A compressed size a little over the bytes: the slack holds them,
        // and the count disagrees.
        let z = tampered(&zip, 0, |rec| {
            let n = u32::from_le_bytes(rec[20..24].try_into().unwrap()) + 10;
            rec[20..24].copy_from_slice(&n.to_le_bytes());
        });
        let (_dir, _t, f) = setup(Canned::ranged(&z));
        let e = f
            .fetch_picks(&r, &pick_of(&[0]))
            .await
            .map(err)
            .unwrap_err();
        assert!(
            e.to_string().contains("bytes where the directory says"),
            "{e}"
        );
        // A compressed size far past the slack on the last entry: the rest
        // is asked for, and the file ends first.
        let z = tampered(&zip, 4, |rec| {
            let n = u32::from_le_bytes(rec[20..24].try_into().unwrap()) + 1000;
            rec[20..24].copy_from_slice(&n.to_le_bytes());
        });
        let (_dir, t, f) = setup(Canned::ranged(&z));
        let e = f
            .fetch_picks(&r, &pick_of(&[4]))
            .await
            .map(err)
            .unwrap_err();
        assert!(
            e.to_string().contains("shorter than the directory says"),
            "{e}"
        );
        assert_eq!(t.request_count(), 3, "the tail, the span, the rest");
        // A local header that is not one.
        let mut z = zip.clone();
        z[0] = b'X';
        let (_dir, _t, f) = setup(Canned::ranged(&z));
        let e = f
            .fetch_picks(&r, &pick_of(&[0]))
            .await
            .map(err)
            .unwrap_err();
        assert!(e.to_string().contains("no local header"), "{e}");
        // A directory that points past the file.
        let z = tampered(&zip, 0, |rec| {
            rec[42..46].copy_from_slice(&u32::MAX.to_le_bytes());
        });
        let (_dir, _t, f) = setup(Canned::ranged(&z));
        let e = f
            .fetch_picks(&r, &pick_of(&[0]))
            .await
            .map(err)
            .unwrap_err();
        assert!(
            e.to_string().contains("shorter than the directory says"),
            "{e}"
        );
        // The archive cannot take the file.
        let (dir, _t, f) = setup(Canned::ranged(&zip));
        std::fs::write(dir.path().join("h"), b"a file where the host folder goes").unwrap();
        let e = f
            .fetch_picks(&r, &pick_of(&[0]))
            .await
            .map(err)
            .unwrap_err();
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
    }

    #[test]
    fn unpacking_checks_the_stream_the_size_and_the_crc() {
        let e = |method: u16, size: u64, crc32: u32| Entry {
            name: "x".into(),
            size,
            compressed: 0,
            crc32,
            method,
            dos_date: 0,
            dos_time: 0,
            local_header_offset: 0,
        };
        assert_eq!(
            unpack(&e(0, 3, crc32fast::hash(b"abc")), b"abc").unwrap(),
            b"abc"
        );
        assert!(
            unpack(&e(8, 3, 0), &[0xff, 0xff, 0xff])
                .unwrap_err()
                .contains("does not inflate")
        );
        assert!(
            unpack(&e(0, 2, 0), b"abc")
                .unwrap_err()
                .contains("3 bytes where the directory says 2")
        );
        assert!(
            unpack(&e(0, u64::MAX, 0), b"abc")
                .unwrap_err()
                .contains("where the directory says")
        );
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(b"hello").unwrap();
        let deflated = enc.finish().unwrap();
        assert_eq!(
            unpack(&e(8, 5, crc32fast::hash(b"hello")), &deflated).unwrap(),
            b"hello"
        );
        // The inflated bytes are cut at one past the declared size, so a
        // lying directory cannot make a bomb of a small entry.
        assert!(
            unpack(&e(8, 2, 0), &deflated)
                .unwrap_err()
                .contains("3 bytes where the directory says 2")
        );
        let _ = Query {
            tables: Vec::new(),
            exclude: Vec::new(),
            range: phasor_core::TimeRange::months(
                YearMonth::new(2024, 1).unwrap(),
                YearMonth::new(2024, 1).unwrap(),
            ),
            instants: None,
        };
    }

    /// The month zip with a comment long enough to push its directory out
    /// of the tail.
    fn commented(zip: &[u8]) -> Vec<u8> {
        let mut z = zip.to_vec();
        let eocd = z.len() - 22;
        z[eocd + 20..eocd + 22].copy_from_slice(&0xFFFFu16.to_le_bytes());
        z.extend(std::iter::repeat_n(b'c', 0xFFFF));
        z
    }

    #[tokio::test]
    async fn a_directory_outside_the_tail_takes_a_second_span_and_its_damage_is_told() {
        let (zip, inners, _) = month_zip();
        let z = commented(&zip);
        let (_dir, t, f) = setup(Canned::ranged(&z));
        let got = entries(f.fetch_picks(&remote(ZIP), &pick_of(&[0])).await.unwrap());
        assert_eq!(std::fs::read(&got[0].path).unwrap(), inners[0]);
        assert_eq!(t.request_count(), 3, "the tail, the directory, the entry");
        // The directory, read apart, is not one.
        let mut broken = commented(&tampered(&zip, 0, |rec| rec[0] = b'X'));
        let _ = &mut broken;
        let (_dir, _t, f) = setup(Canned::ranged(&broken));
        let e = f
            .fetch_picks(&remote(ZIP), &pick_of(&[0]))
            .await
            .expect_err("damaged");
        assert!(
            matches!(&e, FetchError::EntryCorrupt { entry, .. } if entry.is_empty()),
            "{e}"
        );
        // The tail itself cannot be fetched.
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let f = Fetcher::new(Arc::new(t), Archive::new(dir.path()), fast());
        let e = f
            .fetch_picks(&remote(ZIP), &pick_of(&[0]))
            .await
            .expect_err("a 404");
        assert!(
            matches!(e, FetchError::HttpStatus { status: 404, .. }),
            "{e}"
        );
    }

    #[tokio::test]
    async fn the_archive_refusing_a_file_or_an_entry_is_reported() {
        let (zip, _, _) = month_zip();
        let r = remote(ZIP);
        // A directory where the whole zip would go: it cannot be renamed
        // into place.
        let (dir, _t, f) = setup(Canned::ranged(&zip));
        std::fs::create_dir_all(f.archive().path_for(ZIP)).unwrap();
        let e = f.fetch(&r).await.expect_err("cannot rename onto a folder");
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
        drop(dir);
        // A file where the host folder would go: nothing can be created.
        let (dir, _t, f) = setup(Canned::ranged(&zip));
        std::fs::write(dir.path().join("h"), b"a file").unwrap();
        let e = f.fetch(&r).await.expect_err("no folder");
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
        drop(dir);
        // A directory where an entry would go.
        let (_dir, _t, f) = setup(Canned::ranged(&zip));
        let entry_url = format!("{ZIP}!PUBLIC_DVD_A_201401010000.zip");
        std::fs::create_dir_all(f.archive().path_for(&entry_url)).unwrap();
        let e = f
            .fetch_picks(&r, &pick_of(&[0]))
            .await
            .expect_err("cannot rename onto a folder");
        assert!(matches!(e, FetchError::CacheIo { .. }), "{e}");
        // A body that breaks while a span is read is retried, then given up.
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(StaticTransport::default());
        let mut broken = Canned::ranged(&zip);
        broken.fail_after = Some(2);
        t.add(ZIP, broken);
        let f = Fetcher::new(Arc::new(t.clone()), Archive::new(dir.path()), fast());
        let e = f
            .fetch_range(ZIP, Span::Last(10))
            .await
            .expect_err("broken");
        assert!(
            matches!(&e, FetchError::RetriesExhausted { last, .. } if matches!(**last, FetchError::Body { .. })),
            "{e}"
        );
        assert_eq!(t.request_count(), 2);
    }
}
