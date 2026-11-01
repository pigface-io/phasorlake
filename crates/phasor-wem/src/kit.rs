//! The WEM as one kit: its datafiles and WEMDE documents, read by the plain decoders.

use std::sync::Arc;

use phasor_acquire::FeedFolders;
use phasor_acquire::FetchError;
use phasor_kit::Source;
use phasor_kit::SourceKit;
use phasor_kit::{Serves, TableOf};
use phasor_mms_conform::{PlainCsvDecoder, WemdeDecoder};
use phasor_plain_decode::BlockName;
use phasor_plain_decode::feeds::feed;
use phasor_registry::{Market, Registry, RegistryCatalog, catalog};

use crate::config::WemConfig;
use crate::sources::datafiles::{DatasetsOf, WemDatafiles};

/// The kit's id, as a command names it.
pub const KIT_ID: &str = "wem";

/// The source id of the WEMDE documents.
pub const WEMDE_SOURCE_ID: &str = "wem-wemde";

/// The dataset folders the census saw a table's blocks in under the datafiles
/// feed (folders.json): every dataset of the feed for `None`.
pub fn datasets_of(registry: Arc<Registry>) -> DatasetsOf {
    Arc::new(move |table: Option<&str>| {
        let folders: Vec<&phasor_registry::Folder> = match table {
            None => registry.folders().iter().collect(),
            Some(t) => registry
                .table_by_name(&t.to_ascii_uppercase())
                .map(|id| registry.folders_of(id))
                .unwrap_or_default(),
        };
        let mut out: Vec<String> = folders
            .into_iter()
            .filter(|f| f.feed == "datafiles")
            .map(|f| f.folder.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    })
}

/// The table that claims a block of the WEMDE documents: the one declared for
/// its pair, else, for a document's zip named by its dataset alone
/// (`referenceTradingPrice`), the first table declared for an array of it
/// (`REFERENCE_TRADING_PRICE_REFERENCE_TRADING_PRICES`). A document with
/// several arrays is listed under the first; once read, each array's block
/// goes to its own table.
pub fn document_table_of(registry: Arc<Registry>) -> TableOf {
    let exact = catalog::table_of(registry.clone(), Market::Wem);
    Arc::new(move |b: &BlockName| {
        exact(b).or_else(|| {
            let prefix = format!("{}_", b.name);
            let mut names: Vec<String> = registry
                .tables()
                .filter(|(_, t)| t.market == Market::Wem)
                .filter(|(_, t)| {
                    t.report_pairs
                        .iter()
                        .any(|(family, name)| *family == b.family && name.starts_with(&prefix))
                })
                .map(|(_, t)| t.name.clone())
                .collect();
            names.sort();
            names.into_iter().next()
        })
    })
}

/// The WEM kit: datafiles and WEMDE `previous/` zips as sources, each serving
/// the tables the census saw in its folders, the plain CSV and WEMDE decoders,
/// the registry's WEM tables as the catalog.
pub fn kit(config: &WemConfig, registry: Arc<Registry>) -> Result<SourceKit, FetchError> {
    let table_of: TableOf = catalog::table_of(registry.clone(), Market::Wem);
    let datafiles = feed("datafiles").expect("the feeds table names the WEM datafiles");
    let wemde = feed("wemde").expect("the feeds table names WEMDE");
    let serves_datafiles: Serves = catalog::serves_feed(registry.clone(), datafiles.id);
    let serves_wemde: Serves = catalog::serves_feed(registry.clone(), wemde.id);
    let files = WemDatafiles::new(&config.base_url, config.datasets.clone())?
        .with_tables(table_of.clone())?
        .with_datasets_of(datasets_of(registry.clone()))
        .with_tables_served(serves_datafiles);
    let documents = FeedFolders::new(
        WEMDE_SOURCE_ID,
        &config.base_url,
        wemde,
        document_table_of(registry.clone()),
    )?
    .with_tables_served(serves_wemde);
    let sources: Vec<Arc<dyn Source>> = vec![Arc::new(files), Arc::new(documents)];
    Ok(SourceKit {
        id: KIT_ID,
        sources,
        decoders: vec![
            Arc::new(PlainCsvDecoder::new(registry.clone(), datafiles)),
            Arc::new(WemdeDecoder::new(registry.clone(), wemde)),
        ],
        catalog: Arc::new(RegistryCatalog::new(registry, Market::Wem)),
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
        assert!(k.catalog.table("x").is_none());
        // Each source serves the tables the census saw in its folders, and
        // the datafiles source lists a table's datasets by them.
        let (files, documents) = (&k.sources[0], &k.sources[1]);
        let prices = k.catalog.table("WEM_MARKET_CLEARING_PRICES").unwrap();
        let reference = k.catalog.table("WEMDE_REFERENCE_TRADING_PRICE").unwrap();
        assert!(files.serves(&prices.name) && !documents.serves(&prices.name));
        assert!(documents.serves(&reference.name) && !files.serves(&reference.name));
        assert!(!files.serves("NO_SUCH_TABLE") && !documents.serves("NO_SUCH_TABLE"));
        let datasets = datasets_of(Registry::load_embedded());
        assert_eq!(
            datasets(Some("wem_market_clearing_prices")),
            ["market-clearing-prices-csv"]
        );
        assert!(datasets(None).contains(&"facilities".to_owned()));
        assert!(datasets(None).len() > 1);
        assert!(datasets(Some("NO_SUCH_TABLE")).is_empty());
        // A document's zip is named by its dataset alone; its table is the
        // one declared for an array of it.
        let of = document_table_of(Registry::load_embedded());
        let block = |name: &str| BlockName {
            family: "WEMDE".into(),
            name: name.into(),
            version: 1,
        };
        assert_eq!(
            of(&block("REFERENCE_TRADING_PRICE")).as_deref(),
            Some("WEMDE_REFERENCE_TRADING_PRICE")
        );
        assert_eq!(
            of(&block("REFERENCE_TRADING_PRICE_REFERENCE_TRADING_PRICES")).as_deref(),
            Some("WEMDE_REFERENCE_TRADING_PRICE")
        );
        assert!(of(&block("NO_SUCH_DATASET")).is_none());
        assert_eq!(
            documents
                .describe(&Url::parse("https://data.wa.aemo.com.au/public/market-data/wemde/referenceTradingPrice/previous/ReferenceTradingPrice_20261021.zip").unwrap())
                .unwrap()
                .table
                .as_deref(),
            Some("WEMDE_REFERENCE_TRADING_PRICE")
        );
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
