//! The schema model: each table's columns and its history as an Iceberg
//! table, and the rules a table must satisfy.
//!
//! Pure and synchronous. It knows nothing of Arrow, so the registry's
//! `build.rs` compiles this module on its own to validate `schemas/` at
//! compile time and emit a compact blob; the rest of the crate adds the
//! Arrow mapping.
//!
//! A NEM table is the `table.json` utilake-schemas publishes, pulled into
//! `schemas/nem/pulled/{TABLE}/` (see [`mod@doc`]): its columns with exact
//! types and stable ids, and the schemas and runs of its history. A user's
//! own table may say the same by hand in a `table.toml` ([`mod@file`]).
//! [`validate`] holds the invariants and [`blob`] the packed set.

pub mod blob;
pub mod doc;
pub mod error;
pub mod file;
pub mod model;
pub mod temporal;
pub mod validate;

pub use error::SchemaFileError;
pub use model::{
    Column, ColumnId, LakeType, LoadMode, Market, MarketDef, Run, Table, TableSchema,
    TimestampFormat,
};
pub use temporal::{RoleOrigin, Temporal, ValidAxis};
