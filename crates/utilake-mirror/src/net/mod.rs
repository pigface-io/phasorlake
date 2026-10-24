//! Moving bytes from nemweb and the other hosts into the archive:
//! transports, the paced fetcher with its refusal breaker, one per host,
//! and the archive that keeps each URL's
//! bytes at its path with its provenance record.

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
    Address, Archive, Content, DirNames, Held, Provenance, Served, Sha256, Via, content_of,
    is_apple_double,
};
pub use error::FetchError;
pub use event::Event;
pub use fetcher::{Fetcher, FetcherConfig, LocalFile, RangeRead, Same};
pub use fetchers::{Fetchers, HostFetcher, per_host};
pub use source::RemoteFile;
pub use transport::{HttpResponse, OfflineTransport, ReqwestTransport, Span, Transport};
