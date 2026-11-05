//! Where phasor keeps things: `~/.phasor` by default, `--home` or `PHASOR_HOME` otherwise.
//! The archive alone may live elsewhere (`archive` in `config.toml`: a drive of its own).

use std::path::{Path, PathBuf};

/// The home directory and the fixed layout beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
    /// Where the archive is when not `{root}/archive`.
    archive: Option<PathBuf>,
}

impl Home {
    /// A home at an explicit path.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            archive: None,
        }
    }

    /// Resolves the home: the flag (also filled from `PHASOR_HOME`), else `~/.phasor`.
    pub fn resolve(flag: Option<&Path>, user_home: Option<&Path>) -> Option<Self> {
        match flag {
            Some(p) => Some(Self::at(p)),
            None => user_home.map(|h| Self::at(h.join(".phasor"))),
        }
    }

    /// This home with its archive where `archive` says (the config file's
    /// `archive`), or under the root when none. A configured archive is
    /// never created ([`Self::ensure_dirs`]): a drive that is not mounted
    /// must not be stood in for by a directory on the boot disk.
    #[must_use]
    pub fn with_archive(mut self, archive: Option<PathBuf>) -> Self {
        self.archive = archive;
        self
    }

    /// The root.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// The lake of Parquet files, by market then table.
    pub fn lake(&self) -> PathBuf {
        self.root.join("lake")
    }
    /// Every downloaded file, untransformed, at a path mirroring its URL beside its record. Never cleaned.
    /// `{root}/archive`, unless the config file put it elsewhere.
    pub fn archive(&self) -> PathBuf {
        self.archive
            .clone()
            .unwrap_or_else(|| self.root.join("archive"))
    }
    /// What can be thrown away: scratch files.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    /// Scratch space for spooled nested zips and in-progress files.
    pub fn tmp(&self) -> PathBuf {
        self.cache().join("tmp")
    }
    /// Rows that failed to parse, as JSONL per table and source.
    pub fn quarantine(&self) -> PathBuf {
        self.root.join("quarantine")
    }
    /// User-supplied schema overlays and drafts.
    pub fn schemas(&self) -> PathBuf {
        self.root.join("schemas")
    }
    /// The SQLite manifest.
    pub fn manifest(&self) -> PathBuf {
        self.root.join("manifest.sqlite")
    }
    /// The configuration file.
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Creates every directory of the layout under the root but the
    /// archive ([`Self::ensure_archive`]), which the config file under the
    /// root may put elsewhere.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [self.lake(), self.tmp(), self.quarantine(), self.schemas()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    /// Creates the archive under the root; one configured elsewhere is
    /// left to exist or not.
    pub fn ensure_archive(&self) -> std::io::Result<()> {
        if self.archive.is_none() {
            std::fs::create_dir_all(self.archive())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence() {
        let flag = Path::new("/f");
        let home = Path::new("/u");
        assert_eq!(Home::resolve(Some(flag), Some(home)).unwrap().root(), flag);
        assert_eq!(
            Home::resolve(None, Some(home)).unwrap().root(),
            Path::new("/u/.phasor")
        );
        assert_eq!(Home::resolve(None, None), None);
    }

    #[test]
    fn layout_and_ensure() {
        let dir = tempfile::tempdir().unwrap();
        let h = Home::at(dir.path());
        h.ensure_dirs().unwrap();
        assert!(!h.archive().exists(), "the archive is the second step");
        h.ensure_archive().unwrap();
        for p in [
            h.lake(),
            h.archive(),
            h.cache(),
            h.tmp(),
            h.quarantine(),
            h.schemas(),
        ] {
            assert!(p.is_dir(), "{}", p.display());
        }
        assert_eq!(h.manifest(), dir.path().join("manifest.sqlite"));
        assert_eq!(h.config_file(), dir.path().join("config.toml"));
    }

    #[test]
    fn an_archive_elsewhere_is_named_and_never_made() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("drive/archive");
        let h = Home::at(dir.path().join("home")).with_archive(Some(elsewhere.clone()));
        assert_eq!(h.archive(), elsewhere);
        assert_eq!(h.lake(), dir.path().join("home/lake"));
        h.ensure_dirs().unwrap();
        h.ensure_archive().unwrap();
        assert!(h.lake().is_dir());
        assert!(!elsewhere.exists(), "a configured archive is not created");
        assert!(!dir.path().join("home/archive").exists());
        // Without one, the archive is under the root, and made.
        let h = h.with_archive(None);
        assert_eq!(h.archive(), dir.path().join("home/archive"));
        h.ensure_archive().unwrap();
        assert!(h.archive().is_dir());
    }
}
