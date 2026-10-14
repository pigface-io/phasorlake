//! How far the mirror has got, from its own files and the archive alone.

use std::path::Path;

use nemlake_acquire::Archive;
use serde::Serialize;
use url::Url;

use crate::crawl::{Mirror, RemoteKind, read_record};
use crate::error::MirrorError;

/// Files listed and files archived under one root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
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
}

/// The state of the mirror.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
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
/// against the files in the archive at their listed size.
pub fn status(mirror: &Mirror, archive: &Archive, roots: &[Url]) -> Result<Status, MirrorError> {
    let mut status = Status {
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
                let archived = std::fs::metadata(archive.path_for(&remote.url))
                    .is_ok_and(|m| m.is_file() && m.len() == size);
                if archived {
                    root.archived_files += 1;
                    root.archived_bytes += size;
                }
            }
        }
    }
    Ok(status)
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
            listed_on: "https://h/A/".to_owned(),
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
            }
        );
        assert_eq!((s.roots[1].files, s.roots[1].archived_files), (1, 0));
    }
}
