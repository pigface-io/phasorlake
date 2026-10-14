//! Why a mirror command failed.

use nemlake_acquire::FetchError;
use nemlake_core::{Category, Diagnose, Diagnostic};

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
        "The crawl in {path} is not finished; run `nemlake mirror crawl` (or `mirror run`) first."
    )]
    CrawlIncomplete {
        /// The crawl directory.
        path: String,
    },
    /// Another run holds the lock.
    #[error("Another mirror run holds {path}; wait for it to end, or stop it.")]
    Locked {
        /// The lock file.
        path: String,
    },
    /// The committed summary is not the summary of the current map.
    #[error(
        "{path} is not the summary of the current map; run `nemlake mirror summary` and commit it."
    )]
    SummaryStale {
        /// The summary file.
        path: String,
    },
    /// A root is not a folder URL.
    #[error("The root {url} is not a folder: a root is an http(s) URL ending in `/`.")]
    Root {
        /// The root as given.
        url: String,
    },
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
                Diagnostic::error("listing.unparsed_line", Category::Listing, msg)
                    .with_url(url.clone())
                    .with_sample(source.text.as_bytes())
            }
            Self::Fetch(e) => e.diagnostic(),
            Self::Io { path, source } => Diagnostic::error("mirror.io", Category::Mirror, msg)
                .with_file(path.clone())
                .caused_by(source),
            Self::Record { path, line, .. } => {
                let mut d = Diagnostic::error("mirror.record_corrupt", Category::Mirror, msg)
                    .with_file(path.clone());
                d.line = Some(*line);
                d
            }
            Self::CrawlIncomplete { path } => {
                Diagnostic::error("mirror.crawl_incomplete", Category::Mirror, msg)
                    .with_file(path.clone())
            }
            Self::Locked { path } => {
                Diagnostic::error("mirror.locked", Category::Mirror, msg).with_file(path.clone())
            }
            Self::SummaryStale { path } => {
                Diagnostic::error("mirror.summary_stale", Category::Mirror, msg)
                    .with_file(path.clone())
            }
            Self::Root { url } => {
                Diagnostic::error("config.mirror_root", Category::Config, msg).with_url(url.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for (e, code) in cases {
            let d = e.diagnostic();
            assert_eq!(d.code, code);
            assert!(d.file.is_some(), "{code}");
            assert!(e.to_string().ends_with('.'), "{e}");
        }
    }
}
