//! Moving AEMO's files over HTTP into the archive, for the lake and the mirror.
//! [`transport`] moves bytes, [`fetch`] paces and retries requests, [`archive`]
//! keeps each URL's bytes beside their provenance record; every failure is a [`FetchError`].
//! [`bloblist`] reads a blob store's folder listing, the one kind of listing
//! that is not an IIS page.

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
