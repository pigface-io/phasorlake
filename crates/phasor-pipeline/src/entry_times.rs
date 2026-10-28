//! When an entry of a rollup first went public: the `Last-Modified` of the
//! same bytes the mirror fetched alone, whose record names the rollup's entry.

use std::collections::HashSet;
use std::path::PathBuf;

use phasor_kit::LocalFile;
use phasor_provenance::archive::Archive;

/// The records beside one source file, read once, for its entries' times.
pub struct EntryTimes {
    dir: Option<PathBuf>,
    zip_url: String,
    /// The folder's file names, read once, so a record is opened only where one exists.
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

    /// The server's `Last-Modified` for a direct `entry`'s own bytes, microseconds since 1970 UTC.
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
