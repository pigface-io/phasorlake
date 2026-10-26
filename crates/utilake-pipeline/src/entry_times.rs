//! When an entry of a rollup first went public. A daily rollup holds the
//! report files the server published one at a time days before; the
//! mirror keeps each of those it fetched beside the rollup, its record
//! naming the rollup's entry as another address of the same bytes (matched
//! by name, size and CRC-32). That file's `Last-Modified` is when the
//! entry's rows went public: earlier, and exact, where the rollup's own
//! time is only an upper bound.

use std::collections::HashSet;
use std::path::PathBuf;

use utilake_acquire::LocalFile;
use utilake_provenance::archive::Archive;

/// The records beside one source file, read once, for its entries' times.
pub struct EntryTimes {
    dir: Option<PathBuf>,
    zip_url: String,
    /// The folder's file names, read on the first entry asked of: a
    /// record is opened only where one is, as on exFAT a look for an
    /// absent name reads the whole folder.
    names: Option<HashSet<String>>,
}

impl EntryTimes {
    /// The entries of `local`.
    pub fn new(local: &LocalFile) -> Self {
        Self {
            dir: local.path.parent().map(PathBuf::from),
            zip_url: local.remote.url.to_string(),
            names: None,
        }
    }

    /// The server's `Last-Modified` for `entry`'s own bytes (a direct
    /// entry, not one inside a nested zip), where the mirror also holds
    /// them from their own address, in microseconds since 1970 (UTC). None
    /// when they are not held, or the server gave no time.
    pub fn server_last_modified(&mut self, entry: &str) -> Option<i64> {
        if entry.is_empty() || entry.contains(['!', '/', '\\']) {
            return None;
        }
        let dir = self.dir.as_ref()?;
        let names = self.names.get_or_insert_with(|| {
            std::fs::read_dir(dir)
                .map(|rd| {
                    rd.filter_map(Result::ok)
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| n.ends_with(".json"))
                        .collect()
                })
                .unwrap_or_default()
        });
        let record = Archive::record_where(&dir.join(entry), |n| names.contains(n))?;
        let p = Archive::read_record(&record).ok()?;
        // The same bytes: the mirror matched the rollup's entry to them.
        if !p.names(&format!("{}!{entry}", self.zip_url)) {
            return None;
        }
        Some(p.modified()?.unix() * 1_000_000)
    }
}
