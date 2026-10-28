//! AEMO's east-coast gas markets (GBB, GSH, STTM, DWGM) as one kit: each feed's
//! nemweb report folders a [`FeedFolders`] source, a `.csv` read by what its first
//! bytes say it is, the registry's gas tables as the catalog.

pub mod config;
pub mod decoder;
pub mod kit;

pub use config::GasConfig;
pub use decoder::GasCsv;
pub use kit::{KIT_ID, SOURCE_IDS, kit, source_id};
pub use phasor_acquire::FeedFolders;
