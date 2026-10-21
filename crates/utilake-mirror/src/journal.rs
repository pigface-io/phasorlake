//! Each folder's journal: what became of its files, one line per event,
//! appended and never written over (`docs/archive.md`, Journal).
//!
//! A download pass notes each file it downloads: a file new to the
//! archive, a later version with new data, or bytes the same as a version
//! archived already (listed again with nothing changed, or a zip packed
//! again). A watch notes each file nemweb stops listing, and each folder
//! it stops serving.

use std::io::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::crawl::Mirror;
use crate::error::MirrorError;
use crate::net::Archive;
use crate::stamp::Stamp;

/// What became of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// Downloaded for the first time.
    New,
    /// Listed again with other bytes: a later version, kept as `file`.
    Version,
    /// Listed again with the bytes of `file`, a version archived already.
    Same,
    /// A zip listed again, packed again: other bytes (`sha256`), the same
    /// files as `file` (names, CRC-32 and sizes).
    Repacked,
    /// No longer listed.
    Dropped,
    /// Its folder no longer served (HTTP 404).
    FolderGone,
}

/// One line of a folder's journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// When it was seen, UTC.
    pub seen_at: Stamp,
    /// The file, or the folder when it is gone.
    pub url: String,
    /// What became of it.
    pub event: Change,
    /// The archived file that holds its bytes or its files: its name,
    /// `{name}` or `{name}@{at}` (`docs/archive.md`, Versions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// sha256 hex of the bytes nemweb served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Their size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The time nemweb listed it with, as listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified: Option<String>,
    /// The same time as an instant (AEST).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_modified_at: Option<Stamp>,
    /// The server's `Last-Modified`, as sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// The same time as an instant (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_at: Option<Stamp>,
}

impl Event {
    /// An event of `url` now, nothing else known.
    pub fn now(url: &str, event: Change) -> Self {
        Self {
            seen_at: Stamp::now(),
            url: url.to_owned(),
            event,
            file: None,
            sha256: None,
            size: None,
            listed_modified: None,
            listed_modified_at: None,
            last_modified: None,
            last_modified_at: None,
        }
    }
}

impl Mirror {
    /// The journal of the folder `folder` (a URL ending in `/`), at its
    /// URL's path under `{home}/mirror/journal/`.
    pub fn journal_path(&self, folder: &str) -> PathBuf {
        Archive::new(self.dir().join("journal"))
            .path_for(folder)
            .join("changes.ndjson")
    }

    /// Appends `event` to the journal of `folder`.
    pub fn note(&self, folder: &str, event: &Event) -> Result<(), MirrorError> {
        let path = self.journal_path(folder);
        let dir = path.parent().expect("a journal is in a folder");
        std::fs::create_dir_all(dir).map_err(MirrorError::io(dir))?;
        let mut line = serde_json::to_vec(event).expect("an Event always serialises");
        line.push(b'\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(&line))
            .map_err(MirrorError::io(&path))
    }

    /// The journal of `folder`, oldest first; empty when it has none. A
    /// line that does not read (the last, cut short) is passed over.
    pub fn journal(&self, folder: &str) -> Result<Vec<Event>, MirrorError> {
        let path = self.journal_path(folder);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(MirrorError::io(&path)(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_journal_is_appended_and_read_back() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let folder = "https://h/R/C/G/";
        assert!(mirror.journal(folder).unwrap().is_empty());
        let mut e = Event::now("https://h/R/C/G/a.csv", Change::New);
        e.size = Some(3);
        mirror.note(folder, &e).unwrap();
        mirror
            .note(
                folder,
                &Event::now("https://h/R/C/G/a.csv", Change::Dropped),
            )
            .unwrap();
        let path = mirror.journal_path(folder);
        assert_eq!(
            path,
            home.path().join("mirror/journal/h/R/C/G/changes.ndjson")
        );
        let mut text = std::fs::read_to_string(&path).unwrap();
        crate::stamp::check::assert_aware_lines(&text);
        text.push_str("{\"cut");
        std::fs::write(&path, text).unwrap();
        let read = mirror.journal(folder).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(
            (read[0].event, read[1].event),
            (Change::New, Change::Dropped)
        );
        assert_eq!(read[0], e);
        // A journal that is not a file cannot be read or written.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(mirror.journal(folder).is_err());
        assert!(mirror.note(folder, &e).is_err());
    }
}
