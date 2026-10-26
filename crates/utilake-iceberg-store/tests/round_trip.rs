//! A table that goes A, B, A', B: Parquet written by the lake's own Arrow,
//! metadata and manifests by the store, read back here by field id. With
//! `UTILAKE_KEEP_TABLE=<empty dir>` the table is kept for other readers:
//! `scripts/iceberg-readers.py` reads it with pyiceberg and DuckDB.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Decimal128Array, Float64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field as ArrowField, Schema as ArrowSchema, TimeUnit};
use parquet::arrow::ArrowWriter;
use utilake_core::Diagnose;
use utilake_iceberg::table::{TableMetadata, read_manifest, read_manifest_list};
use utilake_iceberg::{Field, IcebergType, Schema};
use utilake_iceberg_store::{Append, Error, NewFile, Store};

const TS: i64 = 1_722_470_400_000_000; // 2024-08-01T00:00:00Z in microseconds

fn field(id: u32, name: &str, ty: IcebergType, required: bool) -> Field {
    Field {
        id,
        name: name.into(),
        ty,
        required,
        doc: None,
    }
}

fn schema(fields: Vec<Field>) -> Schema {
    let last = fields.iter().map(|f| f.id).max().unwrap_or(0);
    Schema {
        schema_id: 0,
        fields,
        identifier_field_ids: vec![1, 2],
        last_column_id: last,
    }
}

fn dec(precision: u8) -> IcebergType {
    IcebergType::decimal(precision, 5).unwrap()
}

fn a() -> Schema {
    schema(vec![
        field(1, "settlementdate", IcebergType::Timestamptz, true),
        field(2, "regionid", IcebergType::String, true),
        field(3, "rrp", dec(15), false),
    ])
}

/// B: rrp widened, raise1secrrp added and placed before it.
fn b() -> Schema {
    schema(vec![
        field(1, "settlementdate", IcebergType::Timestamptz, true),
        field(2, "regionid", IcebergType::String, true),
        field(4, "raise1secrrp", IcebergType::Double, false),
        field(3, "rrp", dec(18), false),
    ])
}

/// A again, but types only widen: rrp stays decimal(18, 5).
fn a_widened() -> Schema {
    schema(vec![
        field(1, "settlementdate", IcebergType::Timestamptz, true),
        field(2, "regionid", IcebergType::String, true),
        field(3, "rrp", dec(18), false),
    ])
}

fn arrow_field(name: &str, ty: DataType, id: u32, nullable: bool) -> ArrowField {
    ArrowField::new(name, ty, nullable).with_metadata(HashMap::from([(
        "PARQUET:field_id".to_owned(),
        id.to_string(),
    )]))
}

/// Writes rows `(minutes after TS, region, raise, rrp in 1e-5)` with or
/// without the raise column; the size on disk.
fn parquet(path: &Path, precision: u8, raise: bool, rows: &[(i64, &str, f64, i128)]) -> u64 {
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    let mut fields = vec![
        arrow_field("settlementdate", ts, 1, false),
        arrow_field("regionid", DataType::Utf8, 2, false),
    ];
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(
            TimestampMicrosecondArray::from(
                rows.iter()
                    .map(|r| TS + r.0 * 60_000_000)
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(StringArray::from(
            rows.iter().map(|r| r.1).collect::<Vec<_>>(),
        )),
    ];
    if raise {
        fields.push(arrow_field("raise1secrrp", DataType::Float64, 4, true));
        columns.push(Arc::new(Float64Array::from(
            rows.iter().map(|r| r.2).collect::<Vec<_>>(),
        )));
    }
    fields.push(arrow_field(
        "rrp",
        DataType::Decimal128(precision, 5),
        3,
        true,
    ));
    columns.push(Arc::new(
        Decimal128Array::from(rows.iter().map(|r| r.3).collect::<Vec<_>>())
            .with_precision_and_scale(precision, 5)
            .unwrap(),
    ));
    let batch = RecordBatch::try_new(Arc::new(ArrowSchema::new(fields)), columns).unwrap();
    let mut w = ArrowWriter::try_new(File::create(path).unwrap(), batch.schema(), None).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    std::fs::metadata(path).unwrap().len()
}

fn append(
    store: &Store,
    path: PathBuf,
    rows: u64,
    bytes: u64,
    id: i64,
    file: &str,
) -> TableMetadata {
    store
        .append(Append {
            files: vec![NewFile { path, rows, bytes }],
            snapshot_id: id,
            at_ms: 1_000 + id,
            properties: BTreeMap::from([("utilake.first-file".to_owned(), file.to_owned())]),
        })
        .unwrap()
}

fn local(location: &str) -> PathBuf {
    PathBuf::from(location.strip_prefix("file://").unwrap())
}

#[test]
fn schemas_come_back_and_every_snapshot_sees_every_file_before_it() {
    let tmp = tempfile::tempdir().unwrap();
    // Iceberg locations are absolute, so a kept table stays where it is written.
    let base = std::env::var("UTILAKE_KEEP_TABLE")
        .map_or_else(|_| tmp.path().to_path_buf(), PathBuf::from);
    let dir = base.join("dispatchprice");
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let store = Store::at(&dir);
    store
        .create("9c2f4f4e-1d1a-4b7a-9a37-0d6c1e1a2b3c", &a(), 1_000)
        .unwrap();
    assert_eq!(
        store.set_schema(&a(), 1_000).unwrap(),
        0,
        "the current schema again is no change"
    );
    assert_eq!(store.current().unwrap().0, 1, "and writes no version");

    let fa = data.join("a.parquet");
    let n = parquet(
        &fa,
        15,
        false,
        &[(-5, "NSW1", 0.0, 10_112_345), (0, "NSW1", 0.0, 9_950_000)],
    );
    append(&store, fa, 2, n, 11, "A.zip");

    assert_eq!(store.set_schema(&b(), 1_011).unwrap(), 1);
    let fb = data.join("b.parquet");
    let n = parquet(
        &fb,
        18,
        true,
        &[
            (5, "NSW1", 1.5, 123_456_789_012_350_000),
            (10, "QLD1", 2.25, 8_800_000),
        ],
    );
    append(&store, fb, 2, n, 12, "B.zip");

    assert_eq!(store.set_schema(&a_widened(), 1_012).unwrap(), 2);
    let fc = data.join("c.parquet");
    let n = parquet(&fc, 18, false, &[(15, "VIC1", 0.0, 7_770_000)]);
    append(&store, fc, 1, n, 13, "C.zip");

    assert_eq!(
        store.set_schema(&b(), 1_013).unwrap(),
        1,
        "B comes back with its id"
    );
    let fd = data.join("d.parquet");
    let n = parquet(&fd, 18, true, &[(20, "SA1", 3.0, 6_660_000)]);
    let meta = append(&store, fd, 1, n, 14, "D.zip");

    assert_eq!(
        meta.schemas.iter().map(|s| s.schema_id).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(meta.current_schema_id, 1);
    assert_eq!(meta.last_column_id, 4);
    assert_eq!(meta.next_row_id, 6);
    assert_eq!(meta.last_sequence_number, 4);
    assert_eq!(
        meta.snapshots
            .iter()
            .map(|s| s.schema_id)
            .collect::<Vec<_>>(),
        [0, 1, 2, 1]
    );
    assert_eq!(
        meta.snapshots
            .iter()
            .map(|s| s.parent_snapshot_id)
            .collect::<Vec<_>>(),
        [None, Some(11), Some(12), Some(13)]
    );
    assert_eq!(meta.snapshots[3].summary["utilake.first-file"], "D.zip");
    assert_eq!(meta.snapshots[1].summary["added-records"], "2");
    assert_eq!(meta.snapshots[1].summary["operation"], "append");
    assert_eq!(meta.metadata_log.len(), 7);
    assert_eq!(meta.refs["main"].snapshot_id, 14);
    assert_eq!(
        store.current().unwrap().0,
        8,
        "created, then three schema changes and four appends"
    );

    // The last snapshot sees every manifest, each with its rows' first id.
    let list = read_manifest_list(&std::fs::read(local(&meta.snapshots[3].manifest_list)).unwrap())
        .unwrap();
    assert_eq!(
        list.iter()
            .map(|m| (m.added_snapshot_id, m.sequence_number, m.first_row_id))
            .collect::<Vec<_>>(),
        [
            (11, 1, Some(0)),
            (12, 2, Some(2)),
            (13, 3, Some(4)),
            (14, 4, Some(5))
        ]
    );
    let mut files = Vec::new();
    for m in &list {
        let bytes = std::fs::read(local(&m.path)).unwrap();
        assert_eq!(m.length, bytes.len() as i64);
        for e in read_manifest(&bytes).unwrap() {
            assert_eq!(e.snapshot_id, m.added_snapshot_id);
            files.push((
                e.file.path.rsplit('/').next().unwrap().to_owned(),
                e.file.records,
            ));
        }
    }
    assert_eq!(
        files,
        [
            ("a.parquet".to_owned(), 2),
            ("b.parquet".to_owned(), 2),
            ("c.parquet".to_owned(), 1),
            ("d.parquet".to_owned(), 1)
        ]
    );

    // The same history writes the same bytes, manifests and all.
    let replay = || {
        let root = tmp.path().join("replay");
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        let s = Store::at(&root);
        s.create("9c2f4f4e-1d1a-4b7a-9a37-0d6c1e1a2b3c", &a(), 1_000)
            .unwrap();
        s.set_schema(&b(), 1_001).unwrap();
        append(&s, data.join("b.parquet"), 2, 9, 1, "B.zip");
        s.set_schema(&a_widened(), 1_002).unwrap();
        append(&s, data.join("c.parquet"), 1, 9, 2, "C.zip");
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(root.join("metadata"))
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    };
    let first = replay();
    assert_eq!(
        first.len(),
        10,
        "5 versions, the hint, 2 manifests, 2 lists"
    );
    assert_eq!(first, replay());
}

#[test]
fn refusals_name_their_cause() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(&tmp.path().join("t"));
    let err = store.current().unwrap_err();
    assert!(matches!(err, Error::Io(_)), "no table yet");
    assert_eq!(err.code(), "store.iceberg_io");

    store
        .create("9c2f4f4e-1d1a-4b7a-9a37-0d6c1e1a2b3c", &a(), 1)
        .unwrap();
    let file = || NewFile {
        path: tmp.path().join("x.parquet"),
        rows: 1,
        bytes: 1,
    };
    let one = Append {
        files: vec![file()],
        snapshot_id: 1,
        at_ms: 1,
        properties: BTreeMap::new(),
    };
    let meta = store.append(one.clone()).unwrap();
    assert_eq!(meta.current_snapshot().map(|s| s.snapshot_id), Some(1));

    // A version that cannot be written leaves the table as it was.
    std::fs::create_dir(tmp.path().join("t/metadata/v3.metadata.json")).unwrap();
    let err = store.set_schema(&b(), 3).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err}");
    assert_eq!(store.current().unwrap().0, 2);
    std::fs::remove_dir(tmp.path().join("t/metadata/v3.metadata.json")).unwrap();

    // A manifest list that does not read.
    let list = tmp.path().join("t/metadata/snap-1.avro");
    std::fs::write(&list, b"not avro").unwrap();
    let err = store
        .append(Append {
            snapshot_id: 2,
            ..one.clone()
        })
        .unwrap_err();
    assert!(matches!(err, Error::Manifest(_)), "{err}");
    assert_eq!(err.code(), "store.iceberg_manifest");
    assert!(err.to_string().contains("not valid"), "{err}");

    // A manifest list somewhere the store cannot open.
    let meta_file = tmp.path().join("t/metadata/v2.metadata.json");
    let text = std::fs::read_to_string(&meta_file).unwrap();
    std::fs::write(&meta_file, text.replace("file://", "s3://")).unwrap();
    let err = store
        .append(Append {
            snapshot_id: 2,
            ..one
        })
        .unwrap_err();
    assert!(matches!(err, Error::Location(_)), "{err}");
    assert_eq!(err.code(), "store.iceberg_location");
    assert!(err.diagnostic().to_string().contains("s3://"), "{err}");

    let meta = tmp.path().join("t/metadata");
    std::fs::write(meta.join("version-hint.text"), "two").unwrap();
    let err = store.current().unwrap_err();
    assert!(err.to_string().contains("not a version"), "{err}");
    std::fs::write(meta.join("version-hint.text"), "1").unwrap();
    std::fs::write(meta.join("v1.metadata.json"), "{").unwrap();
    let err = store.current().unwrap_err();
    assert!(matches!(err, Error::Json(_)), "{err}");
    assert!(err.to_string().contains("not valid"), "{err}");
    assert_eq!(err.code(), "store.iceberg_metadata");
}

#[test]
fn a_tables_properties_are_set_once() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    store.create("u", &a(), 1_000).unwrap();
    let props = std::collections::BTreeMap::from([("utilake.key".to_owned(), "[]".to_owned())]);
    store.set_properties(props.clone(), 1_001).unwrap();
    let (v, m) = store.current().unwrap();
    assert_eq!((v, &m.properties), (2, &props));
    // The same again writes nothing.
    store.set_properties(props, 1_002).unwrap();
    assert_eq!(store.current().unwrap().0, 2);
}
