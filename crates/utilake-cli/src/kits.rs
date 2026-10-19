//! The one place that lists the kits this binary reads with.

use std::sync::Arc;

use anyhow::Result;
use utilake_core::config::Config;
use utilake_kit::KitRegistry;
use utilake_mms_registry::Registry;

/// Every kit, each over its own settings from the config file.
pub fn assemble(config: &Config, registry: Arc<Registry>) -> Result<Arc<KitRegistry>> {
    let nem = utilake_nem::NemConfig::from_sources(&config.sources)
        .map_err(|e| anyhow::anyhow!("{e} (in the config file)"))?;
    let nem = utilake_nem::kit(registry, &nem.base_url)
        .map_err(|e| anyhow::anyhow!("{}", utilake_core::Diagnose::diagnostic(&e).render()))?;
    Ok(Arc::new(KitRegistry::new(vec![nem])))
}
