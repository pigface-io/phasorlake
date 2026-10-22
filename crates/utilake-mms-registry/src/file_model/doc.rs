//! A table as utilake-schemas publishes it: its Iceberg table metadata,
//! `nem/<TABLE>/metadata/v1.metadata.json`, pulled into
//! `schemas/nem/pulled/<TABLE>/metadata/` (`just schemas-pull`).
//!
//! The metadata is read through `utilake_iceberg::from_metadata`: the
//! table's history (the Iceberg schemas its files were written under and
//! the runs of files of each, one snapshot per run), its declaration
//! (report pairs, key, time column, time roles and load mode, as
//! `utilake.*` properties) and the data model's words. The table's columns
//! are the history's fields, by id, each with its Iceberg type; a column
//! takes nulls unless every schema requires it. The schema a reader of the
//! table gets today is the metadata's `current-schema-id`.
//!
//! A table the data model does not declare has no report pairs; the
//! registry leaves it out, and a block of it is derived from its file.

use std::path::Path;

use utilake_core::YearMonth;
use utilake_iceberg::from_metadata;
use utilake_iceberg::table::TableMetadata;

use crate::file_model::error::SchemaFileError;
use crate::file_model::file::{TemporalFile, norm};
use crate::file_model::model::{
    Column, ColumnId, LakeType, LoadMode, Market, Run, Table, TableSchema,
};

/// The path of a pulled table's metadata under its directory.
pub const DOC_FILE: &str = "metadata/v1.metadata.json";

/// Parses a pulled table's metadata: the table, or `None` for one the data
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
    let m: TableMetadata = serde_json::from_str(text).map_err(|e| SchemaFileError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
        line: u64::try_from(e.line()).ok(),
    })?;
    let p = from_metadata(&m).map_err(|e| SchemaFileError::Invalid {
        path: path.display().to_string(),
        table: m
            .properties
            .get("utilake.table")
            .cloned()
            .unwrap_or_default(),
        message: e.to_string(),
    })?;
    let h = &p.history;
    let invalid = |message: String| SchemaFileError::Invalid {
        path: path.display().to_string(),
        table: h.table.clone(),
        message,
    };
    let Some(decl) = &p.declaration else {
        return Ok(None);
    };
    let load_mode: LoadMode =
        serde_json::from_value(serde_json::json!(decl.load_mode)).map_err(|_| {
            invalid(format!(
                "load mode {:?} is not append or snapshot",
                decl.load_mode
            ))
        })?;
    let temporal: TemporalFile = serde_json::to_value(&decl.temporal)
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| invalid("its temporal roles are not ones utilake knows".into()))?;
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
    let mut report_pairs: Vec<(String, String)> = decl
        .report_pairs
        .iter()
        .map(|[a, b]| (a.clone(), b.clone()))
        .collect();
    for (a, b) in &mut report_pairs {
        norm(a, &mut buf);
        norm(b, &mut buf);
    }
    Ok(Some(Table {
        market,
        name: h.table.clone(),
        description: p.description.clone(),
        report_pairs,
        load_mode,
        temporal,
        primary_key: decl.primary_key.clone(),
        time_column: decl.time_column.clone().filter(is_column),
        columns,
        schemas,
        runs,
        current_schema: schema_id(h.current_schema_id)?,
    }))
}

/// Reads `{dir}/metadata/v1.metadata.json`.
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
    use utilake_iceberg::publish::{Declaration, Published, Temporal, Valid};
    use utilake_iceberg::to_metadata;

    use super::*;
    use crate::file_model::model::tests::unit_scada;

    /// DISPATCH_UNIT_SCADA as utilake-schemas publishes it: the fixture's
    /// history and declaration as Iceberg table metadata.
    pub(crate) fn scada_doc() -> serde_json::Value {
        let t = unit_scada();
        let p = Published {
            history: t.history(),
            declaration: Some(Declaration {
                package: Some("DISPATCH".into()),
                report_pairs: t
                    .report_pairs
                    .iter()
                    .map(|(a, b)| [a.clone(), b.clone()])
                    .collect(),
                primary_key: t.primary_key.clone(),
                time_column: t.time_column.clone(),
                load_mode: "append".into(),
                visibility: "PUBLIC".into(),
                temporal: Temporal {
                    valid: Valid {
                        kind: "interval_end".into(),
                        column: Some("SETTLEMENTDATE".into()),
                    },
                    entity: vec!["DUID".into()],
                    order: vec!["LASTCHANGED".into()],
                    cutoff: Some("LASTCHANGED".into()),
                    origin: "derived".into(),
                },
            }),
            description: t.description.clone(),
            docs: Default::default(),
            model_types: Default::default(),
        };
        let table = to_metadata(&p, "l", "u").unwrap();
        serde_json::to_value(&table.metadata).unwrap()
    }

    /// Writes `doc` where [`read_doc`] reads it under `dir`.
    pub(crate) fn write_doc(dir: &Path, doc: &serde_json::Value) {
        let path = dir.join(DOC_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, doc.to_string()).unwrap();
    }

    fn parse(v: &serde_json::Value) -> Result<Option<Table>, SchemaFileError> {
        parse_doc(Path::new(DOC_FILE), &v.to_string(), Market::Nem)
    }

    #[test]
    fn a_document_reads_as_its_table() {
        assert_eq!(parse(&scada_doc()).unwrap(), Some(unit_scada()));
        // Not declared: no table.
        let mut d = scada_doc();
        d["properties"]["utilake.declared"] = json!("false");
        assert_eq!(parse(&d).unwrap(), None);
        // A field one schema requires and another lacks takes nulls.
        let mut d = scada_doc();
        d["schemas"][1]["fields"][3]["required"] = json!(true);
        let t = parse(&d).unwrap().unwrap();
        assert!(t.columns[3].nullable);
        assert_eq!(t.schemas[1].fields[3], (ColumnId(4), true));
        // A time column and roles naming a column no file has: dropped.
        let mut d = scada_doc();
        d["properties"]["utilake.time"] = json!("NOWHERE");
        d["properties"]["utilake.temporal.valid.column"] = json!("NOWHERE");
        d["properties"]["utilake.temporal.entity"] = json!("[\"DUID\",\"NOWHERE\"]");
        d["properties"]["utilake.temporal.cutoff"] = json!("NOWHERE");
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
        let err = |d: &serde_json::Value| parse(d).unwrap_err().to_string();
        // One column under two fields.
        let mut d = scada_doc();
        d["schemas"][1]["fields"][3]["id"] = json!(9);
        d["schemas"][1]["fields"][3]["name"] = json!("SCADAVALUE");
        assert!(err(&d).contains("field 3 and field 9"), "{}", err(&d));
        // Not a published table.
        let mut d = scada_doc();
        d["properties"]["utilake.declared"] = json!("maybe");
        assert!(err(&d).contains("utilake.declared"), "{}", err(&d));
        let mut d = scada_doc();
        d["properties"]["utilake.load"] = json!("weekly");
        assert!(err(&d).contains("is not append or snapshot"), "{}", err(&d));
        let mut d = scada_doc();
        d["properties"]["utilake.temporal.origin"] = json!("guessed");
        assert!(err(&d).contains("not ones utilake knows"), "{}", err(&d));
        let mut d = scada_doc();
        d["snapshots"][0]["summary"]["utilake.from.release"] = json!("2024");
        assert!(err(&d).contains("not a month"), "{}", err(&d));
        let mut d = scada_doc();
        d["current-schema-id"] = json!(70_000);
        assert!(err(&d).contains("beyond"), "{}", err(&d));
        let mut d = scada_doc();
        d["schemas"][0]["schema-id"] = json!(70_000);
        d["snapshots"][0]["schema-id"] = json!(70_000);
        assert!(err(&d).contains("beyond"), "{}", err(&d));
        let mut d = scada_doc();
        d["properties"]["utilake.temporal.valid.kind"] = json!("timeless");
        d["properties"]["utilake.temporal.valid.column"] = json!("X");
        assert!(err(&d).contains("takes no column"), "{}", err(&d));
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
        write_doc(dir.path(), &scada_doc());
        assert_eq!(
            read_doc(dir.path(), Market::Nem).unwrap(),
            Some(unit_scada())
        );
    }
}
