//! The census gate: every shape the census recorded for every month must
//! be one the registry reads as declared, and every type class a control
//! file ever loaded a column as must fit the type the anchor gives it.
//!
//! A shape passes when it resolves, in the month it was seen, exactly (to
//! a declared version or variant) or through a hand-written rule. Anything
//! else is a place a rule or a declaration is needed before the months are
//! backfilled. A class that does not widen into the anchor's type is
//! reported with the months it was seen in; nothing is widened here.
//!
//! Every finding is one JSON diagnostic per line.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};
use nemlake_census::census::{census_dir, read_month};
use nemlake_core::{Category, Diagnostic, IRow, ShapeId, TypeClass, YearMonth};
use nemlake_registry::file_model::compat::class_fits;
use nemlake_registry::file_model::shapes::ShapeEntry;
use nemlake_registry::{Mode, Origin, Registry, ResolveOptions, Status, TypeOptions};

/// The name of a declared type's class, as a table file writes it.
fn months_text(months: &BTreeSet<YearMonth>) -> String {
    months
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every finding of the census of `months` (file name to shape ids, per
/// month) against `registry`, whose census holds the shapes.
pub fn check(
    registry: &Registry,
    months: &BTreeMap<YearMonth, BTreeMap<String, Vec<ShapeId>>>,
) -> Vec<Diagnostic> {
    let by_id: BTreeMap<ShapeId, (&str, &ShapeEntry)> = registry
        .census()
        .iter()
        .flat_map(|(t, shapes)| shapes.iter().map(move |e| (e.id, (t.as_str(), e))))
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
    // Per anchor column and class that does not fit: the months.
    let mut regressions: BTreeMap<(String, String, TypeClass), BTreeSet<YearMonth>> =
        BTreeMap::new();
    for (id, (shape_months, file)) in &seen {
        let Some((table, entry)) = by_id.get(id) else {
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
        let irow = IRow {
            report_type: &entry.report_type,
            report_subtype: &entry.report_subtype,
            version: entry.version,
            columns: &names,
        };
        for &month in shape_months {
            let res = registry.resolve_in(
                &irow,
                ResolveOptions {
                    mode: Mode::Permissive,
                },
                Some(month),
            );
            let res = match res {
                Ok(r) if r.status == Status::Exact || !r.routed.is_empty() => r,
                other => {
                    let why = match &other {
                        Ok(r) => match &r.status {
                            Status::Drift {
                                missing,
                                extra,
                                reordered,
                            } => format!(
                                "it drifts from version {}: missing {missing:?}, unknown {extra:?}{}",
                                r.version,
                                if *reordered { ", reordered" } else { "" }
                            ),
                            Status::Aliased { matched } => format!(
                                "its version {} is not declared; its columns are those of version {matched}",
                                entry.version
                            ),
                            _ => "no table claims its report pair".to_owned(),
                        },
                        Err(e) => e.to_string(),
                    };
                    let mut d = Diagnostic::error(
                        "schema.census_unresolved",
                        Category::Schema,
                        format!(
                            "Shape {id} of table {table}, seen in {} ({file} and others), is not declared and no rule reads it: {why}. Declare it or write a rule for it.",
                            months_text(shape_months)
                        ),
                    )
                    .with_table((*table).to_owned())
                    .with_version(entry.version)
                    .with_entry(file.clone());
                    if let Ok(r) = &other
                        && let Some(v) = registry.table(r.table).version(r.version)
                    {
                        d.expected = Some(v.columns.join(","));
                    }
                    d.observed = Some(names.join(","));
                    out.push(d);
                    break;
                }
            };
            let t = registry.table(res.table);
            for (route, (_, class)) in res.plan.routes.iter().zip(&entry.columns) {
                if !matches!(route.origin, Origin::Declared | Origin::Rule) {
                    continue;
                }
                let Some(anchor) = route.target_name.as_deref().and_then(|n| t.column(n)) else {
                    continue;
                };
                let target = &anchor.name;
                if !class_fits(*class, anchor.ty) {
                    regressions
                        .entry((t.name.clone(), target.clone(), *class))
                        .or_default()
                        .insert(month);
                }
            }
        }
    }
    for ((table, column, class), months) in regressions {
        let anchor = registry
            .table_by_name(&table)
            .and_then(|id| registry.table(id).column(&column))
            .map_or("", |c| c.ty.kind_name());
        let mut d = Diagnostic::error(
            "schema.census_type_regression",
            Category::Schema,
            format!(
                "Column {column} of table {table} is declared {anchor}, but the control files of {} load it as {class}, which does not widen into it. Nothing is widened by this check: declare the type, or write a rule for those months.",
                months_text(&months)
            ),
        )
        .with_table(table)
        .with_column(column);
        d.expected = Some(anchor.to_owned());
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
    let registry = Registry::load_dir(&schemas, TypeOptions::default())
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
    use nemlake_registry::file_model::blob::SchemaSet;

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
        let embedded = Registry::load_embedded(TypeOptions::default());
        let t = embedded
            .table(embedded.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone();
        Registry::from_set(
            SchemaSet {
                markets: vec![],
                tables: vec![t],
                shapes: [("DISPATCH_UNIT_SCADA".to_owned(), shapes)].into(),
            },
            TypeOptions::default(),
        )
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

    const DECLARED: [(&str, TypeClass); 4] = [
        ("SETTLEMENTDATE", TypeClass::Timestamp),
        ("DUID", TypeClass::Varchar),
        ("SCADAVALUE", TypeClass::Number),
        ("LASTCHANGED", TypeClass::Timestamp),
    ];

    #[test]
    fn a_declared_census_passes() {
        let ok = entry(&DECLARED, 1, "2026-08");
        let r = registry(vec![ok.clone()]);
        assert!(check(&r, &census(&[("2026-08", "A.zip", ok.id)])).is_empty());
    }

    #[test]
    fn a_drifted_shape_fails_with_its_columns() {
        let mut cols = DECLARED.to_vec();
        cols.push(("QUALITY", TypeClass::Varchar));
        let drifted = entry(&cols, 1, "2015-03");
        let r = registry(vec![drifted.clone()]);
        let found = check(
            &r,
            &census(&[
                ("2015-03", "B.zip", drifted.id),
                ("2015-04", "C.zip", drifted.id),
            ]),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        let d = &found[0];
        assert_eq!(d.code, "schema.census_unresolved");
        assert_eq!(d.table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
        assert!(d.message.contains(&drifted.id.to_string()), "{}", d.message);
        assert!(d.message.contains("2015-03, 2015-04"), "{}", d.message);
        assert!(d.message.contains("\"QUALITY\""), "{}", d.message);
        assert_eq!(
            d.expected.as_deref(),
            Some("SETTLEMENTDATE,DUID,SCADAVALUE,LASTCHANGED")
        );
        assert!(d.observed.as_deref().unwrap().ends_with(",QUALITY"));
        // An id no shapes file records.
        let found = check(&r, &census(&[("2015-03", "D.zip", ShapeId(42))]));
        assert_eq!(found[0].code, "schema.census_unknown_shape");
    }

    #[test]
    fn a_type_that_does_not_widen_is_reported_with_its_months() {
        let mut cols = DECLARED.to_vec();
        cols[2].1 = TypeClass::Varchar;
        let texty = entry(&cols, 1, "2011-01");
        let r = registry(vec![texty.clone()]);
        let found = check(
            &r,
            &census(&[
                ("2011-01", "E.zip", texty.id),
                ("2011-02", "F.zip", texty.id),
            ]),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        let d = &found[0];
        assert_eq!(d.code, "schema.census_type_regression");
        assert_eq!(d.column.as_deref(), Some("SCADAVALUE"));
        assert_eq!(d.observed.as_deref(), Some("varchar"));
        assert!(d.message.contains("2011-01, 2011-02"), "{}", d.message);
        // A class that widens into the anchor is fine: a date into a
        // timestamp; an unknown class says nothing.
        cols[2].1 = TypeClass::Unknown;
        cols[0].1 = TypeClass::Date;
        let fine = entry(&cols, 1, "2011-03");
        let r = registry(vec![fine.clone()]);
        assert!(check(&r, &census(&[("2011-03", "G.zip", fine.id)])).is_empty());
    }
}
