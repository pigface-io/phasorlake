//! Our schema per table, in increments AEMO's control files set.
//!
//! A table is `schemas/{market}/{TABLE}/table.toml` (see [`mod@file`]): its
//! lake columns with exact types and stable ids, which persist, and its
//! increments (`TABLE@n`), each the file shape of one AEMO control file
//! kept byte for byte under `{TABLE}/ctl/`. [`validate`] holds the
//! invariants, [`compat`] what the lake survives, and [`blob`] the packed
//! set.

pub mod blob;
pub mod compat;
pub mod file;
pub mod model;
pub mod validate;
