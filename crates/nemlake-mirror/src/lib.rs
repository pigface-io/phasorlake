//! A copy of every file under nemweb's archive folders, and a map of it.
//!
//! [`crawl()`] reads every listing page under the roots, strictly
//! ([`listing`]), into one record of every folder and file. Every file of
//! that record is downloaded through the [`nemlake_acquire::Fetcher`] into
//! the same archive `nemlake fetch` uses, so a file is never downloaded
//! twice. [`status()`] counts how far that has got.

pub mod crawl;
pub mod download;
pub mod error;
pub mod listing;
pub mod lock;
pub mod status;

pub use crawl::{
    CrawlSummary, Mirror, PageRead, Remote, RemoteKind, crawl, default_roots, read_record, root,
};
pub use download::{DownloadSummary, FileDone, SOURCE, download};
pub use error::MirrorError;
pub use lock::RunLock;
pub use status::{RootStatus, Status, status};
pub use url::Url;
