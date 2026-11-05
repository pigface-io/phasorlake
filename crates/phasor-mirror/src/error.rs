//! Why a mirror command failed.

use crate::diag::{Category, Diagnose, Diagnostic, MIRROR};
use crate::net::FetchError;

use crate::listing::Unparsed;

/// Why a mirror command failed.
#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    /// A listing page held something that is neither a folder nor a file.
    #[error(
        "The listing {url} holds a line that is not a folder or a file ({source}); the crawl stops so that no link is passed over."
    )]
    Listing {
        /// The page.
        url: String,
        /// What could not be read.
        source: Unparsed,
    },
    /// Downloading a page or a file failed.
    #[error(transparent)]
    Fetch(#[from] FetchError),
    /// Reading or writing the mirror's own files failed.
    #[error("Reading or writing {path} failed: {source}")]
    Io {
        /// The path.
        path: String,
        /// Cause.
        source: std::io::Error,
    },
    /// A line of the crawl record is not a record.
    #[error("Line {line} of the crawl record {path} is not a record: {message}")]
    Record {
        /// The path.
        path: String,
        /// The 1-based line.
        line: u64,
        /// Cause.
        message: String,
    },
    /// Downloading needs a finished crawl.
    #[error(
        "The crawl in {path} is not finished; run `phasor-mirror crawl` (or `phasor-mirror run`) first."
    )]
    CrawlIncomplete {
        /// The crawl directory.
        path: String,
    },
    /// The run lock cannot be taken, most often because another run holds it.
    #[error(
        "The run lock {path} cannot be taken ({cause}); another run most likely holds it: wait for it to end, or stop it."
    )]
    Locked {
        /// The lock file.
        path: String,
        /// Why.
        cause: String,
    },
    /// The committed summary is not the summary of the current map.
    #[error(
        "{path} is not the summary of the current map; run `phasor-mirror summary` and commit it."
    )]
    SummaryStale {
        /// The summary file.
        path: String,
    },
    /// A zip in the archive could not be read.
    #[error("The zip {path} could not be read: {message}")]
    Zip {
        /// The zip.
        path: String,
        /// Cause.
        message: String,
    },
    /// Only a zip found the same as its folder is removed.
    #[error("{url} was not found the same as its folder, so it is kept.")]
    NotSame {
        /// The zip's URL.
        url: String,
    },
    /// `prune` with no month to keep from.
    #[error("Prune needs a month to keep from: set keep_from in config.toml or pass --keep-from.")]
    NoKeepFrom,
    /// A root is not a folder URL.
    #[error("The root {url} is not a folder: a root is an http(s) URL ending in `/`.")]
    Root {
        /// The root as given.
        url: String,
    },
    /// Nothing to read: the config names no host and no root was given.
    #[error(
        "No host to read: name one in config.toml ([[hosts]]; `phasor-mirror init --nemweb` writes nemweb's) or pass --root."
    )]
    NoHosts,
    /// Another process on this machine is reading the host.
    #[error(
        "The host {host} is being read by another phasor-mirror on this machine (lock {path}: {cause}); two readers of one site draw its refusal together, so wait for it or stop it."
    )]
    HostBusy {
        /// The host, as its lock names it.
        host: String,
        /// The lock file.
        path: String,
        /// Why the lock could not be taken.
        cause: String,
    },
    /// The host's robots.txt disallows this client under a root.
    #[error(
        "The robots.txt of {host} disallows this client under {root}; the root is not read. Take it out of the config, or ask the host."
    )]
    RobotsDisallow {
        /// The host's name.
        host: String,
        /// The root.
        root: String,
    },
    /// No directory for the host locks.
    #[error(
        "No directory for the host locks: set PHASOR_MIRROR_LOCK_DIR, or run as a user with a cache directory."
    )]
    NoLockDir,
    /// `init` would overwrite a config file.
    #[error("{path} exists; pass --force to write over it.")]
    ConfigExists {
        /// The file.
        path: String,
    },
}

impl From<phasor_provenance::archive::Error> for MirrorError {
    fn from(e: phasor_provenance::archive::Error) -> Self {
        Self::Fetch(e.into())
    }
}

impl MirrorError {
    pub(crate) fn io(path: &std::path::Path) -> impl Fn(std::io::Error) -> Self + '_ {
        move |source| Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    /// The line `i` (from 0) of the record at `path` that does not parse.
    pub(crate) fn record(
        path: &std::path::Path,
        i: usize,
    ) -> impl FnOnce(serde_json::Error) -> Self + '_ {
        move |e| Self::Record {
            path: path.display().to_string(),
            line: i as u64 + 1,
            message: e.to_string(),
        }
    }
}

impl Diagnose for MirrorError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Listing { url, source } => {
                Diagnostic::error("listing.unparsed_line", Category::LISTING, msg)
                    .with_url(url.clone())
                    .with_sample(source.text.as_bytes())
            }
            Self::Fetch(e) => e.diagnostic(),
            Self::Io { path, source } => Diagnostic::error("mirror.io", MIRROR, msg)
                .with_file(path.clone())
                .caused_by(source),
            Self::Record { path, line, .. } => {
                let mut d =
                    Diagnostic::error("mirror.record_corrupt", MIRROR, msg).with_file(path.clone());
                d.line = Some(*line);
                d
            }
            Self::CrawlIncomplete { path } => {
                Diagnostic::error("mirror.crawl_incomplete", MIRROR, msg).with_file(path.clone())
            }
            Self::Locked { path, .. } => {
                Diagnostic::error("mirror.locked", MIRROR, msg).with_file(path.clone())
            }
            Self::SummaryStale { path } => {
                Diagnostic::error("mirror.summary_stale", MIRROR, msg).with_file(path.clone())
            }
            Self::Zip { path, .. } => {
                Diagnostic::error("zip.open_failed", Category::ZIP, msg).with_file(path.clone())
            }
            Self::NotSame { url } => {
                Diagnostic::error("mirror.not_same", MIRROR, msg).with_url(url.clone())
            }
            Self::NoKeepFrom => {
                Diagnostic::error("config.keep_from_missing", Category::CONFIG, msg)
            }
            Self::Root { url } => {
                Diagnostic::error("config.mirror_root", Category::CONFIG, msg).with_url(url.clone())
            }
            Self::NoHosts => Diagnostic::error("config.no_hosts", Category::CONFIG, msg),
            Self::HostBusy { path, .. } => {
                Diagnostic::error("mirror.host_busy", MIRROR, msg).with_file(path.clone())
            }
            Self::RobotsDisallow { root, .. } => {
                Diagnostic::error("mirror.robots_disallow", MIRROR, msg).with_url(root.clone())
            }
            Self::NoLockDir => Diagnostic::error("mirror.no_lock_dir", MIRROR, msg),
            Self::ConfigExists { path } => {
                Diagnostic::error("config.exists", Category::CONFIG, msg).with_file(path.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Archive;

    #[test]
    fn every_error_has_its_code_and_says_where() {
        let cases = [
            (
                MirrorError::CrawlIncomplete {
                    path: "/m/crawl".into(),
                },
                "mirror.crawl_incomplete",
            ),
            (
                MirrorError::Locked {
                    path: "/m/lock".into(),
                    cause: "held".into(),
                },
                "mirror.locked",
            ),
            (
                MirrorError::SummaryStale {
                    path: "nemweb/map.md".into(),
                },
                "mirror.summary_stale",
            ),
            (
                MirrorError::HostBusy {
                    host: "www.nemweb.com.au".into(),
                    path: "/c/locks/www.nemweb.com.au".into(),
                    cause: "held".into(),
                },
                "mirror.host_busy",
            ),
            (
                MirrorError::ConfigExists {
                    path: "/m/config.toml".into(),
                },
                "config.exists",
            ),
        ];
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("f.provenance.json")).unwrap();
        let store = Archive::read(&dir.path().join("f")).unwrap_err();
        let e: MirrorError = store.into();
        assert_eq!(e.diagnostic().code, "cache.io");
        for (e, code) in cases {
            let d = e.diagnostic();
            assert_eq!(d.code, code);
            assert!(d.file.is_some(), "{code}");
            assert!(e.to_string().ends_with('.'), "{e}");
        }
        // The errors with no file say their code and end their sentence too.
        let robots = MirrorError::RobotsDisallow {
            host: "nemweb".into(),
            root: "https://h/Data/".into(),
        };
        assert_eq!(robots.diagnostic().code, "mirror.robots_disallow");
        assert_eq!(robots.diagnostic().url.as_deref(), Some("https://h/Data/"));
        for (e, code) in [
            (MirrorError::NoHosts, "config.no_hosts"),
            (MirrorError::NoLockDir, "mirror.no_lock_dir"),
        ] {
            assert_eq!(e.diagnostic().code, code);
            assert!(e.to_string().ends_with('.'), "{e}");
        }
    }
}
