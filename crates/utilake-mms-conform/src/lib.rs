//! Conforming decoded MMS rows to the registry.
//!
//! Layer L2, synchronous. `utilake-mms-decode` hands over each block's shape and
//! its rows as untyped fields; this crate resolves the shape through the
//! registry and applies the result: casts every field to its lake type,
//! decides which rows are quarantined, clamps timestamps beyond the
//! nanosecond range, and keeps columns no table declares as text.
//! [`ArrowSink`] builds the typed `RecordBatch`es; [`inspect`] summarises a
//! file without building anything; [`kit`] is the format as a kit's
//! decoder and catalog; [`plain`] reads the header-row CSV and WEMDE JSON
//! files of the other feeds through the same sink.

pub mod arrow_sink;
pub mod builders;
pub mod error;
pub mod inspect;
pub mod kit;
pub mod mmstime;
pub mod plain;

pub use arrow_sink::{ArrowSink, BlockBatch};
pub use error::{RowError, RowFailure, ScanError, SinkError, ValueError};
pub use kit::{MmsCatalog, MmsDecoder};
pub use plain::{PlainCsvDecoder, SniffedCsv, WemdeDecoder};
