//! The schema model: our schema per table, in increments AEMO's control
//! files set, and the rules a committed schema must satisfy.
//!
//! Pure and synchronous. It knows nothing of Arrow, so the registry's
//! `build.rs` compiles this module on its own to validate `schemas/` at
//! compile time and emit a compact blob; the rest of the crate adds the
//! Arrow mapping.
//!
//! A table is `schemas/{market}/{TABLE}/table.toml` (see [`mod@file`]): its
//! lake columns with exact types and stable ids, which persist, and its
//! increments (`TABLE@n`), each the file shape of one AEMO control file
//! kept byte for byte under `{TABLE}/ctl/`. [`validate`] holds the
//! invariants, [`compat`] what the lake survives, [`iceberg`] the table as
//! an Iceberg schema, [`blob`] the packed set,
//! and [`ctl`] the parser of AEMO's SQL*Loader control files.

pub mod blob;
pub mod compat;
pub mod ctl;
pub mod error;
pub mod file;
pub mod iceberg;
pub mod model;
pub mod shapes;
pub mod temporal;
pub mod validate;

pub use error::SchemaFileError;
pub use model::{
    Column, ColumnId, Increment, LakeType, LoadMode, Market, MarketDef, Table, TimestampFormat,
};
pub use temporal::{RoleOrigin, Temporal, ValidAxis};
