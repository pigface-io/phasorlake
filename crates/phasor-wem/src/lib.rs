//! The Wholesale Electricity Market kit: datafiles ([`WemDatafiles`]), WEMDE daily
//! zips, the plain decoders and the registry's WEM tables, set by [`WemConfig`].

pub mod config;
pub mod kit;
pub mod sources;

pub use config::WemConfig;
pub use kit::{KIT_ID, WEMDE_SOURCE_ID, kit};
pub use sources::datafiles::{
    ManifestEntry, SOURCE_ID, WemDatafiles, describe_name, parse_manifest,
};
