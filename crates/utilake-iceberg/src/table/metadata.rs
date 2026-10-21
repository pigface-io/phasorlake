//! Table metadata, format version 3, as `vN.metadata.json` holds it.
//!
//! The tables here are unpartitioned and unsorted, written by one writer.
//! Every list is kept in the order it grew (schemas and snapshots by id,
//! logs by time) and maps are sorted, so the same history serialises to the
//! same bytes. Times are the caller's, never the clock's.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::schema::Schema;

/// The metadata of one version of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TableMetadata {
    /// Always 3.
    pub format_version: u8,
    /// The table's UUID.
    pub table_uuid: String,
    /// The table's base location.
    pub location: String,
    /// The sequence number of the last snapshot.
    pub last_sequence_number: i64,
    /// When this version was written, milliseconds since 1970.
    pub last_updated_ms: i64,
    /// The largest field id the table ever gave out.
    pub last_column_id: u32,
    /// Every schema, by id.
    #[serde(with = "schemas")]
    pub schemas: Vec<Schema>,
    /// The schema new data is written with.
    pub current_schema_id: u32,
    /// The one, empty partition spec.
    pub partition_specs: Vec<Spec>,
    /// Its id.
    pub default_spec_id: i32,
    /// The largest partition field id; 999 when there are none.
    pub last_partition_id: i32,
    /// Table properties.
    pub properties: BTreeMap<String, String>,
    /// The current snapshot, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_snapshot_id: Option<i64>,
    /// Every snapshot, by id.
    pub snapshots: Vec<Snapshot>,
    /// When each snapshot became current.
    pub snapshot_log: Vec<SnapshotLog>,
    /// The metadata files before this one.
    pub metadata_log: Vec<MetadataLog>,
    /// The one, empty sort order.
    pub sort_orders: Vec<SortOrder>,
    /// Its id.
    pub default_sort_order_id: i32,
    /// Named references: `main` once there is a snapshot.
    pub refs: BTreeMap<String, Ref>,
    /// The row id the next added row gets (row lineage).
    pub next_row_id: i64,
}

/// A partition spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Spec {
    /// Its id.
    pub spec_id: i32,
    /// Its fields; none here.
    pub fields: Vec<Value>,
}

/// A sort order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SortOrder {
    /// Its id.
    pub order_id: i32,
    /// Its fields; none here.
    pub fields: Vec<Value>,
}

/// A snapshot: the table's files at one point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Snapshot {
    /// Its id.
    pub snapshot_id: i64,
    /// The snapshot it follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_snapshot_id: Option<i64>,
    /// Its sequence number.
    pub sequence_number: i64,
    /// When it was committed, milliseconds since 1970.
    pub timestamp_ms: i64,
    /// Its manifest list.
    pub manifest_list: String,
    /// `operation` and the writer's properties.
    pub summary: BTreeMap<String, String>,
    /// The schema its files were written with.
    pub schema_id: u32,
    /// The row id of the first row it added.
    pub first_row_id: i64,
    /// Rows it added.
    pub added_rows: i64,
}

/// One entry of the snapshot log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SnapshotLog {
    /// When.
    pub timestamp_ms: i64,
    /// Which.
    pub snapshot_id: i64,
}

/// One entry of the metadata log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MetadataLog {
    /// When that version was written.
    pub timestamp_ms: i64,
    /// Its file.
    pub metadata_file: String,
}

/// A named reference to a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Ref {
    /// The snapshot.
    pub snapshot_id: i64,
    /// `branch`.
    #[serde(rename = "type")]
    pub kind: String,
}

mod schemas {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    use crate::schema::Schema;

    pub fn serialize<S: Serializer>(v: &[Schema], s: S) -> Result<S::Ok, S::Error> {
        v.iter()
            .map(Schema::to_json)
            .collect::<Vec<_>>()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Schema>, D::Error> {
        Vec::<Value>::deserialize(d)?
            .iter()
            .map(|v| {
                Schema::from_json(v)
                    .ok_or_else(|| D::Error::custom(format!("not a schema of this lake: {v}")))
            })
            .collect()
    }
}

impl TableMetadata {
    /// A new table at `location` with its first schema, written at `at_ms`.
    pub fn new(uuid: &str, location: &str, schema: &Schema, at_ms: i64) -> Self {
        Self {
            format_version: 3,
            table_uuid: uuid.to_owned(),
            location: location.to_owned(),
            last_sequence_number: 0,
            last_updated_ms: at_ms,
            last_column_id: schema.last_column_id,
            schemas: vec![schema.clone()],
            current_schema_id: schema.schema_id,
            partition_specs: vec![Spec {
                spec_id: 0,
                fields: Vec::new(),
            }],
            default_spec_id: 0,
            last_partition_id: 999,
            properties: BTreeMap::new(),
            current_snapshot_id: None,
            snapshots: Vec::new(),
            snapshot_log: Vec::new(),
            metadata_log: Vec::new(),
            sort_orders: vec![SortOrder {
                order_id: 0,
                fields: Vec::new(),
            }],
            default_sort_order_id: 0,
            refs: BTreeMap::new(),
            next_row_id: 0,
        }
    }

    /// The schema new data is written with.
    pub fn current_schema(&self) -> &Schema {
        self.schemas
            .iter()
            .find(|s| s.schema_id == self.current_schema_id)
            .unwrap_or(&self.schemas[0])
    }

    /// The current snapshot.
    pub fn current_snapshot(&self) -> Option<&Snapshot> {
        let id = self.current_snapshot_id?;
        self.snapshots.iter().find(|s| s.snapshot_id == id)
    }

    /// The version after this one, which was the file `file`: logged, and
    /// written at `at_ms`.
    #[must_use]
    pub fn next(&self, file: &str, at_ms: i64) -> Self {
        let mut next = self.clone();
        next.metadata_log.push(MetadataLog {
            timestamp_ms: self.last_updated_ms,
            metadata_file: file.to_owned(),
        });
        next.last_updated_ms = at_ms;
        next
    }

    /// Makes `schema` current. An earlier schema with the same columns and
    /// key becomes current again with its id; any other is added with the
    /// next id. Returns the id, and whether the current schema changed.
    pub fn set_schema(&mut self, schema: &Schema) -> (u32, bool) {
        let id = match self.schemas.iter().find(|s| s.same_as(schema)) {
            Some(old) => old.schema_id,
            None => {
                let id = self.schemas.iter().map(|s| s.schema_id).max().unwrap_or(0) + 1;
                self.schemas.push(Schema {
                    schema_id: id,
                    ..schema.clone()
                });
                id
            }
        };
        self.last_column_id = self.last_column_id.max(schema.last_column_id);
        let changed = id != self.current_schema_id;
        self.current_schema_id = id;
        (id, changed)
    }

    /// Commits `snapshot` as current on `main`.
    pub fn add_snapshot(&mut self, snapshot: Snapshot) {
        self.last_sequence_number = snapshot.sequence_number;
        self.next_row_id = snapshot.first_row_id + snapshot.added_rows;
        self.current_snapshot_id = Some(snapshot.snapshot_id);
        self.snapshot_log.push(SnapshotLog {
            timestamp_ms: snapshot.timestamp_ms,
            snapshot_id: snapshot.snapshot_id,
        });
        self.refs.insert(
            "main".to_owned(),
            Ref {
                snapshot_id: snapshot.snapshot_id,
                kind: "branch".to_owned(),
            },
        );
        self.snapshots.push(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, IcebergType};

    fn schema(fields: &[(u32, &str)]) -> Schema {
        Schema {
            schema_id: 0,
            fields: fields
                .iter()
                .map(|(id, name)| Field {
                    id: *id,
                    name: (*name).into(),
                    ty: IcebergType::Long,
                    required: false,
                    doc: None,
                })
                .collect(),
            identifier_field_ids: vec![],
            last_column_id: fields.iter().map(|f| f.0).max().unwrap_or(0),
        }
    }

    #[test]
    fn schemas_are_interned_and_snapshots_committed() {
        let a = schema(&[(1, "A")]);
        let b = schema(&[(1, "A"), (2, "B")]);
        let mut m = TableMetadata::new("u", "file:///t", &a, 10);
        assert_eq!(m.set_schema(&a), (0, false));
        assert_eq!(m.set_schema(&b), (1, true));
        assert_eq!(m.last_column_id, 2);
        assert_eq!(m.set_schema(&a), (0, true), "A comes back with its id");
        assert_eq!(m.current_schema().fields.len(), 1);
        assert!(m.current_snapshot().is_none());
        let mut n = m.next("file:///t/metadata/v1.metadata.json", 20);
        assert_eq!(n.metadata_log[0].timestamp_ms, 10);
        n.add_snapshot(Snapshot {
            snapshot_id: 5,
            parent_snapshot_id: None,
            sequence_number: 1,
            timestamp_ms: 20,
            manifest_list: "file:///t/metadata/snap-5.avro".into(),
            summary: BTreeMap::from([("operation".to_owned(), "append".to_owned())]),
            schema_id: 0,
            first_row_id: 0,
            added_rows: 7,
        });
        assert_eq!(n.current_snapshot().map(|s| s.snapshot_id), Some(5));
        assert_eq!(n.next_row_id, 7);
        assert_eq!(n.refs["main"].kind, "branch");
        let text = serde_json::to_string(&n).unwrap();
        assert!(text.contains("\"type\":\"branch\""), "{text}");
        assert!(!text.contains("parent-snapshot-id"));
        let back: TableMetadata = serde_json::from_str(&text).unwrap();
        assert_eq!(back, n);
        // An unknown current schema falls back to the first.
        n.current_schema_id = 9;
        assert_eq!(n.current_schema().schema_id, 0);
        let bad = text.replace("\"long\"", "\"uuid\"");
        let err = serde_json::from_str::<TableMetadata>(&bad).unwrap_err();
        assert!(
            err.to_string().contains("not a schema of this lake"),
            "{err}"
        );
    }
}
