//! Downloading every file of the crawl record into the archive.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::PathBuf;

use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::compare::{Pair, Verdict, compare, pairs_in, remove};
use crate::crawl::{Mirror, Remote, RemoteKind};
use crate::diag::{Diagnose as _, Diagnostic};
use crate::error::MirrorError;
use crate::journal::{Change, Event};
use crate::net::{Archive, FetchError, Fetchers, LocalFile, RemoteFile, Via};
use crate::peek::{Peeked, current_in};
use crate::stamp::Stamp;

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

/// Downloads every file of the finished crawl not already archived; a zip
/// beside a folder of the same name comes after its folder.
///
/// A file that fails is reported and the rest carry on, except a refusal by
/// nemweb (`net.blocked`) or a full disk, which stop the pass.
pub async fn download(
    fetcher: &Fetchers,
    mirror: &Mirror,
    options: &DownloadOptions<'_>,
    mut on_file: impl FnMut(&FileDone<'_>),
) -> Result<DownloadSummary, MirrorError> {
    let (downloads, only) = (options.downloads, options.only);
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.done_path().display().to_string(),
        });
    }
    let removed = mirror.removed()?;
    let held = mirror.held()?;
    let held_path = mirror.held_path();
    let cache = mirror.cache_dir();
    std::fs::create_dir_all(&cache).map_err(MirrorError::io(&cache))?;
    let mut note = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&held_path)
        .map_err(MirrorError::io(&held_path))?;
    let record = mirror.record()?;
    let paired: HashMap<String, Pair> = pairs_in(&record, &removed, fetcher.archive())
        .into_iter()
        .map(|p| (p.zip.clone(), p))
        .collect();
    let keep_from = mirror.keep_from();
    // Only the files under the prefixes given, when any are.
    let mut kept: Vec<&Remote> = record
        .iter()
        .filter(|r| crate::keep::keeps(&r.url, keep_from))
        .filter(|r| only.is_empty() || only.iter().any(|p| r.url.starts_with(p.as_str())))
        .collect();
    if let Some(folders) = options.folders {
        let folders: std::collections::HashSet<&str> = folders.iter().map(String::as_str).collect();
        kept.retain(|r| folders.contains(r.listed_on.as_str()));
    }
    let listed = kept.iter().filter(|r| r.kind == RemoteKind::File).count();
    // Each file's folder, whose journal notes what became of it.
    let folders: HashMap<&str, &str> = kept
        .iter()
        .map(|r| (r.url.as_str(), r.listed_on.as_str()))
        .collect();
    // Stable: every other file in URL order, then the paired zips.
    kept.sort_by_key(|r| paired.contains_key(&r.url));
    let files: Vec<RemoteFile> = kept
        .into_iter()
        .enumerate()
        .filter(|(_, r)| r.kind == RemoteKind::File && !removed.contains_key(&r.url))
        .map(|(i, r)| {
            Ok(RemoteFile {
                url: Url::parse(&r.url).map_err(|e| MirrorError::Record {
                    path: mirror.listing_path(&r.listed_on).display().to_string(),
                    line: i as u64 + 1,
                    message: format!("{}: {e}", r.url),
                })?,
                size: r.size,
                modified: Some(r.modified.clone()),
                modified_at: r.modified_at,
            })
        })
        .collect::<Result<_, MirrorError>>()?;
    let mut summary = DownloadSummary {
        files: listed,
        removed: listed - files.len(),
        ..DownloadSummary::default()
    };
    let (paired, held, record) = (&paired, &held, &record);
    // The listings, read once, when the first paired zip comes up: peek
    // reads the paired zips' first, so by then they are there.
    let listings: std::sync::OnceLock<HashMap<String, Peeked>> = std::sync::OnceLock::new();
    let listings = &listings;
    // Paired zips come in a second pass, so a zip is never held against a
    // folder still downloading.
    let (zips, plain): (Vec<&RemoteFile>, Vec<&RemoteFile>) = files
        .iter()
        .partition(|r| paired.contains_key(r.url.as_str()));
    let pass = Pass {
        fetcher,
        mirror,
        paired,
        held,
        record,
        listings,
        delete_duplicates: options.delete_duplicates,
    };
    let task = |remote| pass.one(remote);
    let mut results = futures::stream::iter(plain)
        .map(task)
        .buffer_unordered(downloads.max(1))
        .chain(
            futures::stream::iter(zips)
                .map(task)
                .buffer_unordered(downloads.max(1)),
        );
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
                if let Some(event) = &local.event
                    && let Some(folder) = folders.get(remote.url.as_str())
                {
                    mirror.note(folder, event)?;
                }
                if let Some(held) = &local.note {
                    let line = serde_json::to_string(held).expect("a HeldEntry always serialises");
                    note.write_all(format!("{line}\n").as_bytes())
                        .map_err(MirrorError::io(&held_path))?;
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

/// One file a download pass found in the archive, or put there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldEntry {
    /// The file's URL.
    pub url: String,
    /// Its bytes.
    pub size: u64,
    /// The size nemweb listed it with, when not `size` (a zip packed again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_size: Option<u64>,
    /// The time nemweb listed it with, when the line holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Stamp>,
    /// The name of the version file that holds it, when not the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl HeldEntry {
    /// Where its bytes are.
    pub fn path(&self, archive: &Archive) -> PathBuf {
        let path = archive.path_for(&self.url);
        match &self.file {
            Some(name) => path.with_file_name(name),
            None => path,
        }
    }

    /// Whether it is the version nemweb lists at `size` and `modified_at`.
    /// A line without the listed time matches when its file on disk was
    /// written at or after that time.
    pub fn is_listed_as(
        &self,
        archive: &Archive,
        size: Option<u64>,
        modified_at: Option<Stamp>,
    ) -> bool {
        if size.is_some_and(|s| s != self.listed_size.unwrap_or(self.size)) {
            return false;
        }
        match (self.modified_at, modified_at) {
            (Some(a), Some(b)) => a == b,
            (None, Some(at)) => std::fs::metadata(self.path(archive))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|d| d.as_secs() as i64 >= at.unix()),
            (_, None) => true,
        }
    }
}

impl Mirror {
    /// The files download passes found in the archive or put there, one line
    /// each, so a restart need not read every sidecar.
    pub fn held_path(&self) -> PathBuf {
        self.cache_dir().join("held.ndjson")
    }

    /// Every held file, by URL, from its last line. A line that does not
    /// read is passed over.
    pub fn held(&self) -> Result<HashMap<String, HeldEntry>, MirrorError> {
        let path = self.held_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(MirrorError::io(&path)(e)),
        };
        Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str::<HeldEntry>(l).ok())
            .map(|h| (h.url.clone(), h))
            .collect())
    }
}

impl Mirror {
    /// Writes `held.ndjson` again from the provenance records in `archive`.
    /// Returns how many lines were written.
    pub fn reindex(&self, archive: &Archive) -> Result<usize, MirrorError> {
        let mut lines: BTreeMap<String, HeldEntry> = BTreeMap::new();
        for held in crate::net::archive::entries_under(archive, None) {
            let p = &held.provenance;
            let name = held
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            let listed = p
                .addresses
                .iter()
                .filter(|a| matches!(a.via, Via::Fetched { .. } | Via::Same { .. }));
            for a in listed {
                let l = p.listed(&a.url).unwrap_or_default();
                let line = HeldEntry {
                    url: a.url.clone(),
                    size: p.content.size,
                    listed_size: l.size.filter(|&n| n != p.content.size),
                    modified_at: l.at,
                    file: name
                        .clone()
                        .filter(|_| archive.path_for(&a.url) != held.path),
                };
                match lines.get(&a.url) {
                    Some(kept) if kept.modified_at >= line.modified_at => {}
                    _ => {
                        lines.insert(a.url.clone(), line);
                    }
                }
            }
        }
        let text: String = lines
            .values()
            .map(|h| serde_json::to_string(h).expect("a HeldEntry always serialises") + "\n")
            .collect();
        crate::crawl::write_atomic(&self.held_path(), text.as_bytes())?;
        Ok(lines.len())
    }
}

/// How a file of the pass was dealt with.
struct Got {
    cached: bool,
    size: u64,
    /// The line to add to `held.ndjson`, when it does not say so already.
    note: Option<HeldEntry>,
    /// The line to add to its folder's journal, when it was downloaded.
    event: Option<Event>,
}

/// The journal line of a download whose bytes were not kept; none otherwise.
fn event_of(remote: &RemoteFile, l: &LocalFile, file: Option<String>) -> Option<Event> {
    let (event, sha256) = match &l.same_as {
        Some(s) if s.repacked => (Change::Repacked, Some(s.sha256.clone())),
        Some(_) => (Change::Same, None),
        None => return None,
    };
    Some(Event {
        file,
        sha256,
        ..Event::now(remote.url.as_str(), event)
    })
}

/// How a download pass runs.
#[derive(Debug, Clone, Copy)]
pub struct DownloadOptions<'a> {
    /// Downloads at a time.
    pub downloads: usize,
    /// Only the files whose URL starts with one of these, when any.
    pub only: &'a [String],
    /// Only the files listed on one of these folders, when given.
    pub folders: Option<&'a [String]>,
    /// Skip, and record removed, each paired zip whose listing matches its folder.
    pub delete_duplicates: bool,
}

/// One download pass: what every file's fetch shares.
struct Pass<'a> {
    fetcher: &'a Fetchers,
    mirror: &'a Mirror,
    paired: &'a HashMap<String, Pair>,
    held: &'a HashMap<String, HeldEntry>,
    record: &'a [Remote],
    listings: &'a std::sync::OnceLock<HashMap<String, Peeked>>,
    delete_duplicates: bool,
}

impl<'a> Pass<'a> {
    async fn one(
        &self,
        remote: &'a RemoteFile,
    ) -> Result<(&'a RemoteFile, Option<Result<Got, FetchError>>), MirrorError> {
        if let Some(pair) = self.paired.get(remote.url.as_str()) {
            // Found the same as its folder since the pass began?
            if self.mirror.removed()?.contains_key(&pair.zip) {
                return Ok((remote, None));
            }
            let (f, offset) = (self.fetcher, self.fetcher.offset_of(&pair.zip));
            if self.delete_duplicates
                && the_same(self.mirror, f, pair, self.record, self.listings, offset)?
            {
                return Ok((remote, None));
            }
        }
        let url = remote.url.as_str();
        let archive = self.fetcher.archive();
        if let Some(h) = self.held.get(url)
            && h.is_listed_as(archive, remote.size, remote.modified_at)
            && std::fs::metadata(h.path(archive)).is_ok_and(|m| m.len() == h.size)
        {
            // A line without the listed time is written again with it.
            let note =
                (h.modified_at.is_none() && remote.modified_at.is_some()).then(|| HeldEntry {
                    modified_at: remote.modified_at,
                    ..h.clone()
                });
            let got = Got {
                cached: true,
                size: h.size,
                note,
                event: None,
            };
            return Ok((remote, Some(Ok(got))));
        }
        let got = self.fetcher.fetch(remote).await.map(|l| {
            let base = archive.path_for(url);
            let name = l.path.file_name().map(|n| n.to_string_lossy().into_owned());
            let event = event_of(remote, &l, name.clone());
            Got {
                cached: l.cached,
                size: l.size,
                note: Some(HeldEntry {
                    url: url.to_owned(),
                    size: l.size,
                    listed_size: remote.size.filter(|&s| s != l.size),
                    modified_at: remote.modified_at,
                    file: name.filter(|_| l.path != base),
                }),
                event,
            }
        });
        Ok((remote, Some(got)))
    }
}

/// Whether the zip of `pair` matches its folder from its listing, and so is recorded removed.
fn the_same(
    mirror: &Mirror,
    fetcher: &Fetchers,
    pair: &Pair,
    record: &[Remote],
    listings: &std::sync::OnceLock<HashMap<String, Peeked>>,
    offset: i32,
) -> Result<bool, MirrorError> {
    let archive = fetcher.archive();
    if archive.path_for(&pair.zip).is_file() {
        return Ok(false);
    }
    let listings = match listings.get() {
        Some(l) => l,
        None => {
            let l = current_in(mirror, record)?;
            listings.get_or_init(|| l)
        }
    };
    let Some(entries) = listings.get(&pair.zip).and_then(|p| p.entries.as_deref()) else {
        return Ok(false);
    };
    let cmp = compare(archive, pair, Some(entries), &mirror.tmp_dir())?;
    if cmp.verdict != Verdict::Same {
        return Ok(false);
    }
    remove(mirror, archive, &cmp, offset, None)?;
    Ok(true)
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
    #[test]
    fn reindex_writes_a_line_for_every_listed_address() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("state"));
        let archive = Archive::new(home.path().join("archive"));
        let url = "https://h/A/a.zip";
        let path = archive.path_for(url);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let at = |t: &str| Stamp::parse(t).unwrap();
        let record = |bytes: &[u8], listed: Option<Stamp>| {
            let mut r = crate::net::archive::test_record(url, bytes);
            r.addresses[0].via = Via::Fetched {
                listed: Some(phasor_provenance::Listed {
                    size: Some(9),
                    at: listed,
                }),
                served: None,
            };
            r
        };
        std::fs::write(&path, b"one").unwrap();
        let mut first = record(b"one", Some(at("2026-10-12T10:45:00+10:00")));
        first.add(crate::net::Address {
            url: "https://h/B/a.zip".into(),
            seen: Stamp::now(),
            gone: None,
            via: Via::Entry,
        });
        first.add(crate::net::Address {
            url: "https://h/C/a.zip".into(),
            seen: Stamp::now(),
            gone: None,
            via: Via::Same { listed: None },
        });
        Archive::write(&path, &first).unwrap();
        let later = Archive::version_path(&path, at("2026-10-13T10:45:00+10:00"));
        std::fs::write(&later, b"two").unwrap();
        Archive::write(
            &later,
            &record(b"two", Some(at("2026-10-13T10:45:00+10:00"))),
        )
        .unwrap();
        // An older version listed with no time loses to the one listed.
        let older = Archive::version_path(&path, at("2026-10-11T10:45:00+10:00"));
        std::fs::write(&older, b"old").unwrap();
        Archive::write(&older, &record(b"old", None)).unwrap();
        assert_eq!(mirror.reindex(&archive).unwrap(), 2, "a.zip and C's");
        let held = mirror.held().unwrap();
        let a = &held[url];
        assert_eq!(a.file.as_deref(), Some("a.zip@20261013T104500+1000"));
        assert_eq!((a.size, a.listed_size), (3, Some(9)));
        assert_eq!(held["https://h/C/a.zip"].file.as_deref(), Some("a.zip"));
        assert!(
            !held.contains_key("https://h/B/a.zip"),
            "an entry is no listing"
        );
    }

    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::net::transport::{Canned, StaticTransport};
    use crate::net::{Archive, FetcherConfig};

    use super::*;
    use crate::crawl::{crawl, root};

    const PAGE: &str = "<pre> Monday, October 12, 2026 10:45 AM 3 <A HREF=\"/A/x.zip\">x</A><br> Monday, October 12, 2026 10:45 AM 4 <A HREF=\"/A/y.zip\">y</A><br> Monday, October 12, 2026 10:45 AM 5 <A HREF=\"/A/gone.zip\">g</A><br></pre>";

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
    async fn downloads_every_crawled_file_once() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/A/", Canned::ok(PAGE.as_bytes()));
        t.add("https://h/A/x.zip", Canned::ok(b"xxx"));
        t.add("https://h/A/y.zip", Canned::ok(b"yyyy"));
        let f = fetcher(&t, &archive);

        let err = download(
            &f,
            &mirror,
            &DownloadOptions {
                downloads: 2,
                only: &[],
                folders: None,
                delete_duplicates: false,
            },
            ignore,
        )
        .await
        .unwrap_err();
        assert_eq!(err.diagnostic().code, "mirror.crawl_incomplete");

        crawl(&f, &mirror, &[root("https://h/A/").unwrap()], false, |_| {})
            .await
            .unwrap();
        let mut seen = Vec::new();
        let s = download(
            &f,
            &mirror,
            &DownloadOptions {
                downloads: 2,
                only: &[],
                folders: None,
                delete_duplicates: false,
            },
            |d| {
                seen.push((d.url.to_owned(), d.cached, d.total));
            },
        )
        .await
        .unwrap();
        assert_eq!((s.files, s.cached, s.downloaded, s.bytes), (3, 0, 2, 7));
        assert_eq!(s.failed.len(), 1);
        assert_eq!(s.failed[0].code, "net.http_status");
        seen.sort();
        assert_eq!(seen[0], ("https://h/A/x.zip".to_owned(), false, 3));
        assert_eq!(std::fs::read(archive.join("h/A/y.zip")).unwrap(), b"yyyy");
        // The sidecar says what the listing said of it too.
        let sc = Archive::read(&archive.join("h/A/y.zip")).unwrap().unwrap();
        assert_eq!(sc.listed(sc.url()).and_then(|l| l.size), Some(4));
        assert_eq!(
            sc.listed(sc.url()).and_then(|l| l.at).unwrap().to_string(),
            "2026-10-12T10:45:00+10:00"
        );

        let s = download(
            &f,
            &mirror,
            &DownloadOptions {
                downloads: 2,
                only: &[],
                folders: None,
                delete_duplicates: false,
            },
            ignore,
        )
        .await
        .unwrap();
        assert_eq!((s.cached, s.downloaded), (2, 0));
        // Only a part of the tree.
        let only = ["https://h/A/x".to_owned()];
        let s = download(
            &f,
            &mirror,
            &DownloadOptions {
                downloads: 2,
                only: &only,
                folders: None,
                delete_duplicates: false,
            },
            ignore,
        )
        .await
        .unwrap();
        assert_eq!((s.files, s.cached), (1, 1));
        assert_eq!(t.request_count(), 1 + 3 + 1);
        // Only the files a folder lists.
        for (folder, files) in [("https://h/A/", 3), ("https://h/B/", 0)] {
            let folders = [folder.to_owned()];
            let s = download(
                &f,
                &mirror,
                &DownloadOptions {
                    downloads: 2,
                    only: &[],
                    folders: Some(&folders),
                    delete_duplicates: false,
                },
                ignore,
            )
            .await
            .unwrap();
            assert_eq!(s.files, files);
        }
    }

    #[tokio::test]
    async fn a_restart_takes_held_files_without_reading_their_sidecars() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/A/", Canned::ok(PAGE.as_bytes()));
        t.add("https://h/A/x.zip", Canned::ok(b"xxx"));
        t.add("https://h/A/y.zip", Canned::ok(b"yyyy"));
        let f = fetcher(&t, &archive);
        crawl(&f, &mirror, &[root("https://h/A/").unwrap()], false, |_| {})
            .await
            .unwrap();
        let options = DownloadOptions {
            downloads: 1,
            only: &[],
            folders: None,
            delete_duplicates: false,
        };
        download(&f, &mirror, &options, ignore).await.unwrap();
        let held = mirror.held().unwrap();
        let size = |u: &str| held.get(u).map(|h| h.size);
        assert_eq!(
            (size("https://h/A/x.zip"), size("https://h/A/y.zip")),
            (Some(3), Some(4))
        );

        // A sidecar that does not read would stop a pass that read it.
        std::fs::write(archive.join("h/A/x.zip.provenance.json"), "{").unwrap();
        // A file whose size no longer matches is fetched again.
        std::fs::write(archive.join("h/A/y.zip"), b"y").unwrap();
        std::fs::remove_file(archive.join("h/A/y.zip.provenance.json")).unwrap();
        // A line cut short by a kill is passed over.
        let mut text = std::fs::read_to_string(mirror.held_path()).unwrap();
        text.push_str("{\"url\":\"https://h/A/");
        std::fs::write(mirror.held_path(), text).unwrap();
        let s = download(&f, &mirror, &options, ignore).await.unwrap();
        assert_eq!((s.cached, s.downloaded), (1, 1));
        assert_eq!(std::fs::read(archive.join("h/A/y.zip")).unwrap(), b"yyyy");
        // Only the fetched file gains a line.
        let lines = std::fs::read_to_string(mirror.held_path()).unwrap();
        assert_eq!(lines.matches("y.zip").count(), 2);
        assert_eq!(lines.matches("x.zip").count(), 1);

        // A ledger that cannot be read or written stops the pass.
        std::fs::remove_file(mirror.held_path()).unwrap();
        std::fs::create_dir(mirror.held_path()).unwrap();
        let e = download(&f, &mirror, &options, ignore).await.unwrap_err();
        assert!(e.to_string().contains("held.ndjson"), "{e}");
    }

    #[tokio::test]
    async fn a_file_nemweb_rewrites_in_place_is_kept_in_every_version() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let t = Arc::new(StaticTransport::default());
        // Listed again a day later: x with the same bytes, y rewritten.
        let later = "<pre> Tuesday, October 13, 2026 11:53 AM 3 <A HREF=\"/A/x.zip\">x</A><br> Tuesday, October 13, 2026 11:53 AM 5 <A HREF=\"/A/y.zip\">y</A><br></pre>";
        t.add("https://h/A/", Canned::ok(PAGE.as_bytes()))
            .add("https://h/A/", Canned::ok(later.as_bytes()));
        t.add("https://h/A/x.zip", Canned::ok(b"xxx"));
        t.add("https://h/A/y.zip", Canned::ok(b"yyyy"))
            .add("https://h/A/y.zip", Canned::ok(b"yyyyy"));
        let f = fetcher(&t, &archive);
        let roots = [root("https://h/A/").unwrap()];
        crawl(&f, &mirror, &roots, false, |_| {}).await.unwrap();
        let options = DownloadOptions {
            downloads: 1,
            only: &[],
            folders: None,
            delete_duplicates: false,
        };
        download(&f, &mirror, &options, ignore).await.unwrap();

        // Lines without the listed time hold a file written since that
        // time, and are written again with it.
        let old = "{\"url\":\"https://h/A/x.zip\",\"size\":3}\n{\"url\":\"https://h/A/y.zip\",\"size\":4}\n";
        std::fs::write(mirror.held_path(), old).unwrap();
        let s = download(&f, &mirror, &options, ignore).await.unwrap();
        assert_eq!((s.cached, s.downloaded), (2, 0));
        let held = mirror.held().unwrap();
        assert_eq!(
            held["https://h/A/y.zip"].modified_at.unwrap().to_string(),
            "2026-10-12T10:45:00+10:00"
        );
        let requests = t.request_count();

        crawl(&f, &mirror, &roots, true, |_| {}).await.unwrap();
        let s = download(&f, &mirror, &options, ignore).await.unwrap();
        // Both asked for again; x's bytes are the same, so x is not kept twice.
        assert_eq!((s.cached, s.downloaded), (1, 1));
        assert_eq!(t.request_count(), requests + 1 + 2);
        let a = Archive::new(&archive);
        let y = archive.join("h/A/y.zip@20261013T115300+1000");
        assert_eq!(std::fs::read(archive.join("h/A/y.zip")).unwrap(), b"yyyy");
        assert_eq!(std::fs::read(&y).unwrap(), b"yyyyy");
        assert!(!archive.join("h/A/x.zip@20261013T115300+1000").exists());
        let held = mirror.held().unwrap();
        assert_eq!(held["https://h/A/y.zip"].path(&a), y);
        assert_eq!(
            held["https://h/A/x.zip"].path(&a),
            archive.join("h/A/x.zip")
        );
        // The folder's journal: only x, the same bytes again; the downloads
        // are in their records.
        let journal = mirror.journal("https://h/A/").unwrap();
        let events: Vec<(&str, Change)> = journal
            .iter()
            .map(|e| (e.url.rsplit('/').next().unwrap(), e.event))
            .collect();
        assert_eq!(events, [("x.zip", Change::Same)]);
        assert_eq!(journal[0].file.as_deref(), Some("x.zip"));
        // From then on, each is the version listed, with no request.
        let s = download(&f, &mirror, &options, ignore).await.unwrap();
        assert_eq!((s.cached, s.downloaded), (2, 0));
        assert_eq!(t.request_count(), requests + 3);
        // A line kept before the file was rewritten is not the version listed.
        let h = HeldEntry {
            modified_at: None,
            ..held["https://h/A/x.zip"].clone()
        };
        let ahead = crate::stamp::Stamp::from_unix(crate::stamp::Stamp::now().unix() + 3600);
        assert!(!h.is_listed_as(&a, Some(3), Some(ahead)));
        assert!(h.is_listed_as(&a, None, None));
        assert!(!h.is_listed_as(&a, Some(4), None));
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
        let err = download(
            &f,
            &mirror,
            &DownloadOptions {
                downloads: 1,
                only: &[],
                folders: None,
                delete_duplicates: false,
            },
            ignore,
        )
        .await
        .unwrap_err();
        assert_eq!(err.diagnostic().code, "net.blocked");
    }

    #[test]
    fn a_bad_url_in_the_record_names_its_line() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path());
        std::fs::create_dir_all(mirror.dir()).unwrap();
        std::fs::write(mirror.done_path(), "").unwrap();
        mirror.set_record_text(
            "{\"url\":\"no url\",\"kind\":\"file\",\"modified\":\"m\",\"listed_on\":\"https://h/A/\"}\n",
        );
        let t = Arc::new(StaticTransport::default());
        let f = fetcher(&t, home.path());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let d = rt
            .block_on(download(
                &f,
                &mirror,
                &DownloadOptions {
                    downloads: 1,
                    only: &[],
                    folders: None,
                    delete_duplicates: false,
                },
                ignore,
            ))
            .unwrap_err()
            .diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.line),
            ("mirror.record_corrupt", Some(1))
        );
    }

    /// A file done, not looked at.
    fn ignore(_: &FileDone<'_>) {}

    fn root_url() -> url::Url {
        crate::crawl::root("https://h/A/").unwrap()
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
        let s = download(&f, &mirror, &DownloadOptions { downloads: 1, only: &[], folders: None, delete_duplicates: false }, |d| {
            if d.url == "https://h/A/N/a.zip" {
                std::fs::write(
                    &removed,
                    "{\"url\":\"https://h/A/N.zip\",\"size\":1,\"sha256\":\"x\",\"same_as\":\"https://h/A/N/\",\"removed_at\":\"2026-10-15T00:59:33Z\"}\n",
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
        let e = download(
            &f,
            &mirror,
            &DownloadOptions {
                downloads: 1,
                only: &[],
                folders: None,
                delete_duplicates: false,
            },
            ignore,
        )
        .await
        .unwrap_err();
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

    #[tokio::test]
    async fn a_duplicate_found_from_its_listing_is_never_fetched() {
        let line = |href: &str, size: Option<usize>| {
            let what = size.map_or("&lt;dir&gt;".to_owned(), |s| s.to_string());
            format!(" Monday, October 12, 2026 10:45 AM {what} <A HREF=\"{href}\">x</A><br>")
        };
        let a = crate::testzip::stored("A.CSV", b"a");
        let t = Arc::new(StaticTransport::default());
        let mut root = String::from("<pre>");
        for name in ["M", "N", "O", "P"] {
            root += &line(&format!("/A/{name}/"), None);
            root += &line(&format!("/A/{name}.zip"), Some(9));
            let page = format!(
                "<pre>{}</pre>",
                line(&format!("/A/{name}/a.zip"), Some(a.len()))
            );
            t.add(&format!("https://h/A/{name}/"), Canned::ok(page.as_bytes()));
            t.add(&format!("https://h/A/{name}/a.zip"), Canned::ok(&a));
            t.add(&format!("https://h/A/{name}.zip"), Canned::ok(b"wholezip!"));
        }
        root += "</pre>";
        t.add("https://h/A/", Canned::ok(root.as_bytes()));
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = home.path().join("archive");
        let f = fetcher(&t, &archive);
        crawl(&f, &mirror, &[root_url()], false, |_| {})
            .await
            .unwrap();
        // M's listing matches its folder; N's does not; O has none; P is
        // downloaded already.
        let entry = |crc: u32| crate::zipdir::Entry {
            name: "a.zip".into(),
            size: a.len() as u64,
            compressed: a.len() as u64,
            crc32: crc,
            method: 0,
            modified: "m".into(),
            modified_at: None,
        };
        let listing = |zip: &str, crc: u32| crate::peek::Peeked {
            url: format!("https://h/A/{zip}"),
            listed_size: Some(9),
            listed_modified: Some("Monday, October 12, 2026 10:45 AM".into()),
            entries: Some(vec![entry(crc)]),
            ..Default::default()
        };
        let peeks = [
            listing("M.zip", crc32fast::hash(&a)),
            listing("N.zip", 1),
            listing("P.zip", crc32fast::hash(&a)),
        ];
        let lines: String = peeks
            .iter()
            .map(|p| serde_json::to_string(&p.clone().derived()).unwrap() + "\n")
            .collect();
        std::fs::write(mirror.peek_path(), lines).unwrap();
        let p = Archive::new(&archive).path_for("https://h/A/P.zip");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"wholezip!").unwrap();
        Archive::write(
            &p,
            &crate::net::archive::test_record("https://h/A/P.zip", b"wholezip!"),
        )
        .unwrap();

        let options = DownloadOptions {
            downloads: 1,
            only: &[],
            folders: None,
            delete_duplicates: true,
        };
        let s = download(&f, &mirror, &options, |_| {}).await.unwrap();
        assert_eq!((s.files, s.removed, s.cached, s.downloaded), (8, 1, 1, 6));
        let asked = t.requests.lock().unwrap().clone();
        assert!(!asked.iter().any(|u| u.ends_with("/M.zip")), "{asked:?}");
        assert!(asked.iter().any(|u| u.ends_with("/N.zip")));
        assert!(asked.iter().any(|u| u.ends_with("/O.zip")));
        let removed = mirror.removed().unwrap();
        assert!(!removed["https://h/A/M.zip"].downloaded);
        assert_eq!(removed.len(), 1);
    }
}
