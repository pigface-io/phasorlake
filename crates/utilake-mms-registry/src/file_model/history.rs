//! A table's history as an Iceberg table, as the census fold writes it to
//! `nem/iceberg/<TABLE>.json`: the schemas its files were written under and
//! the runs of files of each.
//!
//! Read here for two things. Resolution names the schema a block is written
//! under: the one whose columns, in order, are the block's. Validation holds
//! the table's columns to the history's field ids and types, so a plan
//! writes every column under its Iceberg field id. Only what those need is
//! read; the rest of the file is left to the readers of the export.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use utilake_iceberg::IcebergType;

use super::error::SchemaFileError;
use super::model::{LakeType, Table};

/// One field of a schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryField {
    /// The Iceberg field id.
    pub id: u32,
    /// The column name.
    pub name: String,
    /// The Iceberg type, as table metadata writes it.
    #[serde(rename = "type")]
    pub ty: String,
    /// Whether every row has a value.
    pub required: bool,
}

/// One schema of the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistorySchema {
    /// Its id.
    #[serde(rename = "schema-id")]
    pub schema_id: u32,
    /// Its fields, in file order.
    pub fields: Vec<HistoryField>,
}

/// Where a run starts or ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMark {
    /// The file.
    pub file: String,
    /// The data's time there, RFC 3339.
    #[serde(rename = "data-time")]
    pub data_time: Option<String>,
}

/// Files in a row written under one schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRun {
    /// The schema.
    #[serde(rename = "schema-id")]
    pub schema_id: u32,
    /// AEMO's version of the report.
    pub version: u16,
    /// Its first file.
    pub from: HistoryMark,
    /// Its last.
    pub to: HistoryMark,
    /// Files in it.
    pub files: u64,
}

/// A table's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// The table.
    pub table: String,
    /// Its schemas, by id.
    pub schemas: Vec<HistorySchema>,
    /// Its runs, in release order.
    pub runs: Vec<HistoryRun>,
}

/// Reads a history file.
pub fn read_history(path: &Path) -> Result<History, SchemaFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|e| SchemaFileError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
        line: u64::try_from(e.line()).ok(),
    })
}

impl History {
    /// The schema whose columns are `columns`, in order; the latest when a
    /// column's lineage gave two schemas the same names.
    pub fn schema_of(&self, columns: &[&str]) -> Option<u32> {
        self.schemas
            .iter()
            .rev()
            .find(|s| {
                s.fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .eq(columns.iter().copied())
            })
            .map(|s| s.schema_id)
    }

    /// Every field any schema has, by id: required only where every schema
    /// that has it requires it.
    pub fn fields(&self) -> Vec<HistoryField> {
        let mut out: BTreeMap<u32, HistoryField> = BTreeMap::new();
        for f in self.schemas.iter().flat_map(|s| &s.fields) {
            out.entry(f.id)
                .and_modify(|g| g.required &= f.required)
                .or_insert_with(|| f.clone());
        }
        out.into_values().collect()
    }
}

/// What is wrong between a table and its history: a column whose id or
/// type is not its field's, a field the table lacks, a type Iceberg does
/// not name. Empty when they agree.
pub fn history_issues(t: &Table, h: &History) -> Vec<String> {
    let mut out = Vec::new();
    for f in h.fields() {
        let Some(ty) = IcebergType::parse(&f.ty) else {
            out.push(format!(
                "field {} ({}) has type {:?}, which Iceberg does not name",
                f.id, f.name, f.ty
            ));
            continue;
        };
        match t.column(&f.name) {
            None => out.push(format!(
                "field {} ({}) of the history is not a column of the table; `utilake-nem-census tables` writes the columns from the history",
                f.id, f.name
            )),
            Some(c) if c.id.0 != f.id => out.push(format!(
                "column {} has id {} where its field in the history has {}",
                f.name, c.id.0, f.id
            )),
            Some(c) if c.ty != LakeType::of_iceberg(ty) => out.push(format!(
                "column {} is {} where its field in the history is {}",
                f.name, c.ty, f.ty
            )),
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::file_model::model::tests::unit_scada;

    pub(crate) fn field(id: u32, name: &str, ty: &str, required: bool) -> HistoryField {
        HistoryField {
            id,
            name: name.into(),
            ty: ty.into(),
            required,
        }
    }

    /// DISPATCH_UNIT_SCADA's history: its four columns, then the same four
    /// with DUID first, then three.
    pub(crate) fn scada_history() -> History {
        let t = field(1, "SETTLEMENTDATE", "timestamptz", true);
        let d = field(2, "DUID", "string", true);
        let v = field(3, "SCADAVALUE", "double", false);
        let l = field(4, "LASTCHANGED", "timestamptz", false);
        let schema = |id, fields: &[&HistoryField]| HistorySchema {
            schema_id: id,
            fields: fields.iter().map(|f| (*f).clone()).collect(),
        };
        History {
            table: "DISPATCH_UNIT_SCADA".into(),
            schemas: vec![
                schema(0, &[&t, &d, &v, &l]),
                schema(1, &[&d, &t, &v, &l]),
                schema(2, &[&t, &d, &field(3, "SCADAVALUE", "double", true)]),
            ],
            runs: vec![HistoryRun {
                schema_id: 0,
                version: 1,
                from: HistoryMark {
                    file: "A.zip".into(),
                    data_time: Some("2024-01-01T00:05:00+10:00".into()),
                },
                to: HistoryMark {
                    file: "B.zip".into(),
                    data_time: None,
                },
                files: 2,
            }],
        }
    }

    #[test]
    fn names_the_schema_of_a_column_list() {
        let h = scada_history();
        assert_eq!(
            h.schema_of(&["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"]),
            Some(0)
        );
        assert_eq!(
            h.schema_of(&["DUID", "SETTLEMENTDATE", "SCADAVALUE", "LASTCHANGED"]),
            Some(1)
        );
        assert_eq!(h.schema_of(&["SETTLEMENTDATE", "DUID"]), None);
        // Two schemas of the same names (two lineages): the latest.
        let mut twice = h.clone();
        let mut again = twice.schemas[0].clone();
        again.schema_id = 9;
        twice.schemas.push(again);
        assert_eq!(
            twice.schema_of(&["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"]),
            Some(9)
        );
        // Every field once, required only where every schema requires it.
        let fields: Vec<(u32, bool)> = h.fields().iter().map(|f| (f.id, f.required)).collect();
        assert_eq!(fields, [(1, true), (2, true), (3, false), (4, false)]);
    }

    #[test]
    fn holds_the_table_to_its_fields() {
        let t = unit_scada();
        assert!(history_issues(&t, &scada_history()).is_empty());
        let mut h = scada_history();
        h.schemas[0].fields[0].id = 7;
        h.schemas[0].fields[1].ty = "long".into();
        h.schemas[0].fields.push(field(5, "NEW", "string", false));
        h.schemas[0].fields.push(field(6, "ODD", "uuid", false));
        let issues = history_issues(&t, &h);
        assert_eq!(issues.len(), 4, "{issues:?}");
        assert!(
            issues
                .iter()
                .any(|i| i.contains("id 1 where its field in the history has 7"))
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("DUID is string where its field in the history is long"))
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("NEW") && i.contains("not a column"))
        );
        assert!(issues.iter().any(|i| i.contains("\"uuid\"")));
    }

    #[test]
    fn reads_the_folds_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("DISPATCH_UNIT_SCADA.json");
        std::fs::write(
            &p,
            r#"{"table": "DISPATCH_UNIT_SCADA", "format-version": 3,
                "schemas": [{"schema-id": 0, "type": "struct", "identifier-field-ids": [1],
                             "fields": [{"id": 1, "name": "SETTLEMENTDATE", "type": "timestamptz", "required": true}]}],
                "runs": [{"schema-id": 0, "version": 1, "kinds": ["dvd"], "files": 3, "rows": 9,
                          "from": {"file": "A", "sha256": null, "published": null, "data-time": null},
                          "to": {"file": "B", "sha256": null, "published": null, "data-time": "2024-01-01T00:05:00+10:00"},
                          "changes": ["first"], "updates": [], "overlaps": false}],
                "breaking": []}"#,
        )
        .unwrap();
        let h = read_history(&p).unwrap();
        assert_eq!((h.schemas.len(), h.runs[0].files), (1, 3));
        std::fs::write(&p, "{\n  \"table\": 3\n}").unwrap();
        let e = read_history(&p).unwrap_err();
        assert!(
            matches!(e, SchemaFileError::Parse { line: Some(2), .. }),
            "{e:?}"
        );
        assert!(matches!(
            read_history(&dir.path().join("none.json")),
            Err(SchemaFileError::Read { .. })
        ));
    }
}
