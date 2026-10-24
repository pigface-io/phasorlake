//! The one place that lists the kits this binary reads with.

use std::sync::Arc;

use anyhow::Result;
use utilake_core::config::Config;
use utilake_kit::KitRegistry;
use utilake_mms_registry::Registry;

/// Every kit, each over its own settings from the config file. The kits
/// whose sources know a file by its path (gas, WEM) come before the NEM,
/// whose archive source knows a file by its name alone, so a claim is
/// never taken by the wrong market; the NEM, last, is the kit a file no
/// source claims is read with.
pub fn assemble(config: &Config, registry: Arc<Registry>) -> Result<Arc<KitRegistry>> {
    let settings =
        |e: utilake_core::config::SourcesError| anyhow::anyhow!("{e} (in the config file)");
    let built = |e: utilake_acquire::FetchError| {
        anyhow::anyhow!("{}", utilake_core::Diagnose::diagnostic(&e).render())
    };
    let gas = utilake_gas::GasConfig::from_sources(&config.sources).map_err(settings)?;
    let gas = utilake_gas::kit(registry.clone(), &gas).map_err(built)?;
    let wem = utilake_wem::WemConfig::from_sources(&config.sources).map_err(settings)?;
    let wem = utilake_wem::kit(&wem, registry.clone()).map_err(built)?;
    let nem = utilake_nem::NemConfig::from_sources(&config.sources).map_err(settings)?;
    let nem = utilake_nem::kit(registry, &nem.base_url).map_err(built)?;
    Ok(Arc::new(KitRegistry::new(vec![gas, wem, nem])))
}
