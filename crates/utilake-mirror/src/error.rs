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
        "The crawl in {path} is not finished; run `utilake-mirror crawl` (or `utilake-mirror run`) first."
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
        "{path} is not the summary of the current map; run `utilake-mirror summary` and commit it."
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
    /// The home keeps its records in the layout before `state/`.
    #[error(
        "The mirror's records are at {path}, the layout before `state/`: run `utilake-mirror upgrade` first."
    )]
    OldLayout {
        /// The old records' folder.
        path: String,
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
}

impl From<utilake_provenance::archive::Error> for MirrorError {
    fn from(e: utilake_provenance::archive::Error) -> Self {
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
            Self::OldLayout { path } => {
                Diagnostic::error("mirror.old_layout", MIRROR, msg).with_file(path.clone())
            }
            Self::NoKeepFrom => {
                Diagnostic::error("config.keep_from_missing", Category::CONFIG, msg)
            }
            Self::Root { url } => {
                Diagnostic::error("config.mirror_root", Category::CONFIG, msg).with_url(url.clone())
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
    }
}
