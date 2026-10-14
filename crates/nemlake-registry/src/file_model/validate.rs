//! The invariants every table must satisfy. Run at build time over the
//! committed files, at run time over a user's own tables, and in tests.

use std::collections::{BTreeMap, HashSet};

use crate::file_model::model::{LakeType, LoadMode, Table};
use crate::file_model::temporal::RESERVED_PREFIX;

/// One thing wrong with a table or a set of tables. The message names the
/// key, so the fix is plain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The table, or empty for an issue across tables.
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

/// An issue of table `t`.
pub fn issue(t: &Table, message: impl Into<String>) -> Issue {
    Issue {
        table: t.name.clone(),
        message: message.into(),
    }
}

/// Checks one table.
pub fn validate_table(t: &Table) -> Vec<Issue> {
    let mut out = Vec::new();
    if t.name.is_empty() {
        out.push(issue(t, "name is empty"));
    }
    if t.report_pairs.is_empty() {
        out.push(issue(
            t,
            "report_pairs is empty; a table needs at least one (report_type, report_subtype)",
        ));
    }
    if t.columns.is_empty() {
        out.push(issue(t, "column list is empty"));
    }
    if t.increments.is_empty() {
        out.push(issue(t, "increment list is empty"));
    }
    columns(t, &mut out);
    keys(t, &mut out);
    if let Err(problems) = t.temporal.check(t) {
        out.extend(problems.into_iter().map(|m| issue(t, m)));
    }
    increments(t, &mut out);
    out
}

fn columns(t: &Table, out: &mut Vec<Issue>) {
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
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
        if !ids.insert(c.id) {
            out.push(issue(t, format!("column id {} is used twice", c.id)));
        }
        if c.name.to_ascii_uppercase().starts_with(RESERVED_PREFIX) {
            out.push(issue(
                t,
                format!(
                    "column {} starts with {RESERVED_PREFIX}, which is reserved for the columns nemlake adds to a read",
                    c.name
                ),
            ));
        }
        if c.id.0 == 0 {
            out.push(issue(
                t,
                format!("column {} has id 0; ids start at 1", c.name),
            ));
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
        if !c.nullable && !t.primary_key.contains(&c.name) {
            out.push(issue(
                t,
                format!(
                    "column {} is non-nullable but not in primary_key; only key columns may refuse nulls",
                    c.name
                ),
            ));
        }
        if !c.nullable
            && let Some(i) = t.increments.iter().find(|i| !i.columns.contains(&c.name))
        {
            out.push(issue(
                t,
                format!(
                    "column {} is non-nullable but increment {} does not carry it; set nullable = true",
                    c.name, i.n
                ),
            ));
        }
    }
}

fn keys(t: &Table, out: &mut Vec<Issue>) {
    let mut seen = HashSet::new();
    for k in &t.primary_key {
        if !seen.insert(k) {
            out.push(issue(t, format!("primary_key lists {k} twice")));
        }
        if t.column(k).is_none() {
            out.push(issue(
                t,
                format!("primary_key names {k}, which is not a column"),
            ));
        }
    }
    if let Some(tc) = &t.time_column {
        match t.column(tc) {
            None => out.push(issue(t, format!("time_column {tc} is not a column"))),
            Some(c) if !matches!(c.ty, LakeType::TimestampNs | LakeType::Date) => {
                out.push(issue(
                    t,
                    format!(
                        "time_column {tc} has type {}; it must be timestamp_ns or date",
                        c.ty
                    ),
                ));
            }
            _ => {}
        }
    }
    if t.load_mode == LoadMode::Snapshot && t.primary_key.is_empty() {
        out.push(issue(
            t,
            "load_mode is snapshot but primary_key is empty; snapshots are replaced by key",
        ));
    }
}

fn increments(t: &Table, out: &mut Vec<Issue>) {
    let mut ns = HashSet::new();
    let mut prev = None;
    for i in &t.increments {
        if !ns.insert(i.n) {
            out.push(issue(t, format!("increment {} is declared twice", i.n)));
        }
        if i.n == 0 {
            out.push(issue(t, "increment 0; increments start at 1"));
        }
        if let Some(p) = prev
            && i.from <= p
        {
            out.push(issue(
                t,
                format!(
                    "increment {} applies from {} after one from {p}; from must strictly increase",
                    i.n, i.from
                ),
            ));
        }
        prev = Some(i.from);
        if i.columns.is_empty() {
            out.push(issue(t, format!("increment {} has no columns", i.n)));
        }
        let mut seen = HashSet::new();
        for c in &i.columns {
            if !seen.insert(c) {
                out.push(issue(
                    t,
                    format!("increment {} lists column {c} twice", i.n),
                ));
            }
            if t.column(c).is_none() {
                out.push(issue(
                    t,
                    format!(
                        "increment {} lists column {c}, which is not a column; add it to column",
                        i.n
                    ),
                ));
            }
        }
        if i.ctl.is_some() != i.ctl_sha256.is_some() {
            out.push(issue(
                t,
                format!(
                    "increment {} names a control file without its hash, or a hash without its file",
                    i.n
                ),
            ));
        }
    }
}

/// Checks a set of tables together: unique names and report pairs.
pub fn validate_set<'a>(tables: impl IntoIterator<Item = &'a Table>) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    let mut pairs: BTreeMap<(&str, &str), &str> = BTreeMap::new();
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
    use super::*;
    use crate::file_model::model::tests::unit_scada;
    use crate::file_model::model::{Column, ColumnId};

    fn messages(t: &Table) -> Vec<String> {
        validate_table(t).into_iter().map(|i| i.message).collect()
    }

    fn has(t: &Table, needle: &str) -> bool {
        let m = messages(t);
        assert!(m.iter().any(|x| x.contains(needle)), "{needle}: {m:?}");
        true
    }

    #[test]
    fn a_good_table_has_no_issue() {
        assert_eq!(messages(&unit_scada()), Vec::<String>::new());
        let i = Issue {
            table: String::new(),
            message: "m".into(),
        };
        assert_eq!(i.to_string(), "m");
        assert_eq!(
            issue(&unit_scada(), "m").to_string(),
            "DISPATCH_UNIT_SCADA: m"
        );
    }

    #[test]
    fn columns_and_ids() {
        let mut t = unit_scada();
        t.columns.push(Column {
            id: ColumnId(3),
            name: "DUID".into(),
            ty: LakeType::String,
            nullable: true,
            note: None,
        });
        assert!(has(&t, "DUID is declared twice") && has(&t, "id 3 is used twice"));
        let mut t = unit_scada();
        t.columns[0].id = ColumnId(0);
        t.columns[1].id = ColumnId(9);
        t.columns[1].name = String::new();
        assert!(has(&t, "ids start at 1") && has(&t, "next_column_id is 5"));
        assert!(has(&t, "empty name"));
        let mut t = unit_scada();
        t.columns[2].nullable = false;
        assert!(has(&t, "only key columns may refuse nulls"));
        let mut t = unit_scada();
        t.columns.clear();
        t.increments.clear();
        t.report_pairs.clear();
        t.name = String::new();
        assert!(has(&t, "column list is empty") && has(&t, "increment list is empty"));
        assert!(has(&t, "report_pairs is empty") && has(&t, "name is empty"));
        let mut t = unit_scada();
        t.columns[2].name = "_nl_source".into();
        assert!(has(&t, "_nl_source starts with _NL_"));
    }

    #[test]
    fn temporal_roles() {
        let mut t = unit_scada();
        t.temporal.order.clear();
        assert!(has(&t, "regenerate it"));
        t.temporal.origin = crate::file_model::temporal::RoleOrigin::Hand;
        assert!(
            validate_table(&t).is_empty(),
            "a hand choice is the person's"
        );
        t.temporal.entity.clear();
        assert!(has(&t, "DUID has no temporal role"));
    }

    #[test]
    fn keys_and_time() {
        let mut t = unit_scada();
        t.primary_key.push("DUID".into());
        t.primary_key.push("NOPE".into());
        assert!(has(&t, "lists DUID twice") && has(&t, "NOPE, which is not a column"));
        let mut t = unit_scada();
        t.time_column = Some("DUID".into());
        assert!(has(&t, "must be timestamp_ns or date"));
        t.time_column = Some("NOPE".into());
        assert!(has(&t, "time_column NOPE is not a column"));
        let mut t = unit_scada();
        t.load_mode = LoadMode::Snapshot;
        t.primary_key.clear();
        t.columns[0].nullable = true;
        t.columns[1].nullable = true;
        assert!(has(&t, "snapshots are replaced by key"));
        // A key column an increment lacks must take nulls.
        let mut t = unit_scada();
        t.increments[0].columns.retain(|c| c != "DUID");
        assert!(has(
            &t,
            "DUID is non-nullable but increment 1 does not carry it"
        ));
    }

    #[test]
    fn increments() {
        let mut t = unit_scada();
        t.increments[1].n = 1;
        t.increments[1].from = t.increments[0].from;
        assert!(has(&t, "increment 1 is declared twice") && has(&t, "strictly increase"));
        let mut t = unit_scada();
        t.increments[0].n = 0;
        t.increments[0].columns.push("SCADAVALUE".into());
        t.increments[0].columns.push("GHOST".into());
        t.increments[0].ctl = Some("001.ctl".into());
        assert!(has(&t, "increment 0") && has(&t, "lists column SCADAVALUE twice"));
        assert!(has(&t, "GHOST, which is not a column") && has(&t, "without its hash"));
        let mut t = unit_scada();
        t.increments[1].columns.clear();
        t.columns[0].nullable = true;
        t.columns[1].nullable = true;
        assert!(has(&t, "increment 2 has no columns"));
    }

    #[test]
    fn sets() {
        let a = unit_scada();
        let mut b = unit_scada();
        b.name = "OTHER".into();
        let issues = validate_set([&a, &b, &a]);
        assert_eq!(issues.len(), 3, "{issues:?}");
        assert!(issues[0].message.contains("claimed by both"));
        assert!(issues[2].message.contains("defined 2 times"));
    }
}
