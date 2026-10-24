//! The versions of each file nemweb rewrote in place, each against the
//! ones before it: new data, the same bytes, or a zip packed again.
//!
//! A download pass keeps a version only when its bytes, or for a zip what
//! it holds, are new (`Fetcher::fetch`). Versions kept before it did are
//! found here, and `prune` takes out each that is the same as one before
//! it: noted in its folder's journal first, then its file and sidecar
//! removed, and `held.ndjson` pointed at the version it is the same as.

use std::collections::BTreeSet;
use std::io::Write as _;

use serde::Serialize;

use crate::crawl::Mirror;
use crate::download::Held;
use crate::error::MirrorError;
use crate::journal::{Change, Event};
use crate::net::{Archive, Archived};
use crate::stamp::Stamp;

/// What a look at every version found, or a prune did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VersionsSummary {
    /// Files with more than one version.
    pub files: usize,
    /// Their versions, the first included.
    pub versions: usize,
    /// Versions with the bytes of one before them.
    pub same: usize,
    /// Zips that hold the files of one before them, packed again.
    pub repacked: usize,
    /// Of `same` and `repacked`, those removed.
    pub removed: usize,
    /// Their bytes.
    pub bytes: u64,
}

/// What a zip holds: each file's name, CRC-32 and size.
type Holds = Vec<(String, u32, u64)>;

/// The folder a file URL is in, as its listing's URL.
fn folder_of(url: &str) -> &str {
    url.rfind('/').map_or(url, |i| &url[..=i])
}

/// The name of an archived file.
fn name_of(a: &Archived) -> Option<String> {
    a.path.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Every file `held.ndjson` names a later version of, each version against
/// the ones before it; with `prune`, each that is the same as one before
/// it removed. `on` sees the journal line of each found the same.
pub async fn versions(
    mirror: &Mirror,
    archive: &Archive,
    prune: bool,
    mut on: impl FnMut(&Event),
) -> Result<VersionsSummary, MirrorError> {
    let path = mirror.held_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(MirrorError::io(&path)(e)),
    };
    let urls: BTreeSet<String> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Held>(l).ok())
        .filter(|h| h.file.is_some())
        .map(|h| h.url)
        .collect();
    let held = mirror.held()?;
    let mut summary = VersionsSummary::default();
    let mut lines = Vec::new();
    for url in &urls {
        let all = Archive::versions(&archive.path_for(url), url)?;
        if all.len() < 2 {
            continue;
        }
        summary.files += 1;
        summary.versions += all.len();
        let mut kept: Vec<(Archived, Option<Holds>)> = Vec::new();
        for v in all {
            let holds = crate::zipdir::contents(&v.path).await;
            let same = kept.iter().find_map(|(k, k_holds)| {
                if k.sidecar.sha256 == v.sidecar.sha256 {
                    Some((k, Change::Same))
                } else if holds.is_some() && *k_holds == holds {
                    Some((k, Change::Repacked))
                } else {
                    None
                }
            });
            let Some((k, change)) = same else {
                kept.push((v, holds));
                continue;
            };
            if change == Change::Same {
                summary.same += 1;
            } else {
                summary.repacked += 1;
            }
            let s = &v.sidecar;
            let event = Event {
                seen_at: Stamp::parse(&s.fetched_at).unwrap_or_else(|_| Stamp::now()),
                file: name_of(k),
                sha256: Some(s.sha256.clone()),
                size: Some(s.size),
                listed_modified: s.listed_modified.clone(),
                listed_modified_at: s.listed_modified_at,
                last_modified: s.last_modified.clone(),
                last_modified_at: s.last_modified_at,
                ..Event::now(url, change)
            };
            on(&event);
            if !prune {
                continue;
            }
            mirror.note(folder_of(url), &event)?;
            for p in [Archive::sidecar_path(&v.path), v.path.clone()] {
                std::fs::remove_file(&p).map_err(MirrorError::io(&p))?;
            }
            summary.removed += 1;
            summary.bytes += s.size;
            // The ledger names the version it is the same as instead.
            if held.get(url).is_some_and(|h| h.file == name_of(&v)) {
                let first = k.path == archive.path_for(url);
                let line = Held {
                    url: url.clone(),
                    size: k.sidecar.size,
                    listed_size: Some(s.size).filter(|&n| n != k.sidecar.size),
                    modified_at: held[url].modified_at,
                    file: name_of(k).filter(|_| !first),
                };
                lines.push(serde_json::to_string(&line).expect("a Held always serialises"));
            }
        }
    }
    if !lines.is_empty() {
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .map_err(MirrorError::io(&path))?;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Sidecar;
    use crate::net::archive::sha256_hex;

    fn put(archive: &Archive, url: &str, at: Option<&str>, bytes: &[u8]) -> String {
        let base = archive.path_for(url);
        let path = match at {
            Some(at) => {
                Archive::version_path(&base, Stamp::from_listing(at, crate::stamp::AEST).unwrap())
            }
            None => base,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let sidecar = Sidecar {
            url: url.to_owned(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            crc32: None,
            fetched_at: "2026-10-21T10:00:00Z".to_owned(),
            etag: None,
            last_modified: Some("Wed, 21 Oct 2026 10:00:00 GMT".to_owned()),
            last_modified_at: None,
            listed_size: Some(bytes.len() as u64),
            listed_modified: at.map(str::to_owned),
            listed_modified_at: None,
            tool: "utilake-mirror".to_owned(),
            also: Vec::new(),
            gone_at: None,
        };
        Archive::write_sidecar(&path, &sidecar).unwrap();
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    const TUE: &str = "Tuesday, October 13, 2026 11:53 AM";
    const WED: &str = "Wednesday, October 14, 2026 09:00 AM";
    const THU: &str = "Thursday, October 15, 2026 09:00 AM";

    #[tokio::test]
    async fn versions_the_same_as_one_before_are_found_and_pruned() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        // No ledger: nothing to look at.
        let s = versions(&mirror, &archive, false, |_| {}).await.unwrap();
        assert_eq!(s, VersionsSummary::default());

        let csv = "https://h/R/C/G/a.csv";
        put(&archive, csv, None, b"one");
        let two = put(&archive, csv, Some(TUE), b"two");
        let again = put(&archive, csv, Some(WED), b"two");
        let back = put(&archive, csv, Some(THU), b"one");
        let zip = "https://h/R/C/G/z.zip";
        put(&archive, zip, None, &crate::testzip::stored("a.csv", b"x"));
        let packed = put(
            &archive,
            zip,
            Some(TUE),
            &crate::testzip::deflated("a.csv", b"x"),
        );
        let lone = "https://h/R/C/G/lone.csv";
        put(&archive, lone, None, b"l");
        let held = [
            format!(r#"{{"url":"{csv}","size":3,"file":"{two}"}}"#),
            format!(r#"{{"url":"{csv}","size":3,"file":"{back}"}}"#),
            format!(r#"{{"url":"{zip}","size":1,"file":"{packed}"}}"#),
            format!(r#"{{"url":"{lone}","size":1,"file":"lone.csv@x"}}"#),
        ];
        std::fs::create_dir_all(mirror.dir()).unwrap();
        std::fs::write(mirror.held_path(), held.join("\n") + "\n").unwrap();

        let mut seen = Vec::new();
        let s = versions(&mirror, &archive, false, |e| {
            seen.push((e.event, e.file.clone()));
        })
        .await
        .unwrap();
        assert_eq!(
            (s.files, s.versions, s.same, s.repacked, s.removed),
            (2, 6, 2, 1, 0)
        );
        assert_eq!(
            seen,
            [
                (Change::Same, Some(two.clone())),
                (Change::Same, Some("a.csv".to_owned())),
                (Change::Repacked, Some("z.zip".to_owned()))
            ]
        );
        assert!(mirror.journal("https://h/R/C/G/").unwrap().is_empty());

        let s = versions(&mirror, &archive, true, |_| {}).await.unwrap();
        assert_eq!(
            (s.removed, s.bytes),
            (
                3,
                3 + 3 + crate::testzip::deflated("a.csv", b"x").len() as u64
            )
        );
        let dir = archive.path_for(csv).parent().unwrap().to_path_buf();
        for name in [&again, &back, &packed] {
            assert!(!dir.join(name).exists(), "{name}");
            assert!(!Archive::sidecar_path(&dir.join(name)).exists(), "{name}");
        }
        assert!(dir.join(&two).exists());
        let journal = mirror.journal("https://h/R/C/G/").unwrap();
        assert_eq!(journal.len(), 3);
        assert_eq!(
            journal[0].listed_modified_at.unwrap().to_string(),
            "2026-10-14T09:00:00+10:00"
        );
        // The ledger names the version each is the same as.
        let h = mirror.held().unwrap();
        assert_eq!((h[csv].file.as_deref(), h[csv].size), (None, 3));
        assert_eq!(h[zip].file, None);
        assert!(h[zip].listed_size.is_some());
        // Nothing left to prune.
        let s = versions(&mirror, &archive, true, |_| {}).await.unwrap();
        assert_eq!((s.same, s.repacked, s.removed), (0, 0, 0));

        // A ledger that is not a file cannot be read.
        std::fs::remove_file(mirror.held_path()).unwrap();
        std::fs::create_dir(mirror.held_path()).unwrap();
        assert!(versions(&mirror, &archive, false, |_| {}).await.is_err());
    }
}
