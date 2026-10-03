//! The invariants every table file must satisfy. Run at build time over the
//! committed files, at runtime over user overlays, and in `cargo test`.

use std::collections::{BTreeMap, HashMap, HashSet};

use nemlake_core::Fingerprint;

use crate::model::{ColumnType, LoadMode, SourceKind, TableDef};

/// One thing wrong with a table or a set of tables. The message names the
/// key so the fix is obvious.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The table, or empty for a cross-table issue.
    pub table: String,
    /// What is wrong.
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.table.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.table, self.message)
        }
    }
}

fn issue(t: &TableDef, message: impl Into<String>) -> Issue {
    Issue {
        table: t.name.clone(),
        message: message.into(),
    }
}

/// Checks one table in isolation.
pub fn validate_table(t: &TableDef) -> Vec<Issue> {
    let mut out = Vec::new();
    if t.name.is_empty() {
        out.push(issue(t, "name is empty"));
    }
    if t.package.is_empty() {
        out.push(issue(t, "package is empty"));
    }
    match t.source {
        SourceKind::Mms if t.report_pairs.is_empty() => out.push(issue(
            t,
            "report_pairs is empty; an MMS table needs at least one (report_type, report_subtype)",
        )),
        SourceKind::PlainCsv if t.datasets.is_empty() => out.push(issue(
            t,
            "datasets is empty; a plain-CSV table needs at least one dataset name",
        )),
        _ => {}
    }
    if t.columns.is_empty() {
        out.push(issue(t, "column list is empty"));
    }
    if t.versions.is_empty() {
        out.push(issue(t, "version list is empty"));
    }

    let mut names: HashSet<&str> = HashSet::new();
    let mut ids: HashSet<u32> = HashSet::new();
    for c in &t.columns {
        if c.name.is_empty() {
            out.push(issue(t, format!("column id {} has an empty name", c.id)));
        }
        if !names.insert(c.name.as_str()) {
            out.push(issue(
                t,
                format!("column name {} is declared twice", c.name),
            ));
        }
        for a in &c.aliases {
            if !names.insert(a.as_str()) {
                out.push(issue(
                    t,
                    format!(
                        "alias {} of column {} collides with another name or alias",
                        a, c.name
                    ),
                ));
            }
        }
        if !ids.insert(c.id.0) {
            out.push(issue(t, format!("column id {} is used twice", c.id)));
        }
        if c.id.0 >= t.next_column_id {
            out.push(issue(
                t,
                format!(
                    "column {} has id {} but next_column_id is {}",
                    c.name, c.id, t.next_column_id
                ),
            ));
        }
        if c.id.0 == 0 {
            out.push(issue(
                t,
                format!("column {} has id 0; ids start at 1", c.name),
            ));
        }
        if let Some(r) = c.removed_in
            && r <= c.added_in
        {
            out.push(issue(
                t,
                format!(
                    "column {} has removed_in {} not after added_in {}",
                    c.name, r, c.added_in
                ),
            ));
        }
        if let ColumnType::Number { precision, scale } = c.ty {
            match (precision, scale) {
                (Some(p), Some(s)) if s > p => {
                    out.push(issue(
                        t,
                        format!("column {} has scale {s} greater than precision {p}", c.name),
                    ));
                }
                (Some(p), _) if p == 0 || p > 38 => {
                    out.push(issue(
                        t,
                        format!(
                            "column {} has precision {p}; Decimal128 allows 1..=38",
                            c.name
                        ),
                    ));
                }
                (None, Some(_)) => out.push(issue(
                    t,
                    format!("column {} has a scale but no precision", c.name),
                )),
                _ => {}
            }
        }
        if let ColumnType::Utf8 {
            max_len: Some(0), ..
        } = c.ty
        {
            out.push(issue(t, format!("column {} has max_len 0", c.name)));
        }
    }

    for k in &t.primary_key {
        match t.column(k) {
            None => out.push(issue(
                t,
                format!("primary_key names {k}, which is not a column"),
            )),
            Some(c) if c.nullable => out.push(issue(
                t,
                format!("primary_key column {k} is nullable; set nullable = false"),
            )),
            Some(c) if c.removed_in.is_some() => {
                out.push(issue(t, format!("primary_key column {k} has been removed")));
            }
            _ => {}
        }
    }
    let mut pk_seen = HashSet::new();
    for k in &t.primary_key {
        if !pk_seen.insert(k) {
            out.push(issue(t, format!("primary_key lists {k} twice")));
        }
    }
    for c in &t.columns {
        if !c.nullable && !t.primary_key.contains(&c.name) {
            out.push(issue(t, format!("column {} is non-nullable but not in primary_key; only key columns may refuse nulls", c.name)));
        }
    }

    match (&t.time_column, t.load_mode) {
        (Some(tc), _) => match t.column(tc) {
            None => out.push(issue(t, format!("time_column {tc} is not a column"))),
            Some(c) if !matches!(c.ty, ColumnType::Timestamp | ColumnType::Date) => {
                out.push(issue(
                    t,
                    format!(
                        "time_column {tc} has type {}; it must be timestamp or date",
                        c.ty.kind_name()
                    ),
                ));
            }
            _ => {}
        },
        (None, LoadMode::Append) => out.push(issue(
            t,
            "load_mode is append but time_column is unset; appended tables are partitioned by time",
        )),
        (None, LoadMode::Snapshot) => {}
    }
    if t.load_mode == LoadMode::Snapshot && t.primary_key.is_empty() {
        out.push(issue(
            t,
            "load_mode is snapshot but primary_key is empty; snapshots are replaced by key",
        ));
    }

    let mut prev = None;
    let mut fps: HashMap<Fingerprint, u16> = HashMap::new();
    for v in &t.versions {
        if let Some(p) = prev
            && v.n <= p
        {
            out.push(issue(
                t,
                format!(
                    "version {} follows version {p}; versions must strictly increase",
                    v.n
                ),
            ));
        }
        prev = Some(v.n);
        if v.columns.is_empty() {
            out.push(issue(t, format!("version {} has no columns", v.n)));
        }
        let mut seen = HashSet::new();
        for c in &v.columns {
            if !seen.insert(c) {
                out.push(issue(t, format!("version {} lists column {c} twice", v.n)));
            }
            match t.column(c) {
                None => out.push(issue(
                    t,
                    format!("version {} lists column {c}, which is not declared", v.n),
                )),
                Some(def) => {
                    if def.added_in > v.n {
                        out.push(issue(
                            t,
                            format!(
                                "version {} lists column {c}, which is added_in {}",
                                v.n, def.added_in
                            ),
                        ));
                    }
                    if let Some(r) = def.removed_in
                        && r <= v.n
                    {
                        out.push(issue(
                            t,
                            format!("version {} lists column {c}, which is removed_in {r}", v.n),
                        ));
                    }
                }
            }
        }
        let recomputed = Fingerprint::of_names(v.columns.iter().map(String::as_str));
        if recomputed != v.fingerprint {
            out.push(issue(
                t,
                format!(
                    "version {} declares fingerprint {} but its columns hash to {recomputed}",
                    v.n, v.fingerprint
                ),
            ));
        }
        if let Some(other) = fps.insert(v.fingerprint, v.n.0)
            && recomputed == v.fingerprint
        {
            out.push(issue(t, format!("version {} has the same columns as version {other}; declare one and alias the other in the registry", v.n)));
        }
        if let (Some(a), Some(b)) = (v.first_seen, v.last_seen)
            && b < a
        {
            out.push(issue(
                t,
                format!("version {} has last_seen {b} before first_seen {a}", v.n),
            ));
        }
    }
    for c in &t.columns {
        if !t.versions.iter().any(|v| v.columns.contains(&c.name))
            && c.src.iter().all(|s| s != "manual")
        {
            out.push(issue(t, format!("column {} appears in no version", c.name)));
        }
        if c.src.iter().any(|s| s == "inferred") {
            out.push(issue(
                t,
                format!(
                    "column {} has src inferred; curate it before committing",
                    c.name
                ),
            ));
        }
    }
    out
}

/// Checks a set of tables together: unique names and report pairs.
pub fn validate_set<'a>(tables: impl IntoIterator<Item = &'a TableDef>) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    let mut pairs: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    let mut datasets: BTreeMap<&str, &str> = BTreeMap::new();
    for t in tables {
        *names.entry(t.name.as_str()).or_default() += 1;
        for (a, b) in &t.report_pairs {
            if let Some(other) = pairs.insert((a.as_str(), b.as_str()), t.name.as_str())
                && other != t.name
            {
                out.push(Issue {
                    table: String::new(),
                    message: format!(
                        "report pair ({a}, {b}) is claimed by both {other} and {}",
                        t.name
                    ),
                });
            }
        }
        for d in &t.datasets {
            if let Some(other) = datasets.insert(d.as_str(), t.name.as_str())
                && other != t.name
            {
                out.push(Issue {
                    table: String::new(),
                    message: format!("dataset {d} is claimed by both {other} and {}", t.name),
                });
            }
        }
    }
    for (n, count) in names {
        if count > 1 {
            out.push(Issue {
                table: String::new(),
                message: format!("table {n} is defined {count} times"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use nemlake_core::Fingerprint;

    use super::*;
    use crate::file::tests::unit_scada;
    use crate::model::{ColumnDef, ColumnId, Dedupe, SchemaVersion, VersionDef};

    fn msgs(t: &TableDef) -> Vec<String> {
        validate_table(t).into_iter().map(|i| i.message).collect()
    }

    fn has(t: &TableDef, needle: &str) -> bool {
        let m = msgs(t);
        let found = m.iter().any(|s| s.contains(needle));
        assert!(found, "no issue containing {needle:?} in {m:#?}");
        found
    }

    #[test]
    fn valid_table_has_no_issues() {
        assert_eq!(validate_table(&unit_scada()), vec![]);
        assert!(validate_set([&unit_scada()]).is_empty());
    }

    #[test]
    fn column_issues() {
        let mut t = unit_scada();
        t.columns[1].name = "SETTLEMENTDATE".into();
        has(&t, "declared twice");
        let mut t = unit_scada();
        t.columns[2].aliases.push("DUID".into());
        has(&t, "collides");
        let mut t = unit_scada();
        t.columns[1].id = ColumnId(1);
        has(&t, "used twice");
        let mut t = unit_scada();
        t.columns[2].id = ColumnId(9);
        has(&t, "next_column_id is 4");
        let mut t = unit_scada();
        t.columns[2].id = ColumnId(0);
        has(&t, "id 0");
        let mut t = unit_scada();
        t.columns[2].removed_in = Some(SchemaVersion(1));
        has(&t, "removed_in 1 not after");
        let mut t = unit_scada();
        t.columns[2].ty = ColumnType::Number {
            precision: Some(5),
            scale: Some(6),
        };
        has(&t, "scale 6 greater");
        t.columns[2].ty = ColumnType::Number {
            precision: Some(40),
            scale: None,
        };
        has(&t, "precision 40");
        t.columns[2].ty = ColumnType::Number {
            precision: None,
            scale: Some(2),
        };
        has(&t, "no precision");
        t.columns[2].ty = ColumnType::Utf8 {
            max_len: Some(0),
            dict: false,
        };
        has(&t, "max_len 0");
        let mut t = unit_scada();
        t.columns[0].name = String::new();
        has(&t, "empty name");
        let mut t = unit_scada();
        t.columns[2].nullable = false;
        has(&t, "non-nullable but not in primary_key");
        let mut t = unit_scada();
        t.columns[2].src = vec!["inferred".into()];
        has(&t, "src inferred");
    }

    #[test]
    fn key_and_time_issues() {
        let mut t = unit_scada();
        t.primary_key.push("NOPE".into());
        has(&t, "not a column");
        let mut t = unit_scada();
        t.columns[1].nullable = true;
        has(&t, "is nullable");
        let mut t = unit_scada();
        t.primary_key.push("DUID".into());
        has(&t, "twice");
        let mut t = unit_scada();
        t.columns[1].removed_in = Some(SchemaVersion(3));
        has(&t, "has been removed");
        let mut t = unit_scada();
        t.time_column = Some("DUID".into());
        has(&t, "must be timestamp or date");
        t.time_column = Some("X".into());
        has(&t, "time_column X is not a column");
        t.time_column = None;
        has(&t, "time_column is unset");
        let mut t = unit_scada();
        t.load_mode = LoadMode::Snapshot;
        t.time_column = None;
        t.primary_key.clear();
        t.columns[0].nullable = true;
        t.columns[1].nullable = true;
        has(&t, "snapshot but primary_key is empty");
        let mut t = unit_scada();
        t.name.clear();
        t.package.clear();
        t.report_pairs.clear();
        has(&t, "name is empty");
        has(&t, "package is empty");
        has(&t, "report_pairs is empty");
        let mut t = unit_scada();
        t.source = SourceKind::PlainCsv;
        has(&t, "datasets is empty");
        let mut t = unit_scada();
        t.columns.clear();
        t.versions.clear();
        t.primary_key.clear();
        t.time_column = None;
        t.load_mode = LoadMode::Snapshot;
        t.dedupe = Dedupe::None;
        has(&t, "column list is empty");
        has(&t, "version list is empty");
    }

    #[test]
    fn version_issues() {
        let mut t = unit_scada();
        t.versions.push(t.versions[0].clone());
        has(&t, "must strictly increase");
        has(&t, "same columns as version 1");
        let mut t = unit_scada();
        t.versions[0].columns.push("X".into());
        has(&t, "not declared");
        has(&t, "declares fingerprint");
        let mut t = unit_scada();
        t.versions[0].columns.push("DUID".into());
        has(&t, "twice");
        let mut t = unit_scada();
        t.versions[0].columns.clear();
        t.versions[0].fingerprint = Fingerprint::of_names([]);
        has(&t, "has no columns");
        has(&t, "appears in no version");
        let mut t = unit_scada();
        t.columns[2].added_in = SchemaVersion(2);
        has(&t, "added_in 2");
        let mut t = unit_scada();
        t.columns.push(ColumnDef {
            id: ColumnId(3),
            name: "GONE".into(),
            added_in: SchemaVersion(1),
            removed_in: Some(SchemaVersion(2)),
            ..t.columns[2].clone()
        });
        t.columns[3].id = ColumnId(9);
        t.next_column_id = 10;
        t.versions.push(VersionDef {
            n: SchemaVersion(2),
            columns: vec![
                "SETTLEMENTDATE".into(),
                "DUID".into(),
                "SCADAVALUE".into(),
                "GONE".into(),
            ],
            fingerprint: Fingerprint::of_names(["SETTLEMENTDATE", "DUID", "SCADAVALUE", "GONE"]),
            first_seen: Some("2024-02".parse().unwrap()),
            last_seen: Some("2024-01".parse().unwrap()),
            src: None,
        });
        has(&t, "removed_in 2");
        has(&t, "last_seen 2024-01 before first_seen 2024-02");
        // A manual column may exist in no version (reserved for the future).
        let mut t = unit_scada();
        t.columns.push(ColumnDef {
            id: ColumnId(3),
            name: "FUTURE".into(),
            src: vec!["manual".into()],
            ..t.columns[2].clone()
        });
        t.columns[3].id = ColumnId(3);
        t.columns[2].id = ColumnId(9);
        t.next_column_id = 10;
        assert!(
            !msgs(&t).iter().any(|m| m.contains("FUTURE")),
            "{:?}",
            msgs(&t)
        );
    }

    #[test]
    fn set_issues() {
        let a = unit_scada();
        let mut b = unit_scada();
        b.name = "OTHER".into();
        let issues = validate_set([&a, &b]);
        assert!(
            issues.iter().any(|i| i.message.contains("claimed by both")),
            "{issues:?}"
        );
        let issues = validate_set([&a, &a]);
        assert!(issues.iter().any(|i| i.message.contains("defined 2 times")));
        let mut c = unit_scada();
        c.name = "C".into();
        c.report_pairs.clear();
        c.source = SourceKind::PlainCsv;
        c.datasets = vec!["ds".into()];
        let mut d = c.clone();
        d.name = "D".into();
        let issues = validate_set([&c, &d]);
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("dataset ds is claimed"))
        );
        assert_eq!(issues[0].to_string(), issues[0].message);
        let i = Issue {
            table: "T".into(),
            message: "m".into(),
        };
        assert_eq!(i.to_string(), "T: m");
    }
}
