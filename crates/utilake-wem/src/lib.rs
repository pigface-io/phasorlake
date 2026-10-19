//! The Wholesale Electricity Market kit.
//!
//! What the WEM brings to the lake today: AEMO's datafiles, listed by
//! their YAML manifests, as a [`Source`](utilake_acquire::Source)
//! ([`WemDatafiles`]), and its settings ([`WemConfig`]). The [`kit()`] has
//! no decoder: the datafiles are plain CSV, which no decoder reads yet, so
//! an ingest of one is refused with `config.no_decoder`. The decoder is
//! the seam this crate leaves open.

pub mod config;
pub mod kit;
pub mod sources;

pub use config::WemConfig;
pub use kit::{KIT_ID, WemCatalog, kit};
pub use sources::datafiles::{
    ManifestEntry, SOURCE_ID, WemDatafiles, describe_name, parse_manifest,
};
