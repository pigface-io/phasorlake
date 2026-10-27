//! The WEM's settings, under `[sources.wem]` of the config file.

use phasor_core::config::{Sources, SourcesError};
use serde::{Deserialize, Serialize};

/// The address of AEMO's WEM data site.
pub const DEFAULT_BASE_URL: &str = "https://data.wa.aemo.com.au";

/// What the WEM kit reads from the config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WemConfig {
    /// Where the data site is; a mirror or a test server otherwise.
    pub base_url: String,
    /// The datasets the kit lists, e.g. `facility-scada`.
    pub datasets: Vec<String>,
}

impl Default for WemConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            datasets: Vec::new(),
        }
    }
}

impl WemConfig {
    /// The `[sources.wem]` table, else the `wem_base_url` key the file held
    /// under the tool's earlier name, else the defaults.
    pub fn from_sources(sources: &Sources) -> Result<Self, SourcesError> {
        if let Some(c) = sources.kit::<Self>("wem")? {
            return Ok(c);
        }
        Ok(Self {
            base_url: sources
                .wem_base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
            datasets: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phasor_core::config::Config;

    #[test]
    fn reads_the_kit_table_then_the_earlier_key_then_the_default() {
        let c = Config::parse(
            "[sources.wem]\nbase_url = \"http://a\"\ndatasets = [\"facility-scada\"]\n",
        )
        .unwrap();
        let w = WemConfig::from_sources(&c.sources).unwrap();
        assert_eq!((w.base_url.as_str(), w.datasets.len()), ("http://a", 1));
        let c = Config::parse("[sources]\nwem_base_url = \"http://b\"\n").unwrap();
        assert_eq!(
            WemConfig::from_sources(&c.sources).unwrap().base_url,
            "http://b"
        );
        let c = Config::parse("").unwrap();
        assert_eq!(
            WemConfig::from_sources(&c.sources).unwrap(),
            WemConfig::default()
        );
        let c = Config::parse("[sources.wem]\nbase = 1\n").unwrap();
        assert!(WemConfig::from_sources(&c.sources).is_err());
    }
}
