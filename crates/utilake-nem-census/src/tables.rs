//! Each table's columns from what its files held.
//!
//! A table's shapes (`crate::fold`) are every column list it was published
//! in, with what each column held. Its columns are written again from them:
//! every column in the order the archive first showed it, a fresh id each
//! (ids are Iceberg field ids, so this is done once, before anything is
//! published), the type chosen once from everything observed of the column
//! with AEMO's declaration (`utilake_iceberg::choose`): a `long` only for
//! whole numbers declared whole or in the key, an exact `decimal(P,S)` for
//! the rest (the scale the larger of the declared and the observed), a
//! `double` only for floats printed in full; and nullability from the
//! data. A key
//! column that every increment carries and that never held a null refuses
//! nulls, every other column takes them. The key, time roles, load mode
//! and increments stay as they are.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use utilake_iceberg::{Declared, Evidence, choose};
use utilake_mms_registry::file_model::shapes::ShapeEntry;
use utilake_mms_registry::file_model::{Column, ColumnId, LakeType, Table};
use utilake_mms_registry::shape::TypeClass;

/// What rewriting one table did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rewritten {
    /// Columns whose type the data changed, as `NAME: old -> new`.
    pub retyped: Vec<String>,
    /// Columns that now take nulls, or no longer do.
    pub nullability: Vec<String>,
    /// Columns typed as exact decimals, as `NAME: decimal(P,S)`.
    pub decimals: Vec<String>,
}

/// What a data-model release declares of a numeric column: precision and
/// scale.
pub type ModelNumeric = BTreeMap<String, (u8, u8)>;

/// The table with its columns written from `shapes` (in the order first
/// seen), and what changed; `model` holds the precision and scale the
/// newest data-model release declares of its numeric columns.
pub fn observed_table(
    t: &Table,
    shapes: &[ShapeEntry],
    model: &ModelNumeric,
) -> (Table, Rewritten) {
    let mut order: Vec<String> = Vec::new();
    let mut evidence: BTreeMap<String, Evidence> = BTreeMap::new();
    let mut nulls: BTreeMap<String, u64> = BTreeMap::new();
    let mut classes: BTreeMap<String, BTreeSet<TypeClass>> = BTreeMap::new();
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
            classes.entry(c.name.clone()).or_default().insert(c.ctl);
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
            let ctl = classes.get(name);
            let has = |c: TypeClass| ctl.is_some_and(|s| s.contains(&c));
            let (precision, scale) = model
                .get(name)
                .map_or((None, None), |(p, s)| (Some(*p), Some(*s)));
            let declared = Declared {
                whole: scale == Some(0) || (has(TypeClass::Integer) && !has(TypeClass::Number)),
                fractional: scale.is_some_and(|s| s > 0) || has(TypeClass::Number),
                precision,
                scale,
                // A column never seen with a value takes what the model
                // declares, else what it was.
                fallback: match (precision, scale) {
                    (Some(_), Some(0)) => Some(utilake_iceberg::IcebergType::Long),
                    (Some(p), Some(s)) if p <= 18 => utilake_iceberg::IcebergType::decimal(p, s),
                    _ => None,
                }
                .or_else(|| before.map(|c| c.ty.iceberg())),
                key: t.primary_key.contains(name),
            };
            let choice = choose(evidence.get(name).copied().unwrap_or_default(), declared);
            let ty = LakeType::of_iceberg(choice.ty);
            if let LakeType::Decimal { .. } = ty {
                out.decimals.push(format!("{name}: {ty}"));
            }
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
        // The newest release that declares the table.
        let numeric: ModelNumeric = models
            .iter()
            .rev()
            .find_map(|m| m.tables.get(&t.name))
            .map(|mt| {
                mt.columns
                    .iter()
                    .filter_map(|(n, ty, ..)| match crate::audit::ModelType::parse(ty) {
                        crate::audit::ModelType::Numeric(p, s) => {
                            Some((n.to_ascii_uppercase(), (p, s)))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (table, changes) = observed_table(&t, &shapes, &numeric);
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
                col("SCADAVALUE", Kind::Decimal, 0),
                col("LASTCHANGED", Kind::Timestamp, 2),
                col("WIDE", Kind::Decimal, 0),
                col("COUNT", Kind::Integer, 0),
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
        let model = ModelNumeric::from([
            ("SCADAVALUE".to_owned(), (15, 5)),
            ("NEVER".to_owned(), (3, 0)),
        ]);
        let (out, changed) = observed_table(&t, &[wide, earlier], &model);
        let names: Vec<(&str, u32)> = out
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.id.0))
            .collect();
        assert_eq!(
            &names[..6],
            [
                ("SETTLEMENTDATE", 1),
                ("DUID", 2),
                ("SCADAVALUE", 3),
                ("LASTCHANGED", 4),
                ("WIDE", 5),
                ("COUNT", 6)
            ],
            "in the order the archive first showed them"
        );
        assert_eq!(out.next_column_id as usize, out.columns.len() + 1);
        let ty = |n: &str| out.column(n).unwrap().ty;
        assert_eq!(
            ty("SCADAVALUE"),
            LakeType::Decimal {
                precision: 15,
                scale: 5
            },
            "the model's scale"
        );
        assert_eq!(ty("WIDE"), LakeType::Double, "past 18 digits");
        assert_eq!(
            ty("COUNT"),
            LakeType::Decimal {
                precision: 3,
                scale: 0
            },
            "nothing declares it whole"
        );
        assert!(
            changed
                .decimals
                .contains(&"SCADAVALUE: decimal(15,5)".to_owned()),
            "{changed:?}"
        );
        assert!(
            changed
                .retyped
                .iter()
                .any(|r| r == "SCADAVALUE: double -> decimal(15,5)"),
            "{changed:?}"
        );
        // The key refuses nulls only where every file had it, without one.
        assert!(!out.column("DUID").unwrap().nullable);
        assert!(out.column("LASTCHANGED").unwrap().nullable);
        // Declared whole: a long.
        let whole = ModelNumeric::from([("COUNT".to_owned(), (6, 0))]);
        let (o, _) = observed_table(&t, std::slice::from_ref(&later), &whole);
        assert_eq!(o.column("COUNT").unwrap().ty, LakeType::Long);
    }

    #[test]
    fn a_column_never_valued_takes_the_models_type() {
        let mut t = scada();
        t.increments[0].columns.push("NEVER".into());
        let s = shape(
            "2026-08",
            vec![
                col("SETTLEMENTDATE", Kind::Timestamp, 0),
                col("NEVER", Kind::Empty, 1),
            ],
        );
        let model = ModelNumeric::from([
            ("NEVER".to_owned(), (8, 3)),
            ("SCADAVALUE".to_owned(), (20, 2)),
        ]);
        let (out, _) = observed_table(&t, &[s], &model);
        assert_eq!(
            out.column("NEVER").unwrap().ty,
            LakeType::Decimal {
                precision: 8,
                scale: 3
            }
        );
        // Too wide for a 64-bit decimal: what it was.
        assert_eq!(out.column("SCADAVALUE").unwrap().ty, LakeType::Double);
        let zero = ModelNumeric::from([("NEVER".to_owned(), (3, 0))]);
        let (out, _) = observed_table(
            &t,
            &[shape("2026-08", vec![col("NEVER", Kind::Empty, 1)])],
            &zero,
        );
        assert_eq!(out.column("NEVER").unwrap().ty, LakeType::Long);
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
        let (out, changed) = observed_table(&t, &[s], &ModelNumeric::new());
        assert!(out.column("DUID").unwrap().nullable);
        assert!(
            changed
                .nullability
                .contains(&"DUID: takes nulls".to_owned()),
            "{changed:?}"
        );
    }
}
