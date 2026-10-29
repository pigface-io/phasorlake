//! A table as phasor-schemas publishes it: its Iceberg table metadata,
//! pulled into `schemas/nem/pulled/<TABLE>/metadata/` (`just schemas-pull`).

use std::path::Path;

use phasor_core::YearMonth;
use phasor_iceberg::from_metadata;
use phasor_iceberg::table::TableMetadata;

use crate::file_model::error::SchemaFileError;
use crate::file_model::file::norm;
use crate::file_model::model::{
    Column, ColumnId, LakeType, LoadMode, Market, Run, Table, TableSchema,
};

/// The path of a pulled table's metadata under its directory.
pub const DOC_FILE: &str = "metadata/v1.metadata.json";

/// Parses a pulled table's metadata: the table, or `None` for one the data model does not declare.
///
/// Two fields of one name are an error. A time column or role naming a column
/// no file has carried is dropped.
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
            .get("phasor.table")
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
    let mut columns: Vec<Column> = Vec::new();
    for f in h.fields() {
        if let Some(other) = columns.iter().find(|c| c.name == f.name) {
            return Err(invalid(format!(
                "column {} is field {} and field {} of the history (a breaking change gave it a new field); phasor reads a column by one field, so it cannot read this table yet",
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
            .map_err(|_| invalid(format!("schema id {id} is beyond what phasor numbers")))
    };
    // A field under another name in a schema than its newest: a rename.
    let mut older_names = Vec::new();
    for s in &h.schemas {
        for f in &s.fields {
            if columns.iter().any(|c| c.id.0 == f.id && c.name != f.name) {
                older_names.push((schema_id(s.schema_id)?, ColumnId(f.id), f.name.clone()));
            }
        }
    }
    let mut schemas = Vec::with_capacity(h.schemas.len());
    for s in &h.schemas {
        schemas.push(TableSchema {
            id: schema_id(s.schema_id)?,
            fields: s
                .fields
                .iter()
                .map(|f| (ColumnId(f.id), f.required))
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
        with_columns: decl.with_columns.clone(),
        older_names,
        published: serde_json::to_string(&crate::file_model::model::Published {
            properties: m.properties.clone(),
            docs: p.docs.clone(),
        })
        .expect("maps of strings serialise"),
        load_mode,
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
    use phasor_iceberg::publish::{Declaration, Published};
    use phasor_iceberg::to_metadata;
    use serde_json::json;

    use super::*;
    use crate::file_model::model::tests::unit_scada;

    /// DISPATCH_UNIT_SCADA as phasor-schemas publishes it.
    pub(crate) fn scada_doc() -> serde_json::Value {
        let t = unit_scada();
        let p = Published {
            market: phasor_iceberg::Market::Nem,
            model: Vec::new(),
            history: t.history(),
            declaration: Some(Declaration {
                package: Some("DISPATCH".into()),
                report_pairs: t
                    .report_pairs
                    .iter()
                    .map(|(a, b)| [a.clone(), b.clone()])
                    .collect(),
                with_columns: t.with_columns.clone(),
                time_column: t.time_column.clone(),
                load_mode: "append".into(),
                visibility: "PUBLIC".into(),
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

    /// A table read from a document, its published properties checked and set aside.
    pub(crate) fn bare(t: Option<Table>) -> Option<Table> {
        t.map(|mut t| {
            let p = t.published();
            assert_eq!(p.properties["phasor.declared"], "true");
            assert!(!p.properties.contains_key("phasor.temporal.entity"));
            t.published.clear();
            t
        })
    }

    #[test]
    fn a_document_reads_as_its_table() {
        assert_eq!(bare(parse(&scada_doc()).unwrap()), Some(unit_scada()));
        // Not declared: no table.
        let mut d = scada_doc();
        d["properties"]["phasor.declared"] = json!("false");
        assert_eq!(parse(&d).unwrap(), None);
        // A field one schema requires and another lacks takes nulls.
        let mut d = scada_doc();
        d["schemas"][1]["fields"][3]["required"] = json!(true);
        let t = parse(&d).unwrap().unwrap();
        assert!(t.columns[3].nullable);
        assert_eq!(t.schemas[1].fields[3], (ColumnId(4), true));
        // A time column naming a column no file has: dropped.
        let mut d = scada_doc();
        d["properties"]["phasor.time"] = json!("NOWHERE");
        let t = parse(&d).unwrap().unwrap();
        assert_eq!(t.time_column, None);
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
        d["properties"]["phasor.declared"] = json!("maybe");
        assert!(err(&d).contains("phasor.declared"), "{}", err(&d));
        let mut d = scada_doc();
        d["properties"]["phasor.load"] = json!("weekly");
        assert!(err(&d).contains("is not append or snapshot"), "{}", err(&d));
        let mut d = scada_doc();
        d["snapshots"][0]["summary"]["phasor.from.release"] = json!("2024");
        assert!(err(&d).contains("not a month"), "{}", err(&d));
        let mut d = scada_doc();
        d["current-schema-id"] = json!(70_000);
        assert!(err(&d).contains("beyond"), "{}", err(&d));
        let mut d = scada_doc();
        d["schemas"][0]["schema-id"] = json!(70_000);
        d["snapshots"][0]["schema-id"] = json!(70_000);
        assert!(err(&d).contains("beyond"), "{}", err(&d));
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
            bare(read_doc(dir.path(), Market::Nem).unwrap()),
            Some(unit_scada())
        );
    }
}
