//! A whole set of markets and tables as one postcard blob, written by the
//! registry's build script and embedded in the binary.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file_model::error::SchemaFileError;
use crate::file_model::model::{Market, MarketDef, TableDef};
use crate::file_model::validate::{Issue, validate_set};

/// Everything in a `schemas/` tree.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaSet {
    /// One per market directory.
    pub markets: Vec<MarketDef>,
    /// Every table, sorted by name.
    pub tables: Vec<TableDef>,
}

/// Directories under a market that hold the census's metadata, not table
/// files: which shape each file of a month had, and every shape of a table.
pub const CENSUS_DIRS: [&str; 2] = ["census", "shapes"];

/// Whether `path` (relative to `schemas/`, `/`-separated) is a table file.
pub fn is_table_path(path: &str) -> bool {
    let mut segments: Vec<&str> = path.split('/').collect();
    let name = segments.pop().unwrap_or(path);
    name.ends_with(".toml")
        && name != "market.toml"
        && !name.starts_with('_')
        && !name.starts_with("DRAFT_")
        && !segments.iter().any(|s| CENSUS_DIRS.contains(s))
}

impl SchemaSet {
    /// Reads `schemas/{market}/market.toml` and `schemas/{market}/**/*.toml`.
    ///
    /// Files whose name starts with `_` or `DRAFT_` are skipped: they are
    /// conflict reports and uncommitted drafts. So are the census's
    /// directories ([`CENSUS_DIRS`]).
    pub fn read_dir(root: &Path) -> Result<Self, SchemaFileError> {
        let mut set = Self::default();
        let mut market_dirs: Vec<_> = std::fs::read_dir(root)
            .map_err(|source| SchemaFileError::Read {
                path: root.display().to_string(),
                source,
            })?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        market_dirs.sort();
        for dir in market_dirs {
            let mf = dir.join("market.toml");
            if mf.is_file() {
                set.markets.push(crate::file_model::file::read_market(&mf)?);
            }
            let mut stack = vec![dir];
            while let Some(d) = stack.pop() {
                let mut entries: Vec<_> = std::fs::read_dir(&d)
                    .map_err(|source| SchemaFileError::Read {
                        path: d.display().to_string(),
                        source,
                    })?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .collect();
                entries.sort();
                for p in entries {
                    if p.is_dir() {
                        let census = p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| CENSUS_DIRS.contains(&n));
                        if !census {
                            stack.push(p);
                        }
                        continue;
                    }
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if p.extension().is_some_and(|e| e == "toml")
                        && name != "market.toml"
                        && !name.starts_with('_')
                        && !name.starts_with("DRAFT_")
                    {
                        set.tables.push(crate::file_model::file::read_table(&p)?);
                    }
                }
            }
        }
        set.tables.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(set)
    }

    /// Every issue in the set, per table then across tables; empty when
    /// valid. This is the bar for the committed registry.
    pub fn validate(&self) -> Vec<Issue> {
        let mut out = self.validate_structure();
        out.extend(
            self.tables
                .iter()
                .flat_map(crate::file_model::validate::uncurated),
        );
        out
    }

    /// The issues that make a set unusable, leaving out the demand that
    /// every column be curated. This is the bar for a user's own tables.
    pub fn validate_structure(&self) -> Vec<Issue> {
        let mut out: Vec<Issue> = self
            .tables
            .iter()
            .flat_map(crate::file_model::validate::validate_structure)
            .collect();
        out.extend(validate_set(&self.tables));
        let markets: BTreeMap<Market, &MarketDef> =
            self.markets.iter().map(|m| (m.market, m)).collect();
        for t in &self.tables {
            if !markets.contains_key(&t.market) {
                out.push(Issue {
                    table: t.name.clone(),
                    message: format!("market {} has no market.toml", t.market),
                });
            }
        }
        if markets.len() != self.markets.len() {
            out.push(Issue {
                table: String::new(),
                message: "a market is defined twice".into(),
            });
        }
        out
    }

    /// Encodes as postcard.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("a SchemaSet always serialises")
    }

    /// Decodes a postcard blob.
    pub fn decode(bytes: &[u8]) -> Result<Self, SchemaFileError> {
        postcard::from_bytes(bytes).map_err(SchemaFileError::Blob)
    }

    /// The market definition for `m`.
    pub fn market(&self, m: Market) -> Option<&MarketDef> {
        self.markets.iter().find(|d| d.market == m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::file::tests::unit_scada;
    use crate::file_model::file::{write_market, write_table};
    use crate::file_model::model::TimestampFormat;

    fn nem() -> MarketDef {
        MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
        }
    }

    #[test]
    fn reads_tree_skips_drafts_and_round_trips_blob() {
        let dir = tempfile::tempdir().unwrap();
        let nem_dir = dir.path().join("nem").join("mms");
        std::fs::create_dir_all(&nem_dir).unwrap();
        std::fs::write(
            dir.path().join("nem").join("market.toml"),
            write_market(&nem()),
        )
        .unwrap();
        std::fs::write(
            nem_dir.join("DISPATCH_UNIT_SCADA.toml"),
            write_table(&unit_scada()),
        )
        .unwrap();
        std::fs::write(nem_dir.join("DRAFT_X.toml"), "garbage").unwrap();
        std::fs::write(nem_dir.join("_conflicts.toml"), "garbage").unwrap();
        std::fs::write(nem_dir.join("notes.md"), "text").unwrap();
        for d in CENSUS_DIRS {
            let census = dir.path().join("nem").join(d);
            std::fs::create_dir_all(&census).unwrap();
            std::fs::write(census.join("2026-08.toml"), "\"A.zip\" = \"0\"").unwrap();
        }
        assert!(is_table_path("nem/mms/REGION.toml"));
        assert!(!is_table_path("nem/census/2026-08.toml"));
        assert!(!is_table_path("nem/shapes/REGION.toml"));
        let set = SchemaSet::read_dir(dir.path()).unwrap();
        assert_eq!(set.tables.len(), 1);
        assert_eq!(set.markets, vec![nem()]);
        assert!(set.validate().is_empty(), "{:?}", set.validate());
        let bytes = set.encode();
        let back = SchemaSet::decode(&bytes).unwrap();
        assert_eq!(back, set);
        assert!(SchemaSet::decode(&bytes[..3]).is_err());
        assert_eq!(set.market(Market::Nem), Some(&nem()));
        assert_eq!(set.market(Market::Wem), None);
    }

    #[test]
    fn missing_market_and_duplicate_market_are_issues() {
        let set = SchemaSet {
            markets: vec![],
            tables: vec![unit_scada()],
        };
        assert!(
            set.validate()
                .iter()
                .any(|i| i.message.contains("no market.toml"))
        );
        let set = SchemaSet {
            markets: vec![nem(), nem()],
            tables: vec![],
        };
        assert!(
            set.validate()
                .iter()
                .any(|i| i.message.contains("defined twice"))
        );
    }

    #[test]
    fn read_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SchemaSet::read_dir(&dir.path().join("none")).is_err());
        let bad = dir.path().join("nem");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("T.toml"), "nope = 1").unwrap();
        assert!(matches!(
            SchemaSet::read_dir(dir.path()),
            Err(SchemaFileError::Parse { .. })
        ));
    }
}
