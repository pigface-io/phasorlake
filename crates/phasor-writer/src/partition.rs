//! Splitting a batch by the market-local month of its time column.

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, TimestampMicrosecondType, UInt32Type};
use arrow_array::{Array, PrimitiveArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use phasor_core::calendar::local_year_month;

use crate::error::StoreError;
use crate::writer::Partition;

/// Whether a type is the one timestamp the lake holds: microseconds with a
/// zone (Iceberg's `timestamptz`). Anything that is not a timestamp passes.
fn is_lake_timestamp_or_other(ty: &DataType) -> bool {
    !matches!(ty, DataType::Timestamp(unit, tz) if *unit != TimeUnit::Microsecond || tz.is_none())
}

/// The first field of `schema`, at any depth, whose type is a timestamp
/// without a zone or in a unit other than microseconds: what the lake never
/// writes.
pub fn naive_timestamp(schema: &Schema) -> Option<&Field> {
    fn find(field: &Field) -> Option<&Field> {
        if !is_lake_timestamp_or_other(field.data_type()) {
            return Some(field);
        }
        match field.data_type() {
            DataType::List(f)
            | DataType::LargeList(f)
            | DataType::FixedSizeList(f, _)
            | DataType::Map(f, _) => find(f),
            DataType::Struct(fields) => fields.iter().find_map(|f| find(f)),
            DataType::Dictionary(_, v) if !is_lake_timestamp_or_other(v) => Some(field),
            _ => None,
        }
    }
    schema.fields().iter().find_map(|f| find(f))
}

/// Splits `batch` into one batch per month of `time_col`: UTC microseconds
/// in a column that carries the market offset, partitioned in market time,
/// or a date (a gas day, which has no clock). The common case, every row
/// in one month, returns the batch itself without copying. Null time
/// values go to [`Partition::Unknown`].
pub fn split_by_month(
    batch: &RecordBatch,
    time_col: usize,
    offset_secs: i32,
) -> Result<Vec<(Partition, RecordBatch)>, StoreError> {
    zone!(SPLIT);
    let col = batch.column(time_col);
    let month = |y: i32, m: u32| Partition::Month {
        year: y,
        month: m as u8,
    };
    let parts: Vec<Partition> = match col.data_type() {
        DataType::Timestamp(TimeUnit::Microsecond, Some(_)) => {
            let ts: &PrimitiveArray<TimestampMicrosecondType> = col.as_primitive();
            ts.iter()
                .map(|v| {
                    v.map_or(Partition::Unknown, |v| {
                        let (y, m) = local_year_month(v.saturating_mul(1_000), offset_secs);
                        month(y, m)
                    })
                })
                .collect()
        }
        DataType::Date32 => {
            let days: &PrimitiveArray<Date32Type> = col.as_primitive();
            days.iter()
                .map(|v| {
                    v.map_or(Partition::Unknown, |d| {
                        let (y, m) =
                            local_year_month(i64::from(d).saturating_mul(86_400_000_000_000), 0);
                        month(y, m)
                    })
                })
                .collect()
        }
        other => {
            return Err(StoreError::PartitionColumnNotTimestamp {
                column: batch.schema().field(time_col).name().clone(),
                observed: other.to_string(),
            });
        }
    };
    if batch.num_rows() == 0 {
        return Ok(vec![]);
    }
    let part_of = |i: usize| -> Partition { parts[i] };
    let first = part_of(0);
    let mut all_same = true;
    for i in 1..batch.num_rows() {
        if part_of(i) != first {
            all_same = false;
            break;
        }
    }
    if all_same {
        return Ok(vec![(first, batch.clone())]);
    }
    // A file spanning months, in time order: each month is one run of
    // rows, and a slice of the batch, without copying.
    let mut runs: Vec<(Partition, usize, usize)> = Vec::new();
    let mut in_order = true;
    for (i, &p) in parts.iter().enumerate() {
        if let Some((rp, _, len)) = runs.last_mut()
            && *rp == p
        {
            *len += 1;
        } else if runs.iter().any(|(q, ..)| *q == p) {
            in_order = false;
            break;
        } else {
            runs.push((p, i, 1));
        }
    }
    if in_order {
        return Ok(runs
            .into_iter()
            .map(|(p, start, len)| (p, batch.slice(start, len)))
            .collect());
    }
    // Rarer: months interleaved. Group row indexes per partition.
    let mut groups: Vec<(Partition, Vec<u32>)> = Vec::new();
    for i in 0..batch.num_rows() {
        let p = part_of(i);
        match groups.iter_mut().find(|(gp, _)| *gp == p) {
            Some((_, idx)) => idx.push(i as u32),
            None => groups.push((p, vec![i as u32])),
        }
    }
    let mut out = Vec::with_capacity(groups.len());
    for (p, idx) in groups {
        let indices = PrimitiveArray::<UInt32Type>::from(idx);
        let taken = arrow_select::take::take_record_batch(batch, &indices).map_err(|e| {
            StoreError::Parquet {
                path: String::new(),
                message: format!("splitting a batch by month failed: {e}"),
            }
        })?;
        out.push((p, taken));
    }
    Ok(out)
}

/// Minimum and maximum of a timestamp column, ignoring nulls, in
/// nanoseconds as the manifest records them.
pub fn time_bounds(batch: &RecordBatch, time_col: usize) -> (Option<i64>, Option<i64>) {
    let col = batch.column(time_col);
    let Some(ts) = col.as_primitive_opt::<TimestampMicrosecondType>() else {
        return (None, None);
    };
    let mut min = None;
    let mut max = None;
    for v in ts.iter().flatten().map(|v| v.saturating_mul(1_000)) {
        min = Some(min.map_or(v, |m: i64| m.min(v)));
        max = Some(max.map_or(v, |m: i64| m.max(v)));
    }
    (min, max)
}

/// Whether a timestamp column is non-decreasing, so Parquet can declare it sorted.
pub fn is_sorted(batch: &RecordBatch, time_col: usize) -> bool {
    let Some(ts) = batch
        .column(time_col)
        .as_primitive_opt::<TimestampMicrosecondType>()
    else {
        return false;
    };
    let mut prev: Option<i64> = None;
    for v in ts {
        let Some(v) = v else { return false };
        if prev.is_some_and(|p| v < p) {
            return false;
        }
        prev = Some(v);
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Int64Array, TimestampMicrosecondArray};
    use arrow_schema::{Field, Schema, TimeUnit};
    use phasor_core::calendar::{NEM_OFFSET_SECS, utc_nanos};

    /// `yyyy/mm/dd hh:mm:ss` in NEM time as UTC nanoseconds.
    fn nanos(text: &str) -> i64 {
        let n = |s: &str| s.parse::<u32>().unwrap();
        let (date, clock) = text.split_once(' ').unwrap();
        let [y, m, d] = date.split('/').map(n).collect::<Vec<_>>()[..] else {
            panic!("{text}")
        };
        let [hh, mm, ss] = clock.split(':').map(n).collect::<Vec<_>>()[..] else {
            panic!("{text}")
        };
        utc_nanos(y as i32, m, d, hh, mm, ss, NEM_OFFSET_SECS)
    }

    use super::*;

    fn batch(times: &[Option<&str>]) -> RecordBatch {
        let vals: Vec<Option<i64>> = times.iter().map(|t| t.map(|t| nanos(t) / 1_000)).collect();
        let ts = TimestampMicrosecondArray::from(vals).with_timezone("+10:00");
        let ids = Int64Array::from((0..times.len() as i64).collect::<Vec<_>>());
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "SETTLEMENTDATE",
                DataType::Timestamp(TimeUnit::Microsecond, Some("+10:00".into())),
                true,
            ),
            Field::new("ID", DataType::Int64, false),
        ]));
        RecordBatch::try_new(schema, vec![Arc::new(ts), Arc::new(ids)]).unwrap()
    }

    #[test]
    fn single_month_is_passed_through() {
        let b = batch(&[
            Some("2024/01/01 00:05:00"),
            Some("2024/01/31 23:55:00"),
            Some("2024/02/01 00:00:00"),
        ]);
        // The last interval of January is labelled 2024/02/01 00:00:00: it is February in market time.
        let parts = split_by_month(&b, 0, NEM_OFFSET_SECS).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[0].0,
            Partition::Month {
                year: 2024,
                month: 1
            }
        );
        assert_eq!(parts[0].1.num_rows(), 2);
        assert_eq!(
            parts[1].0,
            Partition::Month {
                year: 2024,
                month: 2
            }
        );
        assert_eq!(parts[1].1.num_rows(), 1);
        // Months in time order are slices of the batch, not copies.
        let ids = |b: &RecordBatch| b.column(1).to_data().buffers()[0].as_ptr();
        assert_eq!(ids(&parts[0].1), ids(&b));
        let one = batch(&[Some("2024/01/01 00:05:00"), Some("2024/01/02 00:05:00")]);
        let parts = split_by_month(&one, 0, NEM_OFFSET_SECS).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].1.num_rows(), 2);
        // Market time matters: 2024/01/01 05:00 AEST is 2023-12-31 in UTC.
        let edge = batch(&[Some("2024/01/01 05:00:00")]);
        assert_eq!(
            split_by_month(&edge, 0, NEM_OFFSET_SECS).unwrap()[0].0,
            Partition::Month {
                year: 2024,
                month: 1
            }
        );
        assert_eq!(
            split_by_month(&edge, 0, 0).unwrap()[0].0,
            Partition::Month {
                year: 2023,
                month: 12
            }
        );
    }

    #[test]
    fn a_date_column_splits_by_its_month_with_no_clock() {
        use arrow_array::Date32Array;
        // 2026-09-30 and 2026-10-01 as days since the epoch, and a null.
        let days = Date32Array::from(vec![Some(20_726), Some(20_727), None]);
        let schema = Arc::new(Schema::new(vec![Field::new(
            "GASDATE",
            DataType::Date32,
            true,
        )]));
        let b = RecordBatch::try_new(schema, vec![Arc::new(days)]).unwrap();
        let parts = split_by_month(&b, 0, NEM_OFFSET_SECS).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0].0,
            Partition::Month {
                year: 2026,
                month: 9
            }
        );
        assert_eq!(
            parts[1].0,
            Partition::Month {
                year: 2026,
                month: 10
            }
        );
        assert_eq!(parts[2].0, Partition::Unknown);
        let empty = b.slice(0, 0);
        assert!(
            split_by_month(&empty, 0, NEM_OFFSET_SECS)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_market_two_hours_behind_splits_the_boundary_in_its_own_clock() {
        // 2024/02/01 01:00 in WEM time is 2024/02/01 03:00 in NEM time, and
        // 2024/01/31 23:30 WEM is 2024/02/01 01:30 NEM: the same instants
        // fall in different months by market.
        let b = batch(&[Some("2024/02/01 01:30:00"), Some("2024/02/01 03:00:00")]);
        let nem = split_by_month(&b, 0, NEM_OFFSET_SECS).unwrap();
        assert_eq!(nem.len(), 1);
        assert_eq!(
            nem[0].0,
            Partition::Month {
                year: 2024,
                month: 2
            }
        );
        let wem = split_by_month(&b, 0, phasor_core::calendar::WEM_OFFSET_SECS).unwrap();
        assert_eq!(wem.len(), 2);
        assert_eq!(
            wem[0].0,
            Partition::Month {
                year: 2024,
                month: 1
            }
        );
        assert_eq!(wem[0].1.num_rows(), 1);
        assert_eq!(
            wem[1].0,
            Partition::Month {
                year: 2024,
                month: 2
            }
        );
    }

    #[test]
    fn nulls_go_to_unknown_and_bounds_and_sortedness() {
        let b = batch(&[
            Some("2024/01/02 00:00:00"),
            None,
            Some("2024/01/01 00:00:00"),
        ]);
        let parts = split_by_month(&b, 0, NEM_OFFSET_SECS).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(
            parts
                .iter()
                .any(|(p, b)| *p == Partition::Unknown && b.num_rows() == 1)
        );
        let (min, max) = time_bounds(&b, 0);
        assert_eq!(min, Some(utc_nanos(2024, 1, 1, 0, 0, 0, NEM_OFFSET_SECS)));
        assert_eq!(max, Some(utc_nanos(2024, 1, 2, 0, 0, 0, NEM_OFFSET_SECS)));
        assert!(!is_sorted(&b, 0));
        let sorted = batch(&[
            Some("2024/01/01 00:00:00"),
            Some("2024/01/01 00:00:00"),
            Some("2024/01/02 00:00:00"),
        ]);
        assert!(is_sorted(&sorted, 0));
        assert!(!is_sorted(&b, 1), "not a timestamp column");
        assert_eq!(time_bounds(&b, 1), (None, None));
        let empty = batch(&[]);
        assert!(
            split_by_month(&empty, 0, NEM_OFFSET_SECS)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            split_by_month(&b, 1, NEM_OFFSET_SECS),
            Err(StoreError::PartitionColumnNotTimestamp { .. })
        ));
    }

    #[test]
    fn a_timestamp_without_a_zone_is_refused() {
        let naive_ty = DataType::Timestamp(TimeUnit::Microsecond, None);
        let schema = Arc::new(Schema::new(vec![Field::new("T", naive_ty.clone(), true)]));
        let ts = TimestampMicrosecondArray::from(vec![Some(0i64)]);
        let b = RecordBatch::try_new(schema, vec![Arc::new(ts)]).unwrap();
        assert!(matches!(
            split_by_month(&b, 0, NEM_OFFSET_SECS),
            Err(StoreError::PartitionColumnNotTimestamp { .. })
        ));
        // Found at the top, under a nullable struct, in a list, in a
        // dictionary, and in another unit.
        let zoned = DataType::Timestamp(TimeUnit::Microsecond, Some("+10:00".into()));
        let ok = Schema::new(vec![
            Field::new("T", zoned.clone(), false),
            Field::new(
                "S",
                DataType::Struct(vec![Field::new("inner", zoned.clone(), true)].into()),
                true,
            ),
            Field::new("D", DataType::Date32, true),
            Field::new("N", DataType::Float64, true),
        ]);
        assert!(naive_timestamp(&ok).is_none());
        // Each offender, and the name of the field reported: the nested
        // one when the type is nested.
        let offenders = [
            ("top", Field::new("top", naive_ty.clone(), true)),
            (
                "inner",
                Field::new(
                    "s",
                    DataType::Struct(vec![Field::new("inner", naive_ty.clone(), true)].into()),
                    true,
                ),
            ),
            (
                "item",
                Field::new(
                    "l",
                    DataType::List(Arc::new(Field::new("item", naive_ty.clone(), true))),
                    true,
                ),
            ),
            (
                "dict",
                Field::new(
                    "dict",
                    DataType::Dictionary(Box::new(DataType::Int32), Box::new(naive_ty)),
                    true,
                ),
            ),
            (
                "nanos",
                Field::new(
                    "nanos",
                    DataType::Timestamp(TimeUnit::Nanosecond, Some("+10:00".into())),
                    true,
                ),
            ),
        ];
        for (reported, f) in offenders {
            let s = Schema::new(vec![Field::new("ok", zoned.clone(), false), f]);
            assert_eq!(
                naive_timestamp(&s).map(|f| f.name().as_str()),
                Some(reported)
            );
        }
    }
}
