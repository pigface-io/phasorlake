//! Vocabulary shared by every utilake crate.
//!
//! This is layer L0: identifiers, the calendar and periods, the number
//! format, the home-directory layout, configuration, and the
//! `Diagnostic` type every error and warning in the system is reported as.
//! Nothing here performs I/O beyond reading a config file, and nothing here
//! knows about Arrow, SQLite, HTTP or tokio, so every other crate can depend
//! on it without a cycle and the hot path can use it without cost.

pub mod calendar;
pub mod clock;
pub mod config;
pub mod diag;
pub mod event;
pub mod num;
pub mod paths;
pub mod period;
pub mod report;
pub mod source_times;
pub mod timetext;

pub use diag::{Category, Diagnose, Diagnostic, Sample, Severity};
pub use period::{CivilDay, Period, TimeRange, YearMonth};
pub use utilake_mms_decode::{Fingerprint, normalize_name};
