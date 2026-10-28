//! The NEM as one kit.

use std::sync::Arc;

use phasor_acquire::FetchError;
use phasor_kit::SourceKit;
use phasor_mms_conform::MmsDecoder;
use phasor_registry::{Market, Registry, RegistryCatalog};

use crate::sources::mmsdm::MmsdmArchive;
use crate::sources::reports::ReportFolders;

/// The kit's id, as a command names it.
pub const KIT_ID: &str = "nem";

/// Documents AEMO republishes in place that readers join against (the NEM
/// Registration and Exemption List); each distinct version is kept.
pub const DOCUMENTS: &[&str] = &[
    "https://www.aemo.com.au/-/media/files/electricity/nem/Participant_Information/NEM-Registration-and-Exemption-List.xlsx",
];

/// The tables the kit was built with: the schema tree compiled into the binary.
pub fn registry() -> Arc<Registry> {
    Registry::load_embedded()
}

/// The NEM kit over `registry`: the report folders, then the monthly archive, as
/// sources (files known by path before files known by name), the MMS decoder and the catalog.
pub fn kit(registry: Arc<Registry>, base_url: &str) -> Result<SourceKit, FetchError> {
    let reports = ReportFolders::new(base_url, registry.clone())?;
    let archive = MmsdmArchive::new(base_url)?;
    Ok(SourceKit {
        id: KIT_ID,
        sources: vec![Arc::new(reports), Arc::new(archive)],
        decoders: vec![Arc::new(MmsDecoder::new(registry.clone(), Market::Nem))],
        catalog: Arc::new(RegistryCatalog::new(registry, Market::Nem)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kit_has_its_source_decoder_and_catalog() {
        let k = kit(registry(), "https://h").unwrap();
        assert_eq!(k.id, KIT_ID);
        assert_eq!(k.sources[0].id(), crate::REPORTS_SOURCE_ID);
        assert_eq!(k.sources[1].id(), crate::SOURCE_ID);
        assert!(k.decoder_for("A.CSV").is_some());
        assert!(k.decoder_for("A.ctl").is_none());
        assert_eq!(k.decoders[0].id(), "mms");
        assert_eq!(k.catalog.market(), "nem");
        assert!(kit(registry(), "not a url").is_err());
    }
}
