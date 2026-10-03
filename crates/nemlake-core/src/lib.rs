//! Vocabulary shared by every nemlake crate.
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

pub use diag::{Category, Diagnose, Diagnostic, Sample, Severity};
pub use irow::{Fingerprint, IRow, normalize_name};
pub use period::YearMonth;
