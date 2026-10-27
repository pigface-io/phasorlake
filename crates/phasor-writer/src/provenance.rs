//! The four columns every lake file ends with (see
//! [`phasor_iceberg::provenance`]): each row's source, `url!entry`, and
//! every time that source was stamped with, each transcribed from where it
//! was found and named for that place: the web server's `Last-Modified`,
//! the zip entry's modified time in its zip's central directory, and the
//! fourteen-digit timestamp in its file name. Nothing is combined here: a
//! reader chooses which time to be known by.
//!
//! The columns are built once per source and filled per batch with no
//! string copied: a dictionary of one value and keys of zero, and each time
//! repeated.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_array::types::UInt16Type;
use arrow_array::{
    Array, ArrayRef, DictionaryArray, StringArray, TimestampMicrosecondArray, UInt16Array,
};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef, TimeUnit};
use phasor_iceberg::provenance::{
    FILENAME_TIMESTAMP, SERVER_LAST_MODIFIED, SOURCE, ZIP_ENTRY_MODIFIED,
};

/// The format of a lake file: 5 since every footer and table key is under
/// `phasor.`; a table's files are under its `data/` directory, as Iceberg
/// lays a table out, so a reader finds them in a lake moved whole, since
/// 4; every row carries each time its source was stamped with, under the
/// `_ul_` prefix, since 3.
pub const LAKE_FORMAT: &str = "5";

/// The names of the three time columns, in the order a file holds them.
pub const TIME_COLUMNS: [&str; 3] = [
    SERVER_LAST_MODIFIED.name,
    ZIP_ENTRY_MODIFIED.name,
    FILENAME_TIMESTAMP.name,
];

/// The four fields, as a lake file's schema ends.
pub fn fields() -> [Field; 4] {
    let id = |id: u32| {
        HashMap::from([(
            parquet::arrow::PARQUET_FIELD_ID_META_KEY.to_owned(),
            id.to_string(),
        )])
    };
    let time = |c: phasor_iceberg::provenance::Column| {
        Field::new(
            c.name,
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        )
        .with_metadata(id(c.id))
    };
    [
        Field::new(
            SOURCE.name,
            DataType::Dictionary(Box::new(DataType::UInt16), Box::new(DataType::Utf8)),
            false,
        )
        .with_metadata(id(SOURCE.id)),
        time(SERVER_LAST_MODIFIED),
        time(ZIP_ENTRY_MODIFIED),
        time(FILENAME_TIMESTAMP),
    ]
}

/// `schema` as a lake file holds it: its columns, then the four.
pub fn lake_schema(schema: &Schema) -> SchemaRef {
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    fields.extend(self::fields());
    Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()))
}

/// `batch` with `stamp`'s four columns after its own, in `schema`, the
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

/// Every time a source was stamped with, each as found, in microseconds
/// since 1970 (UTC); none where the source carries no such stamp.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Times {
    /// The HTTP `Last-Modified` the web server gave for the source's bytes.
    pub server_last_modified: Option<i64>,
    /// The entry's modified time in its zip's central directory, read in
    /// the packer's clock.
    pub zip_entry_modified: Option<i64>,
    /// The fourteen-digit `YYYYMMDDHHMMSS` stamp in the source's file
    /// name, read in the host's clock.
    pub filename_timestamp: Option<i64>,
}

impl Times {
    /// The three, in the order a file holds them.
    pub const fn all(&self) -> [Option<i64>; 3] {
        [
            self.server_last_modified,
            self.zip_entry_modified,
            self.filename_timestamp,
        ]
    }
}

/// One source's four columns, ready to be cut to any batch's length.
#[derive(Debug, Clone)]
pub struct Stamp {
    name: Arc<str>,
    values: ArrayRef,
    times: Times,
}

impl Stamp {
    /// The stamp of source `name` (`url!entry`) with the times it was
    /// stamped with.
    pub fn new(name: &str, times: Times) -> Self {
        Self {
            name: Arc::from(name),
            values: Arc::new(StringArray::from(vec![name])),
            times,
        }
    }

    /// The source's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The times the source was stamped with.
    pub const fn times(&self) -> Times {
        self.times
    }

    /// The columns for `rows` rows.
    pub fn columns(&self, rows: usize) -> [ArrayRef; 4] {
        let keys = UInt16Array::from_value(0, rows);
        let source = DictionaryArray::<UInt16Type>::try_new(keys, self.values.clone())
            .expect("a key of 0 into one value");
        let time = |t: Option<i64>| -> ArrayRef {
            Arc::new(
                match t {
                    Some(t) => TimestampMicrosecondArray::from_value(t, rows),
                    None => TimestampMicrosecondArray::new_null(rows),
                }
                .with_timezone("UTC"),
            )
        };
        let [server, zip, name] = self.times.all();
        [Arc::new(source), time(server), time(zip), time(name)]
    }
}

/// The rows of each source in `column`, a batch's `_ul_source`, added to
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
    use arrow_array::types::TimestampMicrosecondType;

    fn server(t: i64) -> Times {
        Times {
            server_last_modified: Some(t),
            ..Times::default()
        }
    }

    #[test]
    fn a_batch_is_stamped_after_its_columns() {
        let schema = Schema::new(vec![Field::new("X", DataType::Int64, true)]);
        let lake = lake_schema(&schema);
        let names: Vec<&str> = lake.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            [
                "X",
                "_ul_source",
                "_ul_server_last_modified",
                "_ul_zip_entry_modified",
                "_ul_filename_timestamp",
            ]
        );
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(arrow_array::Int64Array::from(vec![1, 2]))],
        )
        .unwrap();
        let b = stamped(&batch, &Stamp::new("s", server(5)), &lake).unwrap();
        assert_eq!((b.num_rows(), b.num_columns()), (2, 5));
        assert!(stamped(&batch, &Stamp::new("s", Times::default()), &batch.schema()).is_err());
    }

    #[test]
    fn each_time_is_kept_as_found_and_none_is_null() {
        let times = Times {
            server_last_modified: Some(3_000),
            zip_entry_modified: Some(1_000),
            filename_timestamp: None,
        };
        let s = Stamp::new("https://h/a.zip!A.CSV", times);
        assert_eq!(s.name(), "https://h/a.zip!A.CSV");
        assert_eq!(s.times(), times);
        let [source, server, zip, name] = s.columns(3);
        assert_eq!(source.len(), 3);
        for (column, field) in [&source, &server, &zip, &name].iter().zip(fields()) {
            assert_eq!(column.data_type(), field.data_type());
        }
        // Each time independently: nothing is combined or chosen.
        assert_eq!(
            server.as_primitive::<TimestampMicrosecondType>().value(2),
            3_000
        );
        assert_eq!(
            zip.as_primitive::<TimestampMicrosecondType>().value(0),
            1_000
        );
        assert_eq!(name.null_count(), 3);
        assert_eq!(
            TIME_COLUMNS,
            [
                fields()[1].name().as_str(),
                fields()[2].name().as_str(),
                fields()[3].name().as_str()
            ]
        );
    }

    #[test]
    fn rows_are_counted_back_per_source() {
        let [source, ..] = Stamp::new("https://h/a.zip!A.CSV", Times::default()).columns(3);
        let mut counts = Vec::new();
        count(source.as_ref(), &mut counts);
        count(
            Stamp::new("y", Times::default()).columns(2)[0].as_ref(),
            &mut counts,
        );
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
        assert_eq!(
            ids,
            ["2147483000", "2147483001", "2147483002", "2147483003"]
        );
    }
}
