//! Reading one file of the lake as rows of the target schema: each
//! column read, cast or null as the plan says, the source and the time
//! each row was known appended, and the rows the window and the entity
//! filters exclude dropped before anything else sees them.

use std::fs::File;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, TimestampMicrosecondType};
use arrow_array::{
    Array, ArrayRef, BooleanArray, RecordBatch, Scalar, StringArray, TimestampMicrosecondArray,
    UInt32Array, new_null_array,
};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef, TimeUnit};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use utilake_core::{Category, Diagnostic};
use utilake_iceberg::Column;

use crate::error::QueryError;
use crate::output::Report;
use crate::query::{Bound, BoundFilter, FieldIx, FileRead, Resolution};
use crate::time::{KnownTime, MarketOffset};
use crate::{Lake, LakeFile, window};

/// The column of a scanned row that says which source it came from: its
/// position in the lake's files. Replaced by `_nl_source` on output.
pub const SOURCE_IX: &str = "_nl_src";
/// The column of a scanned row that says when it was known: the cutoff
/// column's value, else its source's publish time.
pub const KNOWN: &str = "_nl_known";

/// Rows per batch read from a file.
const BATCH_ROWS: usize = 65_536;

/// The schema scanned rows have: the target's columns, then the source
/// index and the known time.
pub fn scan_schema(target: &SchemaRef, tz: &str) -> SchemaRef {
    let mut fields: Vec<Field> = target.fields().iter().map(|f| f.as_ref().clone()).collect();
    fields.push(Field::new(SOURCE_IX, DataType::UInt32, false));
    fields.push(Field::new(
        KNOWN,
        DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from(tz))),
        true,
    ));
    Arc::new(Schema::new(fields))
}

/// Text as one value of a type, through Arrow's cast of a string.
pub fn scalar(text: &str, ty: &DataType) -> Result<ArrayRef, ArrowError> {
    let s: ArrayRef = Arc::new(StringArray::from(vec![text]));
    let v = arrow_cast::cast_with_options(
        &s,
        ty,
        &arrow_cast::CastOptions {
            safe: false,
            ..Default::default()
        },
    )?;
    if v.is_null(0) {
        return Err(ArrowError::CastError(format!("{text:?} is not a {ty}")));
    }
    Ok(v)
}

/// Whether text reads as a value of the type.
pub fn reads_as(text: &str, ty: &DataType) -> bool {
    scalar(text, ty).is_ok()
}

/// A time column's values as microseconds since 1970: a timestamp as it
/// is, a date as its local midnight.
pub fn micros_of(col: &ArrayRef, off: MarketOffset) -> Option<Vec<Option<i64>>> {
    match col.data_type() {
        DataType::Timestamp(TimeUnit::Microsecond, _) => Some(
            col.as_primitive::<TimestampMicrosecondType>()
                .iter()
                .collect(),
        ),
        DataType::Date32 => Some(
            col.as_primitive::<Date32Type>()
                .iter()
                .map(|d| d.map(|d| off.midnight_us(d)))
                .collect(),
        ),
        _ => None,
    }
}

/// One file's rows of the target schema, in the window, through the
/// entity filters. `known_at` set and the file has neither a cutoff
/// column nor a publish time: nothing of it can be shown known by then,
/// so it is left out with a warning.
pub fn scan(
    b: &Bound,
    lake: &Lake<'_>,
    read: &FileRead,
    source_ix: u32,
    known_at: Option<KnownTime>,
    report: &mut Report,
) -> Result<Vec<RecordBatch>, QueryError> {
    let file: &LakeFile = &lake.files[read.file];
    let cutoff = b.spec.as_ref().and_then(|s| s.cutoff);
    if known_at.is_some() && cutoff.is_none() && file.published.is_none() {
        report.warnings.push(
            Diagnostic::warning(
                "query.stamp_unknown",
                Category::Query,
                format!(
                    "{} has no publish time, so none of its rows can be shown to have been known by the time asked for; they are left out.",
                    file.source()
                ),
            )
            .with_table(b.table.clone())
            .with_url(file.url.to_string())
            .with_entry(file.entry.to_string()),
        );
        return Ok(Vec::new());
    }
    let tz = b.offset.tz();
    let out_schema = scan_schema(&b.schema, &tz);
    let path = file.path.as_str();
    let parquet_err = |e| QueryError::parquet(path, e);
    let arrow_err = |e| QueryError::arrow(path, e);
    let f = File::open(path)
        .map_err(|e| parquet_err(parquet::errors::ParquetError::External(Box::new(e))))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(f).map_err(parquet_err)?;
    let file_schema = builder.schema().clone();
    let wanted: Vec<&str> = read
        .columns
        .iter()
        .filter_map(|c| match c {
            Column::Read { source, .. } if file_schema.index_of(source).is_ok() => {
                Some(source.as_str())
            }
            _ => None,
        })
        .collect();
    let mask = ProjectionMask::columns(builder.parquet_schema(), wanted.iter().copied());
    let axis = b.spec.as_ref().map(|s| &s.valid);
    let valid_ix = match (axis, b.time) {
        (Some(a), _) => a.column().copied(),
        (None, t) => t,
    };
    let trim_now = window::before_resolution(axis) || b.query.resolution == Resolution::Raw;
    // Row groups whose valid column lies outside the window, by their
    // statistics, are not read at all.
    let groups = if trim_now && let Some(ix) = valid_ix {
        row_groups_in(
            &builder,
            &read.columns[ix.0],
            window::plan_bounds(axis, &b.query.window, b.day_start_us),
            b.offset,
        )
    } else {
        None
    };
    let mut builder = builder.with_projection(mask).with_batch_size(BATCH_ROWS);
    if let Some(g) = groups {
        builder = builder.with_row_groups(g);
    }
    let reader = builder.build().map_err(parquet_err)?;
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch.map_err(arrow_err)?;
        let n = batch.num_rows();
        report.rows_read += n as u64;
        if n == 0 {
            continue;
        }
        let mut columns: Vec<ArrayRef> = Vec::with_capacity(out_schema.fields().len());
        for (i, c) in read.columns.iter().enumerate() {
            let target_ty = out_schema.field(i).data_type();
            let col = match c {
                Column::Read { source, .. } => match batch.schema().index_of(source) {
                    Ok(j) => {
                        let a = batch.column(j);
                        if a.data_type() == target_ty {
                            a.clone()
                        } else {
                            arrow_cast::cast(a, target_ty).map_err(arrow_err)?
                        }
                    }
                    Err(_) => new_null_array(target_ty, n),
                },
                Column::Null { .. } => new_null_array(target_ty, n),
            };
            columns.push(col);
        }
        columns.push(Arc::new(UInt32Array::from(vec![source_ix; n])));
        // When each row was known: its cutoff value, else (a null cutoff
        // or none at all) when its source was published.
        let published = file.published.map(KnownTime::get);
        let known: Vec<Option<i64>> = match cutoff {
            Some(FieldIx(ix)) => micros_of(&columns[ix], b.offset)
                .unwrap_or_else(|| vec![None; n])
                .into_iter()
                .map(|v| v.or(published))
                .collect(),
            None => vec![published; n],
        };
        let known: ArrayRef =
            Arc::new(TimestampMicrosecondArray::from(known).with_timezone(tz.as_str()));
        columns.push(known);
        let mut batch = RecordBatch::try_new(out_schema.clone(), columns).map_err(arrow_err)?;
        if trim_now && let Some(ix) = valid_ix {
            let us = micros_of(batch.column(ix.0), b.offset).unwrap_or_else(|| vec![None; n]);
            let mask = BooleanArray::from_iter(us.iter().map(|v| {
                Some(window::keeps(
                    axis,
                    &b.query.window,
                    *v,
                    None,
                    b.day_start_us,
                ))
            }));
            let kept =
                arrow_select::filter::filter_record_batch(&batch, &mask).map_err(arrow_err)?;
            report.dropped_window += (batch.num_rows() - kept.num_rows()) as u64;
            batch = kept;
        }
        if !b.entity_filters.is_empty() {
            batch = apply_filters(&batch, &b.entity_filters).map_err(arrow_err)?;
        }
        if batch.num_rows() > 0 {
            out.push(batch);
        }
    }
    Ok(out)
}

/// The row groups whose valid column may hold a value in `[t0, t1)`, by
/// the column's statistics; none when every group may (the column is not
/// in the file, or a group has no statistics).
fn row_groups_in(
    builder: &ParquetRecordBatchReaderBuilder<File>,
    column: &Column,
    (t0, t1): (i64, i64),
    off: MarketOffset,
) -> Option<Vec<usize>> {
    use parquet::file::statistics::Statistics;
    let Column::Read { source, .. } = column else {
        return None;
    };
    let col = builder
        .parquet_schema()
        .columns()
        .iter()
        .position(|c| c.name() == source)?;
    let mut keep = Vec::new();
    for (i, rg) in builder.metadata().row_groups().iter().enumerate() {
        let (min, max) = match rg.column(col).statistics() {
            Some(Statistics::Int64(s)) => match (s.min_opt(), s.max_opt()) {
                (Some(a), Some(z)) => (*a, *z),
                _ => return None,
            },
            Some(Statistics::Int32(s)) => match (s.min_opt(), s.max_opt()) {
                (Some(a), Some(z)) => (off.midnight_us(*a), off.midnight_us(*z)),
                _ => return None,
            },
            _ => return None,
        };
        if max >= t0 && min < t1 {
            keep.push(i);
        }
    }
    Some(keep)
}

/// The rows whose columns are among the filters' values.
pub fn apply_filters(
    batch: &RecordBatch,
    filters: &[BoundFilter],
) -> Result<RecordBatch, ArrowError> {
    let mut batch = batch.clone();
    for f in filters {
        let col = batch.column(f.column.0).clone();
        let mut mask: Vec<bool> = vec![false; batch.num_rows()];
        for v in &f.values {
            let s = scalar(v, col.data_type())?;
            let eq = arrow_ord::cmp::eq(&col, &Scalar::new(s))?;
            for (i, hit) in eq.iter().enumerate() {
                if hit == Some(true) {
                    mask[i] = true;
                }
            }
        }
        batch = arrow_select::filter::filter_record_batch(&batch, &BooleanArray::from(mask))?;
    }
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_reads_as_a_type() {
        assert!(reads_as("1", &DataType::Int64));
        assert!(reads_as("1.5", &DataType::Float64));
        assert!(!reads_as("x", &DataType::Float64));
        assert!(reads_as("x", &DataType::Utf8));
        assert!(reads_as("2024-01-01", &DataType::Date32));
        assert!(!reads_as("", &DataType::Int64));
        let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("+10:00")));
        assert!(reads_as("2024-01-01T00:00:00", &ts));
        assert!(reads_as("1", &DataType::Decimal128(5, 2)));
        assert!(!reads_as("abc", &DataType::Decimal128(5, 2)));
    }

    #[test]
    fn micros_of_timestamps_and_dates() {
        let off = MarketOffset(36_000);
        let ts: ArrayRef =
            Arc::new(TimestampMicrosecondArray::from(vec![Some(5), None]).with_timezone("+10:00"));
        assert_eq!(micros_of(&ts, off), Some(vec![Some(5), None]));
        let d: ArrayRef = Arc::new(arrow_array::Date32Array::from(vec![Some(19_723), None]));
        assert_eq!(
            micros_of(&d, off),
            Some(vec![Some(1_704_031_200_000_000), None])
        );
        let s: ArrayRef = Arc::new(StringArray::from(vec!["x"]));
        assert_eq!(micros_of(&s, off), None);
    }

    #[test]
    fn filters_keep_any_of_the_values() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("A", DataType::Utf8, false),
            Field::new("B", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["x", "y", "z"])),
                Arc::new(arrow_array::Float64Array::from(vec![0.0, 1.0, 0.0])),
            ],
        )
        .unwrap();
        let kept = apply_filters(
            &batch,
            &[
                BoundFilter {
                    column: FieldIx(0),
                    values: vec!["x".into(), "z".into()],
                },
                BoundFilter {
                    column: FieldIx(1),
                    values: vec!["0".into()],
                },
            ],
        )
        .unwrap();
        assert_eq!(kept.num_rows(), 2);
        assert!(
            apply_filters(
                &batch,
                &[BoundFilter {
                    column: FieldIx(1),
                    values: vec!["no".into()],
                }]
            )
            .is_err()
        );
        let s = scan_schema(&batch.schema(), "+10:00");
        assert_eq!(s.fields().len(), 4);
        assert_eq!(s.field(2).name(), SOURCE_IX);
        assert_eq!(s.field(3).name(), KNOWN);
    }
}

#[cfg(test)]
mod file_tests {
    //! The paths of a scan a real table's files do not take: a cast the
    //! plan asks for, a column the file lacks, a date cutoff, a file with
    //! no publish time under `known_at`, a file that is not there.

    use std::sync::Arc;

    use arrow_array::{Date32Array, Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use utilake_iceberg::Column;
    use utilake_mms_registry::file_model::ValidAxis;

    use super::*;
    use crate::query::{Bound, BoundSpec, Query, Unit};
    use crate::time::{ValidTime, ValidWindow};

    const NEM: MarketOffset = MarketOffset(36_000);

    fn file(dir: &std::path::Path) -> String {
        let path = dir.join("f.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("A", DataType::Int64, true),
            Field::new("DAY", DataType::Date32, true),
            Field::new("S", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(2), None])),
                Arc::new(Date32Array::from(vec![Some(19_723), Some(19_724), None])),
                Arc::new(StringArray::from(vec!["x", "y", "z"])),
            ],
        )
        .unwrap();
        let mut w =
            parquet::arrow::ArrowWriter::try_new(File::create(&path).unwrap(), schema, None)
                .unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        path.display().to_string()
    }

    fn bound(cutoff: Option<FieldIx>, resolution: Resolution) -> Bound {
        let tz = "+10:00";
        let schema = Arc::new(Schema::new(vec![
            Field::new("A", DataType::Float64, true),
            Field::new("DAY", DataType::Date32, true),
            Field::new("MISSING", DataType::Utf8, true),
            Field::new("GONE", DataType::Utf8, true),
        ]));
        let _ = tz;
        Bound {
            query: Query::new(
                "T",
                ValidWindow::Range {
                    after: ValidTime::new(0),
                    upto: ValidTime::new(i64::MAX - 1),
                },
                resolution,
            ),
            table: "T".into(),
            schema_id: None,
            schema,
            spec: Some(BoundSpec {
                entity: vec![],
                valid: ValidAxis::Instant(FieldIx(1)),
                order: vec![],
                cutoff,
                unit: Unit::Table,
                collapse_all: false,
            }),
            time: None,
            files: vec![FileRead {
                file: 0,
                columns: vec![
                    Column::Read {
                        source: "A".into(),
                        name: "A".into(),
                        cast: Some(utilake_iceberg::IcebergType::Double),
                    },
                    Column::Read {
                        source: "DAY".into(),
                        name: "DAY".into(),
                        cast: None,
                    },
                    Column::Read {
                        source: "NOT_IN_FILE".into(),
                        name: "MISSING".into(),
                        cast: None,
                    },
                    Column::Null {
                        name: "GONE".into(),
                        ty: utilake_iceberg::IcebergType::String,
                    },
                ],
            }],
            entity_filters: vec![],
            value_filters: vec![],
            keep: None,
            offset: NEM,
            day_start_us: 0,
            warnings: vec![],
        }
    }

    fn lake<'a>(
        registry: &'a utilake_mms_registry::Registry,
        path: &str,
        published: Option<i64>,
    ) -> Lake<'a> {
        Lake {
            registry,
            files: vec![LakeFile {
                table: "T".into(),
                path: path.into(),
                schema_id: None,
                partition: String::new(),
                min_us: None,
                max_us: None,
                url: Arc::from("file:///f"),
                entry: Arc::from(""),
                source_period: None,
                published: published.map(KnownTime::new),
            }],
            version: "test",
        }
    }

    #[test]
    fn casts_nulls_and_a_date_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path());
        let registry = utilake_nem::registry();
        let lake = lake(&registry, &path, None);
        let b = bound(Some(FieldIx(1)), Resolution::Latest { known_at: None });
        let mut report = Report::default();
        let out = scan(&b, &lake, &b.files[0], 0, None, &mut report).unwrap();
        assert_eq!(out.len(), 1);
        let batch = &out[0];
        assert_eq!(batch.column(0).data_type(), &DataType::Float64);
        assert_eq!(batch.column(2).null_count(), batch.num_rows());
        assert_eq!(batch.column(3).null_count(), batch.num_rows());
        // The null day is outside every window; the others are kept.
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(report.dropped_window, 1);
        // The cutoff is a date: its midnight in the market's clock.
        let known = micros_of(batch.column(5), NEM).unwrap();
        assert_eq!(
            known,
            [Some(NEM.midnight_us(19_723)), Some(NEM.midnight_us(19_724))]
        );
        // A raw read trims too, and a zero-row batch is not returned.
        let b = bound(None, Resolution::Raw);
        let out = scan(&b, &lake, &b.files[0], 0, None, &mut report).unwrap();
        assert_eq!(out[0].num_rows(), 2);
        assert_eq!(micros_of(out[0].column(5), NEM).unwrap(), [None, None]);
    }

    #[test]
    fn no_stamp_under_known_at_and_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path());
        let registry = utilake_nem::registry();
        let lake = lake(&registry, &path, None);
        let b = bound(
            None,
            Resolution::Latest {
                known_at: Some(KnownTime::new(5)),
            },
        );
        let mut report = Report::default();
        let out = scan(
            &b,
            &lake,
            &b.files[0],
            0,
            Some(KnownTime::new(5)),
            &mut report,
        )
        .unwrap();
        assert!(out.is_empty());
        assert_eq!(report.warnings[0].code, "query.stamp_unknown");
        // With a stamp the file is read and every row knows it.
        let lake = self::lake(&registry, &path, Some(7));
        let out = scan(
            &b,
            &lake,
            &b.files[0],
            0,
            Some(KnownTime::new(5)),
            &mut report,
        )
        .unwrap();
        assert_eq!(
            micros_of(out[0].column(5), NEM).unwrap(),
            [Some(7), Some(7)]
        );
        let gone = self::lake(&registry, "/nowhere/f.parquet", Some(7));
        let err = scan(&b, &gone, &b.files[0], 0, None, &mut report).unwrap_err();
        assert!(matches!(err, QueryError::ReadParquet { .. }), "{err}");
        let not_parquet = dir.path().join("x.parquet");
        std::fs::write(&not_parquet, b"nope").unwrap();
        let bad = self::lake(&registry, not_parquet.to_str().unwrap(), Some(7));
        let err = scan(&b, &bad, &b.files[0], 0, None, &mut report).unwrap_err();
        assert!(matches!(err, QueryError::ReadParquet { .. }), "{err}");
        let s: ArrayRef = Arc::new(StringArray::from(vec!["x"]));
        assert!(scalar("x", &DataType::Int64).is_err());
        assert_eq!(
            scalar("x", &DataType::Utf8).unwrap().as_ref().to_data(),
            s.to_data()
        );
    }
}
