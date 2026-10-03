//! Where nemlake keeps things: `~/.nemlake` by default, `--home` or
//! `NEMLAKE_HOME` otherwise. Tests always point this at a temp directory.

use std::path::{Path, PathBuf};

/// The home directory and the fixed layout beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

/// Environment variable overriding the home directory.
pub const HOME_ENV: &str = "NEMLAKE_HOME";

impl Home {
    /// A home at an explicit path.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolves the home: the explicit flag, then `NEMLAKE_HOME`, then
    /// `~/.nemlake` under the user's home directory.
    pub fn resolve(
        flag: Option<&Path>,
        env: Option<&str>,
        user_home: Option<&Path>,
    ) -> Option<Self> {
        if let Some(p) = flag {
            return Some(Self::at(p));
        }
        if let Some(e) = env.filter(|e| !e.is_empty()) {
            return Some(Self::at(e));
        }
        user_home.map(|h| Self::at(h.join(".nemlake")))
    }

    /// The root.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// The lake of Parquet files, by market then table.
    pub fn lake(&self) -> PathBuf {
        self.root.join("lake")
    }
    /// Downloaded files and listings.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    /// Cached directory listings.
    pub fn listings(&self) -> PathBuf {
        self.cache().join("listings")
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

    /// Creates every directory of the layout.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [
            self.lake(),
            self.listings(),
            self.tmp(),
            self.quarantine(),
            self.schemas(),
        ] {
            std::fs::create_dir_all(d)?;
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
        assert_eq!(
            Home::resolve(Some(flag), Some("/e"), Some(home))
                .unwrap()
                .root(),
            flag
        );
        assert_eq!(
            Home::resolve(None, Some("/e"), Some(home)).unwrap().root(),
            Path::new("/e")
        );
        assert_eq!(
            Home::resolve(None, Some(""), Some(home)).unwrap().root(),
            Path::new("/u/.nemlake")
        );
        assert_eq!(Home::resolve(None, None, None), None);
    }

    #[test]
    fn layout_and_ensure() {
        let dir = tempfile::tempdir().unwrap();
        let h = Home::at(dir.path());
        h.ensure_dirs().unwrap();
        for p in [
            h.lake(),
            h.cache(),
            h.listings(),
            h.tmp(),
            h.quarantine(),
            h.schemas(),
        ] {
            assert!(p.is_dir(), "{}", p.display());
        }
        assert_eq!(h.manifest(), dir.path().join("manifest.sqlite"));
        assert_eq!(h.config_file(), dir.path().join("config.toml"));
    }
}
