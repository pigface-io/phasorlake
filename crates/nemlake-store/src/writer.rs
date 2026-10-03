//! The port the pipeline writes through.

use std::fmt;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use nemlake_core::Fingerprint;
use serde::{Deserialize, Serialize};

use crate::error::StoreError;

/// Where in a table's directory a file belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Partition {
    /// Appended rows for a market-local month: `year=YYYY/month=MM`.
    Month {
        /// Year.
        year: i32,
        /// Month 1..=12.
        month: u8,
    },
    /// A snapshot table's full copy from an archive month: `snapshot=YYYYMM`.
    Snapshot {
        /// Year.
        year: i32,
        /// Month 1..=12.
        month: u8,
    },
    /// Rows whose partition value was null.
    Unknown,
}

impl Partition {
    /// The directory segment(s) under the table directory.
    pub fn path_segment(&self) -> String {
        match self {
            Self::Month { year, month } => format!("year={year:04}/month={month:02}"),
            Self::Snapshot { year, month } => format!("snapshot={year:04}{month:02}"),
            Self::Unknown => {
                "year=__HIVE_DEFAULT_PARTITION__/month=__HIVE_DEFAULT_PARTITION__".to_owned()
            }
        }
    }
}

impl fmt::Display for Partition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path_segment())
    }
}

/// A table plus a partition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartitionKey {
    /// Market directory, `nem` or `wem`.
    pub market: String,
    /// Table directory name (lower-case table name).
    pub table_dir: String,
    /// The partition.
    pub partition: Partition,
}

/// How the rows of a file came to be: the shape that was read and the rules
/// it was read with. Recorded in the footer of every Parquet file and with
/// every ingest in the manifest, so the path from an archived source to a
/// lake file can be read back and checked.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Transform {
    /// Report type of the block's I row.
    pub report_type: String,
    /// Report subtype of the block's I row.
    pub report_subtype: String,
    /// Version the I row was published with.
    pub source_version: u16,
    /// Columns of the I row, in order.
    pub source_columns: Vec<String>,
    /// The month of the source, `YYYY-MM`, when it is known. Rules can be
    /// bounded by months, so it is part of what was read by which rules.
    #[serde(default)]
    pub source_period: Option<String>,
    /// Hash of the rules the shape was read with (`rules:<32 hex digits>`).
    /// Empty for an ingest recorded before rules were.
    pub rules: String,
    /// Where the table's definition came from: `registry`, `user`, `file`.
    pub schema_source: String,
    /// How the shape matched the definition: `exact`, `aliased`, `drift`,
    /// `derived`; with `+rule` when a hand-written rule routed a column.
    pub resolution: String,
}

/// What a writer records about the file it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    /// Table name as in the registry.
    pub table: String,
    /// Source file stem (lower-case), the first part of the file name.
    pub source_stem: String,
    /// Schema version of the block.
    pub schema_version: u16,
    /// Fingerprint of the block's I row.
    pub fingerprint: Fingerprint,
    /// Where the source came from.
    pub source_url: String,
    /// sha256 of the source file, hex.
    pub source_sha256: String,
    /// Zip entry inside the source.
    pub entry: String,
    /// Index of the time column in the schema, when the table has one.
    pub time_column: Option<usize>,
    /// The shape that was read and the rules it was read with.
    pub transform: Transform,
}

/// Handle to an open writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WriterId(pub u32);

/// A finished file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrittenFile {
    /// Final path.
    pub path: String,
    /// The partition.
    pub partition: Partition,
    /// Rows.
    pub rows: u64,
    /// Bytes on disk.
    pub bytes: u64,
    /// Minimum of the time column, UTC nanoseconds.
    pub min_time: Option<i64>,
    /// Maximum of the time column, UTC nanoseconds.
    pub max_time: Option<i64>,
    /// Whether rows arrived in non-decreasing time order.
    #[serde(default)]
    pub sorted: bool,
}

/// Writes batches of one table into one partition at a time.
pub trait LakeWriter: Send {
    /// Opens a file for a partition.
    fn open(
        &mut self,
        key: &PartitionKey,
        schema: SchemaRef,
        meta: &FileMeta,
    ) -> Result<WriterId, StoreError>;
    /// Appends a batch in the writer's schema.
    fn write(&mut self, id: WriterId, batch: &RecordBatch) -> Result<(), StoreError>;
    /// Finishes the file and makes it visible.
    fn close(&mut self, id: WriterId) -> Result<WrittenFile, StoreError>;
    /// Discards the file.
    fn abort(&mut self, id: WriterId);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_paths() {
        assert_eq!(
            Partition::Month {
                year: 2024,
                month: 1
            }
            .path_segment(),
            "year=2024/month=01"
        );
        assert_eq!(
            Partition::Snapshot {
                year: 2024,
                month: 12
            }
            .to_string(),
            "snapshot=202412"
        );
        assert!(
            Partition::Unknown
                .path_segment()
                .contains("__HIVE_DEFAULT_PARTITION__")
        );
        let j = serde_json::to_string(&Partition::Month {
            year: 2024,
            month: 1,
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Partition>(&j).unwrap(),
            Partition::Month {
                year: 2024,
                month: 1
            }
        );
    }
}
