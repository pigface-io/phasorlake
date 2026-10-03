//! The schema registry: every table nemlake knows, and the resolution of an
//! MMS I row to a typed projection.
//!
//! The committed `schemas/` tree is validated and packed by `build.rs`;
//! [`Registry::load_embedded`] decodes it once per process and precomputes a
//! [`Projection`] (Arrow schema plus per-field casts) for every declared
//! version, so resolving an I row on the hot path is a hash lookup and an
//! `Arc` clone.
//!
//! Resolution outcomes, in order of preference: an exact fingerprint match;
//! an unknown version whose columns match a known one (aliased); a known
//! version whose columns differ (drift, compatible or not); an unknown table
//! (inferred into a draft, or refused in strict mode). See [`resolve`].

pub mod arrow;
pub mod error;
pub mod infer;
pub mod registry;
pub mod resolve;

pub use error::ResolveError;
pub use nemlake_schema_model::{ColumnDef, ColumnType, LoadMode, Market, SchemaVersion, TableDef};
pub use registry::{LineageEntry, Registry, TableId, TypeOptions};
pub use resolve::{Cast, ColumnCast, Mode, Projection, ResolveOptions, Resolved, Status};
