//! The National Electricity Market kit.
//!
//! What the NEM brings to the lake: nemweb's monthly archive as a
//! [`Source`](utilake_acquire::Source) ([`MmsdmArchive`]), the names AEMO
//! gives its files ([`names`]), its settings ([`NemConfig`]), its command
//! line ([`cli`]), and the [`kit()`] that bundles them with the MMS decoder
//! and the registry's catalog for the pipeline. Nothing here reads rows:
//! the format is `utilake-mms-*`'s.

pub mod cli;
pub mod config;
pub mod kit;
pub mod sources;

pub use config::NemConfig;
pub use kit::{DOCUMENTS, KIT_ID, kit, registry};
pub use sources::mmsdm::{MmsdmArchive, SOURCE_ID, describe_name, period_of_name};
pub use sources::reports::{REPORTS_SOURCE_ID, ReportFolders, ReportName, parse_report_name};
pub use utilake_mms_decode::filename as names;
