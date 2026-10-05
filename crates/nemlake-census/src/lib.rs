//! Regenerates `schemas/nem/mms/*.toml` from three sources of truth:
//!
//! 1. Oracle SQL*Loader control files (`CTL/*.ctl` in each monthly archive):
//!    table name, column order and loader types. See [`nemlake_registry::file_model::ctl`].
//! 2. The MMS Data Model report (read into JSON by [`dmreport`]): Oracle
//!    types with precision, primary keys, descriptions, visibility, package.
//!    See [`datamodel`].
//! 3. The I rows of real files: the `(report_type, report_subtype, version)`
//!    each table is published under and the exact column list of each version.
//!    See [`sample`].
//!
//! [`fixtures`] cuts those inputs from a month of the local archive: every
//! control file, the README, and the head of every data file (each block's
//! I row and a few D rows), committed as small fixtures.
//!
//! [`census`] records every block of every archived file of every month,
//! as metadata under `schemas/nem/census/` and `schemas/nem/shapes/`.
//!
//! [`merge`] combines them with a fixed precedence and preserves anything a
//! human marked `curated = true` in the existing file. The output is
//! deterministic so CI can regenerate and diff.

pub mod census;
pub mod datamodel;
pub mod dmreport;
pub mod fixtures;
pub mod merge;
pub mod sample;
pub mod standing;
