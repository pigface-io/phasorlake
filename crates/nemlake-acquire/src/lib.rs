//! Where AEMO's files are and how to get them.
//!
//! Layer L3, async. A [`Source`] lists the remote files a request needs
//! ([`MmsdmArchive`] for the monthly NEM archive, [`WemDatafiles`] for WEM
//! manifests); a [`Transport`] moves bytes (reqwest, or an offline stub); the
//! [`Fetcher`] combines a transport with the [`Archive`] on disk, retries and
//! politeness into one `fetch(remote) -> LocalFile` call: each file is
//! downloaded whole, once, and kept.

pub mod archive;
pub mod error;
pub mod fetcher;
pub mod listing;
pub mod mmsdm;
pub mod source;
pub mod transport;
pub mod wem;

pub use archive::{Archive, Archived, Sidecar, sha256_file};
pub use error::FetchError;
pub use fetcher::{Fetcher, FetcherConfig, LocalFile};
pub use mmsdm::MmsdmArchive;
pub use source::{Query, RemoteFile, Source};
pub use transport::{HttpResponse, OfflineTransport, ReqwestTransport, Transport};
pub use wem::WemDatafiles;
