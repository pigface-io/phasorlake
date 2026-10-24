//! AEMO's east-coast gas markets as one kit.
//!
//! The Gas Bulletin Board, the Gas Supply Hub, the Short Term Trading
//! Market and the Declared Wholesale Gas Market are published under
//! nemweb's report folders (`Reports/CURRENT/{GBB,GSH,STTM,VicGas}/` and
//! `Reports/ARCHIVE/GSH/`), each a feed of the shared feeds table. The kit
//! lists each feed's folders as a source of its own ([`FeedFolders`]),
//! reads a `.csv` by what its first bytes say it is (the GSH files are MMS
//! rows, the rest header-row CSV) and serves the registry's gas tables as
//! its catalog.

pub mod config;
pub mod decoder;
pub mod kit;

pub use config::GasConfig;
pub use decoder::GasCsv;
pub use kit::{KIT_ID, SOURCE_IDS, kit, source_id};
pub use utilake_kit::FeedFolders;
