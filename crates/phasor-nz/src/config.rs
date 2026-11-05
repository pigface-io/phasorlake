//! New Zealand's settings, under `[sources.nz]` of the config file.

use phasor_core::config::{Sources, SourcesError};
use serde::{Deserialize, Serialize};

/// The Electricity Authority's blob account, where its EMI datasets are.
/// The container and the folders under it are the `emi` feed's roots.
pub const DEFAULT_BASE_URL: &str = "https://emidatasets.blob.core.windows.net";

/// What the New Zealand kit reads from the config file. The host is a
/// setting because the Authority is retiring EMI and may move its datasets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NzConfig {
    /// Where the blob store is; a mirror or a test server otherwise.
    pub base_url: String,
}

impl Default for NzConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }
}

impl NzConfig {
    /// The `[sources.nz]` table, else the defaults.
    pub fn from_sources(sources: &Sources) -> Result<Self, SourcesError> {
        Ok(sources.kit::<Self>("nz")?.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phasor_core::config::Config;

    #[test]
    fn reads_the_kit_table_else_the_default() {
        let c = Config::parse("[sources.nz]\nbase_url = \"http://a\"\n").unwrap();
        assert_eq!(
            NzConfig::from_sources(&c.sources).unwrap().base_url,
            "http://a"
        );
        let c = Config::parse("").unwrap();
        assert_eq!(
            NzConfig::from_sources(&c.sources).unwrap(),
            NzConfig::default()
        );
        assert_eq!(NzConfig::default().base_url, DEFAULT_BASE_URL);
        let c = Config::parse("[sources.nz]\nbase = 1\n").unwrap();
        assert!(NzConfig::from_sources(&c.sources).is_err());
    }
}
