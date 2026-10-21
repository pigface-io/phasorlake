//! A table schema: fields with ids that are never reused.

use serde_json::{Value, json};

use crate::types::IcebergType;

/// One column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The field id, never reused within the table.
    pub id: u32,
    /// The column name.
    pub name: String,
    /// Its type.
    pub ty: IcebergType,
    /// Whether every row has a value.
    pub required: bool,
    /// What the column holds, when known.
    pub doc: Option<String>,
}

/// A table's columns at one point in its history.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schema {
    /// Counts up with each update applied.
    pub schema_id: u32,
    /// The columns, in display order.
    pub fields: Vec<Field>,
    /// The fields that form the key.
    pub identifier_field_ids: Vec<u32>,
    /// The largest id ever given out in the table, deleted fields included:
    /// the next column gets the one after.
    pub last_column_id: u32,
}

impl Schema {
    /// The field with this id.
    pub fn field(&self, id: u32) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// The field with this name.
    pub fn by_name(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Whether `other` has the same columns (ids, names, types, nullability,
    /// docs, order) and the same key, whatever ids the two schemas carry.
    pub fn same_as(&self, other: &Self) -> bool {
        let key = |s: &Self| {
            let mut ids = s.identifier_field_ids.clone();
            ids.sort_unstable();
            ids
        };
        self.fields == other.fields && key(self) == key(other)
    }

    /// A schema as Iceberg table metadata writes it, when it is one this
    /// crate's types can hold. `last_column_id` is the largest field id in
    /// it: the table's metadata knows the true one.
    pub fn from_json(v: &Value) -> Option<Self> {
        let fields = v
            .get("fields")?
            .as_array()?
            .iter()
            .map(|f| {
                Some(Field {
                    id: u32::try_from(f.get("id")?.as_u64()?).ok()?,
                    name: f.get("name")?.as_str()?.to_owned(),
                    ty: IcebergType::parse(f.get("type")?.as_str()?)?,
                    required: f.get("required")?.as_bool()?,
                    doc: f.get("doc").and_then(Value::as_str).map(str::to_owned),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let identifier_field_ids = match v.get("identifier-field-ids") {
            None => Vec::new(),
            Some(ids) => ids
                .as_array()?
                .iter()
                .map(|i| i.as_u64().and_then(|i| u32::try_from(i).ok()))
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self {
            schema_id: u32::try_from(v.get("schema-id")?.as_u64()?).ok()?,
            last_column_id: fields.iter().map(|f| f.id).max().unwrap_or(0),
            fields,
            identifier_field_ids,
        })
    }

    /// The schema as Iceberg table metadata writes it.
    pub fn to_json(&self) -> Value {
        json!({
            "type": "struct",
            "schema-id": self.schema_id,
            "identifier-field-ids": self.identifier_field_ids,
            "fields": self.fields.iter().map(|f| {
                let mut v = json!({
                    "id": f.id,
                    "name": f.name,
                    "required": f.required,
                    "type": f.ty.name(),
                });
                if let Some(doc) = &f.doc {
                    v["doc"] = json!(doc);
                }
                v
            }).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_iceberg_metadata_json() {
        let s = Schema {
            schema_id: 2,
            fields: vec![
                Field {
                    id: 1,
                    name: "SETTLEMENTDATE".into(),
                    ty: IcebergType::Timestamptz,
                    required: true,
                    doc: Some("Interval end".into()),
                },
                Field {
                    id: 3,
                    name: "RRP".into(),
                    ty: IcebergType::Double,
                    required: false,
                    doc: None,
                },
            ],
            identifier_field_ids: vec![1],
            last_column_id: 3,
        };
        assert_eq!(
            s.to_json(),
            json!({
                "type": "struct", "schema-id": 2, "identifier-field-ids": [1],
                "fields": [
                    {"id": 1, "name": "SETTLEMENTDATE", "required": true, "type": "timestamptz", "doc": "Interval end"},
                    {"id": 3, "name": "RRP", "required": false, "type": "double"},
                ]
            })
        );
        assert_eq!(s.by_name("RRP").map(|f| f.id), Some(3));
        assert_eq!(Schema::from_json(&s.to_json()), Some(s.clone()));
        let mut v = s.to_json();
        v.as_object_mut().unwrap().remove("identifier-field-ids");
        assert_eq!(
            Schema::from_json(&v).map(|s| s.identifier_field_ids),
            Some(vec![])
        );
        for bad in [
            json!({}),
            json!({"schema-id": 0, "fields": [{"id": 1, "name": "A", "type": "uuid", "required": true}]}),
            json!({"schema-id": 0, "fields": [], "identifier-field-ids": ["a"]}),
            json!({"schema-id": -1, "fields": []}),
        ] {
            assert_eq!(Schema::from_json(&bad), None, "{bad}");
        }
        assert!(s.field(2).is_none());
    }

    #[test]
    fn sameness_ignores_ids_and_key_order() {
        let field = |id, name: &str| Field {
            id,
            name: name.into(),
            ty: IcebergType::Long,
            required: true,
            doc: None,
        };
        let a = Schema {
            schema_id: 0,
            fields: vec![field(1, "A"), field(2, "B")],
            identifier_field_ids: vec![1, 2],
            last_column_id: 2,
        };
        let b = Schema {
            schema_id: 7,
            identifier_field_ids: vec![2, 1],
            last_column_id: 9,
            ..a.clone()
        };
        assert!(a.same_as(&b));
        let moved = Schema {
            fields: vec![field(2, "B"), field(1, "A")],
            ..a.clone()
        };
        assert!(!a.same_as(&moved));
        let rekeyed = Schema {
            identifier_field_ids: vec![1],
            ..a.clone()
        };
        assert!(!a.same_as(&rekeyed));
    }
}
