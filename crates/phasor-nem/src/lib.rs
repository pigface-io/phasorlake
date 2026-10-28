//! The National Electricity Market kit: nemweb's monthly archive ([`MmsdmArchive`])
//! and report folders as sources, file names ([`names`]), settings ([`NemConfig`])
//! and the [`kit()`] that bundles them. Nothing here reads rows.

pub mod cli;
pub mod config;
pub mod kit;
pub mod sources;

pub use config::NemConfig;
pub use kit::{DOCUMENTS, KIT_ID, kit, registry};
pub use phasor_mms_decode::filename as names;
pub use sources::mmsdm::{MmsdmArchive, SOURCE_ID, describe_name, period_of_name};
pub use sources::reports::{REPORTS_SOURCE_ID, ReportFolders, ReportName, parse_report_name};
