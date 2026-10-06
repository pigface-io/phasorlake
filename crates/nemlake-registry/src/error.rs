//! Errors of resolving an I row against the registry.

use nemlake_core::{Category, Diagnose, Diagnostic};

/// Why an I row could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// No table claims the report pair.
    #[error(
        "No table in the registry claims report pair ({report_type}, {report_subtype}); rerun with --permissive to keep its rows as text."
    )]
    UnknownTable {
        /// Report type.
        report_type: String,
        /// Report subtype.
        report_subtype: String,
    },
    /// The block's columns are not its increment's, and strict mode.
    #[error(
        "Table {table}@{increment} lists columns {expected} but the I row has {observed}; missing {missing:?}, not listed {extra:?}{}. If the file is right, fix the increment's columns in its table.toml; --permissive reads it as it is.",
        if *reordered { ", in another order" } else { "" }
    )]
    Deviates {
        /// Table.
        table: String,
        /// The increment in force.
        increment: u16,
        /// The increment's columns, comma separated.
        expected: String,
        /// The I row's columns, comma separated.
        observed: String,
        /// Increment columns the I row lacks.
        missing: Vec<String>,
        /// I-row columns the increment does not list.
        extra: Vec<String>,
        /// The columns both have come in another order.
        reordered: bool,
    },
    /// A primary-key column is absent from the I row.
    #[error(
        "Table {table}@{increment}: the I row lacks primary-key column {column}; rows could not be deduplicated, so the block is refused."
    )]
    MissingKey {
        /// Table.
        table: String,
        /// The increment in force.
        increment: u16,
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
            Self::Deviates {
                table,
                increment,
                expected,
                observed,
                ..
            } => Diagnostic::error("schema.deviates", Category::Schema, msg)
                .with_table(table.clone())
                .with_version(*increment)
                .expected_observed(expected.clone(), observed.clone()),
            Self::MissingKey {
                table,
                increment,
                column,
            } => Diagnostic::error("schema.missing_key", Category::Schema, msg)
                .with_table(table.clone())
                .with_version(*increment)
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
        let e = ResolveError::Deviates {
            table: "T".into(),
            increment: 2,
            expected: "A,B".into(),
            observed: "B,A,X".into(),
            missing: vec![],
            extra: vec!["X".into()],
            reordered: true,
        };
        let d = e.diagnostic();
        assert_eq!(d.code, "schema.deviates");
        assert_eq!(d.table.as_deref(), Some("T"));
        assert_eq!(d.version, Some(2));
        assert_eq!(d.observed.as_deref(), Some("B,A,X"));
        assert!(d.message.contains("T@2") && d.message.contains("another order"));
        assert_eq!(
            ResolveError::UnknownTable {
                report_type: "A".into(),
                report_subtype: "B".into()
            }
            .diagnostic()
            .code,
            "schema.unknown_table"
        );
        let d = ResolveError::MissingKey {
            table: "T".into(),
            increment: 1,
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
