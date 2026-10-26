//! A whole `schemas/` tree as one postcard blob, written by the registry's
//! build script and embedded in the binary.
//!
//! On disk a market is `schemas/{market}/market.toml` and its tables: the
//! ones pulled from utilake-schemas under `pulled/`
//! (`{TABLE}/metadata/v1.metadata.json`, held to their manifest by
//! `just schemas-check`), and, in a user's own schemas, a directory per
//! table with a hand-written `table.toml` or a `metadata/v1.metadata.json`
//! of the same form as a pulled one.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file_model::doc::{DOC_FILE, read_doc};
use crate::file_model::error::SchemaFileError;
use crate::file_model::file::{TABLE_FILE, read_market, read_table};
use crate::file_model::folders::{FOLDERS_FILE, Folder, read_folders};
use crate::file_model::model::{Market, MarketDef, Table};
use crate::file_model::validate::{Issue, issue, validate_set, validate_table};

/// The directory of a market that holds the tables pulled from
/// utilake-schemas.
pub const PULLED_DIR: &str = "pulled";

/// Everything in a `schemas/` tree.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaSet {
    /// One per market directory.
    pub markets: Vec<MarketDef>,
    /// Every table, sorted by name.
    pub tables: Vec<Table>,
    /// Which report folder carries each block, from the markets'
    /// `pulled/folders.json`; empty until one is pulled.
    #[serde(default)]
    pub folders: Vec<Folder>,
}

fn read_err(path: &Path, source: std::io::Error) -> SchemaFileError {
    SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    }
}

fn sorted_dirs(dir: &Path) -> Result<Vec<std::path::PathBuf>, SchemaFileError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| read_err(dir, e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    Ok(entries)
}

impl SchemaSet {
    /// Reads every market directory under `root`.
    pub fn read_dir(root: &Path) -> Result<Self, SchemaFileError> {
        let mut set = Self::default();
        for dir in sorted_dirs(root)? {
            let mf = dir.join("market.toml");
            let market = if mf.is_file() {
                let m = read_market(&mf)?;
                set.markets.push(m.clone());
                m.market
            } else {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(Market::parse)
                    .unwrap_or(Market::Nem)
            };
            set.read_market_dir(&dir, market)?;
        }
        set.tables.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(set)
    }

    fn read_market_dir(&mut self, dir: &Path, market: Market) -> Result<(), SchemaFileError> {
        for p in sorted_dirs(dir)? {
            if p.file_name().is_some_and(|n| n == PULLED_DIR) {
                for t in sorted_dirs(&p)? {
                    self.tables.extend(read_doc(&t, market)?);
                }
                let folders = p.join(FOLDERS_FILE);
                if folders.is_file() {
                    self.folders.extend(read_folders(&folders)?);
                }
            } else if p.join(TABLE_FILE).is_file() {
                self.tables.push(read_table(&p, market)?);
            } else if p.join(DOC_FILE).is_file() {
                self.tables.extend(read_doc(&p, market)?);
            }
        }
        Ok(())
    }

    /// Every issue in the set: per table and across tables. Empty when
    /// valid.
    pub fn validate(&self) -> Vec<Issue> {
        let mut out: Vec<Issue> = self.tables.iter().flat_map(validate_table).collect();
        out.extend(validate_set(&self.tables));
        let mut markets: Vec<Market> = self.markets.iter().map(|m| m.market).collect();
        markets.sort();
        let before = markets.len();
        markets.dedup();
        if markets.len() != before {
            out.push(Issue {
                table: String::new(),
                message: "a market is defined twice".into(),
            });
        }
        for t in &self.tables {
            if !markets.contains(&t.market) {
                out.push(issue(t, format!("market {} has no market.toml", t.market)));
            }
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
    use crate::file_model::doc::tests::{scada_doc, write_doc};
    use crate::file_model::file::{write_market, write_table};
    use crate::file_model::model::TimestampFormat;
    use crate::file_model::model::tests::unit_scada;

    fn nem() -> MarketDef {
        MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
            trading_day_start_secs: 14_400,
        }
    }

    /// A tree with DISPATCH_UNIT_SCADA pulled, an undeclared table beside
    /// it, and a hand-written table.
    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let nem_dir = dir.path().join("nem");
        let pulled = nem_dir.join(PULLED_DIR);
        for (name, doc) in [
            ("DISPATCH_UNIT_SCADA", scada_doc()),
            ("UNDECLARED", {
                let mut d = scada_doc();
                d["properties"]["utilake.table"] = "UNDECLARED".into();
                d["properties"]["utilake.declared"] = "false".into();
                d
            }),
        ] {
            write_doc(&pulled.join(name), &doc);
        }
        std::fs::write(pulled.join("manifest.json"), "{}").unwrap();
        std::fs::write(nem_dir.join("market.toml"), write_market(&nem())).unwrap();
        let mut own = unit_scada();
        own.name = "OWN".into();
        own.report_pairs = vec![("OWN".into(), "TABLE".into())];
        std::fs::create_dir_all(nem_dir.join("OWN")).unwrap();
        std::fs::write(nem_dir.join("OWN").join(TABLE_FILE), write_table(&own)).unwrap();
        let mut doc = scada_doc();
        doc["properties"]["utilake.table"] = "DOC".into();
        doc["properties"]["utilake.reports"] = "[[\"DOC\",\"TABLE\"]]".into();
        for s in doc["snapshots"].as_array_mut().unwrap() {
            s["summary"]["utilake.report-type"] = "DOC".into();
            s["summary"]["utilake.report-subtype"] = "TABLE".into();
        }
        write_doc(&nem_dir.join("DOC"), &doc);
        // Not tables: notes, a directory without a table file.
        std::fs::write(nem_dir.join("notes.md"), "text").unwrap();
        std::fs::create_dir_all(nem_dir.join("DRAFT")).unwrap();
        dir
    }

    #[test]
    fn reads_tree_and_round_trips_blob() {
        let dir = tree();
        let set = SchemaSet::read_dir(dir.path()).unwrap();
        let names: Vec<&str> = set.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["DISPATCH_UNIT_SCADA", "DOC", "OWN"]);
        assert_eq!(
            crate::file_model::doc::tests::bare(Some(set.tables[0].clone())),
            Some(unit_scada())
        );
        assert_eq!(set.markets, vec![nem()]);
        assert!(set.validate().is_empty(), "{:?}", set.validate());
        let back = SchemaSet::decode(&set.encode()).unwrap();
        assert_eq!(back, set);
        assert!(SchemaSet::decode(&set.encode()[..3]).is_err());
        assert_eq!(set.market(Market::Nem), Some(&nem()));
        assert_eq!(set.market(Market::Wem), None);
    }

    #[test]
    fn markets_and_read_errors() {
        let set = SchemaSet {
            tables: vec![unit_scada()],
            ..Default::default()
        };
        assert!(
            set.validate()
                .iter()
                .any(|i| i.message.contains("no market.toml"))
        );
        let set = SchemaSet {
            markets: vec![nem(), nem()],
            ..Default::default()
        };
        assert!(
            set.validate()
                .iter()
                .any(|i| i.message.contains("defined twice"))
        );
        let dir = tempfile::tempdir().unwrap();
        assert!(SchemaSet::read_dir(&dir.path().join("none")).is_err());
        let bad = dir.path().join("wem").join("T");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(TABLE_FILE), "nope = 1").unwrap();
        assert!(matches!(
            SchemaSet::read_dir(dir.path()),
            Err(SchemaFileError::Parse { .. })
        ));
        // A market directory without market.toml takes its name's market.
        std::fs::write(bad.join(TABLE_FILE), write_table(&unit_scada())).unwrap();
        let set = SchemaSet::read_dir(dir.path()).unwrap();
        assert_eq!(set.tables[0].market, Market::Wem);
        let pulled = dir.path().join("wem").join(PULLED_DIR).join("X");
        std::fs::create_dir_all(pulled.join("metadata")).unwrap();
        std::fs::write(pulled.join(DOC_FILE), "{").unwrap();
        assert!(SchemaSet::read_dir(dir.path()).is_err());
    }
}
