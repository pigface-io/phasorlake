//! Hive-partitioned Parquet files under `{lake}/{market}/{table}/data/{partition}/`,
//! beside the table's Iceberg metadata in `{table}/metadata/`: the layout
//! Iceberg readers expect, so one finds a table's files again under a lake
//! moved or copied whole (DuckDB's `allow_moved_paths` re-roots a path at
//! its `data/` directory).
//!
//! Each source file (zip entry) writes one Parquet file per partition it
//! touches, named `{source_stem}-v{version}-{sha8}.parquet`, so a re-ingest
//! of the same source replaces exactly its own files. A file is written to a
//! `.tmp-{pid}-{n}` sibling and handed to the disk when closed; a commit
//! flushes the disk once for every file closed since the last, then renames
//! each into place, so a reader never sees a partial file. One flush of the
//! disk costs milliseconds (`F_FULLFSYNC` on macOS), so it is paid per
//! commit, not per file.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};

use phasor_core::config::{Codec, Lake, Statistics};

use crate::error::StoreError;
use crate::partition::{is_sorted, time_bounds};
use crate::writer::{FileMeta, LakeWriter, PartitionKey, WriterId, WrittenFile};

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
    /// Rows of each source, in the order they came.
    sources: Vec<(String, u64)>,
}

/// A file closed, on the disk under its temporary name until a commit.
/// Only its names are kept: a run closes more files than it may hold open.
struct Closed {
    tmp: PathBuf,
    final_path: PathBuf,
}

/// Parquet's compression for `codec`; `zstd_level` counts for zstd only
/// and is held to 1..=22.
pub fn compression(codec: Codec, zstd_level: i32) -> Compression {
    match codec {
        Codec::Zstd => {
            Compression::ZSTD(ZstdLevel::try_new(zstd_level.clamp(1, 22)).expect("held to 1..=22"))
        }
        Codec::Lz4 => Compression::LZ4_RAW,
        Codec::Snappy => Compression::SNAPPY,
        Codec::None => Compression::UNCOMPRESSED,
    }
}

/// The directory under a table's that holds its data files.
pub const DATA: &str = "data";

/// Bytes of a row group in memory, as the writer counts them, past which it
/// ends short of its row count. The writer's count is low: a table of 100
/// columns holds about five times as much, so 8 MiB keeps a wide table's
/// ingest near 50 MB of heap where its row count alone let it reach 340.
pub const ROW_GROUP_BYTES: usize = 8 << 20;

/// The production lake writer.
pub struct ParquetHive {
    lake_root: PathBuf,
    layout: Lake,
    open: HashMap<u32, Open>,
    closed: Vec<Closed>,
    version: &'static str,
    row_group_bytes: usize,
}

impl ParquetHive {
    /// A writer rooted at `lake_root` (the `lake/` directory under the home).
    pub fn new(lake_root: impl Into<PathBuf>) -> Self {
        Self {
            lake_root: lake_root.into(),
            layout: Lake::default(),
            open: HashMap::new(),
            closed: Vec::new(),
            version: env!("CARGO_PKG_VERSION"),
            row_group_bytes: ROW_GROUP_BYTES,
        }
    }

    /// Lays files out as `layout` says, in place of the defaults.
    #[must_use]
    pub fn with_layout(mut self, layout: Lake) -> Self {
        self.layout = layout;
        self
    }

    /// The bytes an open file holds so far, written and buffered: what
    /// decides when it is closed for the next.
    pub fn bytes_in_file(&self, id: WriterId) -> u64 {
        self.open.get(&id.0).map_or(0, |o| {
            (o.writer.bytes_written() + o.writer.in_progress_size()) as u64
        })
    }

    /// The directory of a partition.
    pub fn partition_dir(&self, key: &PartitionKey) -> PathBuf {
        self.lake_root
            .join(&key.market)
            .join(&key.table_dir)
            .join(DATA)
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
        // The shape's fingerprint keeps two column lists of one version,
        // from one source, in files of their own. A further file of the
        // same partition is told apart by its part number.
        let part = if meta.part > 1 {
            format!("-p{}", meta.part)
        } else {
            String::new()
        };
        format!(
            "{}-v{}-{:08x}-{sha8}{part}.parquet",
            meta.source_stem, meta.schema_version, meta.fingerprint.0 as u32
        )
    }

    fn props(&self, schema: &SchemaRef, meta: &FileMeta) -> WriterProperties {
        let l = &self.layout;
        let mut b = WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_2_0)
            // PLAIN where a dictionary gives up: the version 2 default,
            // DELTA, reserves 1 MiB per integer column for every row group
            // a writer has open, and zstd after PLAIN does as well here.
            .set_encoding(Encoding::PLAIN)
            .set_compression(compression(l.compression, l.zstd_level))
            .set_max_row_group_row_count(Some(l.row_group_rows))
            .set_data_page_size_limit(l.page_kb as usize * 1024)
            .set_data_page_row_count_limit(l.page_rows)
            .set_dictionary_page_size_limit(l.dictionary_page_kb as usize * 1024)
            .set_statistics_enabled(match l.statistics {
                Statistics::None => EnabledStatistics::None,
                Statistics::Chunk => EnabledStatistics::Chunk,
                Statistics::Page => EnabledStatistics::Page,
            })
            .set_created_by(format!("phasor {}", self.version))
            .set_key_value_metadata(Some(vec![
                KeyValue::new("phasor.version".to_owned(), self.version.to_owned()),
                KeyValue::new(
                    "phasor.write_options".to_owned(),
                    serde_json::to_string(&self.layout).expect("the layout is plain data"),
                ),
                KeyValue::new("phasor.table".to_owned(), meta.table.clone()),
                KeyValue::new(
                    "phasor.schema_version".to_owned(),
                    meta.schema_version.to_string(),
                ),
                KeyValue::new(
                    "phasor.fingerprint".to_owned(),
                    meta.fingerprint.to_string(),
                ),
                KeyValue::new("phasor.source_url".to_owned(), meta.source_url.clone()),
                KeyValue::new(
                    "phasor.source_sha256".to_owned(),
                    meta.source_sha256.clone(),
                ),
                KeyValue::new(
                    "phasor.lake_format".to_owned(),
                    crate::provenance::LAKE_FORMAT.to_owned(),
                ),
                // The transform: what was read, and by which plan.
                KeyValue::new(
                    "phasor.report_type".to_owned(),
                    meta.transform.block_family.clone(),
                ),
                KeyValue::new(
                    "phasor.report_subtype".to_owned(),
                    meta.transform.block_name.clone(),
                ),
                KeyValue::new(
                    "phasor.source_version".to_owned(),
                    meta.transform.source_version.to_string(),
                ),
                KeyValue::new(
                    "phasor.source_columns".to_owned(),
                    meta.transform.source_columns.join(","),
                ),
                KeyValue::new(
                    "phasor.source_period".to_owned(),
                    meta.transform.source_period.clone().unwrap_or_default(),
                ),
                KeyValue::new("phasor.plan".to_owned(), meta.transform.plan_hash.clone()),
                KeyValue::new(
                    "phasor.schema_source".to_owned(),
                    meta.transform.schema_source.clone(),
                ),
                KeyValue::new(
                    "phasor.resolution".to_owned(),
                    meta.transform.resolution.clone(),
                ),
                KeyValue::new(
                    "phasor.shape_id".to_owned(),
                    meta.transform.shape_id.clone().unwrap_or_default(),
                ),
                KeyValue::new(
                    "phasor.schema_id".to_owned(),
                    meta.transform
                        .schema_id
                        .map_or_else(String::new, |s| s.to_string()),
                ),
            ]));
        // Measures never benefit from dictionaries; ids do, and so do a
        // source's times, one value each over many rows.
        for f in schema.fields() {
            if !crate::provenance::TIME_COLUMNS.contains(&f.name().as_str())
                && matches!(
                    f.data_type(),
                    arrow_schema::DataType::Float64
                        | arrow_schema::DataType::Decimal128(_, _)
                        | arrow_schema::DataType::Timestamp(_, _)
                )
            {
                b = b.set_column_dictionary_enabled(
                    parquet::schema::types::ColumnPath::from(f.name().as_str()),
                    false,
                );
            }
        }
        b.build()
    }
}

impl ParquetHive {
    /// How many files are closed and not yet committed: a mark to
    /// [`discard`](Self::discard) back to.
    pub fn closed(&self) -> usize {
        self.closed.len()
    }

    /// Removes the files closed after `mark` and not yet committed.
    pub fn discard(&mut self, mark: usize) {
        for c in self.closed.drain(mark.min(self.closed.len())..) {
            let _ = std::fs::remove_file(&c.tmp);
        }
    }
}

impl LakeWriter for ParquetHive {
    fn open(
        &mut self,
        key: &PartitionKey,
        schema: SchemaRef,
        meta: &FileMeta,
    ) -> Result<WriterId, StoreError> {
        zone!(OPEN);
        // A lake file ends with every row's source and the times its
        // source was stamped with.
        let tail = crate::provenance::fields();
        let ends = schema.fields().len() >= tail.len()
            && schema.fields()[schema.fields().len() - tail.len()..]
                .iter()
                .zip(tail)
                .all(|(f, p)| f.name() == p.name() && f.data_type() == p.data_type());
        if !ends {
            return Err(StoreError::NoProvenance {
                observed: schema
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect::<Vec<_>>()
                    .join(","),
            });
        }
        if let Some(f) = crate::partition::naive_timestamp(&schema) {
            return Err(StoreError::NaiveTimestamp {
                column: f.name().clone(),
                observed: f.data_type().to_string(),
            });
        }
        let dir = self.partition_dir(key);
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let final_path = dir.join(Self::file_name(meta));
        // Unique in the process, not only in this writer: two writers of
        // one run may open the same final name at once.
        let id = NEXT_TMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                sources: Vec::new(),
            },
        );
        Ok(WriterId(id))
    }

    fn write(&mut self, id: WriterId, batch: &RecordBatch) -> Result<(), StoreError> {
        zone!(WRITE);
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
            zone!(BOUNDS);
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
        let encoded = {
            zone!(ENCODE);
            o.writer.write(batch)
        };
        encoded.map_err(|e| StoreError::Parquet {
            path: o.tmp.display().to_string(),
            message: e.to_string(),
        })?;
        // The writer holds a row group in memory until it ends: a wide
        // table's ends at a size too, not only at its row count.
        if o.writer.memory_size() >= self.row_group_bytes {
            zone!(ENCODE);
            o.writer.flush().map_err(|e| StoreError::Parquet {
                path: o.tmp.display().to_string(),
                message: e.to_string(),
            })?;
        }
        o.rows += batch.num_rows() as u64;
        let source = batch.column(batch.num_columns() - crate::provenance::fields().len());
        crate::provenance::count(source.as_ref(), &mut o.sources);
        Ok(())
    }

    fn close(&mut self, id: WriterId) -> Result<WrittenFile, StoreError> {
        zone!(CLOSE);
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
            sources,
            ..
        } = o;
        let mut writer = writer;
        let sources = serde_json::Value::Array(
            sources
                .iter()
                .map(|(source, rows)| serde_json::json!({"source": source, "rows": rows}))
                .collect(),
        );
        writer.append_key_value_metadata(KeyValue::new(
            "phasor.sources".to_owned(),
            sources.to_string(),
        ));
        let sorted = sorted && time_column.is_some();
        let file = writer.into_inner().map_err(|e| StoreError::Parquet {
            path: tmp.display().to_string(),
            message: e.to_string(),
        })?;
        // To the disk. Where `fsync` stops at the disk's cache (macOS), the
        // commit flushes that once for every file.
        rustix::fs::fsync(&file).map_err(|e| StoreError::Io {
            path: tmp.display().to_string(),
            source: e.into(),
        })?;
        let bytes = file.metadata().map_or(0, |m| m.len());
        drop(file);
        let path = final_path.display().to_string();
        self.closed.push(Closed { tmp, final_path });
        Ok(WrittenFile {
            path,
            partition,
            rows,
            bytes,
            min_time: min,
            max_time: max,
            sorted,
        })
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        zone!(COMMIT);
        let Some(last) = self.closed.last() else {
            return Ok(());
        };
        // One flush of the disk's cache makes every file before it durable.
        File::open(&last.tmp)
            .and_then(|f| f.sync_all())
            .map_err(|source| StoreError::Io {
                path: last.tmp.display().to_string(),
                source,
            })?;
        let mut closed = std::mem::take(&mut self.closed).into_iter();
        while let Some(c) = closed.next() {
            if let Err(source) = std::fs::rename(&c.tmp, &c.final_path) {
                let err = StoreError::Rename {
                    from: c.tmp.display().to_string(),
                    to: c.final_path.display().to_string(),
                    source,
                };
                // Not put in place: neither this file nor those after it.
                for c in std::iter::once(c).chain(closed) {
                    let _ = std::fs::remove_file(&c.tmp);
                }
                return Err(err);
            }
        }
        Ok(())
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
        // Anything still open was not closed on purpose, and anything
        // closed but not committed was not kept: remove its temp file.
        let ids: Vec<u32> = self.open.keys().copied().collect();
        for id in ids {
            self.abort(WriterId(id));
        }
        self.discard(0);
    }
}

/// The number in each temporary file's name, unique in the process.
static NEXT_TMP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

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
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
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
    use phasor_core::{Diagnose, Fingerprint};

    use super::*;
    use crate::writer::{Partition, Transform};

    fn schema() -> SchemaRef {
        crate::provenance::lake_schema(&data_schema())
    }

    fn data_schema() -> Schema {
        Schema::new(vec![
            Field::new(
                "SETTLEMENTDATE",
                DataType::Timestamp(TimeUnit::Microsecond, Some("+10:00".into())),
                false,
            ),
            Field::new("DUID", DataType::Utf8, false),
            Field::new("SCADAVALUE", DataType::Float64, true),
        ])
    }

    fn batch(times: &[&str], ids: &[&str], vals: &[Option<f64>]) -> RecordBatch {
        let ts: Vec<i64> = times.iter().map(|t| nanos(t) / 1_000).collect();
        let data = RecordBatch::try_new(
            Arc::new(data_schema()),
            vec![
                Arc::new(TimestampMicrosecondArray::from(ts).with_timezone("+10:00")),
                Arc::new(StringArray::from(ids.to_vec())),
                Arc::new(Float64Array::from(vals.to_vec())),
            ],
        )
        .unwrap();
        let stamp = crate::provenance::Stamp::new(
            "https://x/y.zip!Y.CSV",
            crate::provenance::Times {
                server_last_modified: Some(1_704_067_200_000_000),
                ..Default::default()
            },
        );
        crate::provenance::stamped(&data, &stamp, &schema()).unwrap()
    }

    fn meta() -> FileMeta {
        FileMeta {
            table: "DISPATCH_UNIT_SCADA".into(),
            source_stem: "public_dvd_dispatch_unit_scada_202401010000".into(),
            schema_version: 1,
            fingerprint: Fingerprint(7),
            source_url: "https://x/y.zip".into(),
            source_sha256: "abcdef0123456789".into(),
            time_column: Some(0),
            part: 1,
            transform: Transform {
                block_family: "DISPATCH".into(),
                block_name: "UNIT_SCADA".into(),
                source_version: 1,
                source_columns: vec!["SETTLEMENTDATE".into(), "DUID".into()],
                source_period: Some("2024-01".into()),
                plan_hash: "plan:00000000000000000000000000000abc".into(),
                shape_id: Some("0123456789abcdef".into()),
                schema_id: Some(3),
                schema_source: "registry".into(),
                resolution: "exact".into(),
            },
        }
    }

    #[test]
    fn a_row_group_ends_at_its_size_before_its_row_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = ParquetHive::new(dir.path());
        w.row_group_bytes = 1;
        let id = w.open(&key(1), schema(), &meta()).unwrap();
        for t in ["2024/01/01 00:05:00", "2024/01/01 00:10:00"] {
            w.write(id, &batch(&[t], &["A"], &[Some(1.0)])).unwrap();
        }
        let f = w.close(id).unwrap();
        w.commit().unwrap();
        let file = File::open(&f.path).unwrap();
        let b = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        assert_eq!(b.metadata().num_row_groups(), 2);
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
        let mut w = ParquetHive::new(dir.path()).with_layout(Lake {
            zstd_level: 1,
            ..Lake::default()
        });
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
                .join("nem/dispatch_unit_scada/data/year=2024/month=01"),
        )
        .unwrap()
        .collect();
        assert_eq!(dir_listing.len(), 1, "only the temp file while open");
        let f = w.close(id).unwrap();
        w.commit().unwrap();
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
            Some(utc_nanos(2024, 1, 1, 0, 5, 0, NEM_OFFSET_SECS))
        );
        assert_eq!(
            f.max_time,
            Some(utc_nanos(2024, 1, 1, 0, 15, 0, NEM_OFFSET_SECS))
        );
        assert!(
            f.path.ends_with(
                "dispatch_unit_scada/data/year=2024/month=01/public_dvd_dispatch_unit_scada_202401010000-v1-00000007-abcdef01.parquet"
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
                .any(|k| k.key == "phasor.table"
                    && k.value.as_deref() == Some("DISPATCH_UNIT_SCADA"))
        );
        assert!(kv.iter().any(|k| k.key == "phasor.source_sha256"));
        let value = |key: &str| {
            kv.iter()
                .find(|k| k.key == key)
                .and_then(|k| k.value.clone())
                .unwrap_or_default()
        };
        assert_eq!(value("phasor.report_type"), "DISPATCH");
        assert_eq!(value("phasor.report_subtype"), "UNIT_SCADA");
        assert_eq!(value("phasor.source_version"), "1");
        assert_eq!(value("phasor.source_columns"), "SETTLEMENTDATE,DUID");
        assert_eq!(value("phasor.source_period"), "2024-01");
        assert_eq!(
            value("phasor.plan"),
            "plan:00000000000000000000000000000abc"
        );
        assert_eq!(value("phasor.shape_id"), "0123456789abcdef");
        assert_eq!(value("phasor.schema_id"), "3");
        assert_eq!(value("phasor.schema_source"), "registry");
        assert_eq!(value("phasor.resolution"), "exact");
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
    fn a_timestamp_without_a_zone_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = ParquetHive::new(dir.path());
        for (name, ty) in [
            ("NAIVE", DataType::Timestamp(TimeUnit::Microsecond, None)),
            (
                "NANOS",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+10:00".into())),
            ),
        ] {
            let s = crate::provenance::lake_schema(&Schema::new(vec![
                Field::new("DUID", DataType::Utf8, false),
                Field::new(name, ty, true),
            ]));
            let err = w.open(&key(1), s, &meta()).unwrap_err();
            assert!(
                matches!(&err, StoreError::NaiveTimestamp { column, .. } if column == name),
                "{err}"
            );
            assert_eq!(err.diagnostic().code, "store.naive_timestamp");
        }
        // Nor one without each row's source and publish time.
        let err = w
            .open(&key(1), Arc::new(data_schema()), &meta())
            .unwrap_err();
        assert!(matches!(err, StoreError::NoProvenance { .. }), "{err}");
        assert_eq!(err.diagnostic().code, "store.no_provenance");
        let one = Arc::new(Schema::new(vec![Field::new("X", DataType::Int64, true)]));
        assert!(w.open(&key(1), one, &meta()).is_err());
        // Nothing was created for a schema the lake refuses.
        assert!(w.open.is_empty());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn the_layout_reaches_the_file() {
        use parquet::file::reader::{FileReader, SerializedFileReader};
        let dir = tempfile::tempdir().unwrap();
        for (layout, codec) in [
            (Lake::default(), "ZSTD"),
            (
                Lake {
                    compression: Codec::Lz4,
                    row_group_rows: 1024,
                    statistics: Statistics::Chunk,
                    ..Lake::default()
                },
                "LZ4_RAW",
            ),
            (
                Lake {
                    compression: Codec::Snappy,
                    statistics: Statistics::None,
                    ..Lake::default()
                },
                "SNAPPY",
            ),
            (
                Lake {
                    compression: Codec::None,
                    ..Lake::default()
                },
                "UNCOMPRESSED",
            ),
        ] {
            let mut w = ParquetHive::new(dir.path()).with_layout(layout);
            let id = w.open(&key(7), schema(), &meta()).unwrap();
            let times: Vec<String> = (0..2000)
                .map(|i| format!("2024/07/01 {:02}:{:02}:00", i / 60 % 24, i % 60))
                .collect();
            let times: Vec<&str> = times.iter().map(String::as_str).collect();
            let ids = vec!["A"; 2000];
            let vals = vec![Some(1.5); 2000];
            w.write(id, &batch(&times, &ids, &vals)).unwrap();
            assert!(w.bytes_in_file(id) > 0);
            assert_eq!(w.bytes_in_file(WriterId(999)), 0);
            let f = w.close(id).unwrap();
            w.commit().unwrap();
            let r = SerializedFileReader::new(File::open(&f.path).unwrap()).unwrap();
            let md = r.metadata();
            // A reader sees the codec, not the level it was written at.
            let seen = md.row_group(0).column(0).compression().to_string();
            assert!(seen.starts_with(codec), "{seen}");
            let groups = 2000usize.div_ceil(layout.row_group_rows);
            assert_eq!(md.num_row_groups(), groups);
            let kv = md.file_metadata().key_value_metadata().unwrap();
            let written = kv
                .iter()
                .find(|k| k.key == "phasor.write_options")
                .and_then(|k| k.value.as_deref())
                .unwrap();
            assert_eq!(serde_json::from_str::<Lake>(written).unwrap(), layout);
            let value = |key: &str| {
                kv.iter()
                    .find(|k| k.key == key)
                    .and_then(|k| k.value.clone())
                    .unwrap()
            };
            assert_eq!(value("phasor.lake_format"), "5");
            assert_eq!(
                value("phasor.sources"),
                r#"[{"rows":2000,"source":"https://x/y.zip!Y.CSV"}]"#
            );
            std::fs::remove_file(&f.path).unwrap();
        }
    }

    #[test]
    fn pages_are_cut_at_their_size_and_rows() {
        use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
        use parquet::file::metadata::PageIndexPolicy;
        let dir = tempfile::tempdir().unwrap();
        let rows = 50_000;
        let times: Vec<String> = (0..rows)
            .map(|i| {
                format!(
                    "2024/08/{:02} {:02}:{:02}:00",
                    1 + i / 1440 % 28,
                    i / 60 % 24,
                    i % 60
                )
            })
            .collect();
        let times: Vec<&str> = times.iter().map(String::as_str).collect();
        let ids: Vec<String> = (0..rows)
            .map(|i| format!("U{:08x}", i * 2_654_435_761u64 as usize))
            .collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let vals: Vec<Option<f64>> = (0..rows).map(|i| Some(i as f64 * 0.731)).collect();
        // Each page of the DUID column of the first row group: its size
        // and its first row.
        let pages = |layout: Lake| -> Vec<(i32, i64)> {
            let mut w = ParquetHive::new(dir.path()).with_layout(layout);
            let id = w.open(&key(8), schema(), &meta()).unwrap();
            w.write(id, &batch(&times, &ids, &vals)).unwrap();
            let f = w.close(id).unwrap();
            w.commit().unwrap();
            let b = ParquetRecordBatchReaderBuilder::try_new_with_options(
                File::open(&f.path).unwrap(),
                ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required),
            )
            .unwrap();
            let found = b
                .metadata()
                .page_index_for_row_group(0)
                .offset_index(1)
                .unwrap()
                .page_locations()
                .iter()
                .map(|p| (p.compressed_page_size, p.first_row_index))
                .collect();
            std::fs::remove_file(&f.path).unwrap();
            found
        };
        let big = pages(Lake::default());
        let small = pages(Lake {
            page_kb: 8,
            ..Lake::default()
        });
        assert!(
            small.len() > big.len(),
            "{} pages at 8 KB, {} at 1 MB",
            small.len(),
            big.len()
        );
        // A page closes once it passes its size, checked each write batch:
        // a few kilobytes over at most.
        assert!(small.iter().all(|(s, _)| *s < 64 * 1024), "{small:?}");
        // And once it passes its rows, checked each 1024 values.
        let few = pages(Lake {
            page_rows: 1000,
            ..Lake::default()
        });
        let mut starts: Vec<i64> = few.iter().map(|(_, r)| *r).collect();
        starts.push(rows as i64);
        assert!(starts.windows(2).all(|w| w[1] - w[0] <= 1024), "{starts:?}");
        assert!(few.len() > big.len());
    }

    #[test]
    fn a_closed_file_is_visible_only_once_committed() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = ParquetHive::new(dir.path());
        w.commit().unwrap();
        let id = w.open(&key(3), schema(), &meta()).unwrap();
        w.write(id, &batch(&["2024/03/01 00:00:00"], &["A"], &[None]))
            .unwrap();
        let f = w.close(id).unwrap();
        assert!(!Path::new(&f.path).exists());
        w.commit().unwrap();
        assert!(Path::new(&f.path).exists());
        // Closed and dropped without a commit: nothing is left, under
        // either name.
        let id = w.open(&key(4), schema(), &meta()).unwrap();
        w.write(id, &batch(&["2024/04/01 00:00:00"], &["A"], &[None]))
            .unwrap();
        let g = w.close(id).unwrap();
        let folder = Path::new(&g.path).parent().unwrap().to_path_buf();
        drop(w);
        assert!(std::fs::read_dir(&folder).unwrap().next().is_none());
        // Discarded back to a mark: the files before it are kept.
        let mut w = ParquetHive::new(dir.path());
        let id = w.open(&key(5), schema(), &meta()).unwrap();
        w.write(id, &batch(&["2024/05/01 00:00:00"], &["A"], &[None]))
            .unwrap();
        let kept = w.close(id).unwrap();
        let mark = w.closed();
        let id = w.open(&key(6), schema(), &meta()).unwrap();
        w.write(id, &batch(&["2024/06/01 00:00:00"], &["A"], &[None]))
            .unwrap();
        let gone = w.close(id).unwrap();
        w.discard(mark);
        w.commit().unwrap();
        assert!(Path::new(&kept.path).exists());
        let gone = Path::new(&gone.path).parent().unwrap();
        assert!(std::fs::read_dir(gone).unwrap().next().is_none());
        // A file whose place is taken by a folder cannot be put there; one
        // whose temporary file went cannot be flushed.
        let mut w = ParquetHive::new(dir.path());
        let id = w.open(&key(4), schema(), &meta()).unwrap();
        w.write(id, &batch(&["2024/04/01 00:00:00"], &["A"], &[None]))
            .unwrap();
        let taken = w.close(id).unwrap();
        std::fs::create_dir_all(Path::new(&taken.path).join("x")).unwrap();
        assert!(matches!(w.commit(), Err(StoreError::Rename { .. })));
        let mut w = ParquetHive::new(dir.path());
        let id = w.open(&key(4), schema(), &meta()).unwrap();
        w.write(id, &batch(&["2024/04/01 00:00:00"], &["A"], &[None]))
            .unwrap();
        w.close(id).unwrap();
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(matches!(w.commit(), Err(StoreError::Io { .. })));
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
        w.commit().unwrap();
        assert!(!f.sorted, "the second batch went backwards");
        assert_eq!(
            f.min_time,
            Some(utc_nanos(2024, 2, 1, 0, 0, 0, NEM_OFFSET_SECS))
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
                    .join("nem/dispatch_unit_scada/data/year=2024/month=03")
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
            .join("nem/dispatch_unit_scada/data/year=2024/month=04");
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
        w.commit().unwrap();
        assert!(f.path.contains("snapshot=202401/"));
        // With no source hash the shape's fingerprint stands in for it too.
        assert!(
            f.path.ends_with("-v1-00000007-00000007.parquet"),
            "{}",
            f.path
        );
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
