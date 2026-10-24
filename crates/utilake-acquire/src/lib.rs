//! Where AEMO's files are and how to get them.
//!
//! Layer L3, async. A [`Source`] lists the remote files a request needs (a
//! kit brings its own); a [`Transport`] moves bytes (reqwest, or an
//! offline stub); the
//! [`Fetcher`] combines a transport with the [`Archive`] on disk, retries and
//! politeness into one `fetch(remote) -> LocalFile` call: each file is
//! downloaded whole, once, and kept. A zip that holds more than a request
//! needs is read by its directory first, through ranged requests, and only
//! the entries the source picks are fetched (`fetch_picks`), each kept as
//! the file it is.

pub mod archive;
pub mod error;
pub mod fetcher;
pub mod listing;
pub mod source;
pub mod transport;

pub use archive::{Address, Archive, Content, Held, Provenance, Served, Via, sha256_file};
pub use error::FetchError;
pub use fetcher::{Fetcher, FetcherConfig, LocalFile, Picked, RangeRead};
pub use source::{Entry, Granularity, Pick, Picks, Query, RemoteFile, Source};
pub use transport::{HttpResponse, OfflineTransport, ReqwestTransport, Span, Transport};
