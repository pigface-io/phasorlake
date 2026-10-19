//! Vocabulary shared by every utilake crate.
//!
//! This is layer L0: identifiers, calendar months, the MMS timestamp and
//! number formats, the home-directory layout, configuration, and the
//! `Diagnostic` type every error and warning in the system is reported as.
//! Nothing here performs I/O beyond reading a config file, and nothing here
//! knows about Arrow, SQLite, HTTP or tokio, so every other crate can depend
//! on it without a cycle and the hot path can use it without cost.

pub mod archive_name;
pub mod clock;
pub mod config;
pub mod diag;
pub mod event;
pub mod irow;
pub mod nemtime;
pub mod num;
pub mod paths;
pub mod period;
pub mod report;
pub mod shape;
pub mod timetext;

pub use diag::{Category, Diagnose, Diagnostic, Sample, Severity};
pub use irow::{Fingerprint, IRow, IRowError, IRowScratch, normalize_name};
pub use period::YearMonth;
pub use shape::{CanonicalShape, SHAPE_FORMAT, ShapeId, TypeClass};
