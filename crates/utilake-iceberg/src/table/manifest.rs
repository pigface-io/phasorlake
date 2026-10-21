//! Manifests and manifest lists, format version 3, as Avro files.
//!
//! A manifest lists the data files one snapshot added; a manifest list
//! names every manifest a snapshot can see. Both carry the field ids the
//! Iceberg spec gives their columns, so any reader resolves them. Tables
//! here are unpartitioned, so a data file's partition is the empty record.

use std::collections::BTreeMap;

use crate::schema::Schema;
use crate::table::avro::{self, AvroError, FieldTy, Ty, Value};

/// A data file a snapshot adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFile {
    /// Its location, as readers open it (`file:///…`).
    pub path: String,
    /// Its rows.
    pub records: u64,
    /// Its size in bytes.
    pub bytes: u64,
}

/// One entry of a manifest: a file and the snapshot that added it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The snapshot that added the file.
    pub snapshot_id: i64,
    /// The sequence number it was added at.
    pub sequence_number: i64,
    /// The file.
    pub file: DataFile,
}

/// One entry of a manifest list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    /// The manifest's location.
    pub path: String,
    /// Its size in bytes.
    pub length: i64,
    /// The sequence number it was added at.
    pub sequence_number: i64,
    /// The least sequence number of its files.
    pub min_sequence_number: i64,
    /// The snapshot that added it.
    pub added_snapshot_id: i64,
    /// Files it added.
    pub added_files: i32,
    /// Rows those files hold.
    pub added_rows: i64,
    /// The row id of its first row (row lineage).
    pub first_row_id: Option<i64>,
}

/// An `ADDED` manifest entry's status.
const ADDED: i32 = 1;

fn manifest_entry() -> Ty {
    Ty::Record {
        name: "manifest_entry",
        fields: vec![
            FieldTy::required("status", 0, Ty::Int),
            FieldTy::optional("snapshot_id", 1, Ty::Long),
            FieldTy::optional("sequence_number", 3, Ty::Long),
            FieldTy::optional("file_sequence_number", 4, Ty::Long),
            FieldTy::required(
                "data_file",
                2,
                Ty::Record {
                    name: "r2",
                    fields: vec![
                        FieldTy::required("content", 134, Ty::Int),
                        FieldTy::required("file_path", 100, Ty::String),
                        FieldTy::required("file_format", 101, Ty::String),
                        FieldTy::required(
                            "partition",
                            102,
                            Ty::Record {
                                name: "r102",
                                fields: vec![],
                            },
                        ),
                        FieldTy::required("record_count", 103, Ty::Long),
                        FieldTy::required("file_size_in_bytes", 104, Ty::Long),
                        FieldTy::optional("first_row_id", 142, Ty::Long),
                    ],
                },
            ),
        ],
    }
}

fn manifest_file() -> Ty {
    Ty::Record {
        name: "manifest_file",
        fields: vec![
            FieldTy::required("manifest_path", 500, Ty::String),
            FieldTy::required("manifest_length", 501, Ty::Long),
            FieldTy::required("partition_spec_id", 502, Ty::Int),
            FieldTy::required("content", 517, Ty::Int),
            FieldTy::required("sequence_number", 515, Ty::Long),
            FieldTy::required("min_sequence_number", 516, Ty::Long),
            FieldTy::required("added_snapshot_id", 503, Ty::Long),
            FieldTy::required("added_files_count", 504, Ty::Int),
            FieldTy::required("existing_files_count", 505, Ty::Int),
            FieldTy::required("deleted_files_count", 506, Ty::Int),
            FieldTy::required("added_rows_count", 512, Ty::Long),
            FieldTy::required("existing_rows_count", 513, Ty::Long),
            FieldTy::required("deleted_rows_count", 514, Ty::Long),
            FieldTy::optional("first_row_id", 520, Ty::Long),
        ],
    }
}

fn long(v: u64) -> Value {
    Value::Long(i64::try_from(v).unwrap_or(i64::MAX))
}

/// A manifest of `files`, all added by snapshot `snapshot_id` at
/// `sequence` under `schema`.
pub fn write_manifest(
    schema: &Schema,
    snapshot_id: i64,
    sequence: i64,
    files: &[DataFile],
) -> Result<Vec<u8>, AvroError> {
    let records: Vec<Value> = files
        .iter()
        .map(|f| {
            Value::Record(vec![
                Value::Int(ADDED),
                Value::opt(Some(Value::Long(snapshot_id))),
                Value::opt(Some(Value::Long(sequence))),
                Value::opt(Some(Value::Long(sequence))),
                Value::Record(vec![
                    Value::Int(0),
                    Value::String(f.path.clone()),
                    Value::String("PARQUET".into()),
                    Value::Record(vec![]),
                    long(f.records),
                    long(f.bytes),
                    // Null: the file's first row id follows from the manifest's.
                    Value::opt(None),
                ]),
            ])
        })
        .collect();
    let metadata = BTreeMap::from([
        ("schema".to_owned(), schema.to_json().to_string()),
        ("schema-id".to_owned(), schema.schema_id.to_string()),
        ("partition-spec".to_owned(), "[]".to_owned()),
        ("partition-spec-id".to_owned(), "0".to_owned()),
        ("format-version".to_owned(), "3".to_owned()),
        ("content".to_owned(), "data".to_owned()),
    ]);
    avro::write_container(&manifest_entry(), &metadata, &records)
}

/// A record's fields, read by position with their expected shape.
struct Fields(Vec<Value>);

impl Fields {
    fn of(v: Value) -> Result<Self, AvroError> {
        match v {
            Value::Record(f) => Ok(Self(f)),
            _ => Err(AvroError::Mismatch { expected: "record" }),
        }
    }

    fn take(&mut self, i: usize) -> Value {
        std::mem::replace(&mut self.0[i], Value::Null)
    }

    fn long(&mut self, i: usize) -> Result<i64, AvroError> {
        match self.take(i) {
            Value::Long(l) => Ok(l),
            Value::Int(n) => Ok(i64::from(n)),
            _ => Err(AvroError::Mismatch { expected: "long" }),
        }
    }

    fn string(&mut self, i: usize) -> Result<String, AvroError> {
        match self.take(i) {
            Value::String(s) => Ok(s),
            _ => Err(AvroError::Mismatch { expected: "string" }),
        }
    }

    fn opt_long(&mut self, i: usize) -> Result<Option<i64>, AvroError> {
        match self.take(i) {
            Value::Union(_, v) if *v == Value::Null => Ok(None),
            Value::Union(_, v) => Self(vec![*v]).long(0).map(Some),
            _ => Err(AvroError::Mismatch { expected: "union" }),
        }
    }
}

fn unsigned(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// The entries of a manifest this module wrote.
pub fn read_manifest(bytes: &[u8]) -> Result<Vec<Entry>, AvroError> {
    let (_, records) = avro::read_container(&manifest_entry(), bytes)?;
    records
        .into_iter()
        .map(|r| {
            let mut e = Fields::of(r)?;
            let mut f = Fields::of(e.take(4))?;
            Ok(Entry {
                snapshot_id: e.opt_long(1)?.unwrap_or(0),
                sequence_number: e.opt_long(2)?.unwrap_or(0),
                file: DataFile {
                    path: f.string(1)?,
                    records: unsigned(f.long(4)?),
                    bytes: unsigned(f.long(5)?),
                },
            })
        })
        .collect()
}

/// The manifest list of snapshot `snapshot_id`.
pub fn write_manifest_list(
    snapshot_id: i64,
    parent: Option<i64>,
    sequence: i64,
    first_row_id: i64,
    manifests: &[ManifestFile],
) -> Result<Vec<u8>, AvroError> {
    let records: Vec<Value> = manifests
        .iter()
        .map(|m| {
            Value::Record(vec![
                Value::String(m.path.clone()),
                Value::Long(m.length),
                Value::Int(0),
                Value::Int(0),
                Value::Long(m.sequence_number),
                Value::Long(m.min_sequence_number),
                Value::Long(m.added_snapshot_id),
                Value::Int(m.added_files),
                Value::Int(0),
                Value::Int(0),
                Value::Long(m.added_rows),
                Value::Long(0),
                Value::Long(0),
                Value::opt(m.first_row_id.map(Value::Long)),
            ])
        })
        .collect();
    let metadata = BTreeMap::from([
        ("snapshot-id".to_owned(), snapshot_id.to_string()),
        (
            "parent-snapshot-id".to_owned(),
            parent.map_or_else(|| "null".to_owned(), |p| p.to_string()),
        ),
        ("sequence-number".to_owned(), sequence.to_string()),
        ("first-row-id".to_owned(), first_row_id.to_string()),
        ("format-version".to_owned(), "3".to_owned()),
    ]);
    avro::write_container(&manifest_file(), &metadata, &records)
}

/// The entries of a manifest list this module wrote.
pub fn read_manifest_list(bytes: &[u8]) -> Result<Vec<ManifestFile>, AvroError> {
    let (_, records) = avro::read_container(&manifest_file(), bytes)?;
    records
        .into_iter()
        .map(|r| {
            let mut f = Fields::of(r)?;
            Ok(ManifestFile {
                path: f.string(0)?,
                length: f.long(1)?,
                sequence_number: f.long(4)?,
                min_sequence_number: f.long(5)?,
                added_snapshot_id: f.long(6)?,
                added_files: i32::try_from(f.long(7)?).unwrap_or(i32::MAX),
                added_rows: f.long(10)?,
                first_row_id: f.opt_long(13)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, IcebergType};

    fn schema() -> Schema {
        Schema {
            schema_id: 2,
            fields: vec![Field {
                id: 1,
                name: "SETTLEMENTDATE".into(),
                ty: IcebergType::Timestamptz,
                required: true,
                doc: None,
            }],
            identifier_field_ids: vec![1],
            last_column_id: 1,
        }
    }

    #[test]
    fn a_manifest_round_trips_with_its_header() {
        let files = vec![
            DataFile {
                path: "file:///lake/a.parquet".into(),
                records: 10,
                bytes: 1234,
            },
            DataFile {
                path: "file:///lake/b.parquet".into(),
                records: 3,
                bytes: 99,
            },
        ];
        let bytes = write_manifest(&schema(), 7, 4, &files).unwrap();
        let back = read_manifest(&bytes).unwrap();
        assert_eq!(
            back.iter().map(|e| &e.file).cloned().collect::<Vec<_>>(),
            files
        );
        assert!(
            back.iter()
                .all(|e| e.snapshot_id == 7 && e.sequence_number == 4)
        );
        let (meta, _) = avro::read_container(&manifest_entry(), &bytes).unwrap();
        assert_eq!(meta["schema-id"], "2");
        assert_eq!(meta["content"], "data");
        assert!(meta["schema"].contains("SETTLEMENTDATE"));
    }

    #[test]
    fn a_manifest_list_round_trips() {
        let list = vec![
            ManifestFile {
                path: "file:///t/metadata/1-m0.avro".into(),
                length: 500,
                sequence_number: 1,
                min_sequence_number: 1,
                added_snapshot_id: 1,
                added_files: 2,
                added_rows: 13,
                first_row_id: Some(0),
            },
            ManifestFile {
                path: "file:///t/metadata/2-m0.avro".into(),
                length: 400,
                sequence_number: 2,
                min_sequence_number: 2,
                added_snapshot_id: 2,
                added_files: 1,
                added_rows: 1,
                first_row_id: None,
            },
        ];
        let bytes = write_manifest_list(2, Some(1), 2, 13, &list).unwrap();
        assert_eq!(read_manifest_list(&bytes).unwrap(), list);
        let (meta, _) = avro::read_container(&manifest_file(), &bytes).unwrap();
        assert_eq!(meta["parent-snapshot-id"], "1");
        let first = write_manifest_list(1, None, 1, 0, &list[..1]).unwrap();
        let (meta, _) = avro::read_container(&manifest_file(), &first).unwrap();
        assert_eq!(meta["parent-snapshot-id"], "null");
        assert!(read_manifest_list(&[]).is_err());
        assert!(read_manifest(&first).is_err(), "a list is not a manifest");
    }

    #[test]
    fn fields_of_another_shape_are_refused() {
        assert!(Fields::of(Value::Null).is_err());
        let mut f = Fields(vec![
            Value::Null,
            Value::Null,
            Value::Long(1),
            Value::Int(2),
        ]);
        assert!(f.long(0).is_err());
        assert!(f.string(1).is_err());
        assert!(f.opt_long(2).is_err());
        assert_eq!(Fields(vec![Value::Int(2)]).long(0), Ok(2));
        assert_eq!(long(u64::MAX), Value::Long(i64::MAX));
        assert_eq!(unsigned(-1), 0);
    }
}
