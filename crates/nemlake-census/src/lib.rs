//! Tooling that keeps `schemas/` in step with what AEMO publishes.
//!
//! [`census`] records every block of every archived file of every month,
//! as metadata under `schemas/nem/census/` and `schemas/nem/shapes/`.
//! `nemlake-census sync` starts a table from a month's control file, the
//! data model ([`datamodel`], read from AEMO's report by [`dmreport`]) and
//! the README's list of standing-data tables ([`standing`]), and appends an
//! increment when a month's control file lays a table out anew.
//!
//! [`pdr`] reports where AEMO's PDR control files, kept for reference,
//! differ from the control files of our increments.
//!
//! [`fixtures`] cuts the test fixtures of a month from the local archive:
//! every control file, the README, and the head of every data file.

pub mod census;
pub mod datamodel;
pub mod dmreport;
pub mod fixtures;
pub mod pdr;
pub mod standing;
pub mod sync;
