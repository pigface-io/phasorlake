//! A table's history as an Iceberg table, in the export: one JSON Schema
//! per Iceberg schema, and the history itself in `table.json`.
//!
//! The history is the fold's (`schemas/nem/iceberg/<TABLE>.json`): the
//! schemas, the runs of files written under each with the updates between
//! them, and the columns that needed a new field. A schema's document is
//! named by a hash of its fields and key, so its `$id` never changes when
//! the history grows: `nem/<TABLE>/schema-<hash>.json`. A row of the lake
//! read under that schema validates against it; `x-fields` gives each
//! field's id and Iceberg type, and `x-runs` every run of files written
//! under it, with their first and last file and data time.

use serde_json::{Map, Value, json};
use utilake_mms_registry::file_model::ctl::sha256;

use super::FORMAT;

/// The JSON Schema of a value of an Iceberg type, as the lake holds it.
fn value_schema(ty: &str) -> Value {
    match ty {
        "boolean" => json!({"type": "boolean"}),
        "long" => json!({"type": "integer"}),
        "double" => json!({"type": "number"}),
        "date" => json!({"type": "string", "format": "date"}),
        "timestamptz" => json!({"type": "string", "format": "date-time"}),
        t if t.starts_with("decimal(") => json!({"type": "number"}),
        _ => json!({"type": "string"}),
    }
}

/// The file name of a schema: a hash of its fields and key.
fn file_name(schema: &Value) -> String {
    let identity = json!({
        "fields": schema["fields"],
        "identifier-field-ids": schema.get("identifier-field-ids").cloned().unwrap_or(json!([])),
    });
    let hash = sha256(identity.to_string().as_bytes());
    format!("schema-{}.json", &hash[..16])
}

/// One schema's document.
fn document(base: &str, table: &str, path: &str, schema: &Value, runs: &[&Value]) -> String {
    let id = schema["schema-id"].as_u64().unwrap_or(0);
    let fields = schema["fields"].as_array().cloned().unwrap_or_default();
    let mut properties = Map::new();
    let mut required = Vec::new();
    for f in &fields {
        let name = f["name"].as_str().unwrap_or_default();
        let ty = f["type"].as_str().unwrap_or_default();
        let mut p = value_schema(ty);
        if f["required"].as_bool() == Some(true) {
            required.push(json!(name));
        } else {
            let t = p["type"].clone();
            p["type"] = json!([t, "null"]);
        }
        p["x-field-id"] = f["id"].clone();
        p["x-iceberg-type"] = json!(ty);
        properties.insert(name.to_owned(), p);
    }
    let doc = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": format!("{base}{path}"),
        "title": format!("{table}, Iceberg schema {id}"),
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
        "x-format": FORMAT,
        "x-table": {"name": table, "$ref": "table.json"},
        "x-schema-id": id,
        "x-identifier-field-ids": schema.get("identifier-field-ids").cloned().unwrap_or(json!([])),
        "x-fields": fields
            .iter()
            .map(|f| json!([f["id"], f["name"], f["type"]]))
            .collect::<Vec<_>>(),
        "x-runs": runs
            .iter()
            .map(|r| json!({
                "from": r["from"],
                "to": r["to"],
                "files": r["files"],
                "rows": r["rows"],
                "version": r["version"],
                "kinds": r["kinds"],
            }))
            .collect::<Vec<_>>(),
    });
    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    text.push('\n');
    text
}

/// Every schema document of a table, as `(path, text)`, and the block of
/// its `table.json`: the history with each schema's document named.
pub fn render(base: &str, table: &str, history: &Value) -> (Vec<(String, String)>, Value) {
    let schemas = history["schemas"].as_array().cloned().unwrap_or_default();
    let runs = history["runs"].as_array().cloned().unwrap_or_default();
    let mut files = Vec::new();
    let mut named = Map::new();
    for s in &schemas {
        let id = s["schema-id"].as_u64().unwrap_or(0);
        let path = format!("nem/{table}/{}", file_name(s));
        let of: Vec<&Value> = runs
            .iter()
            .filter(|r| r["schema-id"].as_u64() == Some(id))
            .collect();
        files.push((path.clone(), document(base, table, &path, s, &of)));
        named.insert(
            id.to_string(),
            json!(path.rsplit('/').next().unwrap_or_default()),
        );
    }
    let block = json!({
        "format-version": 3,
        "schemas": schemas,
        "schema-files": named,
        "runs": runs,
        "breaking": history.get("breaking").cloned().unwrap_or(json!([])),
    });
    (files, block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> Value {
        json!({
            "table": "UNIT",
            "format-version": 3,
            "schemas": [
                {"schema-id": 0, "type": "struct", "identifier-field-ids": [1], "fields": [
                    {"id": 1, "name": "SETTLEMENTDATE", "type": "timestamptz", "required": true},
                    {"id": 2, "name": "SCADAVALUE", "type": "decimal(16, 6)", "required": false},
                ]},
                {"schema-id": 1, "type": "struct", "identifier-field-ids": [1], "fields": [
                    {"id": 1, "name": "SETTLEMENTDATE", "type": "timestamptz", "required": true},
                    {"id": 2, "name": "SCADAVALUE", "type": "decimal(16, 6)", "required": false},
                    {"id": 3, "name": "FLAG", "type": "boolean", "required": false},
                    {"id": 4, "name": "N", "type": "long", "required": false},
                    {"id": 5, "name": "X", "type": "double", "required": false},
                    {"id": 6, "name": "D", "type": "date", "required": false},
                    {"id": 7, "name": "S", "type": "string", "required": false},
                ]},
            ],
            "runs": [
                {"schema-id": 0, "version": 1, "kinds": ["dvd"], "files": 2, "rows": 9,
                 "from": {"file": "A", "data-time": "2024-01-01T00:05:00+10:00"},
                 "to": {"file": "B", "data-time": null}, "changes": ["first"], "updates": []},
                {"schema-id": 1, "version": 1, "kinds": ["dvd"], "files": 1, "rows": 3,
                 "from": {"file": "C", "data-time": null}, "to": {"file": "C", "data-time": null},
                 "changes": ["schema"], "updates": [{"action": "add", "field-id": 3}]},
            ],
            "breaking": [],
        })
    }

    #[test]
    fn one_document_per_schema_named_by_its_fields() {
        let (files, block) = render("https://h/", "UNIT", &history());
        assert_eq!(files.len(), 2);
        let (path, text) = &files[0];
        assert!(
            path.starts_with("nem/UNIT/schema-") && path.ends_with(".json"),
            "{path}"
        );
        let doc: Value = serde_json::from_str(text).unwrap();
        assert_eq!(doc["$id"], format!("https://h/{path}"));
        assert_eq!(doc["required"], json!(["SETTLEMENTDATE"]));
        assert_eq!(doc["properties"]["SETTLEMENTDATE"]["format"], "date-time");
        assert_eq!(
            doc["properties"]["SCADAVALUE"]["type"],
            json!(["number", "null"])
        );
        assert_eq!(doc["properties"]["SCADAVALUE"]["x-field-id"], 2);
        assert_eq!(doc["x-runs"][0]["files"], 2);
        assert_eq!(
            doc["x-fields"][1],
            json!([2, "SCADAVALUE", "decimal(16, 6)"])
        );
        let second: Value = serde_json::from_str(&files[1].1).unwrap();
        let ty = |n: &str| second["properties"][n]["type"].clone();
        assert_eq!(
            [ty("FLAG"), ty("N"), ty("X"), ty("S")],
            [
                json!(["boolean", "null"]),
                json!(["integer", "null"]),
                json!(["number", "null"]),
                json!(["string", "null"])
            ]
        );
        assert_eq!(second["properties"]["D"]["format"], "date");
        // The block of table.json: the history, each schema's file named.
        assert_eq!(
            block["schema-files"]["0"],
            json!(path.rsplit('/').next().unwrap())
        );
        assert_eq!(block["runs"][1]["updates"][0]["action"], "add");
        // A history that grows keeps every name: the name is the fields'.
        let mut grown = history();
        grown["runs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"schema-id": 0}));
        assert_eq!(render("https://h/", "UNIT", &grown).0[0].0, *path);
        // Odd input renders, empty.
        let (none, block) = render("https://h/", "UNIT", &json!({}));
        assert!(none.is_empty());
        assert_eq!(block["breaking"], json!([]));
    }
}
