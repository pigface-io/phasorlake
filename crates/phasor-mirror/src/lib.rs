//! A copy of every file under nemweb's archive folders and any other host
//! the config names ([`hosts`]), and the catalog of it, which the lake reads.
//! [`crawl()`] lists, [`net::Fetcher`] downloads, [`map()`] writes the catalog.

/// Opens a zone of `zonetrace` until the end of the block, with the
/// feature `zones`; nothing without it. For synchronous code only.
#[allow(unused_macros)]
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = zonetrace::enter(&crate::zones::$z);
    };
}

/// Records a stretch of time on a path of zones when the block ends, with
/// the feature `zones`; nothing without it. Safe across an `.await`.
#[allow(unused_macros)]
macro_rules! stretch {
    ($p:ident) => {
        #[cfg(feature = "zones")]
        let _stretch = zonetrace::stretch(&crate::zones::$p);
    };
}

#[cfg(feature = "zones")]
pub mod zones;

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
pub mod listers;
pub mod listing;
pub mod lock;
pub mod map;
mod ndjson;
pub mod net;
pub mod peek;
pub mod robots;
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
    write_atomic,
};
pub use diag::{Category, Diagnose, Diagnostic};
pub use download::{DownloadOptions, DownloadSummary, FileDone, download};
pub use error::MirrorError;
pub use hash::{FileHashed, HashSummary, hash};
pub use hosts::{Host, Offsets};
pub use journal::{Change, Event};
pub use keep::{Month, PruneSummary, data_month, keeps, prune};
pub use lock::{HostLocks, RunLock};
pub use map::{MapSummary, Node, NodeKind, State, map};
pub use peek::{PeekSummary, Peeked, ZipPeeked, peek, read_peeks};
pub use stamp::Stamp;
pub use status::{PeekCount, RootStatus, Status, status};
pub use summary::{fold, summary};
pub use url::Url;
pub use versions::{VersionsSummary, versions};
pub use watch::{Cadence, Refreshed, Schedule, refresh};
