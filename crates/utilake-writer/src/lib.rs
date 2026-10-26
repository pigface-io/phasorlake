//! Writing the lake: everything under the home that utilake writes.
//!
//! Layer L3. An [`IngestSession`] writes one source file's blocks into the
//! lake: which partition each row goes to, when a file is full, what
//! columns each file holds and what each row is stamped with are decided
//! here and nowhere else, and the files go into place together or not at
//! all. [`manifest`] is the SQLite record of every file, ingest, output and
//! diagnostic, owned by one thread and driven through an async handle so
//! the pipeline awaits it and SQLite sees one writer; it starts a run
//! (clearing what a crashed one left) and replaces what a file's earlier
//! read wrote. [`verify`] checks the lake against the manifest.
//! [`iceberg`] builds each table's Iceberg metadata from the manifest and
//! the census's history.
//!
//! The parts a session is built from ([`LakeWriter`], [`ParquetHive`],
//! [`split_by_month`], [`lake_schema`], [`stamped`]) are public for the
//! benchmarks and for fixtures that lay files out by hand; production code
//! writes through a session.

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
