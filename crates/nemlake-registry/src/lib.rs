//! The schema registry: every table nemlake knows, and the resolution of an
//! MMS I row to a conformance plan.
//!
//! The committed `schemas/` tree is validated and packed by `build.rs`;
//! [`Registry::load_embedded`] decodes it once per process. Each table is
//! our schema (columns with exact types and stable ids) and its increments,
//! the file shapes AEMO's control files set. A block of a month resolves
//! against the increment in force then: it conforms, deviates (strict mode
//! refuses it) or, for a report pair no table claims, is derived from the
//! file. See [`resolve`].

pub mod arrow;
pub mod error;
pub mod file_model;
pub mod plan;
pub mod registry;
pub mod resolve;

pub use crate::file_model::{Column, Increment, LakeType, LoadMode, Market, Table};
pub use error::ResolveError;
pub use plan::{Cast, ColumnRoute, ConformancePlan, Origin, PLAN_FORMAT, PlanField, PlanHash};
pub use registry::{Registry, SchemaSource, TableId};
pub use resolve::{Mode, ResolveOptions, Resolved, Status};
