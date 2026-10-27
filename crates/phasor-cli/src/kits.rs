//! The one place that lists the kits this binary reads with.

use std::sync::Arc;

use anyhow::Result;
use phasor_core::config::Config;
use phasor_kit::KitRegistry;
use phasor_registry::Registry;

/// Every kit, each over its own settings from the config file. The kits
/// whose sources know a file by its path (gas, WEM) come before the NEM,
/// whose archive source knows a file by its name alone, so a claim is
/// never taken by the wrong market; the NEM, last, is the kit a file no
/// source claims is read with.
pub fn assemble(config: &Config, registry: Arc<Registry>) -> Result<Arc<KitRegistry>> {
    let settings =
        |e: phasor_core::config::SourcesError| anyhow::anyhow!("{e} (in the config file)");
    let built = |e: phasor_acquire::FetchError| {
        anyhow::anyhow!("{}", phasor_core::Diagnose::diagnostic(&e).render())
    };
    let gas = phasor_gas::GasConfig::from_sources(&config.sources).map_err(settings)?;
    let gas = phasor_gas::kit(registry.clone(), &gas).map_err(built)?;
    let wem = phasor_wem::WemConfig::from_sources(&config.sources).map_err(settings)?;
    let wem = phasor_wem::kit(&wem, registry.clone()).map_err(built)?;
    let nem = phasor_nem::NemConfig::from_sources(&config.sources).map_err(settings)?;
    let nem = phasor_nem::kit(registry, &nem.base_url).map_err(built)?;
    Ok(Arc::new(KitRegistry::new(vec![gas, wem, nem])))
}
