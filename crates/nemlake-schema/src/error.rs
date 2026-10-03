//! Errors of resolving an I row against the registry.

use nemlake_core::{Category, Diagnose, Diagnostic};

/// Why an I row could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// No table claims the report pair.
    #[error(
        "No table in the registry claims report pair ({report_type}, {report_subtype}); rerun with --permissive to infer a draft schema."
    )]
    UnknownTable {
        /// Report type.
        report_type: String,
        /// Report subtype.
        report_subtype: String,
    },
    /// Known version, different columns, and strict mode.
    #[error(
        "Table {table} version {version} declares {declared} columns but the I row has {observed}; missing {missing:?}, unexpected {extra:?}."
    )]
    VersionDrift {
        /// Table.
        table: String,
        /// Version.
        version: u16,
        /// Declared count.
        declared: usize,
        /// Observed count.
        observed: usize,
        /// Declared columns the I row lacks.
        missing: Vec<String>,
        /// I-row columns the registry does not know.
        extra: Vec<String>,
    },
    /// Unknown version, strict mode.
    #[error(
        "Table {table} has no declared version {version}; its nearest is {nearest}. Rerun with --permissive to read it as drift."
    )]
    UnknownVersion {
        /// Table.
        table: String,
        /// Observed version.
        version: u16,
        /// Nearest declared version.
        nearest: u16,
    },
    /// A primary-key column is absent from the I row.
    #[error(
        "Table {table} version {version}: the I row lacks primary-key column {column}; rows could not be deduplicated, so the block is refused."
    )]
    MissingKey {
        /// Table.
        table: String,
        /// Version.
        version: u16,
        /// The missing key column.
        column: String,
    },
    /// The I row has no columns after the version field.
    #[error("The I row for ({report_type}, {report_subtype}) version {version} names no columns.")]
    NoColumns {
        /// Report type.
        report_type: String,
        /// Report subtype.
        report_subtype: String,
        /// Version.
        version: u16,
    },
}

impl Diagnose for ResolveError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::UnknownTable { .. } => {
                Diagnostic::error("schema.unknown_table", Category::Schema, msg)
            }
            Self::VersionDrift {
                table,
                version,
                declared,
                observed,
                missing,
                extra,
            } => Diagnostic::error("schema.version_drift", Category::Schema, msg)
                .with_table(table.clone())
                .with_version(*version)
                .expected_observed(
                    format!("{declared} columns"),
                    format!("{observed} columns (missing {missing:?}, unexpected {extra:?})"),
                ),
            Self::UnknownVersion {
                table,
                version,
                nearest,
            } => Diagnostic::error("schema.unknown_version", Category::Schema, msg)
                .with_table(table.clone())
                .with_version(*version)
                .expected_observed(
                    format!("a declared version such as {nearest}"),
                    version.to_string(),
                ),
            Self::MissingKey {
                table,
                version,
                column,
            } => Diagnostic::error("schema.missing_key", Category::Schema, msg)
                .with_table(table.clone())
                .with_version(*version)
                .with_column(column.clone()),
            Self::NoColumns { version, .. } => {
                Diagnostic::error("schema.no_columns", Category::Schema, msg).with_version(*version)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_context() {
        let e = ResolveError::VersionDrift {
            table: "T".into(),
            version: 2,
            declared: 3,
            observed: 4,
            missing: vec![],
            extra: vec!["X".into()],
        };
        let d = e.diagnostic();
        assert_eq!(d.code, "schema.version_drift");
        assert_eq!(d.table.as_deref(), Some("T"));
        assert_eq!(d.version, Some(2));
        assert!(d.observed.as_deref().unwrap().contains("X"));
        assert_eq!(
            ResolveError::UnknownTable {
                report_type: "A".into(),
                report_subtype: "B".into()
            }
            .diagnostic()
            .code,
            "schema.unknown_table"
        );
        assert_eq!(
            ResolveError::UnknownVersion {
                table: "T".into(),
                version: 9,
                nearest: 4
            }
            .diagnostic()
            .code,
            "schema.unknown_version"
        );
        let d = ResolveError::MissingKey {
            table: "T".into(),
            version: 1,
            column: "K".into(),
        }
        .diagnostic();
        assert_eq!(d.code, "schema.missing_key");
        assert_eq!(d.column.as_deref(), Some("K"));
        assert_eq!(
            ResolveError::NoColumns {
                report_type: "A".into(),
                report_subtype: "B".into(),
                version: 1
            }
            .diagnostic()
            .code,
            "schema.no_columns"
        );
    }
}
