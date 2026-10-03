//! Hive-partitioned Parquet files under `{lake}/{market}/{table}/{partition}/`.
//!
//! Each source file (zip entry) writes one Parquet file per partition it
//! touches, named `{source_stem}-v{version}-{sha8}.parquet`, so a re-ingest
//! of the same source replaces exactly its own files. A file is written to a
//! `.tmp-{pid}-{n}` sibling, fsynced and renamed, so a reader never sees a
//! partial file.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};

use crate::error::StoreError;
use crate::partition::{is_sorted, time_bounds};
use crate::writer::{FileMeta, LakeWriter, PartitionKey, WriterId, WrittenFile};

/// Rows per row group. Small enough for fine predicate pushdown, large
/// enough that per-group metadata is noise.
pub const ROW_GROUP_ROWS: usize = 262_144;

struct Open {
    tmp: PathBuf,
    final_path: PathBuf,
    partition: crate::writer::Partition,
    writer: ArrowWriter<File>,
    schema: SchemaRef,
    rows: u64,
    min: Option<i64>,
    max: Option<i64>,
    sorted: bool,
    last_max: Option<i64>,
    time_column: Option<usize>,
}

/// The production lake writer.
pub struct ParquetHive {
    lake_root: PathBuf,
    zstd_level: i32,
    open: HashMap<u32, Open>,
    next: u32,
    version: &'static str,
}

impl ParquetHive {
    /// A writer rooted at `lake_root` (the `lake/` directory under the home).
    pub fn new(lake_root: impl Into<PathBuf>) -> Self {
        Self {
            lake_root: lake_root.into(),
            zstd_level: 3,
            open: HashMap::new(),
            next: 1,
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// Sets the zstd level (1..=22).
    pub fn with_zstd_level(mut self, level: i32) -> Self {
        self.zstd_level = level.clamp(1, 22);
        self
    }

    /// The directory of a partition.
    pub fn partition_dir(&self, key: &PartitionKey) -> PathBuf {
        self.lake_root
            .join(&key.market)
            .join(&key.table_dir)
            .join(key.partition.path_segment())
    }

    /// The final file name for a source in a partition.
    pub fn file_name(meta: &FileMeta) -> String {
        let sha8: String = meta.source_sha256.chars().take(8).collect();
        let sha8 = if sha8.is_empty() {
            format!("{:08x}", meta.fingerprint.0 as u32)
        } else {
            sha8
        };
        format!(
            "{}-v{}-{sha8}.parquet",
            meta.source_stem, meta.schema_version
        )
    }

    fn props(&self, schema: &SchemaRef, meta: &FileMeta) -> WriterProperties {
        let mut b = WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_2_0)
            .set_compression(Compression::ZSTD(
                ZstdLevel::try_new(self.zstd_level).expect("level clamped to 1..=22"),
            ))
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .set_statistics_enabled(EnabledStatistics::Page)
            .set_created_by(format!("nemlake {}", self.version))
            .set_key_value_metadata(Some(vec![
                KeyValue::new("nemlake.version".to_owned(), self.version.to_owned()),
                KeyValue::new("nemlake.table".to_owned(), meta.table.clone()),
                KeyValue::new(
                    "nemlake.schema_version".to_owned(),
                    meta.schema_version.to_string(),
                ),
                KeyValue::new(
                    "nemlake.fingerprint".to_owned(),
                    meta.fingerprint.to_string(),
                ),
                KeyValue::new("nemlake.source_url".to_owned(), meta.source_url.clone()),
                KeyValue::new(
                    "nemlake.source_sha256".to_owned(),
                    meta.source_sha256.clone(),
                ),
                KeyValue::new("nemlake.entry".to_owned(), meta.entry.clone()),
            ]));
        // Measures never benefit from dictionaries; ids do.
        for f in schema.fields() {
            if matches!(
                f.data_type(),
                arrow_schema::DataType::Float64
                    | arrow_schema::DataType::Decimal128(_, _)
                    | arrow_schema::DataType::Timestamp(_, _)
            ) {
                b = b.set_column_dictionary_enabled(
                    parquet::schema::types::ColumnPath::from(f.name().as_str()),
                    false,
                );
            }
        }
        b.build()
    }
}

impl LakeWriter for ParquetHive {
    fn open(
        &mut self,
        key: &PartitionKey,
        schema: SchemaRef,
        meta: &FileMeta,
    ) -> Result<WriterId, StoreError> {
        let dir = self.partition_dir(key);
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let final_path = dir.join(Self::file_name(meta));
        let id = self.next;
        self.next += 1;
        let tmp = dir.join(format!(
            ".tmp-{}-{id}-{}",
            std::process::id(),
            Self::file_name(meta)
        ));
        let file = File::create(&tmp).map_err(|source| StoreError::Io {
            path: tmp.display().to_string(),
            source,
        })?;
        // Sort order is not declared in the footer: the writer learns whether
        // the rows were ordered only as batches arrive, and Parquet properties
        // are fixed at open. `WrittenFile::sorted` records it for the manifest
        // and a later compaction declares it.
        let props = self.props(&schema, meta);
        let writer = ArrowWriter::try_new(file, schema.clone(), Some(props)).map_err(|e| {
            StoreError::Parquet {
                path: tmp.display().to_string(),
                message: e.to_string(),
            }
        })?;
        self.open.insert(
            id,
            Open {
                tmp,
                final_path,
                partition: key.partition,
                writer,
                schema,
                rows: 0,
                min: None,
                max: None,
                sorted: true,
                last_max: None,
                time_column: meta.time_column,
            },
        );
        Ok(WriterId(id))
    }

    fn write(&mut self, id: WriterId, batch: &RecordBatch) -> Result<(), StoreError> {
        let o = self
            .open
            .get_mut(&id.0)
            .ok_or(StoreError::NoSuchWriter(id.0))?;
        if batch.schema().fields() != o.schema.fields() {
            return Err(StoreError::SchemaMismatch {
                path: o.final_path.display().to_string(),
                expected: o
                    .schema
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect::<Vec<_>>()
                    .join(","),
                observed: batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect::<Vec<_>>()
                    .join(","),
            });
        }
        if let Some(tc) = o.time_column {
            let (mn, mx) = time_bounds(batch, tc);
            if o.sorted
                && (!is_sorted(batch, tc)
                    || matches!((o.last_max, mn), (Some(prev), Some(first)) if first < prev))
            {
                o.sorted = false;
            }
            o.min = match (o.min, mn) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            o.max = match (o.max, mx) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            o.last_max = mx.or(o.last_max);
        }
        o.writer.write(batch).map_err(|e| StoreError::Parquet {
            path: o.tmp.display().to_string(),
            message: e.to_string(),
        })?;
        o.rows += batch.num_rows() as u64;
        Ok(())
    }

    fn close(&mut self, id: WriterId) -> Result<WrittenFile, StoreError> {
        let o = self
            .open
            .remove(&id.0)
            .ok_or(StoreError::NoSuchWriter(id.0))?;
        let Open {
            tmp,
            final_path,
            partition,
            writer,
            rows,
            min,
            max,
            sorted,
            time_column,
            ..
        } = o;
        let sorted = sorted && time_column.is_some();
        let file = writer.into_inner().map_err(|e| StoreError::Parquet {
            path: tmp.display().to_string(),
            message: e.to_string(),
        })?;
        file.sync_all().map_err(|source| StoreError::Io {
            path: tmp.display().to_string(),
            source,
        })?;
        let bytes = file.metadata().map_or(0, |m| m.len());
        drop(file);
        std::fs::rename(&tmp, &final_path).map_err(|source| StoreError::Rename {
            from: tmp.display().to_string(),
            to: final_path.display().to_string(),
            source,
        })?;
        Ok(WrittenFile {
            path: final_path.display().to_string(),
            partition,
            rows,
            bytes,
            min_time: min,
            max_time: max,
            sorted,
        })
    }

    fn abort(&mut self, id: WriterId) {
        if let Some(o) = self.open.remove(&id.0) {
            let _ = o.writer.close();
            let _ = std::fs::remove_file(&o.tmp);
        }
    }
}

impl Drop for ParquetHive {
    fn drop(&mut self) {
        // Anything still open was not closed on purpose: remove its temp file.
        let ids: Vec<u32> = self.open.keys().copied().collect();
        for id in ids {
            self.abort(WriterId(id));
        }
    }
}

/// Removes stale `.tmp-*` files left by a crashed process under `lake_root`.
pub fn sweep_tmp(lake_root: &Path) -> std::io::Result<usize> {
    let mut removed = 0;
    let mut stack = vec![lake_root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".tmp-"))
            {
                std::fs::remove_file(&p)?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Float64Array, StringArray, TimestampMicrosecondArray};
    use arrow_schema::{DataType, Field, Schema, TimeUnit};
    use nemlake_core::Fingerprint;
    use nemlake_core::nemtime::{NEM_OFFSET_SECS, parse_micros};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    use super::*;
    use crate::writer::Partition;

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(
                "SETTLEMENTDATE",
                DataType::Timestamp(TimeUnit::Microsecond, Some("+10:00".into())),
                false,
            ),
            Field::new("DUID", DataType::Utf8, false),
            Field::new("SCADAVALUE", DataType::Float64, true),
        ]))
    }

    fn batch(times: &[&str], ids: &[&str], vals: &[Option<f64>]) -> RecordBatch {
        let ts: Vec<i64> = times
            .iter()
            .map(|t| parse_micros(t.as_bytes(), NEM_OFFSET_SECS).unwrap())
            .collect();
        RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(TimestampMicrosecondArray::from(ts).with_timezone("+10:00")),
                Arc::new(StringArray::from(ids.to_vec())),
                Arc::new(Float64Array::from(vals.to_vec())),
            ],
        )
        .unwrap()
    }

    fn meta() -> FileMeta {
        FileMeta {
            table: "DISPATCH_UNIT_SCADA".into(),
            source_stem: "public_dvd_dispatch_unit_scada_202401010000".into(),
            schema_version: 1,
            fingerprint: Fingerprint(7),
            source_url: "https://x/y.zip".into(),
            source_sha256: "abcdef0123456789".into(),
            entry: "Y.CSV".into(),
            time_column: Some(0),
        }
    }

    fn key(month: u8) -> PartitionKey {
        PartitionKey {
            market: "nem".into(),
            table_dir: "dispatch_unit_scada".into(),
            partition: Partition::Month { year: 2024, month },
        }
    }

    #[test]
    fn writes_renames_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = ParquetHive::new(dir.path()).with_zstd_level(1);
        let id = w.open(&key(1), schema(), &meta()).unwrap();
        w.write(
            id,
            &batch(
                &["2024/01/01 00:05:00", "2024/01/01 00:10:00"],
                &["A", "B"],
                &[Some(1.0), None],
            ),
        )
        .unwrap();
        w.write(id, &batch(&["2024/01/01 00:15:00"], &["C"], &[Some(3.0)]))
            .unwrap();
        let dir_listing: Vec<_> = std::fs::read_dir(
            dir.path()
                .join("nem/dispatch_unit_scada/year=2024/month=01"),
        )
        .unwrap()
        .collect();
        assert_eq!(dir_listing.len(), 1, "only the temp file while open");
        let f = w.close(id).unwrap();
        assert_eq!(f.rows, 3);
        assert!(f.bytes > 0);
        assert_eq!(
            f.partition,
            Partition::Month {
                year: 2024,
                month: 1
            }
        );
        assert_eq!(
            f.min_time,
            Some(parse_micros(b"2024/01/01 00:05:00", NEM_OFFSET_SECS).unwrap())
        );
        assert_eq!(
            f.max_time,
            Some(parse_micros(b"2024/01/01 00:15:00", NEM_OFFSET_SECS).unwrap())
        );
        assert!(
            f.path.ends_with(
                "year=2024/month=01/public_dvd_dispatch_unit_scada_202401010000-v1-abcdef01.parquet"
            ),
            "{}",
            f.path
        );
        assert!(Path::new(&f.path).is_file());
        assert_eq!(sweep_tmp(dir.path()).unwrap(), 0);
        let file = File::open(&f.path).unwrap();
        let b = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        let kv = b.metadata().file_metadata().key_value_metadata().unwrap();
        assert!(
            kv.iter()
                .any(|k| k.key == "nemlake.table"
                    && k.value.as_deref() == Some("DISPATCH_UNIT_SCADA"))
        );
        assert!(kv.iter().any(|k| k.key == "nemlake.source_sha256"));
        assert!(f.sorted);
        let batches: Vec<RecordBatch> = b.build().unwrap().map(Result::unwrap).collect();
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 3);
        assert_eq!(
            batches[0].schema().field(0).data_type(),
            &DataType::Timestamp(TimeUnit::Microsecond, Some("+10:00".into()))
        );
        assert_eq!(batches[0].column(2).null_count(), 1);
    }

    #[test]
    fn unsorted_batches_and_errors_and_abort() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = ParquetHive::new(dir.path());
        let id = w.open(&key(2), schema(), &meta()).unwrap();
        w.write(id, &batch(&["2024/02/02 00:00:00"], &["A"], &[None]))
            .unwrap();
        w.write(id, &batch(&["2024/02/01 00:00:00"], &["B"], &[None]))
            .unwrap();
        let f = w.close(id).unwrap();
        assert!(!f.sorted, "the second batch went backwards");
        assert_eq!(
            f.min_time,
            Some(parse_micros(b"2024/02/01 00:00:00", NEM_OFFSET_SECS).unwrap())
        );
        // Schema mismatch.
        let id = w.open(&key(3), schema(), &meta()).unwrap();
        let other = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("X", DataType::Int64, true)])),
            vec![Arc::new(arrow_array::Int64Array::from(vec![1]))],
        )
        .unwrap();
        assert!(matches!(
            w.write(id, &other),
            Err(StoreError::SchemaMismatch { .. })
        ));
        w.abort(id);
        assert!(
            std::fs::read_dir(
                dir.path()
                    .join("nem/dispatch_unit_scada/year=2024/month=03")
            )
            .unwrap()
            .next()
            .is_none()
        );
        assert!(matches!(
            w.write(id, &other),
            Err(StoreError::NoSuchWriter(_))
        ));
        assert!(matches!(w.close(id), Err(StoreError::NoSuchWriter(_))));
        // Drop aborts open writers.
        let id = w.open(&key(4), schema(), &meta()).unwrap();
        let tmp_dir = dir
            .path()
            .join("nem/dispatch_unit_scada/year=2024/month=04");
        assert_eq!(std::fs::read_dir(&tmp_dir).unwrap().count(), 1);
        drop(w);
        assert_eq!(
            std::fs::read_dir(&tmp_dir).unwrap().count(),
            0,
            "{id:?} aborted on drop"
        );
        // A stale temp file is swept.
        std::fs::write(tmp_dir.join(".tmp-999-1-x.parquet"), b"x").unwrap();
        assert_eq!(sweep_tmp(dir.path()).unwrap(), 1);
        // No time column, snapshot partition, no sha.
        let mut w = ParquetHive::new(dir.path());
        let mut m = meta();
        m.time_column = None;
        m.source_sha256 = String::new();
        let k = PartitionKey {
            market: "nem".into(),
            table_dir: "region".into(),
            partition: Partition::Snapshot {
                year: 2024,
                month: 1,
            },
        };
        let id = w.open(&k, schema(), &m).unwrap();
        w.write(id, &batch(&["2024/01/01 00:00:00"], &["A"], &[None]))
            .unwrap();
        let f = w.close(id).unwrap();
        assert!(f.path.contains("snapshot=202401/"));
        assert!(f.path.ends_with("-v1-00000007.parquet"), "{}", f.path);
        assert_eq!(f.min_time, None);
        assert!(!f.sorted, "no time column, no claim");
        // Unwritable root.
        let mut w = ParquetHive::new(dir.path().join("file-not-dir"));
        std::fs::write(dir.path().join("file-not-dir"), b"").unwrap();
        assert!(matches!(
            w.open(&key(1), schema(), &meta()),
            Err(StoreError::Io { .. })
        ));
    }
}
