//! A table that goes A, B, A', B: Parquet written by the lake's own Arrow,
//! metadata by the store, read back through the library's scan.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Decimal128Array, Float64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field as ArrowField, Schema as ArrowSchema, TimeUnit};
use futures::TryStreamExt;
use iceberg::io::FileIO;
use iceberg::table::Table;
use iceberg::{Runtime, TableIdent};
use parquet::arrow::ArrowWriter;
use utilake_core::Diagnose;
use utilake_iceberg::{Field, IcebergType, PartitionSpec, Schema};
use utilake_iceberg_store::{Append, Error, NewFile, Store};

const TS: i64 = 1_722_470_400_000_000; // 2024-08-01T00:00:00Z in microseconds
const AUG_2024: i32 = (2024 - 1970) * 12 + 7;

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
) -> iceberg::spec::TableMetadata {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(store.append(Append {
        files: vec![NewFile {
            path,
            rows,
            bytes,
            month: Some(AUG_2024),
        }],
        snapshot_id: id,
        at_ms: 1_000 + id,
        properties: BTreeMap::from([("utilake.first-file".to_owned(), file.to_owned())]),
    }))
    .unwrap()
}

#[test]
fn schemas_come_back_and_every_file_reads_through_the_last() {
    let tmp = tempfile::tempdir().unwrap();
    // UTILAKE_KEEP_TABLE=<empty dir> writes the table there instead, for other
    // readers (pyiceberg, DuckDB): Iceberg locations are absolute.
    let base = std::env::var("UTILAKE_KEEP_TABLE")
        .map_or_else(|_| tmp.path().to_path_buf(), PathBuf::from);
    let dir = base.join("dispatchprice");
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let store = Store::at(&dir);
    let time = a().fields[0].clone();
    store
        .create(
            "9c2f4f4e-1d1a-4b7a-9a37-0d6c1e1a2b3c",
            &a(),
            &PartitionSpec::by_month(Some(&time)),
            1_000,
        )
        .unwrap();
    assert_eq!(
        store.set_schema(&a(), 1_000).unwrap(),
        0,
        "the current schema again is no change"
    );

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

    let mut schemas: Vec<i32> = meta.schemas_iter().map(|s| s.schema_id()).collect();
    schemas.sort_unstable();
    assert_eq!(schemas, [0, 1, 2]);
    assert_eq!(meta.current_schema_id(), 1);
    assert_eq!(meta.last_column_id(), 4);
    assert_eq!(meta.next_row_id(), 6);
    let mut snaps: Vec<_> = meta.snapshots().collect();
    snaps.sort_by_key(|s| s.sequence_number());
    assert_eq!(
        snaps.iter().map(|s| s.schema_id()).collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2), Some(1)]
    );
    assert_eq!(
        snaps[3]
            .summary()
            .additional_properties
            .get("utilake.first-file")
            .map(String::as_str),
        Some("D.zip")
    );
    assert_eq!(
        snaps[1].summary().additional_properties["added-records"],
        "2"
    );
    assert_eq!(
        store.current().unwrap().0,
        8,
        "created, then three schema changes and four appends"
    );

    // Read back the way any reader does: by field id, through the current schema.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let batches: Vec<_> = rt.block_on(async {
        let table = Table::builder()
            .metadata(meta.clone())
            .metadata_location(format!(
                "file://{}/metadata/v8.metadata.json",
                dir.display()
            ))
            .file_io(FileIO::new_with_fs())
            .identifier(TableIdent::from_strs(["nem", "dispatchprice"]).unwrap())
            .runtime(Runtime::current())
            .build()
            .unwrap();
        let stream = table.scan().build().unwrap().to_arrow().await.unwrap();
        stream.try_collect().await.unwrap()
    });
    let rows = batches.iter().fold(0, |n, b| n + b.num_rows());
    assert_eq!(rows, 6);
    let names: Vec<String> = batches[0]
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["settlementdate", "regionid", "raise1secrrp", "rrp"]);
    let nulls: usize = batches
        .iter()
        .map(|b| b.column_by_name("raise1secrrp").unwrap().null_count())
        .sum();
    assert_eq!(nulls, 3, "the A and A' rows have no raise column");
    for b in &batches {
        let ty = format!(
            "{:?}",
            b.schema().field_with_name("rrp").unwrap().data_type()
        );
        assert_eq!(
            ty, "Decimal128(18, 5)",
            "the old decimal(15, 5) reads widened"
        );
    }
}

fn with_month_file(tmp: &tempfile::TempDir) -> NewFile {
    NewFile {
        path: tmp.path().join("x.parquet"),
        rows: 1,
        bytes: 1,
        month: Some(AUG_2024),
    }
}

#[test]
fn refusals_name_their_cause() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(&tmp.path().join("t"));
    let err = store.current().unwrap_err();
    assert!(matches!(err, Error::Io(_)), "no table yet");
    assert_eq!(err.code(), "store.iceberg_io");

    let unpartitioned = PartitionSpec::by_month(None);
    store
        .create(
            "9c2f4f4e-1d1a-4b7a-9a37-0d6c1e1a2b3c",
            &a(),
            &unpartitioned,
            1,
        )
        .unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let with_month = Append {
        files: vec![with_month_file(&tmp)],
        snapshot_id: 1,
        at_ms: 1,
        properties: BTreeMap::new(),
    };
    let err = rt.block_on(store.append(with_month)).unwrap_err();
    assert!(matches!(err, Error::Partition { .. }));
    assert_eq!(err.code(), "store.iceberg_partition");
    assert!(err.diagnostic().to_string().contains("x.parquet"));
    assert!(err.to_string().contains("x.parquet"), "{err}");
    let without = Append {
        files: vec![NewFile {
            month: None,
            ..with_month_file(&tmp)
        }],
        snapshot_id: 2,
        at_ms: 2,
        properties: BTreeMap::new(),
    };
    let meta = rt.block_on(store.append(without)).unwrap();
    assert_eq!(meta.current_snapshot().map(|s| s.snapshot_id()), Some(2));

    let bad = Store::at(&tmp.path().join("bad"));
    let err = bad
        .create("not a uuid", &a(), &unpartitioned, 1)
        .unwrap_err();
    assert!(matches!(err, Error::Json(_)), "{err}");

    // A version that cannot be written leaves the table as it was.
    std::fs::create_dir(tmp.path().join("t/metadata/v3.metadata.json")).unwrap();
    let err = store.set_schema(&b(), 3).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err}");
    assert_eq!(store.current().unwrap().0, 2);

    // A column named like the partition field is refused by the library.
    let time = a().fields[0].clone();
    let p = Store::at(&tmp.path().join("p"));
    p.create(
        "9c2f4f4e-1d1a-4b7a-9a37-0d6c1e1a2b3d",
        &a(),
        &PartitionSpec::by_month(Some(&time)),
        1,
    )
    .unwrap();
    let mut clash = a();
    clash
        .fields
        .push(field(5, "settlementdate_month", IcebergType::Long, false));
    clash.last_column_id = 5;
    let err = p.set_schema(&clash, 2).unwrap_err();
    assert!(matches!(err, Error::Iceberg(_)), "{err}");
    assert_eq!(err.code(), "store.iceberg_refused");
    assert!(err.to_string().starts_with("Iceberg refused it"), "{err}");

    let meta = tmp.path().join("t/metadata");
    std::fs::write(meta.join("version-hint.text"), "two").unwrap();
    let err = store.current().unwrap_err();
    assert!(err.to_string().contains("not a version"), "{err}");
    std::fs::write(meta.join("version-hint.text"), "1").unwrap();
    std::fs::write(meta.join("v1.metadata.json"), "{").unwrap();
    let err = store.current().unwrap_err();
    assert!(matches!(err, Error::Json(_)), "{err}");
    assert!(err.to_string().contains("not valid JSON"), "{err}");
    assert_eq!(err.code(), "store.iceberg_metadata");
}
