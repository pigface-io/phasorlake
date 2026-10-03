//! The schema model: what a table is, how its versions relate, and the rules
//! a committed schema file must satisfy.
//!
//! Layer L1, pure and synchronous. It knows nothing of Arrow, so the
//! registry's `build.rs` can use it to validate `schemas/` at compile time
//! and emit a compact blob; the registry crate adds the Arrow mapping.
//!
//! On disk a table is one TOML file (see [`mod@file`]) with every column declared
//! once and every version as an ordered list of column names. In memory it is
//! a [`TableDef`]. [`validate`] holds the invariants, [`compat`] the evolution
//! rules, and [`blob`] the postcard encoding of a whole set of tables.

pub mod blob;
pub mod compat;
pub mod error;
pub mod file;
pub mod model;
pub mod validate;

pub use error::SchemaFileError;
pub use model::{
    ColumnDef, ColumnId, ColumnType, Dedupe, LoadMode, Market, MarketDef, Provenance, RuleColumn,
    RuleDef, SchemaVersion, SourceKind, TableDef, TimestampFormat, VariantDef, VersionDef,
    Visibility,
};
