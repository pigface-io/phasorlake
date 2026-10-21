//! Each table's columns from its history as an Iceberg table.
//!
//! The fold (`crate::history`) gives every column of a table a field: an
//! id that is never minted twice and one type for its lineage. A table's
//! columns are written again from those fields, so a conformance plan
//! writes every value under its Iceberg field id: the field's id, its type,
//! and nullability from its schemas (a column refuses nulls only where
//! every schema requires it). A column an increment declares that no file
//! ever held keeps its declaration and takes an id after the history's.
//! The key, time roles, load mode and increments stay as they are.

use std::path::Path;

use utilake_iceberg::IcebergType;
use utilake_mms_registry::file_model::history::History;
use utilake_mms_registry::file_model::{Column, ColumnId, LakeType, Table};

/// What rewriting one table did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rewritten {
    /// Columns whose type the history changed, as `NAME: old -> new`.
    pub retyped: Vec<String>,
    /// Columns that now take nulls, or no longer do.
    pub nullability: Vec<String>,
    /// Columns whose id the history changed, as `NAME: old -> new`.
    pub renumbered: Vec<String>,
}

/// The table with its columns written from `h`, and what changed.
pub fn table_from_history(t: &Table, h: &History) -> (Table, Rewritten) {
    let mut out = Rewritten::default();
    let mut columns: Vec<Column> = h
        .fields()
        .into_iter()
        .map(|f| {
            let ty = IcebergType::parse(&f.ty).map_or(LakeType::String, LakeType::of_iceberg);
            let before = t.column(&f.name);
            if let Some(b) = before {
                if b.ty != ty {
                    out.retyped.push(format!("{}: {} -> {ty}", f.name, b.ty));
                }
                if b.nullable == f.required {
                    let now = if f.required {
                        "refuses nulls"
                    } else {
                        "takes nulls"
                    };
                    out.nullability.push(format!("{}: {now}", f.name));
                }
                if b.id.0 != f.id {
                    out.renumbered
                        .push(format!("{}: {} -> {}", f.name, b.id.0, f.id));
                }
            }
            Column {
                id: ColumnId(f.id),
                name: f.name,
                ty,
                nullable: !f.required,
                note: before.and_then(|c| c.note.clone()),
            }
        })
        .collect();
    let mut next = columns.iter().map(|c| c.id.0).max().unwrap_or(0) + 1;
    // Columns an increment declares that no file held.
    for name in t.increments.iter().flat_map(|i| &i.columns) {
        if columns.iter().any(|c| &c.name == name) {
            continue;
        }
        let before = t.column(name);
        columns.push(Column {
            id: ColumnId(next),
            name: name.clone(),
            ty: before.map_or(LakeType::String, |c| c.ty),
            nullable: true,
            note: before.and_then(|c| c.note.clone()),
        });
        next += 1;
    }
    let mut table = t.clone();
    table.columns = columns;
    table.next_column_id = next;
    (table, out)
}

/// Rewrites every table of `schemas/nem` that has a history; returns each
/// rewritten table's name and what changed.
pub fn rewrite(schemas: &Path) -> Result<Vec<(String, Rewritten)>, Box<utilake_core::Diagnostic>> {
    use utilake_core::Diagnose as _;
    use utilake_mms_registry::file_model::Market;
    use utilake_mms_registry::file_model::file::{read_table, write_table};
    use utilake_mms_registry::file_model::history::read_history;
    let nem = schemas.join("nem");
    let histories = crate::census::history_dir(schemas);
    let mut done = Vec::new();
    let mut dirs: Vec<_> = std::fs::read_dir(&nem)
        .map_err(|e| {
            Box::new(
                crate::census::CensusError::Read {
                    path: nem.display().to_string(),
                    message: e.to_string(),
                }
                .diagnostic(),
            )
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("table.toml").is_file())
        .collect();
    dirs.sort();
    for dir in dirs {
        let t = read_table(&dir, Market::Nem).map_err(|e| Box::new(e.diagnostic()))?;
        let path = histories.join(format!("{}.json", t.name));
        if !path.is_file() {
            continue;
        }
        let h = read_history(&path).map_err(|e| Box::new(e.diagnostic()))?;
        let (table, changes) = table_from_history(&t, &h);
        crate::census::write_text(&dir.join("table.toml"), &write_table(&table))
            .map_err(|e| Box::new(e.diagnostic()))?;
        done.push((table.name.clone(), changes));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use utilake_mms_registry::Registry;
    use utilake_mms_registry::file_model::history::{HistoryField, HistorySchema};

    use super::*;

    fn scada() -> Table {
        let r = Registry::load_embedded();
        r.table(r.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone()
    }

    fn field(id: u32, name: &str, ty: &str, required: bool) -> HistoryField {
        HistoryField {
            id,
            name: name.into(),
            ty: ty.into(),
            required,
        }
    }

    fn history(schemas: Vec<Vec<HistoryField>>) -> History {
        History {
            table: "DISPATCH_UNIT_SCADA".into(),
            schemas: schemas
                .into_iter()
                .enumerate()
                .map(|(i, fields)| HistorySchema {
                    schema_id: u32::try_from(i).unwrap(),
                    fields,
                    identifier_field_ids: Vec::new(),
                })
                .collect(),
            runs: Vec::new(),
        }
    }

    #[test]
    fn columns_are_the_fields_of_the_history() {
        let mut t = scada();
        t.increments[0].columns.push("NEVER".into());
        let h = history(vec![
            vec![
                field(1, "SETTLEMENTDATE", "timestamptz", true),
                field(2, "DUID", "string", true),
                field(3, "SCADAVALUE", "decimal(16, 6)", false),
            ],
            vec![
                field(1, "SETTLEMENTDATE", "timestamptz", true),
                field(2, "DUID", "string", false),
                field(3, "SCADAVALUE", "decimal(16, 6)", false),
                field(4, "LASTCHANGED", "timestamptz", false),
                field(5, "ODD", "uuid", false),
            ],
        ]);
        let (out, changed) = table_from_history(&t, &h);
        let cols: Vec<(&str, u32, bool)> = out
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.id.0, c.nullable))
            .collect();
        assert_eq!(
            cols,
            [
                ("SETTLEMENTDATE", 1, false),
                ("DUID", 2, true),
                ("SCADAVALUE", 3, true),
                ("LASTCHANGED", 4, true),
                ("ODD", 5, true),
                ("NEVER", 6, true),
            ]
        );
        assert_eq!(out.next_column_id, 7);
        assert_eq!(
            out.column("SCADAVALUE").unwrap().ty,
            LakeType::Decimal {
                precision: 16,
                scale: 6
            }
        );
        assert_eq!(out.column("ODD").unwrap().ty, LakeType::String);
        assert!(
            changed
                .retyped
                .iter()
                .any(|r| r.starts_with("SCADAVALUE: ")),
            "{changed:?}"
        );
        assert!(
            changed
                .nullability
                .contains(&"DUID: takes nulls".to_owned()),
            "{changed:?}"
        );
        // The embedded table numbers its columns its own way.
        let renumbered = |n: &str| changed.renumbered.iter().any(|r| r.starts_with(n));
        assert_eq!(
            renumbered("SETTLEMENTDATE: "),
            t.column("SETTLEMENTDATE").unwrap().id.0 != 1
        );
        // Run again on its own output, nothing changes.
        let (again, none) = table_from_history(&out, &h);
        assert_eq!(again, out);
        assert_eq!(none, Rewritten::default());
    }

    #[test]
    fn a_table_refuses_nulls_where_every_schema_does() {
        let t = scada();
        let mut nullable = t.clone();
        for c in &mut nullable.columns {
            c.nullable = true;
        }
        let h = history(vec![vec![
            field(1, "SETTLEMENTDATE", "timestamptz", true),
            field(9, "DUID", "string", false),
        ]]);
        let (out, changed) = table_from_history(&nullable, &h);
        let duid = t.column("DUID").unwrap().id.0;
        assert_eq!(changed.renumbered, [format!("DUID: {duid} -> 9")]);
        assert!(!out.column("SETTLEMENTDATE").unwrap().nullable);
        assert!(
            changed
                .nullability
                .contains(&"SETTLEMENTDATE: refuses nulls".to_owned()),
            "{changed:?}"
        );
    }

    #[test]
    fn rewrites_every_table_with_a_history() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path();
        let nem = schemas.join("nem");
        let src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/nem/DISPATCH_UNIT_SCADA");
        let dst = nem.join("DISPATCH_UNIT_SCADA");
        std::fs::create_dir_all(dst.join("ctl")).unwrap();
        for e in std::fs::read_dir(src.join("ctl")).unwrap().flatten() {
            std::fs::copy(e.path(), dst.join("ctl").join(e.file_name())).unwrap();
        }
        std::fs::copy(src.join("table.toml"), dst.join("table.toml")).unwrap();
        // A table without a history is left alone.
        let other = nem.join("OTHER");
        std::fs::create_dir_all(&other).unwrap();
        let text = std::fs::read_to_string(src.join("table.toml")).unwrap();
        let renamed = text.replace("name = \"DISPATCH_UNIT_SCADA\"", "name = \"OTHER\"");
        std::fs::write(other.join("table.toml"), renamed).unwrap();
        let h = history(vec![vec![field(1, "SETTLEMENTDATE", "timestamptz", true)]]);
        let hist = crate::census::history_dir(schemas);
        std::fs::create_dir_all(&hist).unwrap();
        let json = serde_json::json!({
            "table": h.table,
            "schemas": [{"schema-id": 0, "fields": [{"id": 1, "name": "SETTLEMENTDATE", "type": "timestamptz", "required": true}]}],
            "runs": [],
        });
        std::fs::write(hist.join("DISPATCH_UNIT_SCADA.json"), json.to_string()).unwrap();
        let done = rewrite(schemas).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, "DISPATCH_UNIT_SCADA");
        let text = std::fs::read_to_string(dst.join("table.toml")).unwrap();
        assert!(text.contains("SETTLEMENTDATE"), "{text}");

        std::fs::write(hist.join("DISPATCH_UNIT_SCADA.json"), "{").unwrap();
        assert_eq!(rewrite(schemas).unwrap_err().code, "schema.file_parse");
        assert!(rewrite(&schemas.join("none")).is_err());
    }
}
