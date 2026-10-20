//! Each table's columns from what its files held.
//!
//! A table's shapes (`crate::fold`) are every column list it was published
//! in, with what each column held. Its columns are written again from them:
//! every column in the order the archive first showed it, a fresh id each
//! (ids are Iceberg field ids, so this is done once, before anything is
//! published), the type chosen once from everything observed of the column
//! (`utilake_iceberg::type_for`), and nullability from the data. A column
//! that held only whole numbers but that a data-model release declares
//! with a scale (`NUMERIC(15,5)`) stays `double`: a later value may have a
//! fraction, and Iceberg never makes a `long` a `double`. A key
//! column that every increment carries and that never held a null refuses
//! nulls, every other column takes them. The key, time roles, load mode
//! and increments stay as they are.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use utilake_iceberg::{Evidence, type_for};
use utilake_mms_registry::file_model::shapes::ShapeEntry;
use utilake_mms_registry::file_model::{Column, ColumnId, LakeType, Table};

/// What rewriting one table did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rewritten {
    /// Columns whose type the data changed, as `NAME: old -> new`.
    pub retyped: Vec<String>,
    /// Columns that now take nulls, or no longer do.
    pub nullability: Vec<String>,
    /// Columns the data would type as an exact decimal, kept as text
    /// until the lake writes decimals.
    pub decimals: Vec<String>,
}

/// The table with its columns written from `shapes` (in the order first
/// seen), and what changed; `scaled` names the columns a data-model
/// release declares with a fraction.
pub fn observed_table(
    t: &Table,
    shapes: &[ShapeEntry],
    scaled: &BTreeSet<String>,
) -> (Table, Rewritten) {
    let mut order: Vec<String> = Vec::new();
    let mut evidence: BTreeMap<String, Evidence> = BTreeMap::new();
    let mut nulls: BTreeMap<String, u64> = BTreeMap::new();
    let mut sorted: Vec<&ShapeEntry> = shapes.iter().collect();
    sorted.sort_by(|a, b| (a.first_seen, &a.from.file).cmp(&(b.first_seen, &b.from.file)));
    for e in &sorted {
        for c in &e.columns {
            if !order.contains(&c.name) {
                order.push(c.name.clone());
            }
            let ev = evidence.entry(c.name.clone()).or_default();
            *ev = ev.join(c.evidence());
            *nulls.entry(c.name.clone()).or_default() += c.nulls;
        }
    }
    // Columns an increment or the table declares that no file held.
    for name in t
        .increments
        .iter()
        .flat_map(|i| &i.columns)
        .chain(t.columns.iter().map(|c| &c.name))
    {
        if !order.contains(name) {
            order.push(name.clone());
        }
    }
    let in_every_shape = |name: &str| sorted.iter().all(|e| e.names().any(|n| n == name));
    let mut out = Rewritten::default();
    let columns: Vec<Column> = order
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let before = t.column(name);
            let declared = before.map(|c| c.ty.iceberg());
            let mut choice = type_for(evidence.get(name).copied().unwrap_or_default(), declared);
            if choice.ty == utilake_iceberg::IcebergType::Long && scaled.contains(name) {
                choice.ty = utilake_iceberg::IcebergType::Double;
            }
            let ty = LakeType::of_iceberg(choice.ty).unwrap_or_else(|| {
                out.decimals.push(format!("{name}: {}", choice.ty));
                LakeType::String
            });
            if let Some(b) = before
                && b.ty != ty
            {
                out.retyped.push(format!("{name}: {} -> {ty}", b.ty));
            }
            let required = t.primary_key.contains(name)
                && t.increments.iter().all(|inc| inc.columns.contains(name))
                && in_every_shape(name)
                && nulls.get(name).copied().unwrap_or(0) == 0;
            if before.is_some_and(|b| b.nullable == required) {
                out.nullability.push(format!(
                    "{name}: {}",
                    if required {
                        "refuses nulls"
                    } else {
                        "takes nulls"
                    }
                ));
            }
            Column {
                id: ColumnId(u32::try_from(i + 1).unwrap_or(u32::MAX)),
                name: name.clone(),
                ty,
                nullable: !required,
                note: before.and_then(|c| c.note.clone()),
            }
        })
        .collect();
    let mut table = t.clone();
    table.next_column_id = u32::try_from(columns.len() + 1).unwrap_or(u32::MAX);
    table.columns = columns;
    (table, out)
}

/// Rewrites every table of `schemas/nem` that has a shapes file; returns
/// each rewritten table's name and what changed.
pub fn rewrite(schemas: &Path) -> Result<Vec<(String, Rewritten)>, Box<utilake_core::Diagnostic>> {
    use utilake_core::Diagnose as _;
    use utilake_mms_registry::file_model::Market;
    use utilake_mms_registry::file_model::file::{read_table, write_table};
    let nem = schemas.join("nem");
    let models = crate::models::read_models(schemas).map_err(|e| Box::new(e.diagnostic()))?;
    let models = crate::models::by_first_seen(&models);
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
        let shapes =
            crate::census::read_shapes(schemas, &t.name).map_err(|e| Box::new(e.diagnostic()))?;
        if shapes.is_empty() {
            continue;
        }
        let scaled: BTreeSet<String> = models
            .iter()
            .filter_map(|m| m.tables.get(&t.name))
            .flat_map(|mt| &mt.columns)
            .filter(|(_, ty, ..)| matches!(crate::audit::ModelType::parse(ty), crate::audit::ModelType::Numeric(_, s) if s > 0))
            .map(|(n, ..)| n.to_ascii_uppercase())
            .collect();
        let (table, changes) = observed_table(&t, &shapes, &scaled);
        crate::census::write_text(&dir.join("table.toml"), &write_table(&table))
            .map_err(|e| Box::new(e.diagnostic()))?;
        done.push((table.name.clone(), changes));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use utilake_iceberg::Kind;
    use utilake_mms_registry::Registry;
    use utilake_mms_registry::file_model::shapes::{Mark, ShapeColumn};
    use utilake_mms_registry::shape::{ShapeId, TypeClass};

    use super::*;

    fn col(name: &str, kind: Kind, nulls: u64) -> ShapeColumn {
        ShapeColumn {
            name: name.into(),
            ctl: TypeClass::Unknown,
            kind,
            nulls,
            nulls_since: None,
            int_digits: 3,
            scale: u8::from(kind == Kind::Decimal),
            sig_digits: 4,
            max_len: 4,
            min: None,
            max: None,
        }
    }

    fn shape(month: &str, columns: Vec<ShapeColumn>) -> ShapeEntry {
        let mark = Mark {
            month: month.parse().unwrap(),
            file: format!("F{month}.zip"),
            sha256: None,
            published: None,
            data_time: None,
        };
        ShapeEntry {
            id: ShapeId(1),
            report_type: "DISPATCH".into(),
            report_subtype: "UNIT_SCADA".into(),
            versions: vec![1],
            file_kinds: vec!["dvd".into()],
            first_seen: mark.month,
            last_seen: mark.month,
            from: mark.clone(),
            to: mark,
            files: 1,
            rows: 1,
            concurrent_with: Vec::new(),
            columns,
        }
    }

    fn scada() -> Table {
        let r = Registry::load_embedded();
        r.table(r.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone()
    }

    #[test]
    fn columns_come_from_what_the_files_held() {
        let t = scada();
        let later = shape(
            "2026-08",
            vec![
                col("SETTLEMENTDATE", Kind::Timestamp, 0),
                col("DUID", Kind::Text, 0),
                col("SCADAVALUE", Kind::Integer, 0),
                col("LASTCHANGED", Kind::Timestamp, 2),
                col("WIDE", Kind::Decimal, 0),
            ],
        );
        let mut wide = later.clone();
        wide.columns[4].int_digits = 30;
        wide.columns[4].sig_digits = 33;
        let earlier = shape(
            "2009-07",
            vec![
                col("SETTLEMENTDATE", Kind::Timestamp, 0),
                col("DUID", Kind::Text, 0),
                col("SCADAVALUE", Kind::Integer, 0),
            ],
        );
        let (out, changed) = observed_table(&t, &[wide, earlier], &BTreeSet::new());
        let names: Vec<(&str, u32)> = out
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.id.0))
            .collect();
        assert_eq!(
            &names[..5],
            [
                ("SETTLEMENTDATE", 1),
                ("DUID", 2),
                ("SCADAVALUE", 3),
                ("LASTCHANGED", 4),
                ("WIDE", 5)
            ],
            "in the order the archive first showed them"
        );
        assert_eq!(out.next_column_id as usize, out.columns.len() + 1);
        let ty = |n: &str| out.column(n).unwrap().ty;
        assert_eq!(ty("SCADAVALUE"), LakeType::Long);
        assert_eq!(
            ty("WIDE"),
            LakeType::String,
            "a 33-digit decimal waits for decimals"
        );
        assert!(
            changed.decimals[0].starts_with("WIDE: decimal(31"),
            "{changed:?}"
        );
        assert!(
            changed
                .retyped
                .iter()
                .any(|r| r == "SCADAVALUE: double -> long"),
            "{changed:?}"
        );
        // The key refuses nulls only where every file had it, without one.
        assert!(!out.column("DUID").unwrap().nullable);
        assert!(out.column("LASTCHANGED").unwrap().nullable);
        // A fraction declared by the model keeps a whole-number column a double.
        let (kept, _) = observed_table(&t, &[later], &BTreeSet::from(["SCADAVALUE".to_owned()]));
        assert_eq!(kept.column("SCADAVALUE").unwrap().ty, LakeType::Double);
    }

    #[test]
    fn a_key_column_with_a_null_takes_nulls() {
        let t = scada();
        let s = shape(
            "2026-08",
            vec![
                col("SETTLEMENTDATE", Kind::Timestamp, 0),
                col("DUID", Kind::Text, 1),
                col("SCADAVALUE", Kind::Decimal, 0),
                col("LASTCHANGED", Kind::Timestamp, 0),
            ],
        );
        let (out, changed) = observed_table(&t, &[s], &BTreeSet::new());
        assert!(out.column("DUID").unwrap().nullable);
        assert!(
            changed
                .nullability
                .contains(&"DUID: takes nulls".to_owned()),
            "{changed:?}"
        );
    }
}
