//! A copy of every file under nemweb's archive folders, and under any
//! other host the config names ([`hosts`]), and the catalog of it: every
//! folder, file and zip entry, which the lake reads.
//!
//! [`crawl()`] reads every listing page under the roots, strictly
//! ([`listing`]), into one record of every folder and file. Every file of
//! that record is downloaded through the paced [`net::Fetcher`] into the
//! archive (`docs/archive.md`), each once. [`map()`] writes the catalog
//! (`docs/catalog.md`), [`compare()`] finds the zips that repeat a folder,
//! and [`status()`] counts how far it has got.

pub mod compare;
pub mod config;
pub mod crawl;
pub mod diag;
pub mod download;
pub mod error;
pub mod hash;
pub mod hosts;
pub mod journal;
pub mod keep;
pub mod listing;
pub mod lock;
pub mod map;
pub mod net;
pub mod peek;
pub mod stamp;
pub mod status;
pub mod summary;
#[cfg(test)]
pub mod testzip;
pub mod versions;
pub mod watch;
pub mod zipdir;

pub use compare::{
    Comparison, Pair, Removed, Verdict, Verified, compare, note_entries, pairs, remove, verify,
};
pub use crawl::{
    CrawlSummary, Mirror, PageRead, Remote, RemoteKind, crawl, default_roots, read_record, root,
};
pub use diag::{Category, Diagnose, Diagnostic};
pub use download::{DownloadOptions, DownloadSummary, FileDone, download};
pub use error::MirrorError;
pub use hash::{FileHashed, HashSummary, hash};
pub use hosts::{Host, Offsets};
pub use journal::{Change, Event};
pub use keep::{Month, PruneSummary, data_month, keeps, prune};
pub use lock::RunLock;
pub use map::{MapSummary, Node, NodeKind, State, map};
pub use peek::{PeekSummary, Peeked, ZipPeeked, peek, read_peeks};
pub use stamp::Stamp;
pub use status::{PeekCount, RootStatus, Status, status};
pub use summary::{fold, summary};
pub use url::Url;
pub use versions::{VersionsSummary, versions};
pub use watch::{Cadence, Refreshed, Schedule, refresh};
