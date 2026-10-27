//! Errors of reading and validating schema files.

use phasor_core::{Category, Diagnose, Diagnostic};

/// Why a schema file or set of files is unusable.
#[derive(Debug, thiserror::Error)]
pub enum SchemaFileError {
    /// The file could not be read.
    #[error("The schema file {path} could not be read.")]
    Read {
        /// The file.
        path: String,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not a table definition (TOML by hand, JSON pulled).
    #[error("The schema file {path} is not a valid table definition: {message}")]
    Parse {
        /// The file.
        path: String,
        /// The parser's message, naming the key.
        message: String,
        /// Line, when known.
        line: Option<u64>,
    },
    /// The file parsed but breaks an invariant.
    #[error("The schema file {path} for table {table} is invalid: {message}")]
    Invalid {
        /// The file.
        path: String,
        /// The table.
        table: String,
        /// What is wrong, naming the key.
        message: String,
    },
    /// The compiled blob could not be decoded.
    #[error("The embedded schema registry could not be decoded; the binary is corrupt.")]
    Blob(#[source] postcard::Error),
}

impl Diagnose for SchemaFileError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Read { path, source } => {
                Diagnostic::error("schema.file_read", Category::SCHEMA, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Parse { path, line, .. } => {
                let mut d = Diagnostic::error("schema.file_parse", Category::SCHEMA, msg)
                    .with_file(path.clone());
                d.line = *line;
                d
            }
            Self::Invalid { path, table, .. } => {
                Diagnostic::error("schema.file_invalid", Category::SCHEMA, msg)
                    .with_file(path.clone())
                    .with_table(table.clone())
            }
            Self::Blob(e) => {
                Diagnostic::error("schema.blob_corrupt", Category::SCHEMA, msg).caused_by(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        let e = SchemaFileError::Invalid {
            path: "p".into(),
            table: "T".into(),
            message: "m".into(),
        };
        let d = e.diagnostic();
        assert_eq!(d.code, "schema.file_invalid");
        assert_eq!(d.table.as_deref(), Some("T"));
        assert!(d.message.contains("T is invalid: m"));
        let e = SchemaFileError::Parse {
            path: "p".into(),
            message: "m".into(),
            line: Some(4),
        };
        assert_eq!(e.diagnostic().line, Some(4));
        let e = SchemaFileError::Read {
            path: "p".into(),
            source: std::io::Error::other("x"),
        };
        assert_eq!(e.diagnostic().cause, vec!["x".to_owned()]);
        let e = SchemaFileError::Blob(postcard::Error::DeserializeUnexpectedEnd);
        assert_eq!(e.diagnostic().code, "schema.blob_corrupt");
    }
}
