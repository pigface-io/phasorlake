//! Evolution rules: what may change between two versions of a table, or
//! between two revisions of the same schema file.

use std::collections::BTreeMap;

use crate::model::{ColumnType, SchemaVersion, TableDef};

/// One difference between two versions of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A column appears in `to` but not `from`.
    Added(String),
    /// A column appears in `from` but not `to`.
    Removed(String),
    /// Both have the same columns in a different order.
    Reordered,
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Added(c) => write!(f, "added column {c}"),
            Self::Removed(c) => write!(f, "removed column {c}"),
            Self::Reordered => write!(f, "reordered columns"),
        }
    }
}

/// The differences between version `from` and version `to` of one table.
///
/// Returns `None` when either version is undeclared.
pub fn diff_versions(t: &TableDef, from: SchemaVersion, to: SchemaVersion) -> Option<Vec<Change>> {
    let a = t.version(from)?;
    let b = t.version(to)?;
    let mut out = Vec::new();
    for c in &b.columns {
        if !a.columns.contains(c) {
            out.push(Change::Added(c.clone()));
        }
    }
    for c in &a.columns {
        if !b.columns.contains(c) {
            out.push(Change::Removed(c.clone()));
        }
    }
    if out.is_empty() && a.columns != b.columns {
        out.push(Change::Reordered);
    } else if !out.is_empty() {
        // Order among the shared columns also matters for a reader of raw files.
        let shared_a: Vec<&String> = a.columns.iter().filter(|c| b.columns.contains(c)).collect();
        let shared_b: Vec<&String> = b.columns.iter().filter(|c| a.columns.contains(c)).collect();
        if shared_a != shared_b {
            out.push(Change::Reordered);
        }
    }
    Some(out)
}

/// Whether `to` is a type the lake may widen `from` into without rewriting
/// old files: the same class, never narrower.
pub fn is_widening(from: ColumnType, to: ColumnType) -> bool {
    use ColumnType::{Boolean, Date, Integer, Number, Timestamp, Utf8};
    match (from, to) {
        (Integer, Integer | Number { .. }) => true,
        (Number { precision: p0, scale: s0 }, Number { precision: p1, scale: s1 }) => match (p0, s0, p1, s1) {
            (_, _, None, None) => true,
            (Some(p0), Some(s0), Some(p1), Some(s1)) => p1 - s1 >= p0 - s0 && s1 >= s0,
            (None, None, Some(_), Some(_)) => false,
            _ => p1 >= p0 && s1 >= s0,
        },
        (Utf8 { max_len: a, .. }, Utf8 { max_len: b, .. }) => match (a, b) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(a), Some(b)) => b >= a,
        },
        (Timestamp, Timestamp) | (Date, Date | Timestamp) | (Boolean, Boolean) => true,
        _ => false,
    }
}

/// Changes to a table between two revisions of its schema file that would
/// invalidate files already in the lake or break the registry's promises.
pub fn breaking_changes(old: &TableDef, new: &TableDef) -> Vec<String> {
    let mut out = Vec::new();
    if new.next_column_id < old.next_column_id {
        out.push(format!("next_column_id went from {} to {}", old.next_column_id, new.next_column_id));
    }
    let new_by_id: BTreeMap<_, _> = new.columns.iter().map(|c| (c.id, c)).collect();
    for oc in &old.columns {
        match new_by_id.get(&oc.id) {
            None => out.push(format!("column {} (id {}) was deleted; mark it removed_in instead", oc.name, oc.id)),
            Some(nc) => {
                if nc.name != oc.name && !nc.aliases.contains(&oc.name) {
                    out.push(format!(
                        "column id {} was renamed from {} to {} without keeping the old name as an alias",
                        oc.id, oc.name, nc.name
                    ));
                }
                if !is_widening(oc.ty, nc.ty) {
                    out.push(format!("column {} changed type from {:?} to {:?}, which is not a widening", oc.name, oc.ty, nc.ty));
                }
                if oc.nullable && !nc.nullable {
                    out.push(format!("column {} became non-nullable", oc.name));
                }
            }
        }
    }
    let mut old_names: Vec<_> = old.columns.iter().map(|c| (c.name.as_str(), c.id)).collect();
    old_names.sort_unstable();
    for nc in &new.columns {
        if let Some((_, old_id)) = old_names.iter().find(|(n, id)| *n == nc.name && *id != nc.id) {
            out.push(format!("column {} moved from id {} to id {}", nc.name, old_id, nc.id));
        }
    }
    for ov in &old.versions {
        match new.version(ov.n) {
            None => out.push(format!("version {} was deleted", ov.n)),
            Some(nv) if nv.fingerprint != ov.fingerprint => {
                out.push(format!("version {} changed its column list; a published version is a fact", ov.n));
            }
            _ => {}
        }
    }
    if old.primary_key != new.primary_key {
        out.push(format!("primary_key changed from {:?} to {:?}", old.primary_key, new.primary_key));
    }
    if old.load_mode != new.load_mode {
        out.push(format!("load_mode changed from {:?} to {:?}", old.load_mode, new.load_mode));
    }
    if old.time_column != new.time_column {
        out.push(format!("time_column changed from {:?} to {:?}", old.time_column, new.time_column));
    }
    out
}

#[cfg(test)]
mod tests {
    use nemlake_core::Fingerprint;

    use super::*;
    use crate::file::tests::unit_scada;
    use crate::model::{ColumnDef, ColumnId, VersionDef};

    fn with_v2(extra: &str) -> TableDef {
        let mut t = unit_scada();
        let mut cols = t.versions[0].columns.clone();
        cols.push(extra.to_owned());
        t.columns.push(ColumnDef {
            id: ColumnId(4),
            name: extra.to_owned(),
            aliases: vec![],
            ty: ColumnType::Timestamp,
            nullable: true,
            oracle: None,
            loader: None,
            added_in: SchemaVersion(2),
            removed_in: None,
            description: None,
            src: vec![],
            curated: false,
        });
        t.next_column_id = 5;
        t.versions.push(VersionDef {
            n: SchemaVersion(2),
            fingerprint: Fingerprint::of_names(cols.iter().map(String::as_str)),
            columns: cols,
            first_seen: None,
            last_seen: None,
            src: None,
        });
        t
    }

    #[test]
    fn diffs() {
        let t = with_v2("LASTCHANGED");
        let d = diff_versions(&t, SchemaVersion(1), SchemaVersion(2)).unwrap();
        assert_eq!(d, vec![Change::Added("LASTCHANGED".into())]);
        let d = diff_versions(&t, SchemaVersion(2), SchemaVersion(1)).unwrap();
        assert_eq!(d, vec![Change::Removed("LASTCHANGED".into())]);
        assert!(diff_versions(&t, SchemaVersion(1), SchemaVersion(9)).is_none());
        assert_eq!(Change::Reordered.to_string(), "reordered columns");
        assert!(Change::Added("A".into()).to_string().contains("added"));
        assert!(Change::Removed("A".into()).to_string().contains("removed"));
        // Pure reorder.
        let mut t2 = t.clone();
        t2.versions[1].columns = vec!["DUID".into(), "SETTLEMENTDATE".into(), "SCADAVALUE".into()];
        assert_eq!(diff_versions(&t2, SchemaVersion(1), SchemaVersion(2)).unwrap(), vec![Change::Reordered]);
        // Add plus reorder of the shared part.
        let mut t3 = t.clone();
        t3.versions[1].columns = vec!["DUID".into(), "SETTLEMENTDATE".into(), "SCADAVALUE".into(), "X".into()];
        let d = diff_versions(&t3, SchemaVersion(1), SchemaVersion(2)).unwrap();
        assert!(d.contains(&Change::Reordered) && d.contains(&Change::Added("X".into())));
    }

    #[test]
    fn widening_table() {
        use ColumnType::*;
        let n = |p, s| Number { precision: p, scale: s };
        assert!(is_widening(Integer, Integer));
        assert!(is_widening(Integer, n(Some(15), Some(5))));
        assert!(is_widening(n(Some(10), Some(2)), n(Some(15), Some(5))));
        assert!(!is_widening(n(Some(15), Some(5)), n(Some(10), Some(2))));
        assert!(is_widening(n(Some(15), Some(5)), n(None, None)));
        assert!(!is_widening(n(None, None), n(Some(15), Some(5))));
        assert!(is_widening(n(Some(10), None), n(Some(12), None)));
        assert!(!is_widening(n(Some(12), None), n(Some(10), None)));
        assert!(!is_widening(n(Some(10), Some(2)), n(Some(10), Some(3)))); // integer digits shrink
        assert!(is_widening(Utf8 { max_len: Some(10), dict: false }, Utf8 { max_len: Some(20), dict: true }));
        assert!(!is_widening(Utf8 { max_len: Some(20), dict: false }, Utf8 { max_len: Some(10), dict: false }));
        assert!(is_widening(Utf8 { max_len: Some(20), dict: false }, Utf8 { max_len: None, dict: false }));
        assert!(!is_widening(Utf8 { max_len: None, dict: false }, Utf8 { max_len: Some(1), dict: false }));
        assert!(is_widening(Date, Timestamp));
        assert!(!is_widening(Timestamp, Date));
        assert!(!is_widening(Utf8 { max_len: None, dict: false }, Integer));
        assert!(is_widening(Boolean, Boolean));
        assert!(!is_widening(Number { precision: None, scale: None }, Integer));
    }

    #[test]
    fn breaking_detection() {
        let old = unit_scada();
        let new = with_v2("LASTCHANGED");
        assert!(breaking_changes(&old, &new).is_empty());

        let mut b = new.clone();
        b.next_column_id = 3;
        assert!(breaking_changes(&new, &b)[0].contains("next_column_id"));

        let mut b = new.clone();
        b.columns.remove(3);
        assert!(breaking_changes(&new, &b).iter().any(|m| m.contains("deleted")));

        let mut b = new.clone();
        b.columns[3].name = "LAST_CHANGED".into();
        assert!(breaking_changes(&new, &b).iter().any(|m| m.contains("renamed")));
        b.columns[3].aliases.push("LASTCHANGED".into());
        assert!(breaking_changes(&new, &b).is_empty());

        let mut b = new.clone();
        b.columns[2].ty = ColumnType::Integer;
        assert!(breaking_changes(&new, &b).iter().any(|m| m.contains("not a widening")));

        let mut b = new.clone();
        b.columns[2].nullable = false;
        assert!(breaking_changes(&new, &b).iter().any(|m| m.contains("non-nullable")));

        let mut b = new.clone();
        b.columns[3].id = ColumnId(7);
        b.columns[3].name = "OTHER".into();
        b.columns.push(ColumnDef { id: ColumnId(8), name: "LASTCHANGED".into(), ..new.columns[3].clone() });
        let msgs = breaking_changes(&new, &b);
        assert!(msgs.iter().any(|m| m.contains("moved from id")), "{msgs:?}");

        let mut b = new.clone();
        b.versions.pop();
        assert!(breaking_changes(&new, &b).iter().any(|m| m.contains("version 2 was deleted")));

        let mut b = new.clone();
        b.versions[1].fingerprint = Fingerprint(1);
        assert!(breaking_changes(&new, &b).iter().any(|m| m.contains("changed its column list")));

        let mut b = new.clone();
        b.primary_key.pop();
        b.load_mode = crate::model::LoadMode::Snapshot;
        b.time_column = None;
        let msgs = breaking_changes(&new, &b);
        assert!(msgs.iter().any(|m| m.contains("primary_key")));
        assert!(msgs.iter().any(|m| m.contains("load_mode")));
        assert!(msgs.iter().any(|m| m.contains("time_column")));
    }
}
