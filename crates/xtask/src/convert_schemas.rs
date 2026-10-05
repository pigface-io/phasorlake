//! One-off: convert `schemas/nem/mms/*.toml` (format 1) into
//! `schemas/nem/{TABLE}/table.toml` (format 2) with the bootstrap months'
//! control files as increments. Removed with the old files.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use nemlake_core::YearMonth;
use nemlake_registry::file_model::blob::SchemaSet as OldSet;
use nemlake_registry::file_model::ctl::{CtlFile, parse, sha256};
use nemlake_registry::file_model::v2::file::{CTL_DIR, TABLE_FILE, ctl_file_name, write_table};
use nemlake_registry::file_model::v2::model::{Column, ColumnId, Increment, LakeType, Table};
use nemlake_registry::file_model::{ColumnType, TableDef};
use nemlake_registry::{Registry, TypeOptions};

/// The bootstrap months whose control files are the first increments,
/// oldest first.
const MONTHS: [(&str, &str); 2] = [
    ("2024-01", "testdata/bootstrap/MMSDM_2024_01/CTL"),
    ("2026-08", "testdata/bootstrap/MMSDM_2026_08/CTL"),
];

/// A control file of one month: AEMO's file name, its bytes, parsed.
struct Ctl {
    name: String,
    bytes: Vec<u8>,
    parsed: CtlFile,
}

/// Each month's control files by the table they load, the first by file
/// name standing for the table (`FILE01`, or split `1`).
fn month_ctls(root: &Path) -> Result<Vec<(YearMonth, BTreeMap<String, Ctl>)>> {
    let mut out = Vec::new();
    for (month, dir) in MONTHS {
        let mut files: Vec<_> = std::fs::read_dir(root.join(dir))
            .with_context(|| format!("read {dir}"))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "ctl"))
            .collect();
        files.sort();
        let mut by_table: BTreeMap<String, Ctl> = BTreeMap::new();
        for p in files {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let bytes = std::fs::read(&p)?;
            let parsed = parse(&name, &String::from_utf8_lossy(&bytes))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let table = parsed.table.to_ascii_uppercase();
            match by_table.get(&table) {
                Some(first) if !first.parsed.same_layout(&parsed) => {
                    eprintln!(
                        "note: {table} in {month}: {name} differs from {}; the first stands",
                        first.name
                    );
                }
                Some(_) => {}
                None => {
                    by_table.insert(
                        table,
                        Ctl {
                            name,
                            bytes,
                            parsed,
                        },
                    );
                }
            }
        }
        out.push((month.parse()?, by_table));
    }
    Ok(out)
}

fn lake_type(ty: ColumnType) -> LakeType {
    match ty {
        ColumnType::Utf8 { .. } => LakeType::String,
        ColumnType::Number { .. } => LakeType::Double,
        ColumnType::Integer => LakeType::Long,
        ColumnType::Timestamp => LakeType::TimestampNs,
        ColumnType::Date => LakeType::Date,
        ColumnType::Boolean => LakeType::Boolean,
    }
}

/// The table, and the control files of its increments in order.
fn convert(
    old: &TableDef,
    months: &[(YearMonth, BTreeMap<String, Ctl>)],
) -> Result<(Table, Vec<Vec<u8>>)> {
    if old
        .columns
        .iter()
        .any(|c| !c.aliases.is_empty() || c.is_raw())
        || !old.rules.is_empty()
    {
        bail!("{} has an alias, a raw column or a rule", old.name);
    }
    let mut t = Table {
        market: old.market,
        name: old.name.clone(),
        description: old.description.clone(),
        report_pairs: old.report_pairs.clone(),
        load_mode: old.load_mode,
        dedupe: old.dedupe,
        primary_key: old.primary_key.clone(),
        time_column: old.time_column.clone(),
        next_column_id: old.next_column_id,
        columns: old
            .columns
            .iter()
            .map(|c| Column {
                id: c.id,
                name: c.name.clone(),
                ty: lake_type(c.ty),
                nullable: c.nullable,
                note: None,
            })
            .collect(),
        increments: Vec::new(),
    };
    // A month whose control file lays out what the one before did adds no
    // increment.
    let mut picked: Vec<(YearMonth, &Ctl)> = Vec::new();
    for (month, ctls) in months {
        if let Some(c) = ctls.get(&old.name)
            && picked
                .last()
                .is_none_or(|(_, p)| !p.parsed.same_layout(&c.parsed))
        {
            picked.push((*month, c));
        }
    }
    if picked.is_empty() {
        bail!("{} has no control file in either bootstrap month", old.name);
    }
    let mut files = Vec::new();
    for (k, (from, c)) in picked.iter().enumerate() {
        let n = (k + 1) as u16;
        let until = picked.get(k + 1).map(|(m, _)| *m);
        let columns = c.parsed.column_names();
        for (name, ty) in columns.iter().zip(c.parsed.data_columns()) {
            if t.column(name).is_none() {
                eprintln!("note: {} gains column {name} from {}", t.name, c.name);
                t.columns.push(Column {
                    id: ColumnId(t.next_column_id),
                    name: name.clone(),
                    ty: ty.ty.default_type().expect("a data column"),
                    nullable: true,
                    note: None,
                });
                t.next_column_id += 1;
            }
        }
        // The I-row versions seen in the months the increment is in force.
        let mut versions: Vec<u16> = old
            .versions
            .iter()
            .filter(|v| {
                let first = v.first_seen.unwrap_or(*from);
                let last = v.last_seen.unwrap_or(first);
                last >= *from && until.is_none_or(|u| first < u)
            })
            .map(|v| v.n.0)
            .collect();
        versions.dedup();
        t.increments.push(Increment {
            n,
            from: *from,
            ctl: Some(ctl_file_name(n)),
            ctl_sha256: Some(sha256(&c.bytes)),
            aemo_ctl: Some(c.name.clone()),
            aemo_versions: versions,
            columns,
            note: None,
        });
        files.push(c.bytes.clone());
    }
    // A key column an increment lacks takes nulls in that increment's files.
    for c in &mut t.columns {
        if !c.nullable && t.increments.iter().any(|i| !i.columns.contains(&c.name)) {
            eprintln!(
                "note: {} key column {} is missing from an increment; made nullable",
                t.name, c.name
            );
            c.nullable = true;
        }
    }
    Ok((t, files))
}

/// Every table's lake schema as `(name, field id, Arrow type, nullable)`.
fn fields_old(r: &Registry, name: &str) -> Vec<(String, String, String, bool)> {
    let id = r.table_by_name(name).expect("old table");
    r.lake_schema(id)
        .fields()
        .iter()
        .map(|f| {
            (
                f.name().clone(),
                f.metadata()["PARQUET:field_id"].clone(),
                f.data_type().to_string(),
                f.is_nullable(),
            )
        })
        .collect()
}

pub fn run() -> Result<bool> {
    let root = crate::repo_root();
    let schemas = root.join("schemas");
    let old = OldSet::read_dir(&schemas)?;
    let registry = Registry::from_set(old.clone(), TypeOptions::default());
    let market = old.markets[0].clone();
    let months = month_ctls(&root)?;
    let mut ok = true;
    for t in &old.tables {
        let (new, files) = convert(t, &months)?;
        // Every old column keeps its id, name and Arrow type.
        let before = fields_old(&registry, &t.name);
        for (name, id, ty, nullable) in &before {
            let c = new
                .columns
                .iter()
                .find(|c| c.id.0.to_string() == *id)
                .with_context(|| format!("{}: id {id} lost", t.name))?;
            let new_ty = c.ty.arrow(&market).to_string();
            if c.name != *name || new_ty != *ty {
                eprintln!(
                    "{}: column {id} was {name} {ty}, now {} {new_ty}",
                    t.name, c.name
                );
                ok = false;
            }
            if c.nullable != *nullable {
                eprintln!(
                    "note: {}: {name} nullable {nullable} -> {}",
                    t.name, c.nullable
                );
            }
        }
        let dir = schemas.join("nem").join(&t.name);
        std::fs::create_dir_all(dir.join(CTL_DIR))?;
        for (i, bytes) in new.increments.iter().zip(&files) {
            std::fs::write(dir.join(CTL_DIR).join(ctl_file_name(i.n)), bytes)?;
        }
        std::fs::write(dir.join(TABLE_FILE), write_table(&new))?;
    }
    let set = nemlake_registry::file_model::v2::blob::SchemaSet::read_dir(&schemas)?;
    let issues = set.validate();
    for i in &issues {
        eprintln!("invalid: {i}");
    }
    eprintln!(
        "{} table(s) converted, {} increment(s)",
        set.tables.len(),
        set.tables.iter().map(|t| t.increments.len()).sum::<usize>()
    );
    Ok(ok && issues.is_empty() && set.tables.len() == old.tables.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed tree is what the conversion gives, and keeps every old
    /// column's id, name and Arrow type.
    #[test]
    fn the_committed_tree_is_the_conversion() {
        let root = crate::repo_root();
        let schemas = root.join("schemas");
        let old = OldSet::read_dir(&schemas).unwrap();
        let registry = Registry::from_set(old.clone(), TypeOptions::default());
        let months = month_ctls(&root).unwrap();
        let market = old.markets[0].clone();
        for t in &old.tables {
            let (new, _) = convert(t, &months).unwrap();
            let dir = schemas.join("nem").join(&t.name);
            let on_disk =
                nemlake_registry::file_model::v2::file::read_table(&dir, t.market).unwrap();
            assert_eq!(on_disk, new, "{}", t.name);
            for (name, id, ty, _) in fields_old(&registry, &t.name) {
                let c = new
                    .columns
                    .iter()
                    .find(|c| c.id.0.to_string() == id)
                    .unwrap();
                assert_eq!((&c.name, c.ty.arrow(&market).to_string()), (&name, ty));
            }
        }
        let set = nemlake_registry::file_model::v2::blob::SchemaSet::read_dir(&schemas).unwrap();
        assert!(set.validate().is_empty(), "{:?}", set.validate());
        assert_eq!(set.tables.len(), old.tables.len());
    }
}
