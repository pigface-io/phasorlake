//! The gas markets as one kit.

use std::sync::Arc;

use phasor_acquire::FeedFolders;
use phasor_acquire::FetchError;
use phasor_kit::Source;
use phasor_kit::SourceKit;
use phasor_kit::{Serves, TableOf};
use phasor_plain_decode::feeds::feeds_of;
use phasor_registry::{Market, Registry, RegistryCatalog, catalog};

use crate::config::GasConfig;
use crate::decoder::GasCsv;

/// The kit's id, as a command names it.
pub const KIT_ID: &str = "gas";

/// The source ids, as the manifest records them, by feed: the Gas
/// Bulletin Board, the Gas Supply Hub, the Short Term Trading Market and
/// the Declared Wholesale Gas Market.
pub const SOURCE_IDS: [(&str, &str); 4] = [
    ("gbb", "gas-gbb"),
    ("gsh", "gas-gsh"),
    ("sttm", "gas-sttm"),
    ("dwgm", "gas-dwgm"),
];

/// The source id of a gas feed.
pub fn source_id(feed: &str) -> Option<&'static str> {
    SOURCE_IDS
        .iter()
        .find(|(f, _)| *f == feed)
        .map(|(_, id)| *id)
}

/// The gas kit over `registry`, listing nemweb at `config.base_url`: one
/// source per feed, the feed-aware `.csv` reader, and the registry's gas
/// tables as the catalog.
pub fn kit(registry: Arc<Registry>, config: &GasConfig) -> Result<SourceKit, FetchError> {
    let table_of: TableOf = catalog::table_of(registry.clone(), Market::Gas);
    let mut sources: Vec<Arc<dyn Source>> = Vec::new();
    for feed in feeds_of("gas") {
        let id = source_id(feed.id).expect("every gas feed has a source id");
        // A feed serves the tables the census saw in its folders
        // (folders.json): a table seen in none is served by no feed.
        let serves: Serves = catalog::serves_feed(registry.clone(), feed.id);
        sources.push(Arc::new(
            FeedFolders::new(id, &config.base_url, feed, table_of.clone())?
                .with_tables_served(serves),
        ));
    }
    Ok(SourceKit {
        id: KIT_ID,
        sources,
        decoders: vec![Arc::new(GasCsv::new(&registry))],
        catalog: Arc::new(RegistryCatalog::new(registry, Market::Gas)),
    })
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;

    #[test]
    fn the_kit_has_a_source_per_feed_the_csv_reader_and_the_gas_catalog() {
        let k = kit(Registry::load_embedded(), &GasConfig::default()).unwrap();
        assert_eq!(k.id, KIT_ID);
        let ids: Vec<&str> = k.sources.iter().map(|s| s.id()).collect();
        assert_eq!(ids, ["gas-gsh", "gas-gbb", "gas-sttm", "gas-dwgm"]);
        assert_eq!(source_id("dwgm"), Some("gas-dwgm"));
        assert_eq!(source_id("nem"), None);
        assert!(k.decoder_for("GasBBActualFlowStorage.CSV").is_some());
        assert!(k.decoder_for("x.json").is_none());
        assert_eq!(k.catalog.market(), "gas");
        assert!(k.catalog.tables().len() > 100);
        assert!(k.catalog.table("GBB_ACTUAL_FLOW_STORAGE").is_some());
        // A GBB file is claimed by the GBB source, with its table.
        let url = Url::parse(
            "https://www.nemweb.com.au/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV",
        )
        .unwrap();
        let claimed = k.sources.iter().find_map(|s| s.describe(&url)).unwrap();
        assert_eq!(claimed.source, "gas-gbb");
        assert_eq!(claimed.table.as_deref(), Some("GBB_ACTUAL_FLOW_STORAGE"));
        // Each feed serves its own tables.
        let gbb = k.sources.iter().find(|s| s.id() == "gas-gbb").unwrap();
        let gsh = k.sources.iter().find(|s| s.id() == "gas-gsh").unwrap();
        assert!(gbb.serves("gbb_actual_flow_storage") && !gsh.serves("GBB_ACTUAL_FLOW_STORAGE"));
        assert!(gsh.serves("GSH_BENCHMARK_PRICE") && !gbb.serves("GSH_BENCHMARK_PRICE"));
        // The GSH trades file sits in the GBB folder: the census says so.
        assert!(
            gbb.serves("GAS_GSH_TRADES"),
            "folders.json puts it under GBB"
        );
        assert!(!gbb.serves("NO_SUCH_TABLE"));
        assert!(
            kit(
                Registry::load_embedded(),
                &GasConfig {
                    base_url: "not a url".into()
                }
            )
            .is_err()
        );
    }
}
