//! An interval of a table, read out of the lake's Parquet files into one
//! file: every file whose time span overlaps the interval, each projected
//! onto one schema of the table's history by field id (a promoted type
//! cast, a column the file lacks null), its rows trimmed to the interval
//! on the time column, sorted by it, and written with the lake's field ids
//! beside a sidecar that says which files it came from.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, new_null_array};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{WriterProperties, WriterVersion};
use utilake_iceberg::{Column, DataFile, Read, Target, plan};
use utilake_iceberg::{History, IcebergType};

use crate::error::StoreError;

/// Rows per batch written.
const BATCH_ROWS: usize = 65_536;

/// What to extract.
pub struct ExtractRequest<'a> {
    /// The table.
    pub table: &'a str,
    /// Its history: the schemas files were written under, and the target.
    pub history: &'a History,
    /// The lake's files of the table, with their time spans.
    pub files: Vec<DataFile>,
    /// `[from, to)` as microseconds since the epoch.
    pub interval: (i64, i64),
    /// The schema to project onto.
    pub target: Target,
    /// The time column, as named in the target; none for a table without
    /// one, whose rows are all kept.
    pub time_column: Option<&'a str>,
    /// The market's clock, as Arrow writes it: `+10:00`.
    pub tz: &'a str,
    /// Refuse an extraction whose files hold more rows than this.
    pub max_rows: Option<u64>,
    /// The rows the files hold, as the manifest counted them.
    pub rows_estimate: u64,
    /// The file to write; its sidecar goes beside it as `{out}.json`.
    pub out: &'a Path,
    /// utilake's version, written in the file.
    pub version: &'a str,
    /// Where the rows came from, written in the sidecar as given.
    pub sources: serde_json::Value,
}

/// What was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// The Parquet file.
    pub path: PathBuf,
    /// Its sidecar.
    pub sidecar: PathBuf,
    /// Rows written.
    pub rows: u64,
    /// Lake files read.
    pub files: usize,
    /// The schema the rows are in.
    pub schema_id: u32,
}

/// Arrow's type for an Iceberg type, timestamps in the market's clock.
pub fn arrow_type(t: IcebergType, tz: &str) -> DataType {
    match t {
        IcebergType::Boolean => DataType::Boolean,
        IcebergType::Long => DataType::Int64,
        IcebergType::Double => DataType::Float64,
        IcebergType::Decimal { precision, scale } => {
            DataType::Decimal128(precision, i8::try_from(scale).unwrap_or(i8::MAX))
        }
        IcebergType::Date => DataType::Date32,
        IcebergType::Timestamptz => DataType::Timestamp(TimeUnit::Microsecond, Some(tz.into())),
        IcebergType::String => DataType::Utf8,
    }
}

fn extract_error(table: &str, message: impl Into<String>) -> StoreError {
    StoreError::Extract {
        table: table.to_owned(),
        message: message.into(),
    }
}

/// The target schema as Arrow fields with their Iceberg ids.
fn target_schema(h: &History, target: Target, tz: &str) -> Result<(u32, SchemaRef), String> {
    let id = match target {
        Target::Latest => h.current_schema_id,
        Target::AsOf(id) => id,
    };
    let s = h
        .schema(id)
        .ok_or_else(|| format!("the table has no schema {id}"))?;
    let fields: Vec<Field> = s
        .fields
        .iter()
        .map(|f| {
            Field::new(&f.name, arrow_type(f.ty, tz), !f.required).with_metadata(
                std::collections::HashMap::from([(
                    "PARQUET:field_id".to_owned(),
                    f.id.to_string(),
                )]),
            )
        })
        .collect();
    Ok((id, Arc::new(Schema::new(fields))))
}

/// One file's batches projected onto the target.
fn read_file(
    table: &str,
    path: &str,
    read: &Read,
    schema: &SchemaRef,
    tz: &str,
) -> Result<Vec<RecordBatch>, StoreError> {
    let fail = |m: String| extract_error(table, format!("{path}: {m}"));
    let file = std::fs::File::open(path).map_err(|source| StoreError::Io {
        path: path.to_owned(),
        source,
    })?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| fail(e.to_string()))?;
    let file_schema = builder.schema().clone();
    let mut indices = Vec::new();
    for c in &read.columns {
        if let Column::Read { source, .. } = c {
            let i = file_schema
                .index_of(source)
                .map_err(|_| fail(format!("no column {source} in the file")))?;
            indices.push(i);
        }
    }
    let mask = ProjectionMask::roots(builder.parquet_schema(), indices);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(BATCH_ROWS)
        .build()
        .map_err(|e| fail(e.to_string()))?;
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| fail(e.to_string()))?;
        let rows = batch.num_rows();
        let columns: Vec<ArrayRef> = read
            .columns
            .iter()
            .zip(schema.fields())
            .map(|(c, field)| match c {
                Column::Read { source, .. } => {
                    let col = batch
                        .column_by_name(source)
                        .ok_or_else(|| fail(format!("no column {source} in the file")))?;
                    if col.data_type() == field.data_type() {
                        Ok(col.clone())
                    } else {
                        arrow::compute::cast(col, field.data_type()).map_err(|e| {
                            fail(format!(
                                "{source} is {} and cannot be read as {}: {e}",
                                col.data_type(),
                                field.data_type()
                            ))
                        })
                    }
                }
                Column::Null { ty, .. } => Ok(new_null_array(&arrow_type(*ty, tz), rows)),
            })
            .collect::<Result<_, StoreError>>()?;
        out.push(RecordBatch::try_new(schema.clone(), columns).map_err(|e| fail(e.to_string()))?);
    }
    Ok(out)
}

/// The rows of `batch` whose time lies in `[t0, t1)`.
fn trim(batch: &RecordBatch, time: usize, (t0, t1): (i64, i64)) -> Result<RecordBatch, String> {
    use arrow_array::Scalar;
    use arrow_array::TimestampMicrosecondArray;
    let col = batch.column(time);
    let tz = match col.data_type() {
        DataType::Timestamp(TimeUnit::Microsecond, tz) => tz.clone(),
        other => return Err(format!("the time column holds {other}, not timestamps")),
    };
    let bound = |us: i64| {
        Scalar::new(TimestampMicrosecondArray::from(vec![us]).with_timezone_opt(tz.clone()))
    };
    let from = arrow::compute::kernels::cmp::gt_eq(col, &bound(t0)).map_err(|e| e.to_string())?;
    let to = arrow::compute::kernels::cmp::lt(col, &bound(t1)).map_err(|e| e.to_string())?;
    let keep = arrow::compute::and(&from, &to).map_err(|e| e.to_string())?;
    arrow::compute::filter_record_batch(batch, &keep).map_err(|e| e.to_string())
}

/// Extracts the interval into one Parquet file and its sidecar.
pub fn extract(req: &ExtractRequest<'_>) -> Result<Extracted, StoreError> {
    let table = req.table;
    let fail = |m: String| extract_error(table, m);
    if let Some(max) = req.max_rows
        && req.rows_estimate > max
    {
        return Err(fail(format!(
            "its files hold {} rows, more than the {max} allowed; narrow the interval or raise --max-rows",
            req.rows_estimate
        )));
    }
    let (schema_id, schema) = target_schema(req.history, req.target, req.tz).map_err(fail)?;
    let reads =
        plan(req.history, &req.files, req.interval, req.target).map_err(|e| fail(e.to_string()))?;
    let time = req.time_column.and_then(|t| schema.index_of(t).ok());
    let mut batches: Vec<RecordBatch> = Vec::new();
    let mut files = 0;
    for read in &reads {
        for path in &read.paths {
            files += 1;
            for b in read_file(table, path, read, &schema, req.tz)? {
                let b = match time {
                    Some(i) => trim(&b, i, req.interval).map_err(fail)?,
                    None => b,
                };
                if b.num_rows() > 0 {
                    batches.push(b);
                }
            }
        }
    }
    let mut all =
        arrow::compute::concat_batches(&schema, &batches).map_err(|e| fail(e.to_string()))?;
    if let Some(i) = time
        && all.num_rows() > 1
    {
        let order = arrow::compute::sort_to_indices(all.column(i), None, None)
            .map_err(|e| fail(e.to_string()))?;
        all = arrow::compute::take_record_batch(&all, &order).map_err(|e| fail(e.to_string()))?;
    }
    let rows = all.num_rows() as u64;
    // Write beside the final name, then move into place.
    if let Some(dir) = req.out.parent() {
        std::fs::create_dir_all(dir).map_err(|source| StoreError::Io {
            path: dir.display().to_string(),
            source,
        })?;
    }
    let tmp = req.out.with_extension("parquet.tmp");
    let out_text = req.out.display().to_string();
    let parquet_err = |m: String| StoreError::Parquet {
        path: out_text.clone(),
        message: m,
    };
    let file = std::fs::File::create(&tmp).map_err(|source| StoreError::Io {
        path: tmp.display().to_string(),
        source,
    })?;
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(3).expect("3 is a zstd level"),
        ))
        .set_created_by(format!("utilake {}", req.version))
        .set_key_value_metadata(Some(vec![
            KeyValue::new("utilake.version".to_owned(), req.version.to_owned()),
            KeyValue::new("utilake.table".to_owned(), table.to_owned()),
            KeyValue::new("utilake.schema_id".to_owned(), schema_id.to_string()),
            KeyValue::new(
                "utilake.interval_from_us".to_owned(),
                req.interval.0.to_string(),
            ),
            KeyValue::new(
                "utilake.interval_to_us".to_owned(),
                req.interval.1.to_string(),
            ),
            KeyValue::new("utilake.extract".to_owned(), "1".to_owned()),
        ]))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .map_err(|e| parquet_err(e.to_string()))?;
    let mut at = 0;
    while at < all.num_rows() {
        let n = BATCH_ROWS.min(all.num_rows() - at);
        writer
            .write(&all.slice(at, n))
            .map_err(|e| parquet_err(e.to_string()))?;
        at += n;
    }
    writer.close().map_err(|e| parquet_err(e.to_string()))?;
    std::fs::rename(&tmp, req.out).map_err(|source| StoreError::Rename {
        from: tmp.display().to_string(),
        to: out_text.clone(),
        source,
    })?;
    let sidecar = PathBuf::from(format!("{out_text}.json"));
    let doc = serde_json::json!({
        "table": table,
        "schema_id": schema_id,
        "interval_us": {"from": req.interval.0, "to": req.interval.1},
        "rows": rows,
        "files": files,
        "utilake": req.version,
        "sources": req.sources,
    });
    std::fs::write(
        &sidecar,
        serde_json::to_string_pretty(&doc).expect("a value serialises"),
    )
    .map_err(|source| StoreError::Io {
        path: sidecar.display().to_string(),
        source,
    })?;
    Ok(Extracted {
        path: req.out.to_path_buf(),
        sidecar,
        rows,
        files,
        schema_id,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{
        Array as _, Decimal128Array, Int64Array, StringArray, TimestampMicrosecondArray,
    };
    use utilake_core::Diagnose as _;
    use utilake_iceberg::{Field as IField, Schema as ISchema};

    use super::*;

    const TZ: &str = "+10:00";

    fn field(id: u32, name: &str, ty: IcebergType, required: bool) -> IField {
        IField {
            id,
            name: name.into(),
            ty,
            required,
            doc: None,
        }
    }

    /// Two schemas: the second widens SCADAVALUE's precision and adds EXTRA.
    fn history() -> History {
        let dec = |p, s| IcebergType::Decimal {
            precision: p,
            scale: s,
        };
        History {
            table: "DISPATCH_UNIT_SCADA".into(),
            format_version: 3,
            current_schema_id: 1,
            schemas: vec![
                ISchema {
                    schema_id: 0,
                    fields: vec![
                        field(1, "SETTLEMENTDATE", IcebergType::Timestamptz, true),
                        field(2, "DUID", IcebergType::String, true),
                        field(3, "SCADAVALUE", dec(16, 6), false),
                    ],
                    identifier_field_ids: vec![1, 2],
                    last_column_id: 3,
                },
                ISchema {
                    schema_id: 1,
                    fields: vec![
                        field(1, "SETTLEMENTDATE", IcebergType::Timestamptz, true),
                        field(2, "DUID", IcebergType::String, true),
                        field(3, "SCADAVALUE", dec(18, 6), false),
                        field(4, "EXTRA", IcebergType::Long, false),
                    ],
                    identifier_field_ids: vec![1, 2],
                    last_column_id: 4,
                },
            ],
            runs: vec![],
            breaking: vec![],
            headers: Default::default(),
        }
    }

    /// A lake file of schema 0 or 1 with rows at `times` (micros).
    fn lake_file(dir: &Path, name: &str, schema: u32, times: &[i64]) -> DataFile {
        let (_, arrow_schema) = target_schema(&history(), Target::AsOf(schema), TZ).unwrap();
        let n = times.len();
        let mut cols: Vec<ArrayRef> = vec![
            Arc::new(TimestampMicrosecondArray::from(times.to_vec()).with_timezone(TZ)),
            Arc::new(StringArray::from(
                (0..n).map(|i| format!("U{i}")).collect::<Vec<_>>(),
            )),
        ];
        if schema == 0 {
            cols.push(Arc::new(
                Decimal128Array::from((0..n).map(|i| i as i128 * 1_000_000).collect::<Vec<_>>())
                    .with_precision_and_scale(16, 6)
                    .unwrap(),
            ));
        } else {
            cols.push(Arc::new(
                Decimal128Array::from((0..n).map(|i| i as i128 * 1_000_000).collect::<Vec<_>>())
                    .with_precision_and_scale(18, 6)
                    .unwrap(),
            ));
            cols.push(Arc::new(Int64Array::from(
                (0..n).map(|i| i as i64).collect::<Vec<_>>(),
            )));
        }
        let batch = RecordBatch::try_new(arrow_schema.clone(), cols).unwrap();
        let path = dir.join(name);
        let mut w = ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), arrow_schema, None)
            .unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        DataFile {
            path: path.display().to_string(),
            schema_id: schema,
            min_us: times.iter().min().copied(),
            max_us: times.iter().max().copied(),
        }
    }

    fn read_back(path: &Path) -> RecordBatch {
        let b =
            ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(path).unwrap()).unwrap();
        let schema = b.schema().clone();
        let batches: Vec<RecordBatch> = b.build().unwrap().map(Result::unwrap).collect();
        arrow::compute::concat_batches(&schema, &batches).unwrap()
    }

    #[test]
    fn an_interval_is_read_across_schemas_trimmed_and_sorted_with_its_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let h = history();
        let hour = 3_600_000_000i64;
        // An old-schema file with rows at 0h, 1h, 2h; a new-schema file at
        // 1h30 and 5h; and one entirely outside the interval.
        let old = lake_file(dir.path(), "old.parquet", 0, &[2 * hour, 0, hour]);
        let new = lake_file(dir.path(), "new.parquet", 1, &[5 * hour, hour + hour / 2]);
        let far = lake_file(dir.path(), "far.parquet", 1, &[20 * hour]);
        let out = dir.path().join("out").join("DISPATCH_UNIT_SCADA.parquet");
        let req = || ExtractRequest {
            table: "DISPATCH_UNIT_SCADA",
            history: &h,
            files: vec![old.clone(), new.clone(), far.clone()],
            interval: (hour, 3 * hour),
            target: Target::Latest,
            time_column: Some("SETTLEMENTDATE"),
            tz: TZ,
            max_rows: Some(100),
            rows_estimate: 6,
            out: &out,
            version: "test",
            sources: serde_json::json!(["old.parquet", "new.parquet"]),
        };
        let x = extract(&req()).unwrap();
        assert_eq!(
            (x.rows, x.files, x.schema_id),
            (3, 2, 1),
            "1h and 2h from old, 1h30 from new; far skipped"
        );
        assert_eq!(x.path, out);
        let got = read_back(&out);
        let schema = got.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "EXTRA"]);
        assert_eq!(got.schema().field(3).metadata()["PARQUET:field_id"], "4");
        let t = got
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(
            t.values().to_vec(),
            [hour, hour + hour / 2, 2 * hour],
            "sorted by time"
        );
        let v = got
            .column(2)
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .unwrap();
        assert_eq!(
            (v.precision(), v.scale()),
            (18, 6),
            "the old file's decimals widened"
        );
        assert_eq!(
            v.value(0),
            2 * 1_000_000,
            "2.000000 in (16,6) is 2.000000 in (18,6)"
        );
        let extra = got.column(3).as_any().downcast_ref::<Int64Array>().unwrap();
        assert!(
            extra.is_null(0) && extra.is_null(2),
            "the old file has no EXTRA"
        );
        assert_eq!(extra.value(1), 1);
        let side: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&x.sidecar).unwrap()).unwrap();
        assert_eq!(side["rows"], 3);
        assert_eq!(side["schema_id"], 1);
        assert_eq!(side["interval_us"]["to"], 3 * hour);
        assert_eq!(side["sources"][1], "new.parquet");
        // Projected onto the old schema: EXTRA is left out, the decimals narrow.
        let old_out = dir.path().join("old_out.parquet");
        let x = extract(&ExtractRequest {
            target: Target::AsOf(0),
            files: vec![old.clone()],
            out: &old_out,
            ..req()
        })
        .unwrap();
        assert_eq!(x.rows, 2);
        assert_eq!(read_back(&old_out).num_columns(), 3);
        // No time column: every row of the files, in file order.
        let all = dir.path().join("all.parquet");
        let x = extract(&ExtractRequest {
            time_column: None,
            files: vec![old.clone(), new.clone()],
            out: &all,
            ..req()
        })
        .unwrap();
        assert_eq!(x.rows, 5);
        // An interval no row falls in: an empty file with the schema.
        let empty = dir.path().join("empty.parquet");
        let x = extract(&ExtractRequest {
            interval: (10 * hour, 11 * hour),
            files: vec![old.clone()],
            out: &empty,
            ..req()
        })
        .unwrap();
        assert_eq!(x.rows, 0);
        assert_eq!(read_back(&empty).num_columns(), 4);
    }

    #[test]
    fn what_cannot_be_extracted_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let h = history();
        let old = lake_file(dir.path(), "old.parquet", 0, &[0]);
        let out = dir.path().join("x.parquet");
        let req = || ExtractRequest {
            table: "DISPATCH_UNIT_SCADA",
            history: &h,
            files: vec![old.clone()],
            interval: (0, 10),
            target: Target::Latest,
            time_column: Some("SETTLEMENTDATE"),
            tz: TZ,
            max_rows: Some(0),
            rows_estimate: 1,
            out: &out,
            version: "test",
            sources: serde_json::Value::Null,
        };
        let e = extract(&req()).unwrap_err();
        assert!(e.to_string().contains("more than the 0 allowed"), "{e}");
        assert_eq!(e.diagnostic().code, "store.extract");
        let e = extract(&ExtractRequest {
            max_rows: None,
            target: Target::AsOf(9),
            ..req()
        })
        .unwrap_err();
        assert!(e.to_string().contains("no schema 9"), "{e}");
        // A file recorded under a schema whose column it lacks.
        let wrong = DataFile {
            schema_id: 1,
            ..old.clone()
        };
        let e = extract(&ExtractRequest {
            max_rows: None,
            files: vec![wrong],
            ..req()
        })
        .unwrap_err();
        assert!(e.to_string().contains("no column EXTRA"), "{e}");
        // A file that is gone, and one that is not Parquet.
        let gone = DataFile {
            path: dir.path().join("gone.parquet").display().to_string(),
            ..old.clone()
        };
        assert!(matches!(
            extract(&ExtractRequest {
                max_rows: None,
                files: vec![gone],
                ..req()
            }),
            Err(StoreError::Io { .. })
        ));
        std::fs::write(dir.path().join("text.parquet"), b"not parquet").unwrap();
        let text = DataFile {
            path: dir.path().join("text.parquet").display().to_string(),
            ..old.clone()
        };
        let e = extract(&ExtractRequest {
            max_rows: None,
            files: vec![text],
            ..req()
        })
        .unwrap_err();
        assert!(matches!(e, StoreError::Extract { .. }), "{e}");
        // The time column named is not a timestamp.
        let e = extract(&ExtractRequest {
            max_rows: None,
            time_column: Some("DUID"),
            ..req()
        })
        .unwrap_err();
        assert!(e.to_string().contains("not timestamps"), "{e}");
        // The output cannot be written.
        std::fs::write(dir.path().join("blocked"), b"").unwrap();
        let e = extract(&ExtractRequest {
            max_rows: None,
            out: &dir.path().join("blocked").join("x.parquet"),
            ..req()
        })
        .unwrap_err();
        assert!(matches!(e, StoreError::Io { .. }), "{e}");
        // A file whose column cannot be read as the target's type.
        let bools = dir.path().join("bools.parquet");
        {
            let schema = Arc::new(Schema::new(vec![
                Field::new(
                    "SETTLEMENTDATE",
                    arrow_type(IcebergType::Timestamptz, TZ),
                    false,
                ),
                Field::new("DUID", DataType::Utf8, false),
                Field::new("SCADAVALUE", DataType::Boolean, true),
            ]));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(TimestampMicrosecondArray::from(vec![0]).with_timezone(TZ)),
                    Arc::new(StringArray::from(vec!["U"])),
                    Arc::new(arrow_array::BooleanArray::from(vec![true])),
                ],
            )
            .unwrap();
            let mut w =
                ArrowWriter::try_new(std::fs::File::create(&bools).unwrap(), schema, None).unwrap();
            w.write(&batch).unwrap();
            w.close().unwrap();
        }
        let e = extract(&ExtractRequest {
            max_rows: None,
            files: vec![DataFile {
                path: bools.display().to_string(),
                ..old.clone()
            }],
            ..req()
        })
        .unwrap_err();
        assert!(e.to_string().contains("cannot be read as"), "{e}");
        // The output's place is taken by a folder.
        let onto_dir = dir.path().join("taken.parquet");
        std::fs::create_dir_all(&onto_dir).unwrap();
        let e = extract(&ExtractRequest {
            max_rows: None,
            out: &onto_dir,
            ..req()
        })
        .unwrap_err();
        assert!(matches!(e, StoreError::Rename { .. }), "{e}");
        // The sidecar's place is taken.
        let side_taken = dir.path().join("side.parquet");
        std::fs::create_dir_all(dir.path().join("side.parquet.json")).unwrap();
        let e = extract(&ExtractRequest {
            max_rows: None,
            out: &side_taken,
            ..req()
        })
        .unwrap_err();
        assert!(matches!(e, StoreError::Io { .. }), "{e}");
        assert!(side_taken.is_file(), "the Parquet file was written first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let shut = dir.path().join("shut");
            std::fs::create_dir_all(&shut).unwrap();
            std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o500)).unwrap();
            let e = extract(&ExtractRequest {
                max_rows: None,
                out: &shut.join("x.parquet"),
                ..req()
            })
            .unwrap_err();
            std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(matches!(e, StoreError::Io { .. }), "{e}");
        }
        // Every Iceberg type has an Arrow type.
        assert_eq!(arrow_type(IcebergType::Boolean, TZ), DataType::Boolean);
        assert_eq!(arrow_type(IcebergType::Double, TZ), DataType::Float64);
        assert_eq!(arrow_type(IcebergType::Date, TZ), DataType::Date32);
    }
}
