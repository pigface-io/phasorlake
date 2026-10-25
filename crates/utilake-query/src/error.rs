//! Why a read cannot be planned, bound, scanned or written, each with its
//! stable `query.*` code.

use utilake_core::{Category, Diagnose, Diagnostic};
use utilake_iceberg::PlanError;

/// A read that failed before or while it ran. Warnings (a filter moved
/// after resolution, a row with a NaN rank, a file without a publish
/// time) are not errors: they go on the [`crate::Report`].
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// No table has the name.
    #[error("No table is named {0}.")]
    UnknownTable(String),
    /// A lake file without each row's source and publish time: it was
    /// written by a utilake from before rows named their sources.
    #[error(
        "The lake file {path} does not name each row's source; run `utilake ingest` to write the lake again."
    )]
    LakeFormat {
        /// The file.
        path: String,
    },
    /// A row names a source the manifest does not give its file.
    #[error("A row of {path} names the source {name}, which the manifest does not give the file.")]
    SourceUnknown {
        /// The file.
        path: String,
        /// The source the row names.
        name: String,
    },
    /// The table has no history (it was derived from a file), so its rows
    /// have no roles to resolve by.
    #[error("Table {0} has no history, so it can only be read raw.")]
    RawOnly(String),
    /// The planner could not project every file onto the target schema.
    #[error("The files of {table} cannot be read as one schema: {source}")]
    Plan {
        /// The table.
        table: String,
        /// The planner's reason.
        #[source]
        source: PlanError,
    },
    /// The registry's roles for the table do not hold against its columns.
    #[error("The roles of {table} do not hold: {}", problems.join("; "))]
    Roles {
        /// The table.
        table: String,
        /// Each problem the registry found.
        problems: Vec<String>,
    },
    /// A role names a column the target schema does not have: an `--as-of`
    /// schema from before the column.
    #[error("Schema {schema} of {table} lacks its {role} column {column}.")]
    RoleMissing {
        /// The table.
        table: String,
        /// The target schema.
        schema: u32,
        /// `entity`, `valid`, `order` or `cutoff`.
        role: &'static str,
        /// The column.
        column: String,
    },
    /// A role column is not a type the kernel orders or compares.
    #[error("The {role} column {column} of {table} is {ty}, which a read cannot order.")]
    RoleType {
        /// The table.
        table: String,
        /// The role.
        role: &'static str,
        /// The column.
        column: String,
        /// Its type.
        ty: String,
    },
    /// `--at` on a table whose time ends an interval: the interval
    /// containing an instant is not known without the interval's length.
    #[error(
        "Table {table} ends an interval at {column}, and the length of the interval is not known; read a range with --from and --to."
    )]
    AtOnIntervalTable {
        /// The table.
        table: String,
        /// The valid column.
        column: String,
    },
    /// `after` is not before `upto`.
    #[error("The window is empty: {after} is not before {upto}.")]
    WindowEmpty {
        /// The start, as given.
        after: String,
        /// The end, as given.
        upto: String,
    },
    /// No file of the table has rows in the window: the lake holds nothing
    /// of it there, and nothing was fetched.
    #[error("The lake holds no file of {table} over {window}; fetch it, or widen the window.")]
    WindowUncovered {
        /// The table.
        table: String,
        /// The window, as text.
        window: String,
    },
    /// A filter or projection names a column the target schema lacks.
    #[error("Table {table} has no column {column}.")]
    UnknownColumn {
        /// The table.
        table: String,
        /// The column.
        column: String,
    },
    /// A filter value does not read as the column's type.
    #[error("{value:?} is not a {ty}, the type of column {column}.")]
    BadFilterValue {
        /// The column.
        column: String,
        /// Its type.
        ty: String,
        /// The value.
        value: String,
    },
    /// Text that is not a time.
    #[error("{0:?} is not an RFC 3339 time, a date, or a local date and time.")]
    BadTime(String),
    /// A Parquet file could not be read.
    #[error("Reading {path} failed: {source}")]
    ReadParquet {
        /// The file.
        path: String,
        /// Why.
        #[source]
        source: parquet::errors::ParquetError,
    },
    /// An Arrow kernel failed on a file's rows.
    #[error("Reading {path} failed: {source}")]
    Arrow {
        /// The file, or the output.
        path: String,
        /// Why.
        #[source]
        source: arrow_schema::ArrowError,
    },
    /// The output could not be written.
    #[error("Writing {path} failed: {source}")]
    Write {
        /// The file.
        path: String,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// The output could not be encoded.
    #[error("Writing {path} failed: {source}")]
    WriteParquet {
        /// The file.
        path: String,
        /// Why.
        #[source]
        source: parquet::errors::ParquetError,
    },
}

impl QueryError {
    /// A Parquet file that could not be read.
    pub fn parquet(path: &str, source: parquet::errors::ParquetError) -> Self {
        Self::ReadParquet {
            path: path.to_owned(),
            source,
        }
    }

    /// An Arrow kernel that failed; `path` is the file or the table.
    pub fn arrow(path: &str, source: arrow_schema::ArrowError) -> Self {
        Self::Arrow {
            path: path.to_owned(),
            source,
        }
    }

    /// An output file that could not be written.
    pub fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Write {
            path: path.display().to_string(),
            source,
        }
    }

    /// An output file that could not be encoded.
    pub fn write_parquet(path: &std::path::Path, source: parquet::errors::ParquetError) -> Self {
        Self::WriteParquet {
            path: path.display().to_string(),
            source,
        }
    }
}

impl Diagnose for QueryError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        let e = |code: &'static str| Diagnostic::error(code, Category::Query, msg.clone());
        match self {
            Self::UnknownTable(t) => e("query.unknown_table").with_table(t.clone()),
            Self::LakeFormat { path } => e("query.lake_format").with_file(path.clone()),
            Self::SourceUnknown { path, name } => e("query.source_unknown")
                .with_file(path.clone())
                .with_sample(name.as_bytes()),
            Self::RawOnly(t) => e("query.raw_only").with_table(t.clone()),
            Self::Plan { table, source } => e("query.plan_failed")
                .with_table(table.clone())
                .caused_by(source),
            Self::Roles { table, .. } => e("query.roles_invalid").with_table(table.clone()),
            Self::RoleMissing { table, column, .. } => e("query.role_missing")
                .with_table(table.clone())
                .with_column(column.clone()),
            Self::RoleType {
                table, column, ty, ..
            } => e("query.role_type")
                .with_table(table.clone())
                .with_column(column.clone())
                .expected_observed(
                    "timestamp, date, long, double, decimal or string",
                    ty.clone(),
                ),
            Self::AtOnIntervalTable { table, column } => e("query.at_on_interval_table")
                .with_table(table.clone())
                .with_column(column.clone()),
            Self::WindowEmpty { after, upto } => e("query.window_empty")
                .expected_observed("after < upto", format!("{after} .. {upto}")),
            Self::WindowUncovered { table, window } => e("query.window_uncovered")
                .with_table(table.clone())
                .expected_observed("a file in the window", window.clone()),
            Self::UnknownColumn { table, column } => e("query.unknown_column")
                .with_table(table.clone())
                .with_column(column.clone()),
            Self::BadFilterValue { column, ty, value } => e("query.bad_filter_value")
                .with_column(column.clone())
                .expected_observed(ty.clone(), value.clone()),
            Self::BadTime(t) => e("query.bad_time").with_sample(t.as_bytes()),
            Self::ReadParquet { path, source } => e("query.read_failed")
                .with_file(path.clone())
                .caused_by(source),
            Self::Arrow { path, source } => e("query.read_failed")
                .with_file(path.clone())
                .caused_by(source),
            Self::Write { path, source } => e("query.write_failed")
                .with_file(path.clone())
                .caused_by(source),
            Self::WriteParquet { path, source } => e("query.write_failed")
                .with_file(path.clone())
                .caused_by(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_query_code() {
        let io = std::io::Error::other("gone");
        let cases: Vec<(QueryError, &str)> = vec![
            (QueryError::UnknownTable("T".into()), "query.unknown_table"),
            (QueryError::RawOnly("T".into()), "query.raw_only"),
            (
                QueryError::Plan {
                    table: "T".into(),
                    source: PlanError::UnknownSchema(9),
                },
                "query.plan_failed",
            ),
            (
                QueryError::Roles {
                    table: "T".into(),
                    problems: vec!["x".into()],
                },
                "query.roles_invalid",
            ),
            (
                QueryError::RoleMissing {
                    table: "T".into(),
                    schema: 0,
                    role: "entity",
                    column: "DUID".into(),
                },
                "query.role_missing",
            ),
            (
                QueryError::RoleType {
                    table: "T".into(),
                    role: "order",
                    column: "X".into(),
                    ty: "boolean".into(),
                },
                "query.role_type",
            ),
            (
                QueryError::AtOnIntervalTable {
                    table: "T".into(),
                    column: "SETTLEMENTDATE".into(),
                },
                "query.at_on_interval_table",
            ),
            (
                QueryError::WindowEmpty {
                    after: "b".into(),
                    upto: "a".into(),
                },
                "query.window_empty",
            ),
            (
                QueryError::WindowUncovered {
                    table: "T".into(),
                    window: "{}".into(),
                },
                "query.window_uncovered",
            ),
            (
                QueryError::UnknownColumn {
                    table: "T".into(),
                    column: "X".into(),
                },
                "query.unknown_column",
            ),
            (
                QueryError::BadFilterValue {
                    column: "X".into(),
                    ty: "double".into(),
                    value: "y".into(),
                },
                "query.bad_filter_value",
            ),
            (QueryError::BadTime("soon".into()), "query.bad_time"),
            (
                QueryError::ReadParquet {
                    path: "p".into(),
                    source: parquet::errors::ParquetError::General("x".into()),
                },
                "query.read_failed",
            ),
            (
                QueryError::Arrow {
                    path: "p".into(),
                    source: arrow_schema::ArrowError::ComputeError("x".into()),
                },
                "query.read_failed",
            ),
            (
                QueryError::Write {
                    path: "p".into(),
                    source: io,
                },
                "query.write_failed",
            ),
            (
                QueryError::WriteParquet {
                    path: "p".into(),
                    source: parquet::errors::ParquetError::General("x".into()),
                },
                "query.write_failed",
            ),
        ];
        for (err, code) in cases {
            let d = err.diagnostic();
            assert_eq!(d.code, code, "{err}");
            assert_eq!(d.category, Category::Query);
            assert!(d.render().contains(&err.to_string()));
            assert_eq!(err.code(), code);
        }
        let p = std::path::Path::new("out.parquet");
        let general = || parquet::errors::ParquetError::General("x".into());
        assert_eq!(
            QueryError::parquet("f", general()).code(),
            "query.read_failed"
        );
        assert_eq!(
            QueryError::arrow("f", arrow_schema::ArrowError::ComputeError("x".into())).code(),
            "query.read_failed"
        );
        assert_eq!(
            QueryError::io(p, std::io::Error::other("x")).code(),
            "query.write_failed"
        );
        assert_eq!(
            QueryError::write_parquet(p, general()).code(),
            "query.write_failed"
        );
    }
}
