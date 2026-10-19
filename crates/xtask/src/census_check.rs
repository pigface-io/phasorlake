//! The census gate: the evidence against our schema. For every block the
//! census recorded in month M:
//!
//! - its table exists;
//! - it conforms to the table's increment in force at M;
//! - each type class a control file loads one of its columns as fits the
//!   type our schema gives the column.
//!
//! Each finding names the fix, which goes in the table's `table.toml`:
//! the increment's `columns`, or the column's `type`. Nothing is changed
//! here. Every finding is one JSON diagnostic per line.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};
use utilake_census::census::{census_dir, read_month};
use utilake_core::{Category, Diagnostic, YearMonth};
use utilake_mms_decode::IRow;
use utilake_mms_registry::file_model::shapes::ShapeEntry;
use utilake_mms_registry::{Mode, Registry, ResolveOptions, Status};
use utilake_mms_registry::{ShapeId, TypeClass};

/// Months as a list for a message.
fn months_text(months: &BTreeSet<YearMonth>) -> String {
    months
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The file a table's schema is in, for a message.
fn table_file(table: &str) -> String {
    format!("schemas/nem/{table}/table.toml")
}

/// Every finding of the census of `months` (file name to shape ids, per
/// month) against `registry`, whose census holds the shapes.
pub fn check(
    registry: &Registry,
    months: &BTreeMap<YearMonth, BTreeMap<String, Vec<ShapeId>>>,
) -> Vec<Diagnostic> {
    let by_id: BTreeMap<ShapeId, &ShapeEntry> = registry
        .census()
        .values()
        .flatten()
        .map(|e| (e.id, e))
        .collect();
    let mut out = Vec::new();
    // Per shape, the months it was seen in and the first file.
    let mut seen: BTreeMap<ShapeId, (BTreeSet<YearMonth>, String)> = BTreeMap::new();
    for (month, files) in months {
        for (file, ids) in files {
            for id in ids {
                let s = seen
                    .entry(*id)
                    .or_insert_with(|| (BTreeSet::new(), file.clone()));
                s.0.insert(*month);
            }
        }
    }
    // Per table, column and class that does not fit: the months.
    let mut regressions: BTreeMap<(String, String, TypeClass), BTreeSet<YearMonth>> =
        BTreeMap::new();
    // Per table, increment and shape that deviates from it: the months.
    let mut deviations: BTreeMap<(String, u16, ShapeId), (BTreeSet<YearMonth>, Diagnostic)> =
        BTreeMap::new();
    for (id, (shape_months, file)) in &seen {
        let Some(entry) = by_id.get(id) else {
            out.push(
                Diagnostic::error(
                    "schema.census_unknown_shape",
                    Category::Schema,
                    format!(
                        "The census of {} names shape {id} for {file}, but no shapes file records it; take the census of those months again.",
                        months_text(shape_months)
                    ),
                )
                .with_entry(file.clone()),
            );
            continue;
        };
        let names: Vec<&str> = entry.columns.iter().map(|(n, _)| n.as_str()).collect();
        let Some(tid) = registry.declared_by_pair(&entry.report_type, &entry.report_subtype) else {
            out.push(
                Diagnostic::error(
                    "schema.census_deviates",
                    Category::Schema,
                    format!(
                        "Shape {id} ({}, {}), seen in {} ({file} and others), belongs to no table; `utilake-census sync` adds the table from the month's control file.",
                        entry.report_type,
                        entry.report_subtype,
                        months_text(shape_months)
                    ),
                )
                .with_version(entry.version)
                .with_entry(file.clone()),
            );
            continue;
        };
        let t = registry.table(tid);
        let irow = IRow {
            report_type: &entry.report_type,
            report_subtype: &entry.report_subtype,
            version: entry.version,
            columns: &names,
        };
        for &month in shape_months {
            let res = registry
                .resolve_in(
                    &irow,
                    ResolveOptions {
                        mode: Mode::Permissive,
                    },
                    Some(month),
                )
                .expect("a table's shape resolves permissively");
            if let Status::Deviates {
                missing,
                extra,
                reordered,
            } = &res.status
            {
                let key = (t.name.clone(), res.version, *id);
                let inc = t.increment(res.version).expect("the increment in force");
                deviations
                    .entry(key)
                    .or_insert_with(|| {
                        let mut d = Diagnostic::error(
                            "schema.census_deviates",
                            Category::Schema,
                            String::new(),
                        )
                        .with_table(t.name.clone())
                        .with_version(entry.version)
                        .with_entry(file.clone());
                        d.message = format!(
                            "Shape {id} of table {} deviates from {}@{}: missing {missing:?}, not listed {extra:?}{}. If the files are right, edit increment {}'s columns in {}.",
                            t.name,
                            t.name,
                            inc.n,
                            if *reordered { ", reordered" } else { "" },
                            inc.n,
                            table_file(&t.name)
                        );
                        d.expected = Some(inc.columns.join(","));
                        d.observed = Some(names.join(","));
                        (BTreeSet::new(), d)
                    })
                    .0
                    .insert(month);
            }
            for (name, class) in &entry.columns {
                if let Some(c) = t.column(name)
                    && !c.ty.holds(*class)
                {
                    regressions
                        .entry((t.name.clone(), c.name.clone(), *class))
                        .or_default()
                        .insert(month);
                }
            }
        }
    }
    for (_, (months, mut d)) in deviations {
        d.message = format!("{} Seen in {}.", d.message, months_text(&months));
        out.push(d);
    }
    for ((table, column, class), months) in regressions {
        let ty = registry
            .table_by_name(&table)
            .and_then(|id| registry.table(id).column(&column))
            .map_or_else(String::new, |c| c.ty.to_string());
        let mut d = Diagnostic::error(
            "schema.census_type_regression",
            Category::Schema,
            format!(
                "Column {column} of table {table} is {ty}, but the control files of {} load it as {class}, which {ty} does not hold. Edit the column's type in {}, or add a column for those values.",
                months_text(&months),
                table_file(&table)
            ),
        )
        .with_table(table)
        .with_column(column);
        d.expected = Some(ty);
        d.observed = Some(class.to_string());
        out.push(d);
    }
    out
}

/// Every month's census under `schemas`, file name to shape ids.
fn read_months(schemas: &Path) -> Result<BTreeMap<YearMonth, BTreeMap<String, Vec<ShapeId>>>> {
    let dir = census_dir(schemas);
    let mut months = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(months);
    };
    for e in rd {
        let p = e?.path();
        let Some(month) = p
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<YearMonth>().ok())
        else {
            continue;
        };
        let files = read_month(schemas, month)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .unwrap_or_default();
        months.insert(
            month,
            files
                .into_iter()
                .map(|(f, entry)| (f, entry.ids()))
                .collect(),
        );
    }
    Ok(months)
}

/// `xtask census-check`: passes when no census is recorded.
pub fn run() -> Result<bool> {
    let schemas = crate::repo_root().join("schemas");
    let months = read_months(&schemas)?;
    if months.is_empty() {
        println!("census-check: no census recorded under schemas/nem/census; nothing to check");
        return Ok(true);
    }
    let registry = Registry::load_dir(&schemas)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("loading schemas/")?;
    let found = check(&registry, &months);
    for d in &found {
        eprintln!("{}", serde_json::to_string(d)?);
    }
    let shapes: usize = months.values().flat_map(|f| f.values()).map(Vec::len).sum();
    println!(
        "census-check: {} month(s), {shapes} block(s), {} finding(s)",
        months.len(),
        found.len()
    );
    Ok(found.is_empty())
}

#[cfg(test)]
mod tests {
    use utilake_mms_registry::file_model::blob::SchemaSet;

    use super::*;

    fn entry(columns: &[(&str, TypeClass)], version: u16, month: &str) -> ShapeEntry {
        let mut e = ShapeEntry {
            id: ShapeId(0),
            report_type: "DISPATCH".into(),
            report_subtype: "UNIT_SCADA".into(),
            version,
            first_seen: month.parse().unwrap(),
            last_seen: month.parse().unwrap(),
            columns: columns.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect(),
        };
        e.id = e.shape().id();
        e
    }

    fn registry(shapes: Vec<ShapeEntry>) -> Registry {
        let embedded = Registry::load_embedded();
        let t = embedded
            .table(embedded.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone();
        Registry::from_set(SchemaSet {
            tables: vec![t],
            shapes: [("DISPATCH_UNIT_SCADA".to_owned(), shapes)].into(),
            ..Default::default()
        })
    }

    fn census(
        rows: &[(&str, &str, ShapeId)],
    ) -> BTreeMap<YearMonth, BTreeMap<String, Vec<ShapeId>>> {
        let mut out: BTreeMap<YearMonth, BTreeMap<String, Vec<ShapeId>>> = BTreeMap::new();
        for (month, file, id) in rows {
            out.entry(month.parse().unwrap())
                .or_default()
                .entry((*file).to_owned())
                .or_default()
                .push(*id);
        }
        out
    }

    /// The 2026-08 increment of DISPATCH_UNIT_SCADA; 2024-01's lacks
    /// LASTCHANGED.
    const DECLARED: [(&str, TypeClass); 4] = [
        ("SETTLEMENTDATE", TypeClass::Timestamp),
        ("DUID", TypeClass::Varchar),
        ("SCADAVALUE", TypeClass::Number),
        ("LASTCHANGED", TypeClass::Timestamp),
    ];

    #[test]
    fn a_conforming_census_passes() {
        let now = entry(&DECLARED, 1, "2026-08");
        let then = entry(&DECLARED[..3], 1, "2024-01");
        let r = registry(vec![now.clone(), then.clone()]);
        let c = census(&[("2026-08", "A.zip", now.id), ("2024-01", "B.zip", then.id)]);
        assert!(check(&r, &c).is_empty(), "{:?}", check(&r, &c));
    }

    #[test]
    fn a_deviating_shape_fails_with_its_months_and_fix() {
        let mut cols = DECLARED[..3].to_vec();
        cols.push(("QUALITY", TypeClass::Varchar));
        let odd = entry(&cols, 1, "2015-03");
        let r = registry(vec![odd.clone()]);
        let found = check(
            &r,
            &census(&[("2015-03", "B.zip", odd.id), ("2015-04", "C.zip", odd.id)]),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        let d = &found[0];
        assert_eq!(d.code, "schema.census_deviates");
        assert_eq!(d.table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
        assert!(d.message.contains(&odd.id.to_string()), "{}", d.message);
        assert!(d.message.contains("DISPATCH_UNIT_SCADA@1"), "{}", d.message);
        assert!(d.message.contains("2015-03, 2015-04"), "{}", d.message);
        assert!(d.message.contains("\"QUALITY\""), "{}", d.message);
        assert!(
            d.message
                .contains("schemas/nem/DISPATCH_UNIT_SCADA/table.toml"),
            "{}",
            d.message
        );
        assert_eq!(
            d.expected.as_deref(),
            Some("SETTLEMENTDATE,DUID,SCADAVALUE")
        );
        assert!(d.observed.as_deref().unwrap().ends_with(",QUALITY"));
        // The 2026 shape is a deviation in 2024: the month decides.
        let now = entry(&DECLARED, 1, "2026-08");
        let r = registry(vec![now.clone()]);
        let found = check(&r, &census(&[("2024-06", "D.zip", now.id)]));
        assert_eq!(found[0].code, "schema.census_deviates");
        // An id no shapes file records.
        let found = check(&r, &census(&[("2015-03", "D.zip", ShapeId(42))]));
        assert_eq!(found[0].code, "schema.census_unknown_shape");
        // A shape of a report pair no table claims.
        let mut stray = entry(&DECLARED, 1, "2026-08");
        stray.report_subtype = "NOBODY".into();
        stray.id = stray.shape().id();
        let r = registry(vec![stray.clone()]);
        let found = check(&r, &census(&[("2026-08", "E.zip", stray.id)]));
        assert!(
            found[0].message.contains("belongs to no table"),
            "{found:?}"
        );
    }

    #[test]
    fn a_type_our_column_does_not_hold_is_reported_with_its_months() {
        let mut cols = DECLARED.to_vec();
        cols[2].1 = TypeClass::Varchar;
        let texty = entry(&cols, 1, "2026-08");
        let r = registry(vec![texty.clone()]);
        let found = check(
            &r,
            &census(&[
                ("2026-08", "E.zip", texty.id),
                ("2026-09", "F.zip", texty.id),
            ]),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        let d = &found[0];
        assert_eq!(d.code, "schema.census_type_regression");
        assert_eq!(d.column.as_deref(), Some("SCADAVALUE"));
        assert_eq!(d.expected.as_deref(), Some("double"));
        assert_eq!(d.observed.as_deref(), Some("varchar"));
        assert!(d.message.contains("2026-08, 2026-09"), "{}", d.message);
        // A date into a timestamp is held; an unknown class says nothing.
        cols[2].1 = TypeClass::Unknown;
        cols[0].1 = TypeClass::Date;
        let fine = entry(&cols, 1, "2026-10");
        let r = registry(vec![fine.clone()]);
        assert!(check(&r, &census(&[("2026-10", "G.zip", fine.id)])).is_empty());
    }
}
