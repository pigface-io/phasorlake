//! The MMS C/I/D row format.
//!
//! ```text
//! C,SETP.WORLD,DVD_DISPATCH_UNIT_SCADA,AEMO,PUBLIC,2024/02/07,00:00:07,...   file header
//! I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE                    block header
//! D,DISPATCH,UNIT_SCADA,1,"2024/01/01 00:05:00",BUTLERSG,9.199999            data row
//! C,"END OF REPORT",4193482                                                  footer: total lines
//! ```
//!
//! Layer L2, synchronous and allocation-free per row. [`scan`] splits bytes
//! into rows and drives a [`Sink`]; [`ArrowSink`] resolves each block through
//! the registry and builds typed `RecordBatch`es; [`archive`] walks zips,
//! including zips inside zips; [`inspect`] summarises a file without
//! building anything.

pub mod archive;
pub mod arrow_sink;
pub mod builders;
pub mod error;
pub mod fields;
pub mod inspect;
pub mod quarantine;
pub mod scan;

pub use arrow_sink::{ArrowSink, BlockBatch};
pub use error::{RowError, RowFailure, ScanError, SinkError};
pub use fields::Row;
pub use scan::{Pos, ScanOptions, ScanStats, Sink, scan};
