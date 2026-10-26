//! The schema registry: every table utilake knows, and the resolution of an
//! MMS I row to a conformance plan.
//!
//! The `schemas/` tree, whose NEM tables are the ones utilake-schemas
//! publishes, is validated and packed by `build.rs`;
//! [`Registry::load_embedded`] decodes it once per process. Each table is
//! its columns (exact types and stable ids) and its history as an Iceberg
//! table: the schemas its files were written under and the runs of each.
//! A block resolves to the schema `History::resolve` names for it: it
//! conforms, deviates when no run fits (strict mode refuses it) or, for a
//! report pair no table claims, is derived from the file. See [`resolve`].

pub mod arrow;
pub mod catalog;
pub mod error;
pub mod file_model;
pub mod plan;
pub mod registry;
pub mod resolve;
pub use utilake_mms_decode::shape;

pub use crate::file_model::{
    Column, Folder, FolderBlock, LakeType, LoadMode, Market, Run, Side, Table, TableSchema,
};
pub use catalog::RegistryCatalog;
pub use error::ResolveError;
pub use plan::{Cast, ColumnRoute, ConformancePlan, Origin, PLAN_FORMAT, PlanField, PlanHash};
pub use registry::{Registry, SchemaSource, TableId};
pub use resolve::{Mode, ResolveOptions, Resolved, Status};
pub use utilake_mms_decode::shape::{CanonicalShape, SHAPE_FORMAT, ShapeId};
