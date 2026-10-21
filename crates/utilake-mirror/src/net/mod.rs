//! Moving bytes from nemweb into the archive: transports, the paced
//! fetcher with its refusal breaker, and the archive that keeps each URL's
//! bytes at its path with a sidecar.

pub mod archive;
pub mod error;
pub mod event;
pub mod fetcher;
pub mod source;
#[cfg(test)]
pub mod testserver;
pub mod transport;

pub use archive::{Archive, Archived, Hashes, Listed, Sidecar, hashes_of, sha256_file};
pub use error::FetchError;
pub use event::Event;
pub use fetcher::{Fetcher, FetcherConfig, LocalFile, RangeRead, Same};
pub use source::RemoteFile;
pub use transport::{HttpResponse, OfflineTransport, ReqwestTransport, Span, Transport};
