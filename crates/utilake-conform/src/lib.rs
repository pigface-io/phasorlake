//! Conforming decoded MMS rows to the registry.
//!
//! Layer L2, synchronous. `utilake-decode` hands over each block's shape and
//! its rows as untyped fields; this crate resolves the shape through the
//! registry and applies the result: casts every field to its lake type,
//! decides which rows are quarantined, clamps timestamps beyond the
//! nanosecond range, and keeps columns no table declares as text.
//! [`ArrowSink`] builds the typed `RecordBatch`es; [`inspect`] summarises a
//! file without building anything; [`kit`] is the format as a kit's
//! decoder and catalog.

pub mod arrow_sink;
pub mod builders;
pub mod inspect;
pub mod kit;

pub use arrow_sink::{ArrowSink, BlockBatch};
pub use kit::{MmsCatalog, MmsDecoder, nem_kit};
