//! The ports a source kit implements, and the registry of every kit a binary
//! was assembled with. No format or market is named here; layering is in CONTRIBUTING.md.

pub mod catalog;
pub mod decoder;
pub mod kit;
pub mod source;
pub mod transform;

pub use catalog::{Catalog, Serves, TableOf};
pub use decoder::{
    Block, BlockSink, DecodeCtx, DecodeOptions, DecodeStats, Decoder, Flow, LoadMode, RowAt,
    Strictness, TableRef,
};
pub use kit::{KitRegistry, SourceKit};
pub use phasor_plain_decode::names::version_of;
pub use source::{Entry, Granularity, Listings, LocalFile, Pick, Picks, Query, RemoteFile, Source};
pub use transform::Transform;
