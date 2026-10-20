//! AEMO's declarations held against the data.
//!
//! A table's shapes (`crate::fold`) record what each column held in every
//! file; AEMO declares the same columns twice, in each month's control file
//! (the class a loader reads a column as) and in each data-model release
//! (type, length, precision, and whether nulls are allowed). This lists
//! every place the two disagree, with the file and month it was first seen
//! in, into `schemas/nem/audit/{TABLE}.toml`. Nothing is changed: the
//! findings are evidence for the table's schema and for its readers.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utilake_core::YearMonth;
use utilake_iceberg::Kind;
use utilake_mms_registry::file_model::shapes::{ShapeColumn, ShapeEntry};
use utilake_mms_registry::shape::{ShapeId, TypeClass};

use crate::census::{CensusError, write_text};
use crate::models::Model;

/// One disagreement between a declaration and the data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// What kind of disagreement, a stable name.
    pub kind: String,
    /// The column, when it is about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// The shape it was seen in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<ShapeId>,
    /// Where the declaration is: `ctl` or the model release (`model 5.7`).
    pub declared_by: String,
    /// What it declares.
    pub declared: String,
    /// What the data held.
    pub observed: String,
    /// The month the data first showed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<YearMonth>,
    /// The file the data first showed it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// The findings of a table, as written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    /// Every finding, in the order the table's shapes were first seen.
    #[serde(default)]
    pub finding: Vec<Finding>,
}

/// A type the model writes: `VARCHAR(20)`, `NUMERIC(15,5)`, `TIMESTAMP(0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    /// Text of at most this many characters.
    Varchar(u32),
    /// A number of this precision and scale.
    Numeric(u8, u8),
    /// A date and time.
    Timestamp,
    /// Anything else.
    Other,
}

impl ModelType {
    /// Reads a type as the model's script writes it.
    pub fn parse(s: &str) -> Self {
        let upper = s.trim().to_ascii_uppercase();
        let args = |p: &str| {
            upper
                .strip_prefix(p)
                .and_then(|r| r.strip_prefix('('))
                .and_then(|r| r.strip_suffix(')'))
                .map(|r| {
                    r.split(',')
                        .map(|x| x.trim().to_owned())
                        .collect::<Vec<_>>()
                })
        };
        if let Some(a) = args("VARCHAR").or_else(|| args("CHAR")) {
            return a
                .first()
                .and_then(|n| n.parse().ok())
                .map_or(Self::Other, Self::Varchar);
        }
        if let Some(a) = args("NUMERIC").or_else(|| args("DECIMAL")) {
            let p = a.first().and_then(|n| n.parse().ok());
            let s = a.get(1).map_or(Some(0), |n| n.parse().ok());
            return match (p, s) {
                (Some(p), Some(s)) => Self::Numeric(p, s),
                _ => Self::Other,
            };
        }
        if upper.starts_with("TIMESTAMP") || upper == "DATE" {
            return Self::Timestamp;
        }
        Self::Other
    }
}

/// Whether values of `kind` fit what a control file loads as `class`.
fn class_holds(class: TypeClass, kind: Kind) -> bool {
    match class {
        TypeClass::Unknown | TypeClass::Varchar => true,
        TypeClass::Number => matches!(kind, Kind::Empty | Kind::Integer | Kind::Decimal),
        TypeClass::Integer => matches!(kind, Kind::Empty | Kind::Integer),
        TypeClass::Date => matches!(kind, Kind::Empty | Kind::Date),
        TypeClass::Timestamp => matches!(kind, Kind::Empty | Kind::Date | Kind::Timestamp),
    }
}

/// What a column held, in a few words.
fn held(c: &ShapeColumn) -> String {
    let kind = format!("{:?}", c.kind).to_lowercase();
    match c.kind {
        Kind::Integer | Kind::Decimal => format!(
            "{kind}, {} digit(s) before the point and {} after",
            c.int_digits, c.scale
        ),
        Kind::Text => format!("{kind} of up to {} byte(s)", c.max_len),
        _ => kind,
    }
}

/// The newest model release shipped in any month of the shape: the one in
/// force when it was last seen. An older release beside it in the same
/// months is superseded and not held against the data.
fn models_of<'a>(models: &'a [&'a Model], e: &ShapeEntry) -> Option<&'a Model> {
    models
        .iter()
        .copied()
        .rfind(|m| m.first_seen <= e.last_seen && e.first_seen <= m.last_seen)
}

/// Every finding of one table: `shapes` from its shapes file, `models` the
/// data-model releases oldest first.
pub fn audit_table(table: &str, shapes: &[ShapeEntry], models: &[&Model]) -> Audit {
    let mut out = Vec::new();
    let mut seen_columns = BTreeSet::new();
    for e in shapes {
        let at = |c: &ShapeColumn| {
            (
                Some(e.from.month),
                Some(c.nulls_since.clone().unwrap_or_else(|| e.from.file.clone())),
            )
        };
        if e.versions.len() > 1 {
            out.push(Finding {
                kind: "versions_without_change".into(),
                column: None,
                shape: Some(e.id),
                declared_by: "I row".into(),
                declared: format!("versions {:?}", e.versions),
                observed: "one column list".into(),
                month: Some(e.from.month),
                file: Some(e.from.file.clone()),
            });
        }
        for c in &e.columns {
            seen_columns.insert(c.name.clone());
            let base =
                |kind: &str,
                 by: &str,
                 declared: String,
                 observed: String,
                 (month, file): (Option<YearMonth>, Option<String>)| Finding {
                    kind: kind.into(),
                    column: Some(c.name.clone()),
                    shape: Some(e.id),
                    declared_by: by.into(),
                    declared,
                    observed,
                    month,
                    file,
                };
            if !class_holds(c.ctl, c.kind) {
                out.push(base(
                    "ctl_type_does_not_hold",
                    "ctl",
                    c.ctl.to_string(),
                    held(c),
                    (Some(e.from.month), Some(e.from.file.clone())),
                ));
            } else if c.ctl == TypeClass::Varchar
                && matches!(
                    c.kind,
                    Kind::Integer | Kind::Decimal | Kind::Date | Kind::Timestamp
                )
            {
                out.push(base(
                    "ctl_text_holds_typed_values",
                    "ctl",
                    c.ctl.to_string(),
                    held(c),
                    (Some(e.from.month), Some(e.from.file.clone())),
                ));
            }
            if c.ctl == TypeClass::Unknown && c.kind != Kind::Empty {
                out.push(base(
                    "no_ctl_declares_column",
                    "ctl",
                    "nothing".into(),
                    held(c),
                    (Some(e.from.month), Some(e.from.file.clone())),
                ));
            }
            if c.kind == Kind::Empty {
                out.push(base(
                    "never_a_value",
                    "data",
                    String::new(),
                    format!("{} row(s), every one null", e.rows),
                    (Some(e.from.month), Some(e.from.file.clone())),
                ));
            }
            if let Some(m) = models_of(models, e) {
                let by = format!("model {}", m.version);
                let Some(t) = m.tables.get(table) else {
                    continue;
                };
                let Some((_, ty, nullable, _)) = t
                    .columns
                    .iter()
                    .find(|(n, ..)| n.eq_ignore_ascii_case(&c.name))
                else {
                    out.push(base(
                        "model_lacks_column",
                        &by,
                        "no such column".into(),
                        held(c),
                        (Some(e.from.month), Some(e.from.file.clone())),
                    ));
                    continue;
                };
                if !nullable && c.nulls > 0 {
                    out.push(base(
                        "null_in_not_null",
                        &by,
                        format!("{ty} not null"),
                        format!("{} null(s)", c.nulls),
                        at(c),
                    ));
                }
                match ModelType::parse(ty) {
                    ModelType::Varchar(n) if c.max_len > n => out.push(base(
                        "longer_than_declared",
                        &by,
                        ty.clone(),
                        held(c),
                        (Some(e.from.month), Some(e.from.file.clone())),
                    )),
                    ModelType::Numeric(p, s)
                        if matches!(c.kind, Kind::Integer | Kind::Decimal)
                            && (c.scale > s
                                || u16::from(c.int_digits) > u16::from(p.saturating_sub(s))) =>
                    {
                        out.push(base(
                            "wider_than_declared",
                            &by,
                            ty.clone(),
                            held(c),
                            (Some(e.from.month), Some(e.from.file.clone())),
                        ));
                    }
                    ModelType::Numeric(..)
                        if matches!(c.kind, Kind::Text | Kind::Date | Kind::Timestamp) =>
                    {
                        out.push(base(
                            "model_type_does_not_hold",
                            &by,
                            ty.clone(),
                            held(c),
                            (Some(e.from.month), Some(e.from.file.clone())),
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    // Columns the newest release declares that no file of its months had.
    for m in models.iter().rev().take(1) {
        let Some(t) = m.tables.get(table) else {
            continue;
        };
        let live = shapes
            .iter()
            .any(|e| m.first_seen <= e.last_seen && e.first_seen <= m.last_seen);
        if !live {
            continue;
        }
        for (name, ty, ..) in &t.columns {
            if !seen_columns.iter().any(|s| s.eq_ignore_ascii_case(name)) {
                out.push(Finding {
                    kind: "declared_never_seen".into(),
                    column: Some(name.clone()),
                    shape: None,
                    declared_by: format!("model {}", m.version),
                    declared: ty.clone(),
                    observed: "in no file".into(),
                    month: None,
                    file: None,
                });
            }
        }
    }
    Audit { finding: out }
}

/// The directory of the audit files.
pub fn audit_dir(root: &Path) -> PathBuf {
    root.join("nem").join("audit")
}

/// Audits every table with a shapes file and writes each table's findings;
/// a table without findings has no file. Returns tables and findings.
pub fn audit(root: &Path) -> Result<(usize, usize), CensusError> {
    let models = crate::models::read_models(root).map_err(|e| CensusError::Read {
        path: crate::models::models_dir(root).display().to_string(),
        message: e.to_string(),
    })?;
    let models = crate::models::by_first_seen(&models);
    let dir = audit_dir(root);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|source| CensusError::Write {
            path: dir.display().to_string(),
            source,
        })?;
    }
    let shapes_dir = crate::census::shapes_dir(root);
    let mut tables: Vec<String> = std::fs::read_dir(&shapes_dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|n| n.strip_suffix(".toml"))
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    tables.sort();
    let (mut audited, mut found) = (0, 0);
    for table in tables {
        let shapes = crate::census::read_shapes(root, &table)?;
        let a = audit_table(&table, &shapes, &models);
        if a.finding.is_empty() {
            continue;
        }
        audited += 1;
        found += a.finding.len();
        let text = format!(
            "# Generated by `utilake-nem-census audit`: where AEMO's declarations of {table} and its data disagree.\n{}",
            toml::to_string(&a).expect("an audit serialises")
        );
        write_text(&dir.join(format!("{table}.toml")), &text)?;
    }
    Ok((audited, found))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use utilake_mms_registry::file_model::shapes::Mark;

    use super::*;
    use crate::models::ModelTable;

    fn col(name: &str, ctl: TypeClass, kind: Kind) -> ShapeColumn {
        ShapeColumn {
            name: name.into(),
            ctl,
            kind,
            nulls: 0,
            nulls_since: None,
            int_digits: 0,
            scale: 0,
            sig_digits: 0,
            max_len: 0,
            min: None,
            max: None,
        }
    }

    fn shape(versions: Vec<u16>, columns: Vec<ShapeColumn>) -> ShapeEntry {
        let mark = Mark {
            month: "2026-08".parse().unwrap(),
            file: "F.zip".into(),
            sha256: None,
            published: None,
            data_time: None,
        };
        ShapeEntry {
            id: ShapeId(1),
            report_type: "DUDETAIL".into(),
            report_subtype: String::new(),
            versions,
            file_kinds: vec!["archive".into()],
            first_seen: mark.month,
            last_seen: mark.month,
            from: mark.clone(),
            to: mark,
            files: 1,
            rows: 10,
            concurrent_with: Vec::new(),
            columns,
        }
    }

    fn model(version: &str, columns: &[(&str, &str, bool)]) -> Model {
        Model {
            version: version.into(),
            model: String::new(),
            created: String::new(),
            script_sha256: String::new(),
            first_seen: "2026-05".parse().unwrap(),
            last_seen: "2026-09".parse().unwrap(),
            tables: BTreeMap::from([(
                "DUDETAIL".to_owned(),
                ModelTable {
                    comment: String::new(),
                    primary_key: vec!["DUID".into()],
                    columns: columns
                        .iter()
                        .map(|(n, t, nl)| ((*n).into(), (*t).into(), *nl, String::new()))
                        .collect(),
                },
            )]),
        }
    }

    #[test]
    fn every_disagreement_is_found_with_where_it_was_seen() {
        let mut capacity = col("REGISTEREDCAPACITY", TypeClass::Number, Kind::Decimal);
        (capacity.int_digits, capacity.scale) = (4, 17);
        let mut duid = col("DUID", TypeClass::Varchar, Kind::Text);
        (duid.max_len, duid.nulls, duid.nulls_since) = (12, 2, Some("G.zip".into()));
        let mut interval = col("DISPATCHINTERVAL", TypeClass::Varchar, Kind::Integer);
        interval.int_digits = 11;
        let s = shape(
            vec![1, 2],
            vec![
                duid,
                capacity,
                interval,
                col("NOTE", TypeClass::Number, Kind::Text),
                col("EMPTY", TypeClass::Number, Kind::Empty),
                col("NEW", TypeClass::Unknown, Kind::Integer),
                col("WHEN", TypeClass::Date, Kind::Timestamp),
            ],
        );
        let older = model("5.6", &[("DUID", "VARCHAR(10)", false)]);
        let newer = model(
            "5.7",
            &[
                ("DUID", "VARCHAR(10)", false),
                ("REGISTEREDCAPACITY", "NUMERIC(6,0)", true),
                ("DISPATCHINTERVAL", "VARCHAR(22)", true),
                ("NOTE", "NUMERIC(3,0)", true),
                ("EMPTY", "NUMERIC(3,0)", true),
                ("WHEN", "TIMESTAMP(0)", true),
                ("GONE", "CHAR(1)", true),
            ],
        );
        let a = audit_table("DUDETAIL", std::slice::from_ref(&s), &[&older, &newer]);
        let kinds: Vec<(&str, Option<&str>)> = a
            .finding
            .iter()
            .map(|f| (f.kind.as_str(), f.column.as_deref()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("versions_without_change", None),
                ("null_in_not_null", Some("DUID")),
                ("longer_than_declared", Some("DUID")),
                ("wider_than_declared", Some("REGISTEREDCAPACITY")),
                ("ctl_text_holds_typed_values", Some("DISPATCHINTERVAL")),
                ("ctl_type_does_not_hold", Some("NOTE")),
                ("model_type_does_not_hold", Some("NOTE")),
                ("never_a_value", Some("EMPTY")),
                ("no_ctl_declares_column", Some("NEW")),
                ("model_lacks_column", Some("NEW")),
                ("ctl_type_does_not_hold", Some("WHEN")),
                ("declared_never_seen", Some("GONE")),
            ],
            "{:#?}",
            a.finding
        );
        let nulls = &a.finding[1];
        assert_eq!(
            (nulls.declared_by.as_str(), nulls.file.as_deref()),
            ("model 5.7", Some("G.zip"))
        );
        assert!(a.finding[3].observed.contains("17 after"));
        // A shape outside every release's months is held to its control
        // files only.
        let mut early = s.clone();
        early.first_seen = "2015-01".parse().unwrap();
        early.last_seen = "2015-01".parse().unwrap();
        let a = audit_table("DUDETAIL", &[early], &[&older, &newer]);
        assert!(
            a.finding
                .iter()
                .all(|f| !f.declared_by.starts_with("model"))
        );
        assert!(
            audit_table("OTHER", &[s], &[&newer])
                .finding
                .iter()
                .all(|f| f.declared_by != "model 5.7")
        );
    }

    #[test]
    fn model_types_read() {
        assert_eq!(ModelType::parse("VARCHAR(20)"), ModelType::Varchar(20));
        assert_eq!(ModelType::parse("char(1)"), ModelType::Varchar(1));
        assert_eq!(ModelType::parse("NUMERIC(15,5)"), ModelType::Numeric(15, 5));
        assert_eq!(ModelType::parse("DECIMAL(3)"), ModelType::Numeric(3, 0));
        assert_eq!(ModelType::parse("NUMERIC(x,1)"), ModelType::Other);
        assert_eq!(ModelType::parse("VARCHAR(x)"), ModelType::Other);
        assert_eq!(ModelType::parse("TIMESTAMP(0)"), ModelType::Timestamp);
        assert_eq!(ModelType::parse("DATE"), ModelType::Timestamp);
        assert_eq!(ModelType::parse("BLOB"), ModelType::Other);
    }

    #[test]
    fn a_schemas_tree_is_audited_into_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let s = shape(vec![1, 2], vec![col("A", TypeClass::Number, Kind::Integer)]);
        write_text(
            &crate::census::shapes_dir(root).join("DUDETAIL.toml"),
            &utilake_mms_registry::file_model::shapes::render_shapes(std::slice::from_ref(&s)),
        )
        .unwrap();
        let mut clean = s;
        clean.versions = vec![1];
        write_text(
            &crate::census::shapes_dir(root).join("CLEAN.toml"),
            &utilake_mms_registry::file_model::shapes::render_shapes(&[clean]),
        )
        .unwrap();
        std::fs::create_dir_all(audit_dir(root).join("STALE")).unwrap();
        assert_eq!(audit(root).unwrap(), (1, 1));
        let text = std::fs::read_to_string(audit_dir(root).join("DUDETAIL.toml")).unwrap();
        let back: Audit = toml::from_str(&text).unwrap();
        assert_eq!(back.finding[0].kind, "versions_without_change");
        assert!(!audit_dir(root).join("STALE").exists());
        assert!(!audit_dir(root).join("CLEAN.toml").exists());
    }
}
