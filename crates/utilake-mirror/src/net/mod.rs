//! Moving bytes from nemweb and the other hosts into the archive:
//! transports, the paced fetcher with its refusal breaker, one per host,
//! and the archive that keeps each URL's
//! bytes at its path with a sidecar.

pub mod archive;
pub mod error;
pub mod event;
pub mod fetcher;
pub mod fetchers;
pub mod source;
#[cfg(test)]
pub mod testserver;
pub mod transport;

pub use archive::{
    Also, Archive, Archived, Hashes, How, Listed, Sidecar, hashes_of, is_apple_double, sha256_file,
};
pub use error::FetchError;
pub use event::Event;
pub use fetcher::{Fetcher, FetcherConfig, LocalFile, RangeRead, Same};
pub use fetchers::{Fetchers, HostFetcher, per_host};
pub use source::RemoteFile;
pub use transport::{HttpResponse, OfflineTransport, ReqwestTransport, Span, Transport};
