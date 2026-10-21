//! The Iceberg contract of the lake (format version 3, `docs/iceberg.md`).
//!
//! Every table the lake holds is an Iceberg schema: fields with ids that are
//! never reused, types from the set below, and identifier fields for the key.
//! A table's history is a sequence of schema updates, each one legal in
//! Iceberg or refused here with the reason. The type of a column is chosen
//! once over everything observed of it and AEMO's declaration ([`choose`]), so that later data
//! never needs more than a legal promotion.
//!
//! Std, serde, SHA-256 and the core diagnostics only; no I/O.

mod evidence;
mod partition;
mod plan;
mod schema;
pub mod table;
mod types;
mod update;

pub use evidence::{Choice, Declared, Evidence, Kind, choose};
pub use partition::{PartitionField, PartitionSpec};
pub use plan::{Column, DataFile, PlanError, Read, Target, duckdb_sql, plan};
pub use schema::{Field, Schema};
pub use types::IcebergType;
pub use update::{Update, UpdateError};
