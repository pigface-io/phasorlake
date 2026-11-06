//! Getting AEMO's files: the [`Fetcher`] and [`FeedFolders`] adapters behind
//! the kit's ports. Layering is in CONTRIBUTING.md.

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

pub mod archive;
pub mod error;
pub mod feed_folders;
pub mod fetcher;
pub mod listing;
pub mod transport;

pub use archive::{Address, Archive, Content, Held, Provenance, Served, Via, sha256_file};
pub use error::FetchError;
pub use feed_folders::{Claim, FeedFolders};
pub use fetcher::{Fetcher, FetcherConfig, Picked, RangeRead};
pub use transport::{HttpResponse, OfflineTransport, ReqwestTransport, Span, Transport};
