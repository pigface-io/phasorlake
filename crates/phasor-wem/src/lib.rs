//! The Wholesale Electricity Market kit.
//!
//! AEMO's datafiles, listed by their YAML manifests and found in the
//! archive by the feed's folders, as a [`Source`](phasor_kit::Source)
//! ([`WemDatafiles`]); the WEMDE documents' daily zips as another; the
//! plain CSV and WEMDE decoders of the conform crate; the registry's WEM
//! tables as the catalog; and the kit's settings ([`WemConfig`]).

pub mod config;
pub mod kit;
pub mod sources;

pub use config::WemConfig;
pub use kit::{KIT_ID, WEMDE_SOURCE_ID, kit};
pub use sources::datafiles::{
    ManifestEntry, SOURCE_ID, WemDatafiles, describe_name, parse_manifest,
};
