//! One mirror run at a time, through a lock the system drops with the
//! process, so a run that was killed never leaves a stale lock behind: one
//! per home ([`RunLock`]), and one per host on this machine ([`HostLocks`]),
//! so two homes never read one site at once and draw its refusal together.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use crate::crawl::Mirror;
use crate::error::MirrorError;

/// Held while a run works; released when dropped or when the process ends.
#[derive(Debug)]
pub struct RunLock {
    _file: File,
}

impl Mirror {
    /// Takes the run lock, or says that another run holds it.
    pub fn lock(&self) -> Result<RunLock, MirrorError> {
        let file = self.lock_file()?;
        file.try_lock()
            .map(|()| RunLock { _file: file })
            .map_err(|e| MirrorError::Locked {
                path: self.lock_path().display().to_string(),
                cause: e.to_string(),
            })
    }

    /// Whether a run holds the lock now.
    pub fn running(&self) -> bool {
        self.lock_file()
            .is_ok_and(|f| matches!(f.try_lock(), Err(TryLockError::WouldBlock)))
    }

    fn lock_path(&self) -> PathBuf {
        self.dir().join("lock")
    }

    fn lock_file(&self) -> Result<File, MirrorError> {
        std::fs::create_dir_all(self.dir()).map_err(MirrorError::io(self.dir()))?;
        let path = self.lock_path();
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(MirrorError::io(&path))
    }
}

/// The hosts a network command reads, held against every other process on
/// this machine for as long as the command runs.
#[derive(Debug)]
pub struct HostLocks {
    _files: Vec<File>,
}

impl HostLocks {
    /// Locks each of `bases` (a host's base URL) under `dir`, or says which
    /// host another process is reading. A base's lock is its host and port.
    pub fn take<'a>(
        dir: &Path,
        bases: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, MirrorError> {
        std::fs::create_dir_all(dir).map_err(MirrorError::io(dir))?;
        let mut files = Vec::new();
        for base in bases {
            let key = Self::key(base);
            let path = dir.join(&key);
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
                .map_err(MirrorError::io(&path))?;
            file.try_lock().map_err(|e| MirrorError::HostBusy {
                host: key.clone(),
                path: path.display().to_string(),
                cause: e.to_string(),
            })?;
            files.push(file);
        }
        Ok(Self { _files: files })
    }

    /// A base URL's lock name: its host and port, every other byte `_`.
    fn key(base: &str) -> String {
        let rest = base.split_once("://").map_or(base, |(_, r)| r);
        let authority = rest.split('/').next().unwrap_or(rest);
        let key: String = authority
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if key.is_empty() {
            "host".to_owned()
        } else {
            key
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_read_by_one_process_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let held = HostLocks::take(dir.path(), ["https://h:8080", "https://w"]).unwrap();
        assert!(dir.path().join("h_8080").exists());
        assert!(dir.path().join("w").exists());
        // The same host again, from another home: refused, naming the host.
        let again = HostLocks::take(dir.path(), ["https://other", "https://h:8080/"]);
        assert!(matches!(
            again.unwrap_err(),
            MirrorError::HostBusy { host, .. } if host == "h_8080"
        ));
        // A host nobody holds is free.
        assert!(HostLocks::take(dir.path(), ["https://free"]).is_ok());
        drop(held);
        assert!(HostLocks::take(dir.path(), ["https://h:8080"]).is_ok());
        assert_eq!(HostLocks::key("https://a.b-c.d:1/x/y"), "a.b-c.d_1");
        assert_eq!(HostLocks::key(""), "host");
    }
}
