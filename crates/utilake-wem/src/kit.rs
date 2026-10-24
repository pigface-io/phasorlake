//! The WEM as one kit: its datafiles and WEMDE documents as sources, read
//! by the plain decoders, over the registry's WEM tables.

use std::sync::Arc;

use utilake_acquire::{FetchError, Source};
use utilake_kit::{FeedFolders, SourceKit, TableOf};
use utilake_mms_conform::{MmsCatalog, PlainCsvDecoder, WemdeDecoder};
use utilake_mms_registry::{Market, Registry};
use utilake_plain_decode::BlockName;
use utilake_plain_decode::feeds::feed;

use crate::config::WemConfig;
use crate::sources::datafiles::WemDatafiles;

/// The kit's id, as a command names it.
pub const KIT_ID: &str = "wem";

/// The source id of the WEMDE documents.
pub const WEMDE_SOURCE_ID: &str = "wem-wemde";

/// The WEM kit over `config` and `registry`: the datafiles (listed by
/// their manifests) and the WEMDE `previous/` zips as sources, the plain
/// CSV and WEMDE decoders, and the registry's WEM tables as the catalog.
pub fn kit(config: &WemConfig, registry: Arc<Registry>) -> Result<SourceKit, FetchError> {
    let table_of: TableOf = {
        let registry = registry.clone();
        Arc::new(move |b: &BlockName| {
            registry
                .table_by_pair(Market::Wem, &b.family, &b.name)
                .map(|id| registry.table(id).name.clone())
        })
    };
    let datafiles = feed("datafiles").expect("the feeds table names the WEM datafiles");
    let wemde = feed("wemde").expect("the feeds table names WEMDE");
    let files = WemDatafiles::new(&config.base_url, config.datasets.clone())?
        .with_tables(table_of.clone())?;
    let documents = FeedFolders::new(WEMDE_SOURCE_ID, &config.base_url, wemde, table_of)?;
    let sources: Vec<Arc<dyn Source>> = vec![Arc::new(files), Arc::new(documents)];
    Ok(SourceKit {
        id: KIT_ID,
        sources,
        decoders: vec![
            Arc::new(PlainCsvDecoder::new(registry.clone(), datafiles)),
            Arc::new(WemdeDecoder::new(registry.clone(), wemde)),
        ],
        catalog: Arc::new(MmsCatalog::new(registry, Market::Wem)),
    })
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;

    #[test]
    fn the_kit_has_its_sources_decoders_and_catalog() {
        let k = kit(&WemConfig::default(), Registry::load_embedded()).unwrap();
        assert_eq!(k.id, KIT_ID);
        let ids: Vec<&str> = k.sources.iter().map(|s| s.id()).collect();
        assert_eq!(ids, [crate::SOURCE_ID, WEMDE_SOURCE_ID]);
        assert_eq!(
            k.decoder_for("facility-scada-2023-10.csv").unwrap().id(),
            "csv"
        );
        assert_eq!(
            k.decoder_for("ReferenceTradingPrice_20261021.json")
                .unwrap()
                .id(),
            "json"
        );
        assert!(k.decoder_for("x.zip").is_none());
        assert_eq!(k.catalog.market(), "wem");
        // No WEM table is published yet: the catalog is empty, a block is
        // derived.
        assert!(k.catalog.tables().is_empty());
        assert!(k.catalog.table("x").is_none());
        let claimed = |u: &str| {
            k.sources
                .iter()
                .find_map(|s| s.describe(&Url::parse(u).unwrap()))
        };
        assert_eq!(
            claimed("https://data.wa.aemo.com.au/public/public-data/datafiles/facility-scada/facility-scada-2023-10.csv")
                .unwrap()
                .source,
            crate::SOURCE_ID
        );
        assert_eq!(
            claimed("https://data.wa.aemo.com.au/public/market-data/wemde/referenceTradingPrice/previous/ReferenceTradingPrice_20261021.zip")
                .unwrap()
                .source,
            WEMDE_SOURCE_ID
        );
        assert!(claimed("https://data.wa.aemo.com.au/public/market-data/wemde/referenceTradingPrice/current/x.json").is_none());
        assert!(
            kit(
                &WemConfig {
                    base_url: "not a url".into(),
                    datasets: Vec::new()
                },
                Registry::load_embedded()
            )
            .is_err()
        );
    }
}
