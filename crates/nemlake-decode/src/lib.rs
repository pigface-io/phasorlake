//! The MMS C/I/D row format.
//!
//! ```text
//! C,SETP.WORLD,DVD_DISPATCH_UNIT_SCADA,AEMO,PUBLIC,2024/02/07,00:00:07,...   file header
//! I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE                    block header
//! D,DISPATCH,UNIT_SCADA,1,"2024/01/01 00:05:00",BUTLERSG,9.199999            data row
//! C,"END OF REPORT",4193482                                                  footer: total lines
//! ```
//!
//! Layer L2, synchronous and allocation-free per row. [`scan()`] splits bytes
//! into rows and drives a [`Sink`]: each I row reaches it as the block's
//! shape (report pair, version, ordered column names), each D row as its
//! fields, untyped. [`archive`] walks zips, including zips inside zips.
//!
//! Decode knows no registry: what a shape means, and how its text becomes
//! typed columns, is `nemlake-conform`'s business.

pub mod archive;
pub mod error;
pub mod fields;
pub mod quarantine;
pub mod scan;

pub use error::{RowError, RowFailure, ScanError, SinkError};
pub use fields::{PREFIX_FIELDS, Row};
pub use scan::{Pos, ScanOptions, ScanStats, Sink, scan};
