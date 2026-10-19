//! What may change between two revisions of a table without breaking the
//! files already in the lake.

use std::collections::BTreeMap;

use crate::file_model::model::Table;

/// Changes between two revisions of a table that the lake does not
/// survive: a column id removed or reused, a type changed other than by
/// an Iceberg promotion, a column made non-nullable, an increment removed
/// or changed, or the key, load mode or time column changed.
pub fn breaking_changes(old: &Table, new: &Table) -> Vec<String> {
    let mut out = Vec::new();
    if new.next_column_id < old.next_column_id {
        out.push(format!(
            "next_column_id went from {} to {}",
            old.next_column_id, new.next_column_id
        ));
    }
    let new_by_id: BTreeMap<_, _> = new.columns.iter().map(|c| (c.id, c)).collect();
    for oc in &old.columns {
        let Some(nc) = new_by_id.get(&oc.id) else {
            out.push(format!(
                "column {} (id {}) was removed; a lake column is never removed",
                oc.name, oc.id
            ));
            continue;
        };
        if nc.name != oc.name {
            out.push(format!(
                "column id {} was renamed from {} to {}; a renamed column is a new column",
                oc.id, oc.name, nc.name
            ));
        }
        if !oc.ty.promotes_to(nc.ty) {
            out.push(format!(
                "column {} changed type from {} to {}, which is not an Iceberg promotion; add a new column",
                oc.name, oc.ty, nc.ty
            ));
        }
        if oc.nullable && !nc.nullable {
            out.push(format!("column {} became non-nullable", oc.name));
        }
    }
    for nc in &new.columns {
        if let Some(oc) = old.columns.iter().find(|c| c.name == nc.name)
            && oc.id != nc.id
        {
            out.push(format!(
                "column {} moved from id {} to id {}",
                nc.name, oc.id, nc.id
            ));
        }
    }
    for oi in &old.increments {
        match new.increment(oi.n) {
            None => out.push(format!("increment {} was removed", oi.n)),
            Some(ni) if ni.from != oi.from || ni.ctl_sha256 != oi.ctl_sha256 => {
                out.push(format!(
                    "increment {} changed its month or its control file; add an increment instead",
                    oi.n
                ));
            }
            _ => {}
        }
    }
    if old.primary_key != new.primary_key {
        out.push(format!(
            "primary_key changed from {:?} to {:?}",
            old.primary_key, new.primary_key
        ));
    }
    if old.load_mode != new.load_mode {
        out.push(format!(
            "load_mode changed from {:?} to {:?}",
            old.load_mode, new.load_mode
        ));
    }
    if old.time_column != new.time_column {
        out.push(format!(
            "time_column changed from {:?} to {:?}",
            old.time_column, new.time_column
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::model::tests::unit_scada;
    use crate::file_model::model::{ColumnId, LakeType, LoadMode};

    fn breaks(edit: impl FnOnce(&mut Table)) -> Vec<String> {
        let mut new = unit_scada();
        edit(&mut new);
        breaking_changes(&unit_scada(), &new)
    }

    #[test]
    fn what_breaks_the_lake() {
        assert!(breaks(|_| {}).is_empty());
        // Our own edits that keep the lake: a new column, a new increment,
        // a fixed increment column list, a note, a column made nullable.
        assert!(
            breaks(|t| {
                t.columns[0].nullable = true;
                t.columns[2].note = Some("why".into());
                t.increments[0].columns.pop();
                t.increments[1].aemo_versions.push(2);
            })
            .is_empty()
        );
        let one = |edit: fn(&mut Table), needle: &str| {
            let b = breaks(edit);
            assert!(b.iter().any(|m| m.contains(needle)), "{needle}: {b:?}");
        };
        one(|t| t.next_column_id = 1, "next_column_id went");
        one(|t| drop(t.columns.remove(3)), "was removed");
        one(|t| t.columns[2].name = "MW".into(), "renamed");
        one(
            |t| t.columns[2].ty = LakeType::Long,
            "not an Iceberg promotion",
        );
        one(|t| t.columns[2].nullable = false, "became non-nullable");
        one(|t| t.columns[2].id = ColumnId(7), "moved from id 3 to id 7");
        one(|t| drop(t.increments.remove(0)), "increment 1 was removed");
        one(
            |t| t.increments[1].from = "2026-09".parse().unwrap(),
            "add an increment instead",
        );
        one(
            |t| t.increments[0].ctl_sha256 = Some("x".into()),
            "add an increment instead",
        );
        one(|t| t.primary_key.pop().map(drop).unwrap(), "primary_key");
        one(|t| t.load_mode = LoadMode::Snapshot, "load_mode");
        one(|t| t.time_column = None, "time_column");
    }
}
