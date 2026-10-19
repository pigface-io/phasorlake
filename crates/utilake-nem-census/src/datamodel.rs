//! The MMS Data Model report, as JSON scraped from AEMO's HTML pages.
//!
//! The JSON is one object keyed by table name; each entry carries the
//! package, visibility, primary key, indexes and columns with their Oracle
//! types. The JSON is written by the `datamodel` subcommand, which reads
//! the report's pages (see [`crate::dmreport`]).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use utilake_core::{Category, Diagnose, Diagnostic};

/// One table of the data model.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DmTable {
    /// Report page, e.g. `MMS_123.htm`.
    pub page: String,
    /// Package, e.g. `DISPATCH`.
    pub package: String,
    /// Description, when the report had one.
    #[serde(default)]
    pub description: Option<String>,
    /// `Public`, `Private`, `Private; Public Next-Day`, ...
    #[serde(default)]
    pub visibility: Option<String>,
    /// Primary key column names (alphabetical in the report).
    #[serde(default)]
    pub primary_key: Vec<String>,
    /// Index column names.
    #[serde(default)]
    pub indexes: Vec<String>,
    /// Columns in report order.
    #[serde(default)]
    pub columns: Vec<DmColumn>,
}

/// One column of the data model.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DmColumn {
    /// Name as written.
    pub name: String,
    /// Oracle type as written, e.g. `Number(16,6)`, `varchar2(20)`, `Date`.
    #[serde(rename = "type")]
    pub oracle: String,
    /// Marked mandatory in the report.
    #[serde(default)]
    pub mandatory: bool,
    /// Comment.
    #[serde(default)]
    pub comment: String,
}

/// The whole data model.
#[derive(Debug, Clone, Default)]
pub struct DataModel {
    /// Tables by upper-case name.
    pub tables: BTreeMap<String, DmTable>,
}

/// Why the data-model JSON could not be used.
#[derive(Debug, thiserror::Error)]
pub enum DataModelError {
    /// Could not read.
    #[error("The data-model file {path} could not be read.")]
    Read {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// Not the expected JSON.
    #[error("The data-model file {path} is not the expected JSON: {message}")]
    Parse {
        /// The file.
        path: String,
        /// serde's message.
        message: String,
    },
}

impl Diagnose for DataModelError {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Read { path, source } => {
                Diagnostic::error("schema.datamodel_read", Category::Schema, self.to_string())
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Parse { path, .. } => {
                Diagnostic::error("schema.datamodel_parse", Category::Schema, self.to_string())
                    .with_file(path.clone())
            }
        }
    }
}

impl DataModel {
    /// Loads the JSON file.
    pub fn load(path: &Path) -> Result<Self, DataModelError> {
        let text = std::fs::read_to_string(path).map_err(|source| DataModelError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let raw: BTreeMap<String, DmTable> =
            serde_json::from_str(&text).map_err(|e| DataModelError::Parse {
                path: path.display().to_string(),
                message: e.to_string(),
            })?;
        let tables = raw
            .into_iter()
            .map(|(k, mut v)| {
                v.columns
                    .iter_mut()
                    .for_each(|c| c.name = c.name.to_ascii_uppercase());
                v.primary_key
                    .iter_mut()
                    .for_each(|k| *k = k.to_ascii_uppercase());
                (k.to_ascii_uppercase(), v)
            })
            .collect();
        Ok(Self { tables })
    }

    /// The table named `name`.
    pub fn table(&self, name: &str) -> Option<&DmTable> {
        self.tables.get(&name.to_ascii_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_fixture() {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json"
        );
        let dm = DataModel::load(Path::new(p)).unwrap();
        assert!(dm.tables.len() > 300);
        let t = dm.table("dispatch_unit_scada").unwrap();
        assert_eq!(t.package, "DISPATCH");
        assert_eq!(t.visibility.as_deref(), Some("Public"));
        let mut pk = t.primary_key.clone();
        pk.sort();
        assert_eq!(pk, ["DUID", "SETTLEMENTDATE"]);
        assert_eq!(
            t.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]
        );
        assert!(t.columns[0].mandatory);
        assert!(!t.columns[2].mandatory);
        assert_eq!(t.columns[2].oracle, "Number(16,6)");
        assert!(dm.table("NOPE").is_none());
    }

    #[test]
    fn errors() {
        let dir = tempfile::tempdir().unwrap();
        let e = DataModel::load(&dir.path().join("none.json")).unwrap_err();
        assert_eq!(e.diagnostic().code, "schema.datamodel_read");
        let p = dir.path().join("bad.json");
        std::fs::write(&p, "[1,2]").unwrap();
        let e = DataModel::load(&p).unwrap_err();
        assert_eq!(e.diagnostic().code, "schema.datamodel_parse");
        assert!(e.to_string().contains("bad.json"));
    }
}
