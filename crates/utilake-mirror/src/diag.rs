//! The one shape every error and warning of utilake-mirror is reported
//! as: the family's diagnostic, with a stable code, a category, a severity,
//! a full-sentence message and the context the layer that raised it knew.

pub use utilake_core::diag::{Category, Code, Diagnose, Diagnostic, MIRROR, Sample, Severity};
