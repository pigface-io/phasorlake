//! The registry: every market's tables phasor knows, the resolution of a
//! block's header to its plan ([`resolve`]), and the tables as a kit's [`catalog`].
//! `build.rs` packs `schemas/`; [`Registry::load_embedded`] decodes it once.

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
