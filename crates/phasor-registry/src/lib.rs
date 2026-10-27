//! The registry: every market's tables phasor knows, the resolution of a
//! block's header to the plan its rows are read by, and the tables as a
//! kit's [`catalog`].
//!
//! The `schemas/` tree, whose NEM tables are the ones phasor-schemas
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
pub use phasor_mms_decode::shape;

pub use crate::file_model::{
    Column, Folder, FolderBlock, LakeType, LoadMode, Market, Run, Side, Table, TableSchema,
};
pub use catalog::RegistryCatalog;
pub use error::ResolveError;
pub use phasor_mms_decode::shape::{CanonicalShape, SHAPE_FORMAT, ShapeId};
pub use plan::{Cast, ColumnRoute, ConformancePlan, Origin, PLAN_FORMAT, PlanField, PlanHash};
pub use registry::{Registry, SchemaSource, TableId};
pub use resolve::{Mode, ResolveOptions, Resolved, Status};
