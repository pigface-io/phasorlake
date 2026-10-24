//! Watching nemweb for what changes: each folder's listing read again on a
//! schedule of its own, and what is new or rewritten put in the crawl
//! record for a download pass.
//!
//! nemweb adds files to `Reports/CURRENT/` every few minutes, drops them
//! after days or weeks, and rewrites some in place several times a day
//! (most gas reports). A full crawl reads some 2,900 pages; a watch reads a
//! folder again when its turn comes: the folders that rewrite files often
//! every few minutes, the rest of `Reports/CURRENT/` hourly, and every
//! other folder about daily. A folder found changed is read again sooner,
//! down to the least interval; one found the same goes back towards its
//! own.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use serde::Serialize;
use url::Url;

use crate::crawl::{Mirror, Remote, RemoteKind, read_page, root, write_atomic};
use crate::error::MirrorError;
use crate::journal::{Change, Event};
use crate::net::{Archive, FetchError, Fetchers};
use crate::stamp::Stamp;

/// How often a watch reads each folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cadence {
    /// The folders read most often, by URL prefix.
    pub fast: Vec<String>,
    /// How often they are read.
    pub fast_every: Duration,
    /// The folders read next most often, by URL prefix, apart from `fast`.
    pub current: Vec<String>,
    /// How often they are read.
    pub current_every: Duration,
    /// How often every other folder is read.
    pub every: Duration,
    /// The shortest interval a folder found changed comes down to.
    pub least: Duration,
}

impl Cadence {
    /// A folder's own interval, and whether it is one of the others, whose
    /// first reads are spread over that interval.
    fn base(&self, folder: &str) -> (Duration, bool) {
        let under = |prefixes: &[String]| prefixes.iter().any(|p| folder.starts_with(p.as_str()));
        if under(&self.fast) {
            (self.fast_every, false)
        } else if under(&self.current) {
            (self.current_every, false)
        } else {
            (self.every, true)
        }
    }
}

/// When a folder is read next.
#[derive(Debug, Clone, Copy)]
struct Slot {
    /// Its own interval.
    base: Duration,
    /// Its interval now.
    every: Duration,
    /// When it is read next.
    due: Instant,
}

/// When each folder is read next.
#[derive(Debug, Clone)]
pub struct Schedule {
    cadence: Cadence,
    slots: BTreeMap<String, Slot>,
}

impl Schedule {
    /// Every folder given, at its cadence. The folders read most often and
    /// next most often are due now; the others are spread over their
    /// interval, so a watch does not begin with a full crawl.
    pub fn new(cadence: Cadence, folders: impl IntoIterator<Item = String>, now: Instant) -> Self {
        let mut s = Self {
            cadence,
            slots: BTreeMap::new(),
        };
        for f in folders {
            s.add(f, now);
        }
        let others: Vec<String> = s
            .slots
            .keys()
            .filter(|f| s.cadence.base(f).1)
            .cloned()
            .collect();
        let n = u32::try_from(others.len()).unwrap_or(u32::MAX);
        for (i, f) in (0u32..).zip(&others) {
            let slot = s.slots.get_mut(f).expect("a folder of the schedule");
            slot.due = now + slot.base * i / n;
        }
        s
    }

    /// A folder new to the schedule, due now.
    pub fn add(&mut self, folder: String, now: Instant) {
        let (base, _) = self.cadence.base(&folder);
        self.slots.entry(folder).or_insert(Slot {
            base,
            every: base,
            due: now,
        });
    }

    /// A folder gone from nemweb, and every folder under it.
    pub fn remove(&mut self, folder: &str) {
        self.slots.retain(|f, _| !f.starts_with(folder));
    }

    /// The folders due by `now`, the longest due first.
    pub fn due(&self, now: Instant) -> Vec<String> {
        let mut due: Vec<(&Instant, &String)> = self
            .slots
            .iter()
            .filter(|(_, s)| s.due <= now)
            .map(|(f, s)| (&s.due, f))
            .collect();
        due.sort();
        due.into_iter().map(|(_, f)| f.clone()).collect()
    }

    /// A folder read at `now`: found changed, it is read again in half its
    /// interval, down to the least; found the same, in twice it, up to its
    /// own.
    pub fn checked(&mut self, folder: &str, changed: bool, now: Instant) {
        let least = self.cadence.least;
        if let Some(slot) = self.slots.get_mut(folder) {
            slot.every = if changed {
                (slot.every / 2).max(least).min(slot.base)
            } else {
                (slot.every * 2).min(slot.base)
            };
            slot.due = now + slot.every;
        }
    }

    /// When the next folder is due.
    pub fn next(&self) -> Option<Instant> {
        self.slots.values().map(|s| s.due).min()
    }

    /// Folders in the schedule.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the schedule has no folder.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// What a refresh found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Refreshed {
    /// Folders read, each with whether its listing changed.
    pub folders: Vec<(String, bool)>,
    /// Folders new to the record, read in the same refresh.
    pub added: Vec<String>,
    /// Folders gone from nemweb, out of the record with all they held.
    pub removed: Vec<String>,
    /// Files new to the record, or listed with another size or time.
    pub files: Vec<String>,
}

/// What a listing says of one link, to tell a change.
type Listed = (RemoteKind, Option<u64>, Option<Stamp>);

fn listed(r: &Remote) -> Listed {
    (r.kind, r.size, r.modified_at)
}

/// Reads the listing pages of `folders` again and puts what each lists in
/// place of what it listed in the crawl record, reading every folder new
/// to the record too. A folder nemweb answers with 404 is gone: it and all
/// under it leave the record. Any other failure stops the refresh, and the
/// record keeps what was read before it.
pub async fn refresh(
    fetcher: &Fetchers,
    mirror: &Mirror,
    roots: &[Url],
    folders: &[String],
) -> Result<Refreshed, MirrorError> {
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.crawl_dir().display().to_string(),
        });
    }
    let mut record: BTreeMap<String, Remote> = mirror
        .record()?
        .into_iter()
        .map(|r| (r.url.clone(), r))
        .collect();
    let mut out = Refreshed::default();
    let mut queue: VecDeque<String> = folders.iter().cloned().collect();
    let mut failed = None;
    while let Some(folder) = queue.pop_front() {
        let url = root(&folder)?;
        let html = match fetcher.fetch_listing(&folder).await {
            Ok(html) => html,
            Err(FetchError::HttpStatus { status: 404, .. }) => {
                record.retain(|u, _| u == &folder || !u.starts_with(&folder));
                mirror.folder_gone(&folder)?;
                fetcher.archive().folder_gone(&folder, Stamp::now())?;
                mirror.note(&folder, &Event::now(&folder, Change::FolderGone))?;
                out.removed.push(folder);
                continue;
            }
            Err(e) => {
                failed = Some(e.into());
                break;
            }
        };
        write_atomic(&mirror.page_path(&url), &html)?;
        let offset = fetcher.offset_of(&folder);
        let now = read_page(&url, &html, roots, Some(Stamp::now()), offset)?;
        let before: BTreeMap<String, Remote> = record
            .values()
            .filter(|r| r.listed_on == folder)
            .map(|r| (r.url.clone(), r.clone()))
            .collect();
        let listed_before: BTreeMap<&String, Listed> =
            before.iter().map(|(u, r)| (u, listed(r))).collect();
        let listed_now: BTreeMap<&String, Listed> =
            now.iter().map(|r| (&r.url, listed(r))).collect();
        let changed = listed_before != listed_now;
        if changed {
            for (u, r) in &before {
                if listed_now.contains_key(u) {
                    continue;
                }
                if r.kind == RemoteKind::Dir {
                    record.retain(|v, _| !v.starts_with(u.as_str()));
                    mirror.folder_gone(u)?;
                    fetcher.archive().folder_gone(u, Stamp::now())?;
                    out.removed.push(u.clone());
                } else {
                    // Its held file keeps the bytes; its sidecar notes the
                    // address gone.
                    let held = fetcher.archive().lookup(u)?;
                    held.map(|a| Archive::mark_gone(&a.path, [u.as_str()], Stamp::now()))
                        .transpose()?;
                    // A file, or a link out of the roots, no longer listed.
                    let event = Event {
                        size: r.size,
                        listed_modified: Some(r.modified.clone()),
                        listed_modified_at: r.modified_at,
                        ..Event::now(u, Change::Dropped)
                    };
                    mirror.note(&folder, &event)?;
                }
                record.remove(u);
            }
            for r in &now {
                let was = listed_before.get(&r.url);
                match r.kind {
                    RemoteKind::File if was != Some(&listed(r)) => out.files.push(r.url.clone()),
                    RemoteKind::Dir if was.is_none() => {
                        queue.push_back(r.url.clone());
                        out.added.push(r.url.clone());
                    }
                    _ => {}
                }
            }
            mirror.put_folder(&folder, now.clone())?;
            record.extend(now.into_iter().map(|r| (r.url.clone(), r)));
        }
        out.folders.push((folder, changed));
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::sync::Arc;

    use super::*;
    use crate::crawl::crawl;
    use crate::net::transport::{Canned, StaticTransport};
    use crate::net::{Archive, FetcherConfig};

    fn cadence() -> Cadence {
        Cadence {
            fast: vec!["https://h/R/C/G/".to_owned()],
            fast_every: Duration::from_secs(900),
            current: vec!["https://h/R/C/".to_owned()],
            current_every: Duration::from_secs(3600),
            every: Duration::from_secs(86_400),
            least: Duration::from_secs(300),
        }
    }

    #[test]
    fn each_folder_comes_round_at_its_own_cadence() {
        let now = Instant::now();
        let folders = [
            "https://h/D/",
            "https://h/D/x/",
            "https://h/R/C/",
            "https://h/R/C/G/",
        ];
        let mut s = Schedule::new(cadence(), folders.map(String::from), now);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());
        // The fast and hourly folders are due now; the others spread over a day.
        assert_eq!(
            s.due(now),
            ["https://h/D/", "https://h/R/C/", "https://h/R/C/G/"]
        );
        let half = now + Duration::from_secs(43_200);
        assert_eq!(s.due(half).len(), 4);
        assert_eq!(s.next(), Some(now));

        // Changed: half the interval, not under the least; the same: twice
        // it, not over its own.
        let g = "https://h/R/C/G/";
        let at = |s: &Schedule| s.slots[g].every;
        s.checked(g, true, now);
        assert_eq!(at(&s), Duration::from_secs(450));
        s.checked(g, true, now);
        s.checked(g, true, now);
        assert_eq!(at(&s), Duration::from_secs(300));
        s.checked(g, false, now);
        s.checked(g, false, now);
        s.checked(g, false, now);
        assert_eq!(at(&s), Duration::from_secs(900));
        assert_eq!(s.slots[g].due, now + Duration::from_secs(900));
        s.checked("https://h/none/", true, now);

        s.add("https://h/R/C/new/".to_owned(), half);
        assert_eq!(
            s.slots["https://h/R/C/new/"].every,
            Duration::from_secs(3600)
        );
        s.remove("https://h/R/C/");
        assert_eq!(s.len(), 2);
        s.remove("https://h/D/");
        assert!(s.is_empty());
        assert_eq!(s.next(), None);
    }

    fn page(lines: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut body = String::from("<pre>");
        for (when, size, href) in lines {
            let _ = write!(body, " {when} {size} <A HREF=\"{href}\">x</A><br>");
        }
        body.push_str("</pre>");
        body.into_bytes()
    }

    const MON: &str = "Monday, October 12, 2026 10:45 AM";
    const TUE: &str = "Tuesday, October 13, 2026 11:53 AM";
    const DIR: &str = "&lt;dir&gt;";

    #[tokio::test]
    async fn a_refresh_puts_each_listing_in_place_of_the_last() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let t = Arc::new(StaticTransport::default());
        let f = Fetchers::one(crate::net::Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(home.path().join("archive")),
            FetcherConfig {
                attempts: 1,
                backoff: Duration::ZERO,
                max_backoff: Duration::ZERO,
                min_interval: Duration::ZERO,
            },
        ));
        let roots = [root("https://h/A/").unwrap()];
        let folders = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let e = refresh(&f, &mirror, &roots, &folders(&["https://h/A/"]))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("crawl"), "{e}");

        let first = page(&[
            (MON, DIR, "/A/b/"),
            (MON, DIR, "/A/old/"),
            (MON, "3", "/A/x.csv"),
            (MON, "4", "/A/y.csv"),
            (MON, "6", "/A/dropped.csv"),
        ]);
        // x rewritten, y the same, a new file and folder, old/ gone.
        let second = page(&[
            (MON, DIR, "/A/b/"),
            (MON, DIR, "/A/new/"),
            (TUE, "3", "/A/x.csv"),
            (MON, "4", "/A/y.csv"),
            (TUE, "5", "/A/w.csv"),
        ]);
        // Each URL's answers in turn, the last repeating.
        let z = page(&[(MON, "1", "/A/b/z.csv")]);
        t.add("https://h/A/", Canned::ok(&first))
            .add("https://h/A/", Canned::ok(&second));
        t.add("https://h/A/b/", Canned::ok(&z))
            .add("https://h/A/b/", Canned::ok(&z))
            .add("https://h/A/b/", Canned::status(404));
        t.add(
            "https://h/A/old/",
            Canned::ok(&page(&[(MON, "1", "/A/old/o.csv")])),
        );
        t.add(
            "https://h/A/new/",
            Canned::ok(&page(&[(TUE, "2", "/A/new/n.csv")])),
        )
        .add(
            "https://h/A/new/",
            Canned::ok(&page(&[(TUE, "9", "/A/new/n.csv")])),
        );
        t.add("https://h/A/gone/", Canned::status(403));
        crawl(&f, &mirror, &roots, false, |_| {}).await.unwrap();
        // Held files of the addresses about to go.
        let archive = Archive::new(home.path().join("archive"));
        let hold = |url: &str, also: &[&str]| {
            let p = archive.path_for(url);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"1").unwrap();
            let mut sc: crate::net::Sidecar = serde_json::from_value(serde_json::json!({
                "url": url, "size": 1, "sha256": "s", "fetched_at": "2026-10-05T00:00:00Z",
            }))
            .unwrap();
            for a in also {
                sc.add_also(a, crate::net::How::Same, Stamp::now());
            }
            Archive::write_sidecar(&p, &sc).unwrap();
            p
        };
        let dropped = hold("https://h/A/dropped.csv", &[]);
        let z = hold(
            "https://h/A/b/z.csv",
            &["https://h/A/b/z2.csv", "https://h/C/z.csv"],
        );
        let o = hold("https://h/A/old/o.csv", &[]);

        // The same listing again: nothing changed, nothing written.
        let record = std::fs::read(mirror.record_path()).unwrap();
        let r = refresh(&f, &mirror, &roots, &folders(&["https://h/A/b/"]))
            .await
            .unwrap();
        assert_eq!(r.folders, [("https://h/A/b/".to_owned(), false)]);
        assert_eq!(std::fs::read(mirror.record_path()).unwrap(), record);

        let r = refresh(
            &f,
            &mirror,
            &roots,
            &folders(&["https://h/A/", "https://h/A/b/"]),
        )
        .await
        .unwrap();
        assert_eq!(
            r.folders,
            [
                ("https://h/A/".to_owned(), true),
                ("https://h/A/new/".to_owned(), true)
            ]
        );
        assert_eq!(r.added, ["https://h/A/new/"]);
        assert_eq!(r.removed, ["https://h/A/old/", "https://h/A/b/"]);
        assert_eq!(
            r.files,
            [
                "https://h/A/x.csv",
                "https://h/A/w.csv",
                "https://h/A/new/n.csv"
            ]
        );
        let urls: Vec<String> = mirror
            .record()
            .unwrap()
            .into_iter()
            .map(|r| r.url)
            .collect();
        assert_eq!(
            urls,
            [
                "https://h/A/b/",
                "https://h/A/new/",
                "https://h/A/new/n.csv",
                "https://h/A/w.csv",
                "https://h/A/x.csv",
                "https://h/A/y.csv"
            ]
        );
        let journal = mirror.journal("https://h/A/").unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(
            (journal[0].url.as_str(), journal[0].event, journal[0].size),
            ("https://h/A/dropped.csv", Change::Dropped, Some(6))
        );
        let gone = mirror.journal("https://h/A/b/").unwrap();
        assert_eq!(gone[0].event, Change::FolderGone);
        // The held files stay; their sidecars note the addresses gone.
        let sc = |p: &std::path::Path| Archive::read_sidecar(p).unwrap().unwrap();
        assert!(sc(&dropped).gone_at.is_some());
        assert!(sc(&o).gone_at.is_some());
        let zs = sc(&z);
        assert!(zs.gone_at.is_some() && zs.also[0].gone_at.is_some());
        assert!(zs.also[1].gone_at.is_none(), "not under the folder");
        assert!(z.exists() && dropped.exists());
        // Noted once: a second note changes nothing.
        let mut again = zs.clone();
        assert!(!again.mark_gone("https://h/A/b/z.csv", Stamp::now()));
        assert!(!again.mark_gone("https://h/A/b/z2.csv", Stamp::now()));
        assert!(!Archive::mark_gone(&z, ["https://h/A/b/z.csv"], Stamp::now()).unwrap());
        assert!(!Archive::mark_gone(&home.path().join("none"), ["x"], Stamp::now()).unwrap());
        let kept = std::fs::read(mirror.page_path(&roots[0])).unwrap();
        assert_eq!(kept, second);

        // A refusal stops the refresh; what was read before it is kept.
        let e = refresh(
            &f,
            &mirror,
            &roots,
            &folders(&["https://h/A/new/", "https://h/A/gone/", "https://h/A/"]),
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("403"), "{e}");
        let n = mirror
            .record()
            .unwrap()
            .into_iter()
            .find(|r| r.url == "https://h/A/new/n.csv")
            .unwrap();
        assert_eq!(n.size, Some(9));
    }
}
