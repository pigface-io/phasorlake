//! The NEM's settings, under `[sources.nem]` of the config file.

use phasor_core::config::{Sources, SourcesError};
use serde::{Deserialize, Serialize};

/// The address of nemweb.
pub const DEFAULT_BASE_URL: &str = "https://www.nemweb.com.au";

/// What the NEM kit reads from the config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NemConfig {
    /// Where nemweb is; a mirror or a test server otherwise.
    pub base_url: String,
    /// The documents `fetch --documents` keeps a version of each of
    /// ([`crate::DOCUMENTS`] unless set): AEMO's, a mirror's, or more of one's own.
    pub documents: Vec<String>,
}

impl Default for NemConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            documents: crate::DOCUMENTS.iter().map(|&d| d.to_owned()).collect(),
        }
    }
}

impl NemConfig {
    /// The `[sources.nem]` table, else the legacy `nem_base_url` key, else the defaults.
    pub fn from_sources(sources: &Sources) -> Result<Self, SourcesError> {
        if let Some(c) = sources.kit::<Self>("nem")? {
            return Ok(c);
        }
        Ok(Self {
            base_url: sources
                .nem_base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
            ..Self::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phasor_core::config::Config;

    #[test]
    fn reads_the_kit_table_then_the_earlier_key_then_the_default() {
        let c = Config::parse("[sources.nem]\nbase_url = \"http://a\"\n").unwrap();
        assert_eq!(
            NemConfig::from_sources(&c.sources).unwrap().base_url,
            "http://a"
        );
        let c = Config::parse("[sources]\nnem_base_url = \"http://b\"\n").unwrap();
        assert_eq!(
            NemConfig::from_sources(&c.sources).unwrap().base_url,
            "http://b"
        );
        let c = Config::parse("").unwrap();
        assert_eq!(
            NemConfig::from_sources(&c.sources).unwrap(),
            NemConfig::default()
        );
        // The documents are AEMO's unless named.
        assert_eq!(NemConfig::default().documents, crate::DOCUMENTS);
        let c = Config::parse("[sources.nem]\ndocuments = [\"http://m/a.xlsx\"]\n").unwrap();
        let nem = NemConfig::from_sources(&c.sources).unwrap();
        assert_eq!(nem.documents, ["http://m/a.xlsx"]);
        assert_eq!(nem.base_url, DEFAULT_BASE_URL);
        let c = Config::parse("[sources.nem]\nbase_urll = \"http://a\"\n").unwrap();
        let e = NemConfig::from_sources(&c.sources).unwrap_err();
        assert!(e.to_string().contains("base_urll"), "{e}");
    }
}
