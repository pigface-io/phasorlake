//! One shape as a JSON Schema document, and a table's index of its shapes.

use serde_json::{Map, Value, json};
use utilake_mms_registry::file_model::model::{Column, Increment, LakeType, LoadMode, Table};
use utilake_mms_registry::file_model::temporal::RoleOrigin;
use utilake_mms_registry::shape::{SHAPE_FORMAT, TypeClass};

use super::{FORMAT, ShapeDoc};

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
    let names = doc.columns.iter().map(|(n, _)| n.as_str());
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

impl Matched<'_> {
    /// The increment's number.
    pub fn n(&self) -> u16 {
        self.increment.n
    }
}

/// The document's text: pretty JSON with sorted keys and a final newline.
pub fn render(
    doc: &ShapeDoc<'_>,
    id: &str,
    increment: Option<(Matched<'_>, Option<&str>)>,
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
    let description = doc
        .declared
        .and_then(|t| t.description.as_deref())
        .or_else(|| doc.model.and_then(|m| m.description.as_deref()));
    if let Some(d) = description {
        root.insert("description".into(), json!(d));
    }
    root.insert("type".into(), json!("object"));
    root.insert("additionalProperties".into(), json!(false));
    let required: Vec<&str> = doc
        .columns
        .iter()
        .filter_map(|(n, _)| doc.declared.and_then(|t| t.column(n)))
        .filter(|c| !c.nullable)
        .map(|c| c.name.as_str())
        .collect();
    if !required.is_empty() {
        root.insert("required".into(), json!(required));
    }
    let properties: Map<String, Value> = doc
        .columns
        .iter()
        .map(|(name, class)| {
            let decl = doc.declared.and_then(|t| t.column(name));
            let comment = doc
                .model
                .and_then(|m| m.columns.iter().find(|c| c.name == *name))
                .map(|c| c.comment.as_str())
                .filter(|c| !c.is_empty());
            (name.clone(), property(*class, decl, comment))
        })
        .collect();
    root.insert("properties".into(), Value::Object(properties));
    root.insert("x-format".into(), json!(FORMAT));
    root.insert("x-market".into(), json!("nem"));
    root.insert("x-table".into(), table(doc));
    root.insert(
        "x-shape".into(),
        json!({
            "id": doc.id.to_string(),
            "shape_format": SHAPE_FORMAT,
            "report_type": doc.report_type,
            "report_subtype": doc.report_subtype,
            "columns": doc.columns.iter().map(|(n, c)| json!([n, c.as_str()])).collect::<Vec<_>>(),
        }),
    );
    if let Some((m, version)) = increment {
        root.insert("x-increment".into(), increment_value(m, version));
    }
    let months: Vec<Value> = doc
        .seen
        .iter()
        .map(|(m, rows)| json!([m.to_string(), rows]))
        .collect();
    root.insert(
        "x-seen".into(),
        json!({
            "first": doc.first.to_string(),
            "last": doc.last.to_string(),
            "versions": doc.versions,
            "months": months,
        }),
    );
    text(&Value::Object(root))
}

/// A table's `index.json`: its shapes, first seen first.
pub fn index(table: &str, declared: bool, docs: &[(ShapeDoc<'_>, Option<u16>)]) -> String {
    let shapes: Vec<Value> = docs
        .iter()
        .map(|(d, increment)| {
            json!({
                "id": d.id.to_string(),
                "report_type": d.report_type,
                "report_subtype": d.report_subtype,
                "first": d.first.to_string(),
                "last": d.last.to_string(),
                "columns": d.columns.len(),
                "increment": increment,
            })
        })
        .collect();
    text(&json!({ "table": table, "declared": declared, "shapes": shapes }))
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
fn property(class: TypeClass, decl: Option<&Column>, comment: Option<&str>) -> Value {
    let lake = decl.map(|c| c.ty);
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
    let nullable = decl.is_none_or(|c| c.nullable);
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
    if let Some(c) = comment {
        p.insert("description".into(), json!(c));
    }
    p.insert("x-type-class".into(), json!(class.as_str()));
    if let Some(c) = decl {
        p.insert("x-lake-type".into(), json!(c.ty.as_str()));
        p.insert("x-column-id".into(), json!(c.id.0));
    }
    Value::Object(p)
}

/// `x-table`: the declaration, or only the name for a table we do not declare.
fn table(doc: &ShapeDoc<'_>) -> Value {
    let mut t = Map::new();
    t.insert("name".into(), json!(doc.table));
    t.insert("declared".into(), json!(doc.declared.is_some()));
    if let Some(m) = doc.model {
        t.insert("package".into(), json!(m.package));
        if let Some(v) = &m.visibility {
            t.insert("visibility".into(), json!(v));
        }
    }
    if let Some(d) = doc.declared {
        declared(&mut t, d);
    }
    Value::Object(t)
}

fn declared(t: &mut Map<String, Value>, d: &Table) {
    let mode = match d.load_mode {
        LoadMode::Append => "append",
        LoadMode::Snapshot => "snapshot",
    };
    t.insert("load_mode".into(), json!(mode));
    t.insert("primary_key".into(), json!(d.primary_key));
    if let Some(c) = &d.time_column {
        t.insert("time_column".into(), json!(c));
    }
    let origin = match d.temporal.origin {
        RoleOrigin::Derived => "derived",
        RoleOrigin::Hand => "hand",
    };
    let mut valid = Map::new();
    valid.insert("kind".into(), json!(d.temporal.valid.kind()));
    if let Some(c) = d.temporal.valid.column() {
        valid.insert("column".into(), json!(c));
    }
    let mut temporal = Map::new();
    temporal.insert("origin".into(), json!(origin));
    temporal.insert("entity".into(), json!(d.temporal.entity));
    temporal.insert("valid".into(), Value::Object(valid));
    temporal.insert("order".into(), json!(d.temporal.order));
    if let Some(c) = &d.temporal.cutoff {
        temporal.insert("cutoff".into(), json!(c));
    }
    t.insert("temporal".into(), Value::Object(temporal));
}

fn increment_value(m: Matched<'_>, version: Option<&str>) -> Value {
    let i = m.increment;
    let mut v = Map::new();
    v.insert("status".into(), json!(m.status));
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
    if let Some(mv) = version {
        v.insert("aemo_model_version".into(), json!(mv));
    }
    Value::Object(v)
}
