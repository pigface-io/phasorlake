//! Moving AEMO's files over HTTP into the archive, for the lake and the
//! mirror alike.
//!
//! A [`Transport`] moves bytes: [`ReqwestTransport`] (feature `http`) to a
//! real server, [`OfflineTransport`] to none, and the canned
//! [`transport::StaticTransport`] in tests. Every failure is a
//! [`FetchError`] with a stable code. [`fetch`] (feature `fetch`) paces,
//! retries and stops requests for good when refused, and streams a body to
//! disk. [`archive`] adds hashing, checks and
//! the record edits to the family's archive store, where each URL's bytes
//! are kept at its path beside their provenance record.

pub mod archive;
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
pub use fetch::{FetcherConfig, RangeRead, Requests, Streamed, stream_to_file};
#[cfg(feature = "http")]
pub use transport::ReqwestTransport;
pub use transport::{HttpResponse, OfflineTransport, Span, Transport};
