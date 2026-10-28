//! Getting AEMO's files: the [`Fetcher`] and [`FeedFolders`] adapters behind
//! the kit's ports. Layering is in CONTRIBUTING.md.

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
