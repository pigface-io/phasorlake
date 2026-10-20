//! The two documents of a table: `table.json`, which describes the table
//! once, and one JSON Schema per shape, which a row of that shape
//! validates against and which points back at the table.

use serde_json::{Map, Value, json};
use utilake_mms_registry::file_model::model::{Increment, LakeType, LoadMode, Table};
use utilake_mms_registry::file_model::temporal::RoleOrigin;
use utilake_mms_registry::shape::{SHAPE_FORMAT, TypeClass};

use super::{DmTable, FORMAT, IncrementDoc, ModelFacts, ShapeDoc, logical_type};

/// How AEMO writes a day in a data file.
const DATE_PATTERN: &str = r"^\d{4}/\d{2}/\d{2}$";
const DATE_FORMAT: &str = "YYYY/MM/DD";
/// How AEMO writes a time in a data file.
const TIMESTAMP_PATTERN: &str = r"^\d{4}/\d{2}/\d{2} \d{2}:\d{2}:\d{2}$";
const TIMESTAMP_FORMAT: &str = "YYYY/MM/DD HH24:MI:SS";

/// The increment the shape belongs to: the one whose columns are the
/// shape's, in order, else the one in force when the shape was first seen,
/// which the shape then deviates from. `None` for an undeclared table.
pub fn increment<'a>(doc: &ShapeDoc<'a>) -> Option<Matched<'a>> {
    let table = doc.declared?;
    let names = doc.columns.iter().map(|c| c.name.as_str());
    if let Some(i) = table
        .increments
        .iter()
        .find(|i| i.columns.iter().map(String::as_str).eq(names.clone()))
    {
        return Some(Matched {
            increment: i,
            status: "conforms",
        });
    }
    table.in_force(doc.first).map(|i| Matched {
        increment: i,
        status: "deviates",
    })
}

/// An increment and how the shape relates to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Matched<'a> {
    /// The increment.
    pub increment: &'a Increment,
    /// `conforms` or `deviates`.
    pub status: &'static str,
}

/// A shape's document: pretty JSON with sorted keys and a final newline.
pub fn render(
    doc: &ShapeDoc<'_>,
    id: &str,
    description: Option<&str>,
    matched: Option<Matched<'_>>,
) -> String {
    let mut root = Map::new();
    root.insert(
        "$schema".into(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    root.insert("$id".into(), json!(id));
    root.insert(
        "title".into(),
        json!(format!("{}, shape {}", doc.table, doc.id)),
    );
    if let Some(d) = description {
        root.insert("description".into(), json!(d));
    }
    root.insert("type".into(), json!("object"));
    root.insert("additionalProperties".into(), json!(false));
    let required: Vec<&str> = doc
        .columns
        .iter()
        .filter_map(|c| doc.declared.and_then(|t| t.column(&c.name)))
        .filter(|c| !c.nullable)
        .map(|c| c.name.as_str())
        .collect();
    if !required.is_empty() {
        root.insert("required".into(), json!(required));
    }
    let properties: Map<String, Value> = doc
        .columns
        .iter()
        .map(|c| {
            let (name, class) = (&c.name, &c.ctl);
            let decl = doc.declared.and_then(|t| t.column(name));
            (
                name.clone(),
                property(*class, decl.map(|c| c.ty), decl.is_none_or(|c| c.nullable)),
            )
        })
        .collect();
    root.insert("properties".into(), Value::Object(properties));
    root.insert("x-format".into(), json!(FORMAT));
    root.insert("x-market".into(), json!("nem"));
    root.insert(
        "x-table".into(),
        json!({ "name": doc.table, "declared": doc.declared.is_some(), "$ref": "table.json" }),
    );
    root.insert(
        "x-shape".into(),
        json!({
            "id": doc.id.to_string(),
            "shape_format": SHAPE_FORMAT,
            "report_type": doc.report_type,
            "report_subtype": doc.report_subtype,
            "columns": doc.columns.iter().map(|c| json!([c.name, c.ctl.as_str()])).collect::<Vec<_>>(),
        }),
    );
    if let Some(m) = matched {
        root.insert(
            "x-increment".into(),
            json!({ "n": m.increment.n, "status": m.status }),
        );
    }
    let months: Vec<Value> = doc
        .seen
        .iter()
        .map(|(m, seen)| json!([m.to_string(), seen.rows, seen.order, seen.first, seen.last]))
        .collect();
    // The shape's whole span: the least first and greatest last over its
    // months, of the column the first spanned month names.
    let span = doc.seen.values().filter(|s| s.first.is_some()).fold(
        None::<(String, String, String)>,
        |acc, s| {
            let f = s.first.clone().unwrap_or_default();
            let l = s.last.clone().unwrap_or_default();
            let c = s.column.clone().unwrap_or_default();
            Some(match acc {
                None => (f, l, c),
                Some((af, al, ac)) => (af.min(f), al.max(l), ac),
            })
        },
    );
    root.insert(
        "x-seen".into(),
        json!({
            "first": doc.first.to_string(),
            "last": doc.last.to_string(),
            "versions": doc.versions,
            "months": months,
            "span": span.map(|(first, last, column)| json!({ "first": first, "last": last, "column": column })),
        }),
    );
    text(&Value::Object(root))
}

/// Pretty JSON with a final newline. `serde_json`'s map is ordered by key,
/// so the text is the same on every run.
pub fn text(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("JSON values serialize");
    s.push('\n');
    s
}

/// One column's schema. The value is the field's text in the CSV, so a
/// date or time is a string of AEMO's format; a number is a number unless
/// our lake keeps it whole.
fn property(class: TypeClass, lake: Option<LakeType>, nullable: bool) -> Value {
    let (ty, pattern) = match (class, lake) {
        (TypeClass::Varchar, _) => (Some("string"), None),
        (TypeClass::Number, Some(LakeType::Long)) | (TypeClass::Integer, _) => {
            (Some("integer"), None)
        }
        (TypeClass::Number, _) => (Some("number"), None),
        (TypeClass::Date, _) => (Some("string"), Some((DATE_PATTERN, DATE_FORMAT))),
        (TypeClass::Timestamp, _) => (Some("string"), Some((TIMESTAMP_PATTERN, TIMESTAMP_FORMAT))),
        (TypeClass::Unknown, Some(lake)) => match lake {
            LakeType::String => (Some("string"), None),
            LakeType::Double => (Some("number"), None),
            LakeType::Long => (Some("integer"), None),
            LakeType::Boolean => (Some("boolean"), None),
            LakeType::Date => (Some("string"), Some((DATE_PATTERN, DATE_FORMAT))),
            LakeType::TimestampNs => (Some("string"), Some((TIMESTAMP_PATTERN, TIMESTAMP_FORMAT))),
        },
        (TypeClass::Unknown, None) => (None, None),
    };
    let mut p = Map::new();
    if let Some(ty) = ty {
        p.insert(
            "type".into(),
            if nullable {
                json!([ty, "null"])
            } else {
                json!(ty)
            },
        );
    }
    if let Some((pattern, format)) = pattern {
        p.insert("pattern".into(), json!(pattern));
        p.insert("x-aemo-format".into(), json!(format));
    }
    p.insert("x-type-class".into(), json!(class.as_str()));
    Value::Object(p)
}

/// What `table.json` is built from.
pub struct TableDoc<'a> {
    /// The table's name.
    pub name: &'a str,
    /// Its declaration, when it has one.
    pub declared: Option<&'a Table>,
    /// The data-model report's entry, when it has one.
    pub model: Option<&'a DmTable>,
    /// What the archived releases declare.
    pub facts: &'a ModelFacts<'a>,
    /// The report's version, for a type's source.
    pub report_version: Option<&'a str>,
    /// The increments with their control files read.
    pub increments: &'a [IncrementDoc<'a>],
    /// The shapes, with the increment each matched.
    pub shapes: &'a [(ShapeDoc<'a>, Option<Matched<'a>>)],
}

/// The table as an Iceberg (format version 3) table: its schema, its
/// partition spec, and its eras in the order the archive published them,
/// each the schema updates that take the table from the columns published
/// before it to its own (a column is added once and never dropped: a list
/// without it leaves it null).
fn iceberg(x: &Table, shapes: &[(ShapeDoc<'_>, Option<Matched<'_>>)]) -> Value {
    let (schema, issue) = x.iceberg_schema();
    let mut out = Map::new();
    out.insert("format-version".into(), json!(3));
    out.insert("schema".into(), schema.to_json());
    out.insert("partition-spec".into(), x.partition_spec().to_json());
    if let Some(e) = issue {
        out.insert("x-key-issue".into(), json!(e.to_string()));
    }
    let mut seen: Vec<&str> = Vec::new();
    let eras: Vec<Value> = shapes
        .iter()
        .map(|(s, _)| {
            let names: Vec<&str> = s.columns.iter().map(|c| c.name.as_str()).collect();
            let updates: Vec<Value> = names
                .iter()
                .filter(|n| !seen.contains(n))
                .filter_map(|n| schema.by_name(n))
                .map(|f| {
                    json!({
                        "action": "add-column",
                        "field-id": f.id,
                        "name": f.name,
                        "type": f.ty.name(),
                        "required": f.required,
                    })
                })
                .collect();
            let absent: Vec<&str> = seen
                .iter()
                .copied()
                .filter(|n| !names.contains(n))
                .collect();
            for n in &names {
                if !seen.contains(n) {
                    seen.push(n);
                }
            }
            json!({
                "shape": s.id.to_string(),
                "first": s.first.to_string(),
                "last": s.last.to_string(),
                "versions": s.versions,
                "updates": updates,
                "not-published": absent,
            })
        })
        .collect();
    out.insert("eras".into(), Value::Array(eras));
    Value::Object(out)
}

/// `table.json`.
pub fn table(d: &TableDoc<'_>) -> String {
    let mut t = Map::new();
    t.insert("name".into(), json!(d.name));
    t.insert("declared".into(), json!(d.declared.is_some()));
    t.insert("x-format".into(), json!(FORMAT));
    if let Some(x) = d.declared {
        t.insert("iceberg".into(), iceberg(x, d.shapes));
    }
    let description = d
        .declared
        .and_then(|x| x.description.as_deref())
        .or_else(|| {
            d.facts
                .latest
                .map(|(_, m)| m.comment.as_str())
                .filter(|c| !c.is_empty())
        })
        .or_else(|| d.model.and_then(|m| m.description.as_deref()));
    if let Some(desc) = description {
        t.insert("description".into(), json!(desc));
    }
    if let Some(m) = d.model {
        t.insert("package".into(), json!(m.package));
        if let Some(v) = &m.visibility {
            t.insert("visibility".into(), json!(v));
        }
    }
    if let Some(x) = d.declared {
        declaration(&mut t, x, d);
    }
    if !d.facts.keys.is_empty() {
        t.insert("key_history".into(), key_history(d.facts));
    }
    let shapes: Vec<Value> = d
        .shapes
        .iter()
        .map(|(s, m)| {
            json!({
                "id": s.id.to_string(),
                "report_type": s.report_type,
                "report_subtype": s.report_subtype,
                "first": s.first.to_string(),
                "last": s.last.to_string(),
                "columns": s.columns.len(),
                "versions": s.versions,
                "increment": m.map(|m| m.increment.n),
                "status": m.map(|m| m.status),
            })
        })
        .collect();
    t.insert("shapes".into(), Value::Array(shapes));
    text(&Value::Object(t))
}

/// The key per release, consecutive equal keys folded: each segment
/// applies from the first month its release shipped until the next
/// segment's.
fn key_history(facts: &ModelFacts<'_>) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let mut last: Option<&[String]> = None;
    for (from, key, version) in &facts.keys {
        if last == Some(*key) {
            continue;
        }
        last = Some(key);
        out.push(json!({ "from": from.to_string(), "columns": key, "source": format!("MMSDM_create_v{version}.zip") }));
    }
    Value::Array(out)
}

fn declaration(t: &mut Map<String, Value>, x: &Table, d: &TableDoc<'_>) {
    let mode = match x.load_mode {
        LoadMode::Append => "append",
        LoadMode::Snapshot => "snapshot",
    };
    t.insert("load_mode".into(), json!(mode));
    t.insert("primary_key".into(), json!(x.primary_key));
    if let Some(c) = &x.time_column {
        t.insert("time_column".into(), json!(c));
    }
    let origin = match x.temporal.origin {
        RoleOrigin::Derived => "derived",
        RoleOrigin::Hand => "hand",
    };
    let mut valid = Map::new();
    valid.insert("kind".into(), json!(x.temporal.valid.kind()));
    if let Some(c) = x.temporal.valid.column() {
        valid.insert("column".into(), json!(c));
    }
    let mut temporal = Map::new();
    temporal.insert("origin".into(), json!(origin));
    temporal.insert("entity".into(), json!(x.temporal.entity));
    temporal.insert("valid".into(), Value::Object(valid));
    temporal.insert("order".into(), json!(x.temporal.order));
    if let Some(c) = &x.temporal.cutoff {
        temporal.insert("cutoff".into(), json!(c));
    }
    t.insert("temporal".into(), Value::Object(temporal));

    let columns: Vec<Value> = x
        .columns
        .iter()
        .map(|c| {
            let mut col = Map::new();
            col.insert("name".into(), json!(c.name));
            col.insert("id".into(), json!(c.id.0));
            col.insert("lake_type".into(), json!(c.ty.as_str()));
            col.insert("nullable".into(), json!(c.nullable));
            let from_release = d.facts.latest.and_then(|(v, m)| {
                m.columns
                    .iter()
                    .find(|(n, ..)| *n == c.name)
                    .map(|(_, ty, _, comment)| (ty.clone(), v.to_owned(), comment.clone()))
            });
            let from_report = d.model.and_then(|m| {
                m.columns.iter().find(|r| r.name == c.name).map(|r| {
                    (
                        logical_type(&r.oracle),
                        format!("report {}", d.report_version.unwrap_or("?")),
                        r.comment.clone(),
                    )
                })
            });
            let comment = from_release
                .as_ref()
                .map(|(_, _, c)| c.clone())
                .filter(|c| !c.is_empty())
                .or_else(|| {
                    from_report
                        .as_ref()
                        .map(|(_, _, c)| c.clone())
                        .filter(|c| !c.is_empty())
                });
            if let Some((ty, source, _)) = from_release.or(from_report) {
                col.insert("model_type".into(), json!(ty));
                col.insert("model_type_source".into(), json!(source));
            }
            if let Some(c) = comment {
                col.insert("comment".into(), json!(c));
            }
            if let Some(n) = &c.note {
                col.insert("note".into(), json!(n));
            }
            Value::Object(col)
        })
        .collect();
    t.insert("columns".into(), Value::Array(columns));

    let increments: Vec<Value> = d
        .increments
        .iter()
        .map(|inc| {
            let i = inc.increment;
            let mut v = Map::new();
            v.insert("n".into(), json!(i.n));
            v.insert("from".into(), json!(i.from.to_string()));
            if let Some(c) = &i.ctl {
                v.insert("ctl".into(), json!(c));
            }
            if let Some(h) = &i.ctl_sha256 {
                v.insert("ctl_sha256".into(), json!(h));
            }
            if let Some(a) = &i.aemo_ctl {
                v.insert("aemo_ctl".into(), json!(a));
            }
            if let Some(mv) = &inc.model_version {
                v.insert("aemo_model_version".into(), json!(mv));
            }
            if !i.aemo_versions.is_empty() {
                v.insert("aemo_versions".into(), json!(i.aemo_versions));
            }
            if let Some(n) = &i.note {
                v.insert("note".into(), json!(n));
            }
            let cols: Vec<Value> = i
                .columns
                .iter()
                .map(|c| json!({ "name": c, "loader_type": inc.loader_types.get(c) }))
                .collect();
            v.insert("columns".into(), Value::Array(cols));
            Value::Object(v)
        })
        .collect();
    t.insert("increments".into(), Value::Array(increments));
}
