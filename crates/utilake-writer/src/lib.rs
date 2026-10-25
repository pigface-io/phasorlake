//! Writing the lake.
//!
//! Layer L3. [`LakeWriter`] is the port the pipeline writes through;
//! [`ParquetHive`] is the production adapter (Hive-partitioned Parquet with
//! atomic rename). [`partition`] splits a batch by the market-local month of
//! its time column. [`manifest`] is the SQLite record of every file, ingest,
//! output and diagnostic, owned by one thread and driven through an async
//! handle so the pipeline awaits it and SQLite sees one writer. [`verify`]
//! checks the lake against the manifest. [`iceberg`] builds each table's
//! Iceberg metadata from the manifest and the census's history.

/// Opens a zone of `zonetrace` until the end of the block, with the
/// feature `zones`; nothing without it.
#[allow(unused_macros)]
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = zonetrace::enter(&crate::zones::$z);
    };
}

#[cfg(feature = "zones")]
pub mod zones;

pub mod error;
pub mod iceberg;
pub mod manifest;
pub mod parquet_hive;
pub mod partition;
pub mod verify;
pub mod writer;

pub use error::StoreError;
pub use manifest::{Manifest, ManifestHandle, ManifestThread};
pub use parquet_hive::ParquetHive;
pub use verify::{Problem, VerifyReport, verify_lake};
pub use writer::{FileMeta, LakeWriter, Partition, PartitionKey, Transform, WriterId, WrittenFile};
