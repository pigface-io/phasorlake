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
        match file.try_lock() {
            Ok(()) => Ok(RunLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(MirrorError::Locked {
                path: self.lock_path().display().to_string(),
            }),
            Err(TryLockError::Error(e)) => Err(MirrorError::io(&self.lock_path())(e)),
        }
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
