//! How far the mirror has got, from its own files and the archive alone.

use std::path::Path;

use crate::net::{Archive, Listed};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::crawl::{Mirror, RemoteKind, read_record};
use crate::error::MirrorError;
use crate::stamp::Stamp;

/// Files listed and files archived under one root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootStatus {
    /// The root.
    pub root: String,
    /// Files the crawl listed.
    pub files: usize,
    /// Their listed bytes.
    pub bytes: u64,
    /// Files in the archive at their listed size.
    pub archived_files: usize,
    /// Their bytes.
    pub archived_bytes: u64,
    /// Files removed as the same as their folders.
    pub removed_files: usize,
    /// Their bytes.
    pub removed_bytes: u64,
}

/// Zips whose listing is read, out of the zips to read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeekCount {
    /// Zips peeked, in the record.
    pub done: usize,
    /// Zips peeked, and zips still to peek.
    pub total: usize,
}

/// The state of the mirror.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// When a run wrote these counts, kept up to date as it goes; none
    /// when they were counted just now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<Stamp>,
    /// Zips peeked, when a run counts them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peek: Option<PeekCount>,
    /// Bytes of the files being downloaded now, when a run counts them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloading: Option<u64>,
    /// Whether a mirror run holds the lock now.
    pub running: bool,
    /// Listing pages kept by the current crawl.
    pub pages: usize,
    /// Whether the current crawl has read every page.
    pub crawl_done: bool,
    /// Per root, once the crawl is done.
    pub roots: Vec<RootStatus>,
    /// Links out of the roots.
    pub external: usize,
}

/// Counts the kept pages and, once the crawl is done, the files listed
/// against the files in the archive at their listed size: a file
/// `held.ndjson` lists at that size without a look at the disk, any other
/// only when its directory holds its name.
pub fn status(mirror: &Mirror, archive: &Archive, roots: &[Url]) -> Result<Status, MirrorError> {
    let mut status = Status {
        as_of: None,
        peek: None,
        downloading: None,
        running: mirror.running(),
        pages: count_pages(&mirror.crawl_dir().join("listings")),
        crawl_done: mirror.done_path().exists(),
        roots: roots
            .iter()
            .map(|r| RootStatus {
                root: r.to_string(),
                ..RootStatus::default()
            })
            .collect(),
        external: 0,
    };
    if !status.crawl_done {
        return Ok(status);
    }
    let removed = mirror.removed()?;
    let held = mirror.held()?;
    let mut listed = Listed::default();
    for remote in read_record(&mirror.record_path())? {
        match remote.kind {
            RemoteKind::External => status.external += 1,
            RemoteKind::Dir => {}
            RemoteKind::File => {
                let Some(root) = status
                    .roots
                    .iter_mut()
                    .find(|r| remote.url.starts_with(&r.root))
                else {
                    continue;
                };
                let size = remote.size.unwrap_or(0);
                root.files += 1;
                root.bytes += size;
                if removed.contains_key(&remote.url) {
                    root.removed_files += 1;
                    root.removed_bytes += size;
                    continue;
                }
                let path = archive.path_for(&remote.url);
                let archived = held.get(&remote.url) == Some(&size)
                    || (listed.holds(&path)
                        && std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == size));
                if archived {
                    root.archived_files += 1;
                    root.archived_bytes += size;
                }
            }
        }
    }
    Ok(status)
}

impl Status {
    /// Counts a file downloaded, under its root.
    pub fn add_download(&mut self, url: &str, bytes: u64) {
        if let Some(root) = self.roots.iter_mut().find(|r| url.starts_with(&r.root)) {
            root.archived_files += 1;
            root.archived_bytes += bytes;
        }
    }
}

impl Mirror {
    /// Where a run keeps its counts.
    pub fn progress_path(&self) -> std::path::PathBuf {
        self.dir().join("progress.json")
    }

    /// Writes a run's counts, stamped now, beside their final name and
    /// renamed into place.
    pub fn write_progress(&self, status: &Status) -> Result<(), MirrorError> {
        let mut status = status.clone();
        status.as_of = Some(Stamp::now());
        let path = self.progress_path();
        let part = self.dir().join("progress.json.part");
        let text = serde_json::to_vec(&status).expect("a Status always serialises");
        std::fs::write(&part, text).map_err(MirrorError::io(&part))?;
        std::fs::rename(&part, &path).map_err(MirrorError::io(&path))
    }

    /// A run's latest counts, when there are any.
    pub fn read_progress(&self) -> Result<Option<Status>, MirrorError> {
        let path = self.progress_path();
        match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| MirrorError::Record {
                    path: path.display().to_string(),
                    line: 1,
                    message: e.to_string(),
                }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(MirrorError::io(&path)(e)),
        }
    }
}

fn count_pages(dir: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "index.html") {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crawl::{Remote, root};

    fn remote(url: &str, kind: RemoteKind, size: Option<u64>) -> Remote {
        Remote {
            url: url.to_owned(),
            kind,
            size,
            modified: "m".to_owned(),
            modified_at: None,
            listed_on: "https://h/A/".to_owned(),
            crawled_at: None,
        }
    }

    #[test]
    fn counts_pages_then_files_against_the_archive() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        let roots = [root("https://h/A/").unwrap(), root("https://h/B/").unwrap()];
        let a = root("https://h/A/").unwrap();
        let page = mirror.page_path(&a);
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "").unwrap();

        let s = status(&mirror, &archive, &roots).unwrap();
        assert_eq!((s.pages, s.crawl_done), (1, false));
        assert_eq!(s.roots[0].files, 0);

        let record = [
            remote("https://h/A/", RemoteKind::Dir, None),
            remote("https://h/A/x.zip", RemoteKind::File, Some(3)),
            remote("https://h/A/y.zip", RemoteKind::File, Some(4)),
            remote("https://h/B/z.zip", RemoteKind::File, Some(5)),
            remote("https://h/C/w.zip", RemoteKind::File, Some(6)),
            remote("https://o/", RemoteKind::External, None),
        ];
        let lines: String = record
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        std::fs::write(mirror.record_path(), lines).unwrap();
        std::fs::write(mirror.done_path(), "").unwrap();
        for (url, len) in [("https://h/A/x.zip", 3), ("https://h/A/y.zip", 1)] {
            let p = archive.path_for(url);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![0u8; len]).unwrap();
        }
        let s = status(&mirror, &archive, &roots).unwrap();
        assert!(s.crawl_done);
        assert_eq!(s.external, 1);
        assert_eq!(
            s.roots[0],
            RootStatus {
                root: "https://h/A/".into(),
                files: 2,
                bytes: 7,
                archived_files: 1,
                archived_bytes: 3,
                removed_files: 0,
                removed_bytes: 0,
            }
        );
        assert_eq!((s.roots[1].files, s.roots[1].archived_files), (1, 0));
    }

    #[test]
    fn a_runs_counts_kept_and_read_back() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path());
        assert_eq!(mirror.read_progress().unwrap(), None);
        let mut s = Status {
            roots: vec![RootStatus {
                root: "https://h/A/".into(),
                files: 2,
                bytes: 7,
                ..RootStatus::default()
            }],
            peek: Some(PeekCount { done: 1, total: 3 }),
            ..Status::default()
        };
        s.add_download("https://h/A/x.zip", 3);
        s.add_download("https://o/elsewhere.zip", 9);
        mirror.write_progress(&s).unwrap();
        let back = mirror.read_progress().unwrap().unwrap();
        assert!(back.as_of.is_some());
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(mirror.progress_path()).unwrap()).unwrap();
        crate::stamp::check::assert_aware(&written);
        assert_eq!(
            (back.roots[0].archived_files, back.roots[0].archived_bytes),
            (1, 3)
        );
        assert_eq!(back.peek, Some(PeekCount { done: 1, total: 3 }));
        std::fs::write(mirror.progress_path(), "{").unwrap();
        assert_eq!(
            crate::Diagnose::diagnostic(&mirror.read_progress().unwrap_err()).code,
            "mirror.record_corrupt"
        );
        std::fs::remove_file(mirror.progress_path()).unwrap();
        std::fs::create_dir(mirror.progress_path()).unwrap();
        assert!(mirror.read_progress().is_err());
        assert!(mirror.write_progress(&s).is_err());
        let gone = Mirror::new(home.path().join("none"));
        assert!(gone.write_progress(&s).is_err());
    }
}
