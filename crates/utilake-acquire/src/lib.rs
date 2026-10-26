//! Getting AEMO's files: the adapters behind the kit's ports.
//!
//! Layer L3, async. The [`Fetcher`] implements the kit's
//! [`Listings`](utilake_kit::Listings) and combines `utilake-net`'s paced,
//! retried requests with the [`Archive`] on disk into one `fetch(remote) ->
//! LocalFile` call: each file is downloaded whole, once, and kept. A zip
//! that holds more than a request needs is read by its directory first,
//! through ranged requests, and only the entries the source picks are
//! fetched (`fetch_picks`), each kept as the file it is. [`FeedFolders`] is
//! a feed's folder tree as a [`Source`](utilake_kit::Source), listed by the
//! IIS directory pages [`listing`] reads.

pub mod archive;
pub mod error;
pub mod feed_folders;
pub mod fetcher;
pub mod listing;
pub mod transport;

pub use archive::{Address, Archive, Content, Held, Provenance, Served, Via, sha256_file};
pub use error::FetchError;
pub use feed_folders::FeedFolders;
pub use fetcher::{Fetcher, FetcherConfig, Picked, RangeRead};
pub use transport::{HttpResponse, OfflineTransport, ReqwestTransport, Span, Transport};
