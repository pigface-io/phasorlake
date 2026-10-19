//! The WEM as one kit: its source, and the seam a decoder goes into.

use std::path::Path;
use std::sync::Arc;

use utilake_acquire::FetchError;
use utilake_kit::{Catalog, SourceKit, TableRef, Transform};

use crate::config::WemConfig;
use crate::sources::datafiles::WemDatafiles;

/// The kit's id, as a command names it.
pub const KIT_ID: &str = "wem";

/// The WEM's tables: none are declared yet, so nothing read from a WEM file
/// is current and no table is named.
pub struct WemCatalog;

impl Catalog for WemCatalog {
    fn market(&self) -> &str {
        KIT_ID
    }

    fn tables(&self) -> Vec<Arc<TableRef>> {
        Vec::new()
    }

    fn table(&self, _name: &str) -> Option<Arc<TableRef>> {
        None
    }

    fn plans_current(&self, transforms: &[Transform]) -> bool {
        transforms.is_empty()
    }

    fn with_overlay(&self, _dir: &Path) -> Result<Arc<dyn Catalog>, Box<utilake_core::Diagnostic>> {
        Ok(Arc::new(Self))
    }
}

/// The WEM kit over `config`: the datafiles source, no decoder (an ingest
/// of a WEM file is `config.no_decoder` until the plain-CSV decoder
/// exists), and an empty catalog.
pub fn kit(config: &WemConfig) -> Result<SourceKit, FetchError> {
    let source = WemDatafiles::new(&config.base_url, config.datasets.clone())?;
    Ok(SourceKit {
        id: KIT_ID,
        sources: vec![Arc::new(source)],
        decoders: Vec::new(),
        catalog: Arc::new(WemCatalog),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kit_has_a_source_and_no_decoder() {
        let k = kit(&WemConfig::default()).unwrap();
        assert_eq!(k.id, KIT_ID);
        assert_eq!(k.sources[0].id(), crate::SOURCE_ID);
        assert!(k.decoder_for("facility-scada-2023-10.csv").is_none());
        assert_eq!(k.catalog.market(), "wem");
        assert!(k.catalog.tables().is_empty());
        assert!(k.catalog.table("x").is_none());
        assert!(k.catalog.plans_current(&[]));
        assert!(!k.catalog.plans_current(&[Transform::default()]));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(k.catalog.with_overlay(dir.path()).unwrap().market(), "wem");
        assert!(
            kit(&WemConfig {
                base_url: "not a url".into(),
                datasets: Vec::new()
            })
            .is_err()
        );
    }
}
