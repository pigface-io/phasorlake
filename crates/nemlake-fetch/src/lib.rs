//! Where AEMO's files are and how to get them.
//!
//! Layer L3, async. A [`Source`] lists the remote files a request needs
//! ([`MmsdmArchive`] for the monthly NEM archive, [`WemDatafiles`] for WEM
//! manifests); a [`Transport`] moves bytes (reqwest, or an offline stub); the
//! [`Fetcher`] combines a transport with the on-disk [`DiskCache`], retries,
//! politeness and verification into one `fetch(remote) -> LocalFile` call.

pub mod cache;
pub mod error;
pub mod fetcher;
pub mod listing;
pub mod mmsdm;
pub mod remotezip;
pub mod source;
pub mod transport;
pub mod wem;

pub use cache::{CachedBlob, DiskCache, Sidecar};
pub use error::FetchError;
pub use fetcher::{Fetcher, FetcherConfig, LocalFile};
pub use mmsdm::MmsdmArchive;
pub use source::{FileKind, Query, RemoteFile, Source};
pub use transport::{
    ByteRange, HttpRequest, HttpResponse, OfflineTransport, ReqwestTransport, Transport,
};
pub use wem::WemDatafiles;
