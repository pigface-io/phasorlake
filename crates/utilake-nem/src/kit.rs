//! The NEM as one kit.

use std::sync::Arc;

use utilake_acquire::FetchError;
use utilake_kit::SourceKit;
use utilake_mms_conform::{MmsCatalog, MmsDecoder};
use utilake_mms_registry::Registry;

use crate::sources::mmsdm::MmsdmArchive;
use crate::sources::reports::ReportFolders;

/// The kit's id, as a command names it.
pub const KIT_ID: &str = "nem";

/// The tables the kit was built with: the schema tree compiled into the
/// binary. It stays in the registry crate until a second MMS market needs
/// a tree of its own.
pub fn registry() -> Arc<Registry> {
    Registry::load_embedded()
}

/// The NEM kit over `registry`, listing nemweb at `base_url`: the report
/// folders and the monthly archive as its sources, the MMS decoder, and
/// the registry's catalog. The report folders come first: their files are
/// known by their path, the archive's by their name alone.
pub fn kit(registry: Arc<Registry>, base_url: &str) -> Result<SourceKit, FetchError> {
    let reports = ReportFolders::new(base_url, registry.clone())?;
    let archive = MmsdmArchive::new(base_url)?;
    Ok(SourceKit {
        id: KIT_ID,
        sources: vec![Arc::new(reports), Arc::new(archive)],
        decoders: vec![Arc::new(MmsDecoder::new(registry.clone()))],
        catalog: Arc::new(MmsCatalog::new(registry)),
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
