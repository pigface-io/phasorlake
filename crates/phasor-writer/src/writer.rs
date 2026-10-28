//! The port the pipeline writes through.

use std::fmt;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use phasor_core::Fingerprint;
pub use phasor_kit::Transform;
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
    /// A snapshot table's full copy from a file of one day: `snapshot=YYYYMMDD`.
    SnapshotDay {
        /// Year.
        year: i32,
        /// Month 1..=12.
        month: u8,
        /// Day 1..=31.
        day: u8,
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
            Self::SnapshotDay { year, month, day } => {
                format!("snapshot={year:04}{month:02}{day:02}")
            }
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

/// What a writer records about the file it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    /// Table name as in the registry.
    pub table: String,
    /// The source file's stem (lower-case), the first part of the file name.
    pub source_stem: String,
    /// Schema version of the block.
    pub schema_version: u16,
    /// Fingerprint of the block's I row.
    pub fingerprint: Fingerprint,
    /// Where the source came from.
    pub source_url: String,
    /// sha256 of the source file, hex.
    pub source_sha256: String,
    /// Index of the time column in the schema, when the table has one.
    pub time_column: Option<usize>,
    /// Which file of this source, shape and partition this is, from 1.
    pub part: u32,
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
    /// Finishes the file; it is visible once committed.
    fn close(&mut self, id: WriterId) -> Result<WrittenFile, StoreError>;
    /// Makes every file closed since the last commit durable and visible; drop removes them.
    fn commit(&mut self) -> Result<(), StoreError>;
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
        assert_eq!(
            Partition::SnapshotDay {
                year: 2026,
                month: 10,
                day: 7
            }
            .to_string(),
            "snapshot=20261007"
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
