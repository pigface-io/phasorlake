//! Writing the lake (layer L3): an [`IngestSession`] writes one source file
//! all or nothing, [`manifest`] records it, [`verify`] checks it and
//! [`iceberg`] builds the metadata. Production code writes through a session.

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
mod parquet_hive;
mod partition;
mod provenance;
mod quarantine;
pub mod session;
pub mod verify;
mod writer;

pub use error::StoreError;
pub use manifest::{Manifest, ManifestHandle, ManifestThread};
pub use parquet_hive::{ParquetHive, compression};
pub use partition::split_by_month;
pub use provenance::{Stamp, Times, lake_schema, stamped};
pub use session::{IngestSession, SessionOutput, Shape, ShapeOutput, Source};
pub use verify::{Problem, VerifyReport, verify_lake};
pub use writer::{FileMeta, LakeWriter, Partition, PartitionKey, Transform, WriterId, WrittenFile};
