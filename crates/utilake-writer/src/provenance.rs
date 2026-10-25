//! The two columns every lake file ends with (see
//! [`utilake_iceberg::provenance`]): each row's source, `url!entry`, and
//! when that source became public. They are built once per source and
//! filled per batch with no string copied: a dictionary of one value and
//! keys of zero, and one time repeated.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_array::types::UInt16Type;
use arrow_array::{
    Array, ArrayRef, DictionaryArray, StringArray, TimestampMicrosecondArray, UInt16Array,
};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef, TimeUnit};
use utilake_iceberg::provenance::{PUBLISHED, SOURCE};

/// The format of a lake file: 2 since every row names its source.
pub const LAKE_FORMAT: &str = "2";

/// The two fields, as a lake file's schema ends.
pub fn fields() -> [Field; 2] {
    let id = |id: u32| {
        HashMap::from([(
            parquet::arrow::PARQUET_FIELD_ID_META_KEY.to_owned(),
            id.to_string(),
        )])
    };
    [
        Field::new(
            SOURCE.name,
            DataType::Dictionary(Box::new(DataType::UInt16), Box::new(DataType::Utf8)),
            false,
        )
        .with_metadata(id(SOURCE.id)),
        Field::new(
            PUBLISHED.name,
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        )
        .with_metadata(id(PUBLISHED.id)),
    ]
}

/// `schema` as a lake file holds it: its columns, then the two.
pub fn lake_schema(schema: &Schema) -> SchemaRef {
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    fields.extend(self::fields());
    Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()))
}

/// `batch` with `stamp`'s two columns after its own, in `schema`, the
/// batch's [`lake_schema`].
pub fn stamped(
    batch: &RecordBatch,
    stamp: &Stamp,
    schema: &SchemaRef,
) -> Result<RecordBatch, ArrowError> {
    let mut columns = batch.columns().to_vec();
    columns.extend(stamp.columns(batch.num_rows()));
    RecordBatch::try_new(schema.clone(), columns)
}

/// One source's two columns, ready to be cut to any batch's length.
#[derive(Debug, Clone)]
pub struct Stamp {
    name: Arc<str>,
    values: ArrayRef,
    published: Option<i64>,
}

impl Stamp {
    /// The stamp of source `name` (`url!entry`), public at `published`
    /// (microseconds since 1970, UTC).
    pub fn new(name: &str, published: Option<i64>) -> Self {
        Self {
            name: Arc::from(name),
            values: Arc::new(StringArray::from(vec![name])),
            published,
        }
    }

    /// The source's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// When the source went public, microseconds since 1970 (UTC).
    pub const fn published(&self) -> Option<i64> {
        self.published
    }

    /// The columns for `rows` rows.
    pub fn columns(&self, rows: usize) -> [ArrayRef; 2] {
        let keys = UInt16Array::from_value(0, rows);
        let source = DictionaryArray::<UInt16Type>::try_new(keys, self.values.clone())
            .expect("a key of 0 into one value");
        let published = match self.published {
            Some(t) => TimestampMicrosecondArray::from_value(t, rows),
            None => TimestampMicrosecondArray::new_null(rows),
        }
        .with_timezone("UTC");
        [Arc::new(source), Arc::new(published)]
    }
}

/// The rows of each source in `column`, a batch's `_nl_source`, added to
/// `counts` in the order sources first appear.
pub(crate) fn count(column: &dyn Array, counts: &mut Vec<(String, u64)>) {
    let Some(dict) = column
        .as_any()
        .downcast_ref::<DictionaryArray<UInt16Type>>()
    else {
        return;
    };
    let Some(values) = dict.values().as_any().downcast_ref::<StringArray>() else {
        return;
    };
    let mut per_key = vec![0u64; values.len()];
    for k in dict.keys().values() {
        per_key[usize::from(*k)] += 1;
    }
    for (k, n) in per_key.into_iter().enumerate().filter(|(_, n)| *n > 0) {
        let name = values.value(k);
        match counts.iter_mut().find(|(s, _)| s == name) {
            Some((_, total)) => *total += n,
            None => counts.push((name.to_owned(), n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::cast::AsArray;

    #[test]
    fn a_batch_is_stamped_after_its_columns() {
        let schema = Schema::new(vec![Field::new("X", DataType::Int64, true)]);
        let lake = lake_schema(&schema);
        let names: Vec<&str> = lake.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, ["X", "_nl_source", "_nl_published"]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(arrow_array::Int64Array::from(vec![1, 2]))],
        )
        .unwrap();
        let b = stamped(&batch, &Stamp::new("s", Some(5)), &lake).unwrap();
        assert_eq!((b.num_rows(), b.num_columns()), (2, 3));
        assert!(stamped(&batch, &Stamp::new("s", None), &batch.schema()).is_err());
    }

    #[test]
    fn a_stamp_fills_any_length_and_counts_back() {
        let s = Stamp::new("https://h/a.zip!A.CSV", Some(1_000));
        assert_eq!(s.name(), "https://h/a.zip!A.CSV");
        assert_eq!(s.published(), Some(1_000));
        let [source, published] = s.columns(3);
        assert_eq!(source.len(), 3);
        assert_eq!(source.data_type(), fields()[0].data_type());
        assert_eq!(published.data_type(), fields()[1].data_type());
        let p = published.as_primitive::<arrow_array::types::TimestampMicrosecondType>();
        assert_eq!(p.value(2), 1_000);
        let [_, none] = Stamp::new("x", None).columns(2);
        assert_eq!(none.null_count(), 2);
        let mut counts = Vec::new();
        count(source.as_ref(), &mut counts);
        count(Stamp::new("y", None).columns(2)[0].as_ref(), &mut counts);
        count(source.as_ref(), &mut counts);
        // Not a source column: nothing counted.
        count(&StringArray::from(vec!["z"]), &mut counts);
        let keys = UInt16Array::from(vec![0u16]);
        let not_text =
            DictionaryArray::<UInt16Type>::try_new(keys, Arc::new(UInt16Array::from(vec![1u16])))
                .unwrap();
        count(&not_text, &mut counts);
        assert_eq!(
            counts,
            [("https://h/a.zip!A.CSV".to_owned(), 6), ("y".to_owned(), 2)]
        );
        let fields = fields();
        let ids: Vec<&str> = fields
            .iter()
            .map(|f| f.metadata()[parquet::arrow::PARQUET_FIELD_ID_META_KEY].as_str())
            .collect();
        assert_eq!(ids, ["2147483000", "2147483001"]);
    }
}
