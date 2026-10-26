//! Errors of writing Parquet and recording the manifest.

use utilake_core::{Category, Diagnose, Diagnostic};

/// Why a lake write failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Creating a directory or file failed.
    #[error("The lake path {path} could not be created or written.")]
    Io {
        /// The path.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// The Parquet writer failed.
    #[error("Writing Parquet to {path} failed: {message}")]
    Parquet {
        /// The path.
        path: String,
        /// Parquet's message.
        message: String,
    },
    /// Renaming the finished file into place failed.
    #[error("The finished file {from} could not be moved to {to}.")]
    Rename {
        /// Temp path.
        from: String,
        /// Final path.
        to: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A batch did not match the writer's schema.
    #[error("A batch for {path} has schema {observed} but the writer was opened with {expected}.")]
    SchemaMismatch {
        /// The path.
        path: String,
        /// Expected.
        expected: String,
        /// Observed.
        observed: String,
    },
    /// An unknown writer id.
    #[error("Writer {0} is not open.")]
    NoSuchWriter(u32),
    /// An interval of a table could not be read out of the lake's files.
    #[error("The interval of {table} could not be extracted: {message}")]
    Extract {
        /// The table.
        table: String,
        /// What went wrong.
        message: String,
    },
    /// The time column of a batch is not a timestamp with a zone.
    #[error(
        "Column {column} is the partition column but holds {observed}, not a timestamp with a zone."
    )]
    PartitionColumnNotTimestamp {
        /// Column.
        column: String,
        /// Its Arrow type.
        observed: String,
    },
    /// A schema offered to the writer has a timestamp without a zone, or
    /// in a unit other than nanoseconds. The lake holds one kind of
    /// timestamp: nanoseconds since the epoch in UTC, with the market
    /// offset in the type.
    #[error(
        "Column {column} has type {observed}; the lake writes a timestamp only as nanoseconds with its market offset."
    )]
    NaiveTimestamp {
        /// Column.
        column: String,
        /// Its Arrow type.
        observed: String,
    },
    /// A lake file was opened without each row's source and publish time.
    #[error(
        "A lake file must end with the columns _ul_source, _ul_server_last_modified, _ul_zip_entry_modified and _ul_filename_timestamp; the batch has {observed}."
    )]
    NoProvenance {
        /// The batch's columns.
        observed: String,
    },
    /// A batch could not be arranged into its file's columns.
    #[error("A batch could not be arranged into its file's columns: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    /// A row the decoder refused could not be kept.
    #[error("The quarantine file {} could not be written.", path.display())]
    Quarantine {
        /// The file.
        path: std::path::PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

impl Diagnose for StoreError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Io { path, source } => Diagnostic::error("store.io", Category::Store, msg)
                .with_file(path.clone())
                .caused_by(source),
            Self::Parquet { path, .. } => {
                Diagnostic::error("store.parquet_write", Category::Store, msg)
                    .with_file(path.clone())
            }
            Self::Rename { to, source, .. } => {
                Diagnostic::error("store.rename", Category::Store, msg)
                    .with_file(to.clone())
                    .caused_by(source)
            }
            Self::SchemaMismatch {
                path,
                expected,
                observed,
            } => Diagnostic::error("store.schema_mismatch", Category::Store, msg)
                .with_file(path.clone())
                .expected_observed(expected.clone(), observed.clone()),
            Self::Extract { table, .. } => {
                Diagnostic::error("store.extract", Category::Store, msg).with_table(table.clone())
            }
            Self::NoSuchWriter(_) => {
                Diagnostic::error("internal.no_such_writer", Category::Internal, msg)
            }
            Self::PartitionColumnNotTimestamp { column, observed } => {
                Diagnostic::error("store.partition_column_not_timestamp", Category::Store, msg)
                    .with_column(column.clone())
                    .expected_observed("a timestamp with a zone", observed.clone())
            }
            Self::NaiveTimestamp { column, observed } => {
                Diagnostic::error("store.naive_timestamp", Category::Store, msg)
                    .with_column(column.clone())
                    .expected_observed("Timestamp(Microsecond, Some(offset))", observed.clone())
            }
            Self::NoProvenance { observed } => {
                Diagnostic::error("store.no_provenance", Category::Store, msg)
                    .expected_observed("…,_ul_source,_ul_server_last_modified,_ul_zip_entry_modified,_ul_filename_timestamp", observed.clone())
            }
            Self::Arrow(source) => {
                Diagnostic::error("store.arrow", Category::Store, msg).caused_by(source)
            }
            Self::Quarantine { path, source } => {
                Diagnostic::error("store.quarantine_write", Category::Store, msg)
                    .with_file(path.display().to_string())
                    .caused_by(source)
            }
        }
    }
}

/// Why the manifest failed.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// Open or query failed.
    #[error("The manifest {path} failed: {message}")]
    Sqlite {
        /// The database.
        path: String,
        /// SQLite's message.
        message: String,
    },
    /// A migration failed.
    #[error("The manifest {path} is at migration {at} and migration {failed} failed: {message}")]
    Migration {
        /// The database.
        path: String,
        /// Applied version.
        at: u32,
        /// The migration that failed.
        failed: String,
        /// SQLite's message.
        message: String,
    },
    /// The actor is gone.
    #[error("The manifest thread has stopped; the run cannot record its results.")]
    Closed,
    /// A row referenced something that does not exist.
    #[error("The manifest has no {kind} with id {id}.")]
    NotFound {
        /// What.
        kind: &'static str,
        /// Id.
        id: i64,
    },
}

impl Diagnose for ManifestError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Sqlite { path, .. } => {
                Diagnostic::error("manifest.sqlite", Category::Manifest, msg)
                    .with_file(path.clone())
            }
            Self::Migration { path, .. } => {
                Diagnostic::error("manifest.migration_failed", Category::Manifest, msg)
                    .with_file(path.clone())
            }
            Self::Closed => Diagnostic::error("manifest.closed", Category::Manifest, msg),
            Self::NotFound { .. } => {
                Diagnostic::error("manifest.not_found", Category::Manifest, msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(
            StoreError::NoSuchWriter(3).diagnostic().code,
            "internal.no_such_writer"
        );
        assert_eq!(
            StoreError::Parquet {
                path: "p".into(),
                message: "m".into()
            }
            .diagnostic()
            .file
            .as_deref(),
            Some("p")
        );
        let e = StoreError::Rename {
            from: "a".into(),
            to: "b".into(),
            source: std::io::Error::other("x"),
        };
        assert_eq!(e.diagnostic().code, "store.rename");
        let e = StoreError::from(arrow_schema::ArrowError::SchemaError("s".into()));
        assert_eq!(e.diagnostic().code, "store.arrow");
        let e = StoreError::Quarantine {
            path: "q/t.jsonl".into(),
            source: std::io::Error::other("full"),
        };
        let d = e.diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.file.as_deref(), d.cause.as_slice()),
            (
                "store.quarantine_write",
                Some("q/t.jsonl"),
                &["full".to_owned()][..]
            )
        );
        let e = StoreError::Io {
            path: "p".into(),
            source: std::io::Error::other("x"),
        };
        assert_eq!(e.diagnostic().cause, vec!["x".to_owned()]);
        let e = StoreError::SchemaMismatch {
            path: "p".into(),
            expected: "a".into(),
            observed: "b".into(),
        };
        assert_eq!(e.diagnostic().observed.as_deref(), Some("b"));
        let e = StoreError::PartitionColumnNotTimestamp {
            column: "C".into(),
            observed: "Utf8".into(),
        };
        assert_eq!(e.diagnostic().column.as_deref(), Some("C"));
        assert_eq!(ManifestError::Closed.diagnostic().code, "manifest.closed");
        assert_eq!(
            ManifestError::Sqlite {
                path: "p".into(),
                message: "m".into()
            }
            .diagnostic()
            .code,
            "manifest.sqlite"
        );
        assert_eq!(
            ManifestError::Migration {
                path: "p".into(),
                at: 1,
                failed: "f".into(),
                message: "m".into()
            }
            .diagnostic()
            .code,
            "manifest.migration_failed"
        );
        assert_eq!(
            ManifestError::NotFound { kind: "run", id: 1 }
                .diagnostic()
                .code,
            "manifest.not_found"
        );
    }
}
