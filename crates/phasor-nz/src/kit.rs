//! New Zealand as one kit: the `emi` feed's folders as two sources, one for
//! the files published a day at a time and one for those published a month
//! at a time, each claiming the files its cadence names, so a `fetch` takes
//! whole months from the monthly files and the rest from the daily ones.

use std::sync::Arc;

use phasor_acquire::{Claim, FeedFolders, FetchError};
use phasor_kit::{Granularity, Source, SourceKit, TableOf};
use phasor_mms_conform::PlainCsvDecoder;
use phasor_plain_decode::feeds::feed;
use phasor_plain_decode::names::{Cadence, name_cadence};
use phasor_registry::{Market, Registry, RegistryCatalog, catalog};
use url::Url;

use crate::config::NzConfig;

/// The kit's id, as a command names it.
pub const KIT_ID: &str = "nz";

/// The source id of the files published a day at a time.
pub const DAILY_SOURCE_ID: &str = "nz-emi-daily";

/// The source id of the files published a month at a time.
pub const MONTHLY_SOURCE_ID: &str = "nz-emi-monthly";

/// Whether a file's name says it is published at `cadence`.
fn claims(cadence: Cadence) -> Claim {
    Arc::new(move |u: &Url| name_cadence(u.path()) == Some(cadence))
}

/// The New Zealand kit: the EMI feed's daily and monthly files as sources,
/// each serving the tables the census saw published at its cadence, the
/// plain CSV decoder, the registry's New Zealand tables as the catalog.
pub fn kit(config: &NzConfig, registry: Arc<Registry>) -> Result<SourceKit, FetchError> {
    let table_of: TableOf = catalog::table_of(registry.clone(), Market::Nz);
    let emi = feed("emi").expect("the feeds table names the EMI datasets");
    let daily = FeedFolders::new(DAILY_SOURCE_ID, &config.base_url, emi, table_of.clone())?
        .with_granularity(Granularity::Day)
        .with_claim(claims(Cadence::Day))
        .with_versioned_listing()
        .with_folders_of(catalog::folders_of(registry.clone(), emi.id))
        .with_tables_served(catalog::serves_cadence(registry.clone(), emi.id, "day"));
    let monthly = FeedFolders::new(MONTHLY_SOURCE_ID, &config.base_url, emi, table_of)?
        .with_claim(claims(Cadence::Month))
        .with_versioned_listing()
        .with_folders_of(catalog::folders_of(registry.clone(), emi.id))
        .with_tables_served(catalog::serves_cadence(registry.clone(), emi.id, "month"));
    let sources: Vec<Arc<dyn Source>> = vec![Arc::new(daily), Arc::new(monthly)];
    Ok(SourceKit {
        id: KIT_ID,
        sources,
        decoders: vec![Arc::new(PlainCsvDecoder::new(registry.clone(), emi))],
        catalog: Arc::new(RegistryCatalog::new(registry, Market::Nz)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kit_has_its_sources_decoder_and_catalog() {
        let k = kit(&NzConfig::default(), Registry::load_embedded()).unwrap();
        assert_eq!(k.id, KIT_ID);
        let ids: Vec<&str> = k.sources.iter().map(|s| s.id()).collect();
        assert_eq!(ids, [DAILY_SOURCE_ID, MONTHLY_SOURCE_ID]);
        assert_eq!(
            k.decoder_for("20261103_FinalEnergyPrices.csv")
                .unwrap()
                .id(),
            "csv"
        );
        assert!(k.decoder_for("x.zip").is_none());
        assert_eq!(k.catalog.market(), "nz");
        assert!(k.catalog.table("x").is_none());
        let (daily, monthly) = (&k.sources[0], &k.sources[1]);
        assert_eq!(daily.granularity(), Granularity::Day);
        assert_eq!(monthly.granularity(), Granularity::Month);
        // Each claims the files its cadence names, under the feed's roots.
        let fep = "https://emidatasets.blob.core.windows.net/publicdata/Datasets/Wholesale/DispatchAndPricing/FinalEnergyPrices";
        let day = Url::parse(&format!("{fep}/20261103_FinalEnergyPrices.csv")).unwrap();
        let month = Url::parse(&format!("{fep}/ByMonth/202610_FinalEnergyPrices.csv")).unwrap();
        assert_eq!(
            daily.describe(&day).unwrap().period.unwrap().to_string(),
            "2026-11-03"
        );
        assert!(daily.describe(&month).is_none());
        assert_eq!(
            monthly
                .describe(&month)
                .unwrap()
                .period
                .unwrap()
                .to_string(),
            "2026-10"
        );
        assert!(monthly.describe(&day).is_none());
        assert!(
            daily
                .describe(&Url::parse("https://h/elsewhere/x.csv").unwrap())
                .is_none()
        );
        // The census saw the price tables in a daily folder and a ByMonth
        // one, so both sources serve them; a table of another market, neither.
        assert!(daily.serves("NZ_FINAL_ENERGY_PRICES"));
        assert!(monthly.serves("nz_final_reserve_prices"));
        assert!(!daily.serves("DISPATCH_UNIT_SCADA") && !monthly.serves("NO_SUCH_TABLE"));
        // The generation and metered tables come a month a file, so the
        // monthly source alone serves them.
        assert!(monthly.serves("NZ_GRID_EXPORT") && !daily.serves("NZ_GRID_EXPORT"));
        assert_eq!(k.catalog.tables().len(), 23);
        let prices = k.catalog.table("NZ_FINAL_ENERGY_PRICES").unwrap();
        assert_eq!(prices.utc_offset_secs, 43_200);
        assert!(prices.time_column.is_some());
        assert!(
            kit(
                &NzConfig {
                    base_url: "not a url".into()
                },
                Registry::load_embedded()
            )
            .is_err()
        );
    }
}
