//! A table as utilake-schemas publishes it: `nem/<TABLE>/table.json`,
//! pulled into `schemas/nem/pulled/` (`just schemas-pull`).
//!
//! The document says the table's report pairs, key, time column, time
//! roles and load mode, and under `iceberg` its history: the Iceberg
//! schemas its files were written under and the runs of files of each.
//! The table's columns are the history's fields, by id, each with its
//! Iceberg type; a column takes nulls unless every schema requires it.
//! The rest of the document (the data model's column notes, the package)
//! is for its other readers.
//!
//! A table the data model does not declare has no report pairs; the
//! registry leaves it out, and a block of it is derived from its file.

use std::path::Path;

use serde::Deserialize;
use utilake_core::YearMonth;

use crate::file_model::error::SchemaFileError;
use crate::file_model::file::{TemporalFile, norm};
use crate::file_model::model::{
    Column, ColumnId, LakeType, LoadMode, Market, Run, Table, TableSchema,
};

/// The name of a pulled table's file in its directory.
pub const DOC_FILE: &str = "table.json";

#[derive(Deserialize)]
struct Doc {
    name: String,
    #[serde(default)]
    description: Option<String>,
    declared: bool,
    #[serde(default)]
    report_pairs: Option<Vec<(String, String)>>,
    #[serde(default)]
    primary_key: Option<Vec<String>>,
    #[serde(default)]
    time_column: Option<String>,
    #[serde(default)]
    temporal: Option<TemporalFile>,
    #[serde(default)]
    load_mode: Option<LoadMode>,
    iceberg: utilake_iceberg::History,
}

/// Parses a pulled table's text: the table, or `None` for one the data
/// model does not declare.
///
/// A column of the history is one field: two fields of one name (a column
/// whose values left their type, so the fold gave it a new field) are an
/// error until utilake reads a column by more than one field. A time column
/// or time role naming a column no file has carried is dropped, since no
/// row can have a value for it.
pub fn parse_doc(
    path: &Path,
    text: &str,
    market: Market,
) -> Result<Option<Table>, SchemaFileError> {
    let d: Doc = serde_json::from_str(text).map_err(|e| SchemaFileError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
        line: u64::try_from(e.line()).ok(),
    })?;
    let invalid = |message: String| SchemaFileError::Invalid {
        path: path.display().to_string(),
        table: d.name.clone(),
        message,
    };
    if !d.declared {
        return Ok(None);
    }
    let (Some(report_pairs), Some(load_mode), Some(temporal)) =
        (d.report_pairs.clone(), d.load_mode, d.temporal)
    else {
        return Err(invalid(
            "a declared table needs report_pairs, load_mode and temporal".into(),
        ));
    };
    let h = &d.iceberg;
    let mut columns: Vec<Column> = Vec::new();
    for f in h.fields() {
        if let Some(other) = columns.iter().find(|c| c.name == f.name) {
            return Err(invalid(format!(
                "column {} is field {} and field {} of the history (a breaking change gave it a new field); utilake reads a column by one field, so it cannot read this table yet",
                f.name, other.id, f.id
            )));
        }
        let required = h
            .schemas
            .iter()
            .all(|s| s.field(f.id).is_some_and(|g| g.required));
        columns.push(Column {
            id: ColumnId(f.id),
            name: f.name.clone(),
            ty: LakeType::of_iceberg(f.ty),
            nullable: !required,
            note: None,
        });
    }
    let schema_id = |id: u32| {
        u16::try_from(id)
            .map_err(|_| invalid(format!("schema id {id} is beyond what utilake numbers")))
    };
    let mut schemas = Vec::with_capacity(h.schemas.len());
    for s in &h.schemas {
        schemas.push(TableSchema {
            id: schema_id(s.schema_id)?,
            fields: s
                .fields
                .iter()
                .map(|f| (ColumnId(f.id), f.required))
                .collect(),
            key: s
                .identifier_field_ids
                .iter()
                .map(|&i| ColumnId(i))
                .collect(),
        });
    }
    let month = |m: &str| {
        m.parse::<YearMonth>()
            .map_err(|_| invalid(format!("a run's release {m:?} is not a month (YYYY-MM)")))
    };
    let mut runs = Vec::with_capacity(h.runs.len());
    for r in &h.runs {
        let [a, b] = &r.report;
        runs.push(Run {
            schema: schema_id(r.schema_id)?,
            report: (a.clone(), b.clone()),
            version: r.version,
            kinds: r.kinds.clone(),
            from: month(&r.from.release)?,
            to: month(&r.to.release)?,
            files: r.files,
            rows: r.rows,
        });
    }
    let is_column = |n: &String| columns.iter().any(|c| &c.name == n);
    let mut temporal = temporal.roles().map_err(|m| invalid(m.to_owned()))?;
    temporal.entity.retain(is_column);
    temporal.order.retain(is_column);
    temporal.cutoff = temporal.cutoff.filter(is_column);
    if temporal.valid.column().is_some_and(|c| !is_column(c)) {
        temporal.valid = crate::file_model::temporal::ValidAxis::Timeless;
    }
    let mut buf = String::new();
    let mut report_pairs = report_pairs;
    for (a, b) in &mut report_pairs {
        norm(a, &mut buf);
        norm(b, &mut buf);
    }
    Ok(Some(Table {
        market,
        name: d.name.clone(),
        description: d.description.clone(),
        report_pairs,
        load_mode,
        temporal,
        primary_key: d.primary_key.clone().unwrap_or_default(),
        time_column: d.time_column.clone().filter(is_column),
        columns,
        schemas,
        runs,
    }))
}

/// Reads `{dir}/table.json`.
pub fn read_doc(dir: &Path, market: Market) -> Result<Option<Table>, SchemaFileError> {
    let path = dir.join(DOC_FILE);
    let text = std::fs::read_to_string(&path).map_err(|source| SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    parse_doc(&path, &text, market)
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;
    use crate::file_model::model::tests::unit_scada;

    /// DISPATCH_UNIT_SCADA as utilake-schemas publishes it: the fixture's
    /// schemas and runs.
    pub(crate) fn scada_doc() -> serde_json::Value {
        let t = unit_scada();
        let mark = |m: YearMonth| json!({"file": "F.zip", "sha256": "ab", "release": m.to_string(), "data-time": null});
        let field = |id: &ColumnId, required: &bool| {
            let c = t.column_by_id(*id).unwrap();
            json!({"id": c.id.0, "name": c.name, "type": c.ty.iceberg().name(), "required": required})
        };
        json!({
            "name": t.name,
            "description": t.description,
            "declared": true,
            "package": "DISPATCH",
            "visibility": "PUBLIC",
            "report_pairs": t.report_pairs,
            "primary_key": t.primary_key,
            "time_column": t.time_column,
            "load_mode": "append",
            "temporal": {"origin": "derived", "entity": ["DUID"],
                         "valid": {"kind": "interval_end", "column": "SETTLEMENTDATE"},
                         "order": ["LASTCHANGED"], "cutoff": "LASTCHANGED"},
            "columns": [],
            "x-format": 2,
            "iceberg": {
                "table": t.name,
                "format-version": 3,
                "schemas": t.schemas.iter().map(|s| json!({
                    "type": "struct", "schema-id": s.id,
                    "identifier-field-ids": s.key.iter().map(|k| k.0).collect::<Vec<_>>(),
                    "fields": s.fields.iter().map(|(id, r)| field(id, r)).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "runs": t.runs.iter().map(|r| json!({
                    "schema-id": r.schema, "report": [r.report.0, r.report.1], "version": r.version,
                    "kinds": r.kinds, "from": mark(r.from), "to": mark(r.to), "files": r.files,
                    "rows": r.rows, "changes": ["first"], "updates": [], "overlaps": false,
                })).collect::<Vec<_>>(),
                "breaking": [],
            },
        })
    }

    fn parse(v: &serde_json::Value) -> Result<Option<Table>, SchemaFileError> {
        parse_doc(Path::new("table.json"), &v.to_string(), Market::Nem)
    }

    #[test]
    fn a_document_reads_as_its_table() {
        assert_eq!(parse(&scada_doc()).unwrap(), Some(unit_scada()));
        // Not declared: no table.
        let mut d = scada_doc();
        d["declared"] = json!(false);
        d["report_pairs"] = json!(null);
        assert_eq!(parse(&d).unwrap(), None);
        // A field one schema requires and another lacks takes nulls.
        let mut d = scada_doc();
        d["iceberg"]["schemas"][1]["fields"][3]["required"] = json!(true);
        let t = parse(&d).unwrap().unwrap();
        assert!(t.columns[3].nullable);
        assert_eq!(t.schemas[1].fields[3], (ColumnId(4), true));
        // A time column and roles naming a column no file has: dropped.
        let mut d = scada_doc();
        d["time_column"] = json!("NOWHERE");
        d["temporal"]["valid"]["column"] = json!("NOWHERE");
        d["temporal"]["entity"] = json!(["DUID", "NOWHERE"]);
        d["temporal"]["cutoff"] = json!("NOWHERE");
        let t = parse(&d).unwrap().unwrap();
        assert_eq!(t.time_column, None);
        assert_eq!(
            t.temporal.valid,
            crate::file_model::temporal::ValidAxis::Timeless
        );
        assert_eq!(
            (t.temporal.entity.as_slice(), t.temporal.cutoff),
            (&["DUID".to_owned()][..], None)
        );
    }

    #[test]
    fn what_a_document_cannot_say() {
        // One column under two fields.
        let mut d = scada_doc();
        d["iceberg"]["schemas"][1]["fields"][3]["id"] = json!(9);
        d["iceberg"]["schemas"][1]["fields"][3]["name"] = json!("SCADAVALUE");
        let e = parse(&d).unwrap_err();
        assert!(e.to_string().contains("field 3 and field 9"), "{e}");
        let mut d = scada_doc();
        d["load_mode"] = json!(null);
        assert!(
            parse(&d)
                .unwrap_err()
                .to_string()
                .contains("needs report_pairs")
        );
        let mut d = scada_doc();
        d["iceberg"]["runs"][0]["from"]["release"] = json!("2024");
        assert!(parse(&d).unwrap_err().to_string().contains("not a month"));
        let mut d = scada_doc();
        d["iceberg"]["runs"][0]["schema-id"] = json!(70_000);
        assert!(parse(&d).unwrap_err().to_string().contains("beyond"));
        let mut d = scada_doc();
        d["temporal"]["valid"] = json!({"kind": "timeless", "column": "X"});
        assert!(
            parse(&d)
                .unwrap_err()
                .to_string()
                .contains("takes no column")
        );
        let e = parse_doc(Path::new("t.json"), "{\n\"name\": 3}", Market::Nem).unwrap_err();
        assert!(
            matches!(e, SchemaFileError::Parse { line: Some(2), .. }),
            "{e:?}"
        );
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_doc(dir.path(), Market::Nem),
            Err(SchemaFileError::Read { .. })
        ));
        std::fs::write(dir.path().join(DOC_FILE), scada_doc().to_string()).unwrap();
        assert_eq!(
            read_doc(dir.path(), Market::Nem).unwrap(),
            Some(unit_scada())
        );
    }
}
