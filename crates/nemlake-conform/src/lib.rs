//! Conforming decoded MMS rows to the registry.
//!
//! Layer L2, synchronous. `nemlake-decode` hands over each block's shape and
//! its rows as untyped fields; this crate resolves the shape through the
//! registry and applies the result: casts every field to its lake type,
//! decides which rows are quarantined, clamps timestamps beyond the
//! nanosecond range, and keeps columns no table declares as text.
//! [`ArrowSink`] builds the typed `RecordBatch`es; [`inspect`] summarises a
//! file without building anything.

pub mod arrow_sink;
pub mod builders;
pub mod inspect;

pub use arrow_sink::{ArrowSink, BlockBatch};
