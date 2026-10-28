//! Conforming decoded MMS rows to the registry (layer L2, synchronous).
//!
//! [`ArrowSink`] builds the typed batches, [`inspect`] summarises a file, [`kit`] is the decoder.

/// Opens a zone of `zonetrace` until the end of the block, with the
/// feature `zones`; nothing without it.
#[allow(unused_macros)]
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = zonetrace::enter(&crate::zones::$z);
    };
}

#[cfg(feature = "zones")]
pub mod zones;

pub mod arrow_sink;
pub mod builders;
pub mod error;
pub mod inspect;
pub mod kit;
pub mod mmstime;
pub mod plain;

pub use arrow_sink::{ArrowSink, BlockBatch};
pub use error::{RowError, RowFailure, ScanError, SinkError, ValueError};
pub use kit::MmsDecoder;
pub use plain::{PlainCsvDecoder, SniffedCsv, WemdeDecoder};
