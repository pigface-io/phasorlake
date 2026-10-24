//! The gas kit's settings, under `[sources.gas]` of the config file.

use serde::{Deserialize, Serialize};
use utilake_core::config::{Sources, SourcesError};

/// The address of nemweb, where AEMO publishes the gas markets' reports.
pub const DEFAULT_BASE_URL: &str = "https://www.nemweb.com.au";

/// What the gas kit reads from the config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GasConfig {
    /// Where nemweb is; a mirror or a test server otherwise.
    pub base_url: String,
}

impl Default for GasConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }
}

impl GasConfig {
    /// The `[sources.gas]` table, else the defaults.
    pub fn from_sources(sources: &Sources) -> Result<Self, SourcesError> {
        Ok(sources.kit::<Self>("gas")?.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use utilake_core::config::Config;

    #[test]
    fn reads_the_kit_table_else_the_default() {
        let c = Config::parse("[sources.gas]\nbase_url = \"http://a\"\n").unwrap();
        assert_eq!(
            GasConfig::from_sources(&c.sources).unwrap().base_url,
            "http://a"
        );
        let c = Config::parse("jobs = 1\n").unwrap();
        assert_eq!(
            GasConfig::from_sources(&c.sources).unwrap(),
            GasConfig::default()
        );
        let c = Config::parse("[sources.gas]\nbase_url = 1\n").unwrap();
        assert!(GasConfig::from_sources(&c.sources).is_err());
        let c = Config::parse("[sources.gas]\nother = 1\n").unwrap();
        assert!(GasConfig::from_sources(&c.sources).is_err());
    }
}
