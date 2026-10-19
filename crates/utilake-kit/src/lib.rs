//! The ports a source kit implements, and the registry that holds every
//! kit a binary was assembled with.
//!
//! Layer L1. The pipeline runs over these and nothing else: a
//! [`Source`](utilake_acquire::Source) lists and describes files, a
//! [`Decoder`] reads an entry into [`Block`]s for a [`BlockSink`], a
//! [`Catalog`] declares the tables, and a [`SourceKit`] bundles the three
//! for one market. [`container`] walks the zips the files come in;
//! [`quarantine`] keeps the rows a decoder rejected.
//! [`Transform`] is what every lake file records of how it was read.
//!
//! No format and no market is named here: those live in their own crates
//! and are registered at assembly.

pub mod catalog;
pub mod container;
pub mod decoder;
pub mod kit;
pub mod quarantine;
pub mod transform;

pub use catalog::Catalog;
pub use decoder::{
    Block, BlockSink, DecodeCtx, DecodeOptions, DecodeStats, Decoder, Flow, LoadMode, RowAt,
    Strictness, TableRef,
};
pub use kit::{KitRegistry, SourceKit};
pub use transform::Transform;
