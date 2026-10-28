//! Vocabulary shared by every phasor crate: identifiers, calendar and periods,
//! numbers, the home layout, configuration and diagnostics. No I/O beyond reading
//! a config file, and no Arrow, HTTP or tokio.

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
pub use phasor_decode::{Fingerprint, normalize_name};
