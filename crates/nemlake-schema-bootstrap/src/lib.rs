//! Regenerates `schemas/nem/mms/*.toml` from three sources of truth:
//!
//! 1. Oracle SQL*Loader control files (`CTL/*.ctl` in each monthly archive):
//!    table name, column order and loader types. See [`ctl`].
//! 2. The MMS Data Model report (read into JSON by [`dmreport`]): Oracle
//!    types with precision, primary keys, descriptions, visibility, package.
//!    See [`datamodel`].
//! 3. The I rows of real files: the `(report_type, report_subtype, version)`
//!    each table is published under and the exact column list of each version.
//!    See [`sample`].
//!
//! [`remote`] gathers the inputs from nemweb: every CTL of a month, and the
//! head of every archive zip (a Range GET of its first bytes, inflated) so the
//! I rows and a few D rows of each table can be committed as small fixtures.
//!
//! [`merge`] combines them with a fixed precedence and preserves anything a
//! human marked `curated = true` in the existing file. The output is
//! deterministic so CI can regenerate and diff.

/// The control-file parser, which lives with the schema model so the
/// command-line tool can read control files too.
pub use nemlake_schema_model::ctl;
pub mod datamodel;
pub mod dmreport;
pub mod merge;
pub mod remote;
pub mod sample;
