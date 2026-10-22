//! One mirror run at a time, through a lock the system drops with the
//! process, so a run that was killed never leaves a stale lock behind.

use std::fs::{File, OpenOptions, TryLockError};

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
        // Held by another run, or any other reason it cannot be taken: the
        // cause says which.
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

    fn lock_path(&self) -> std::path::PathBuf {
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
