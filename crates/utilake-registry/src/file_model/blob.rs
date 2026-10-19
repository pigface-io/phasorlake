//! A whole `schemas/` tree as one postcard blob, written by the registry's
//! build script and embedded in the binary.
//!
//! On disk a market is `schemas/{market}/market.toml`, one directory per
//! table (`{TABLE}/table.toml` and its control files under `{TABLE}/ctl/`),
//! and the census's directories. The control files are checked against
//! their hashes when read, and are not packed.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file_model::ctl::sha256;
use crate::file_model::error::SchemaFileError;
use crate::file_model::file::{CTL_DIR, TABLE_FILE, read_market, read_table};
use crate::file_model::model::{Market, MarketDef, Table};
use crate::file_model::shapes::{ShapeEntry, read_shapes, shape_issues};
use crate::file_model::validate::{Issue, issue, validate_set, validate_table};

/// Everything in a `schemas/` tree.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaSet {
    /// One per market directory.
    pub markets: Vec<MarketDef>,
    /// Every table, sorted by name.
    pub tables: Vec<Table>,
    /// The census's shapes of each table, by table name. Packed on their
    /// own ([`SchemaSet::encode_shapes`]), so a run that never asks for a
    /// shape never decodes them.
    #[serde(skip)]
    pub shapes: BTreeMap<String, Vec<ShapeEntry>>,
    /// What reading found wrong with the control files: one missing, or
    /// not the bytes its hash names.
    #[serde(skip)]
    pub file_issues: Vec<Issue>,
}

/// Directories under a market that hold the census's records, not tables.
pub const CENSUS_DIRS: [&str; 2] = ["census", "shapes"];

/// The market a table file is under, when `path` (`/`-separated, ending
/// `{market}/{TABLE}/table.toml`) is one.
pub fn table_path_market(path: &str) -> Option<Market> {
    let segments: Vec<&str> = path.split('/').collect();
    match segments.as_slice() {
        [.., market, table, file] if *file == TABLE_FILE && !CENSUS_DIRS.contains(table) => {
            match *market {
                "nem" => Some(Market::Nem),
                "wem" => Some(Market::Wem),
                _ => None,
            }
        }
        _ => None,
    }
}

fn read_err(path: &Path, source: std::io::Error) -> SchemaFileError {
    SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    }
}

fn sorted_entries(dir: &Path) -> Result<Vec<std::path::PathBuf>, SchemaFileError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| read_err(dir, e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    Ok(entries)
}

/// The issues of a table's control files: each increment's file is in
/// `{dir}/ctl/` with the bytes its hash names.
pub fn ctl_issues(dir: &Path, t: &Table) -> Vec<Issue> {
    let mut out = Vec::new();
    for i in &t.increments {
        let (Some(name), Some(want)) = (&i.ctl, &i.ctl_sha256) else {
            continue;
        };
        let path = dir.join(CTL_DIR).join(name);
        match std::fs::read(&path) {
            Err(e) => out.push(issue(
                t,
                format!(
                    "increment {} names control file {}, which could not be read: {e}",
                    i.n,
                    path.display()
                ),
            )),
            Ok(bytes) => {
                let got = sha256(&bytes);
                if got != *want {
                    out.push(issue(
                        t,
                        format!(
                            "increment {}: control file {} has SHA-256 {got}, not {want}; a control file is kept as AEMO published it",
                            i.n,
                            path.display()
                        ),
                    ));
                }
            }
        }
    }
    out
}

impl SchemaSet {
    /// Reads every market directory under `root`.
    pub fn read_dir(root: &Path) -> Result<Self, SchemaFileError> {
        let mut set = Self::default();
        for dir in sorted_entries(root)?.into_iter().filter(|p| p.is_dir()) {
            let mf = dir.join("market.toml");
            let market = if mf.is_file() {
                let m = read_market(&mf)?;
                set.markets.push(m.clone());
                m.market
            } else {
                match dir.file_name().and_then(|n| n.to_str()) {
                    Some("wem") => Market::Wem,
                    _ => Market::Nem,
                }
            };
            set.read_market_dir(&dir, market)?;
        }
        set.tables.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(set)
    }

    fn read_market_dir(&mut self, dir: &Path, market: Market) -> Result<(), SchemaFileError> {
        for p in sorted_entries(dir)?.into_iter().filter(|p| p.is_dir()) {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "shapes" {
                self.read_shapes_dir(&p)?;
            } else if !CENSUS_DIRS.contains(&name) && p.join(TABLE_FILE).is_file() {
                let t = read_table(&p, market)?;
                self.file_issues.extend(ctl_issues(&p, &t));
                self.tables.push(t);
            }
        }
        Ok(())
    }

    fn read_shapes_dir(&mut self, dir: &Path) -> Result<(), SchemaFileError> {
        for p in sorted_entries(dir)? {
            if p.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let table = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.shapes
                .entry(table)
                .or_default()
                .extend(read_shapes(&p)?);
        }
        Ok(())
    }

    /// Every issue in the set: per table, across tables, of the control
    /// files and of the census's shapes. Empty when valid.
    pub fn validate(&self) -> Vec<Issue> {
        let mut out: Vec<Issue> = self.tables.iter().flat_map(validate_table).collect();
        out.extend(validate_set(&self.tables));
        out.extend(self.file_issues.iter().cloned());
        for (table, shapes) in &self.shapes {
            out.extend(
                shape_issues(table, shapes)
                    .into_iter()
                    .map(|message| Issue {
                        table: table.clone(),
                        message,
                    }),
            );
        }
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

    /// Encodes the census's shapes as a postcard blob of their own.
    pub fn encode_shapes(&self) -> Vec<u8> {
        postcard::to_allocvec(&self.shapes).expect("shapes always serialise")
    }

    /// Decodes a blob of shapes.
    pub fn decode_shapes(
        bytes: &[u8],
    ) -> Result<BTreeMap<String, Vec<ShapeEntry>>, SchemaFileError> {
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
    use crate::file_model::file::{write_market, write_table};
    use crate::file_model::model::TimestampFormat;
    use crate::file_model::model::tests::unit_scada;

    fn nem() -> MarketDef {
        MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
        }
    }

    /// A tree with DISPATCH_UNIT_SCADA, its first increment's control
    /// file, and the census's directories.
    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let nem_dir = dir.path().join("nem");
        let table = nem_dir.join("DISPATCH_UNIT_SCADA");
        std::fs::create_dir_all(table.join("ctl")).unwrap();
        std::fs::write(nem_dir.join("market.toml"), write_market(&nem())).unwrap();
        let ctl = b"LOAD DATA\n";
        std::fs::write(table.join("ctl").join("001.ctl"), ctl).unwrap();
        let mut t = unit_scada();
        t.increments[0].ctl = Some("001.ctl".into());
        t.increments[0].ctl_sha256 = Some(sha256(ctl));
        std::fs::write(table.join(TABLE_FILE), write_table(&t)).unwrap();
        // Not tables: notes, a directory without table.toml, the census.
        std::fs::write(nem_dir.join("notes.md"), "text").unwrap();
        std::fs::create_dir_all(nem_dir.join("DRAFT")).unwrap();
        let census = nem_dir.join("census");
        std::fs::create_dir_all(&census).unwrap();
        std::fs::write(census.join("2026-08.toml"), "\"A.zip\" = \"0\"").unwrap();
        let shapes = nem_dir.join("shapes");
        std::fs::create_dir_all(&shapes).unwrap();
        let entry = crate::file_model::shapes::tests::scada_entry();
        std::fs::write(
            shapes.join("DISPATCH_UNIT_SCADA.toml"),
            crate::file_model::shapes::render_shapes(std::slice::from_ref(&entry)),
        )
        .unwrap();
        std::fs::write(shapes.join("README"), "not a shapes file").unwrap();
        dir
    }

    #[test]
    fn reads_tree_and_round_trips_blob() {
        let dir = tree();
        assert_eq!(
            table_path_market("schemas/nem/REGION/table.toml"),
            Some(Market::Nem)
        );
        assert_eq!(table_path_market("wem/X/table.toml"), Some(Market::Wem));
        assert_eq!(table_path_market("mars/X/table.toml"), None);
        assert_eq!(table_path_market("nem/REGION/ctl/001.ctl"), None);
        assert_eq!(table_path_market("nem/shapes/table.toml"), None);
        assert_eq!(table_path_market("nem/market.toml"), None);
        let set = SchemaSet::read_dir(dir.path()).unwrap();
        assert_eq!(set.tables.len(), 1);
        assert_eq!(set.markets, vec![nem()]);
        assert_eq!(set.shapes["DISPATCH_UNIT_SCADA"].len(), 1);
        assert!(set.validate().is_empty(), "{:?}", set.validate());
        let back = SchemaSet::decode(&set.encode()).unwrap();
        assert_eq!(back.tables, set.tables);
        assert!(back.shapes.is_empty(), "shapes are packed on their own");
        assert_eq!(
            SchemaSet::decode_shapes(&set.encode_shapes()).unwrap(),
            set.shapes
        );
        assert!(SchemaSet::decode_shapes(&[0xff]).is_err());
        assert!(SchemaSet::decode(&set.encode()[..3]).is_err());
        assert_eq!(set.market(Market::Nem), Some(&nem()));
        assert_eq!(set.market(Market::Wem), None);
    }

    #[test]
    fn control_files_are_checked() {
        let dir = tree();
        let ctl = dir.path().join("nem/DISPATCH_UNIT_SCADA/ctl/001.ctl");
        std::fs::write(&ctl, b"LOAD DATA -- edited\n").unwrap();
        let set = SchemaSet::read_dir(dir.path()).unwrap();
        let issues = set.validate();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].message.contains("as AEMO published it"));
        std::fs::remove_file(&ctl).unwrap();
        let set = SchemaSet::read_dir(dir.path()).unwrap();
        assert!(set.validate()[0].message.contains("could not be read"));
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
        let shapes = dir.path().join("wem").join("shapes");
        std::fs::create_dir_all(&shapes).unwrap();
        std::fs::write(shapes.join("T.toml"), "shape = 1").unwrap();
        assert!(SchemaSet::read_dir(dir.path()).is_err());
    }
}
