//! Moving AEMO's files over HTTP into the archive, for the lake and the mirror.
//! [`transport`] moves bytes, [`fetch`] paces and retries requests, [`archive`]
//! keeps each URL's bytes beside their provenance record; every failure is a [`FetchError`].
//! [`bloblist`] reads a blob store's folder listing, the one kind of listing
//! that is not an IIS page.

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
pub mod bloblist;
pub mod error;
#[cfg(feature = "fetch")]
pub mod fetch;
#[cfg(feature = "test-support")]
pub mod testserver;
pub mod transport;

pub use archive::{
    Address, Archive, Content, DirNames, Held, Provenance, Served, Sha256, Via, content_of,
    is_apple_double, sha256_file,
};
pub use error::FetchError;
#[cfg(feature = "fetch")]
pub use fetch::{FetcherConfig, Headers, RangeRead, Requests, Streamed, stream_to_file};
#[cfg(feature = "http")]
pub use transport::ReqwestTransport;
pub use transport::{HttpResponse, OfflineTransport, Span, Transport};
