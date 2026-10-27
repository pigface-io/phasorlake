//! What the typed sink refuses and fails with. The scanner's own errors
//! ([`phasor_mms_decode::error`]) carry these through and diagnose
//! themselves, each of these by its own [`Diagnose`].

use phasor_core::num::NumberError;
use phasor_core::{Category, Diagnose, Diagnostic};

use crate::mmstime::TimestampError;

/// A value that does not fit its column: the row is kept, the value
/// null, unless strict mode refuses the file for it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    /// A non-nullable column is empty.
    #[error("Column {column} is empty but does not allow nulls.")]
    NullNotAllowed {
        /// Column.
        column: String,
    },
    /// A value is not a timestamp.
    #[error("Column {column}: the value {value} {source}.")]
    Timestamp {
        /// Column.
        column: String,
        /// Value as text.
        value: String,
        /// Cause.
        source: TimestampError,
    },
    /// A value is not a number of the column's kind.
    #[error("Column {column}: the value {value} {}.", number_fault(value, source))]
    Number {
        /// Column.
        column: String,
        /// Value as text.
        value: String,
        /// Cause.
        source: NumberError,
    },
    /// A value is not UTF-8.
    #[error("Column {column}: the value is not UTF-8 text.")]
    Utf8 {
        /// Column.
        column: String,
    },
    /// A value is not a boolean.
    #[error("Column {column}: the value {value} is not a boolean (true/false, Y/N, 1/0).")]
    Boolean {
        /// Column.
        column: String,
        /// Value as text.
        value: String,
    },
}

/// What is wrong with `value` as a number of the declared kind, in the
/// words of what was expected and what was found: a whole number was
/// declared and the file has a fraction, not just "not an integer".
fn number_fault(value: &str, source: &NumberError) -> String {
    let is_number = phasor_core::num::parse_f64(value.as_bytes()).is_ok();
    match source {
        NumberError::Integer if is_number && value.contains(['.', 'e', 'E']) => {
            "is a number with a fraction or exponent, but the column is declared a whole number (Int64)".to_owned()
        }
        NumberError::Integer if is_number => {
            "is a whole number too large for the column's 64 bits".to_owned()
        }
        NumberError::Integer => {
            "is not a number, but the column is declared a whole number (Int64)".to_owned()
        }
        NumberError::Float => "is not a number, but the column is declared a number (Float64)".to_owned(),
        other => other.to_string(),
    }
}

impl Diagnose for ValueError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::NullNotAllowed { column } => {
                Diagnostic::error("parse.null_not_allowed", Category::PARSE, msg)
                    .with_column(column.clone())
            }
            Self::Timestamp { column, value, .. } => {
                Diagnostic::error("parse.timestamp", Category::PARSE, msg)
                    .with_column(column.clone())
                    .expected_observed("yyyy/mm/dd hh24:mi:ss", value.clone())
            }
            Self::Number {
                column,
                value,
                source,
            } => {
                let (code, expected) = match source {
                    NumberError::Float => ("parse.float", "a number (Float64)".to_owned()),
                    NumberError::Integer => ("parse.integer", "a whole number (Int64)".to_owned()),
                    NumberError::DecimalScale { scale, .. } => (
                        "parse.decimal",
                        format!("a decimal with at most {scale} fractional digit(s)"),
                    ),
                    _ => ("parse.decimal", "a decimal number".to_owned()),
                };
                Diagnostic::error(code, Category::PARSE, msg)
                    .with_column(column.clone())
                    .expected_observed(expected, value.clone())
            }
            Self::Utf8 { column } => {
                Diagnostic::error("parse.utf8", Category::PARSE, msg).with_column(column.clone())
            }
            Self::Boolean { column, value } => {
                Diagnostic::error("parse.boolean", Category::PARSE, msg)
                    .with_column(column.clone())
                    .expected_observed("a boolean", value.clone())
            }
        }
    }
}

/// What is wrong with a row: its structure, or one of its values.
pub type RowError = phasor_mms_decode::RowError<ValueError>;

/// What the typed sink returns from a row.
pub type RowFailure = phasor_mms_decode::RowFailure<ValueError, SinkError>;

/// A failure that ends the scan of one file.
pub type ScanError = phasor_mms_decode::ScanError<ValueError, SinkError>;

/// A failure of the thing rows are delivered to; ends the scan.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    /// The sink refused the block its I row opens, for example because the
    /// shape did not resolve in strict mode; the diagnostic says why.
    #[error("{}", .0.message)]
    Refused(Box<Diagnostic>),
    /// Writing a batch failed.
    #[error("Writing a batch of table {table} failed: {message}")]
    Write {
        /// Table.
        table: String,
        /// What went wrong.
        message: String,
        /// The chain of causes under it, outermost first: what the
        /// operating system or the Parquet writer actually said.
        cause: Vec<String>,
    },
    /// Building an Arrow array failed; a bug.
    #[error("Building the Arrow batch for table {table} failed: {message}")]
    Arrow {
        /// Table.
        table: String,
        /// Arrow's message.
        message: String,
    },
}

impl SinkError {
    /// A failed write of `table`, with every cause under `err` kept: the
    /// message of an I/O error is what tells a full disk from a missing
    /// directory from too many open files.
    pub fn write(table: impl Into<String>, err: &(dyn std::error::Error + 'static)) -> Self {
        let mut cause = Vec::new();
        let mut source = err.source();
        while let Some(s) = source {
            cause.push(s.to_string());
            source = s.source();
        }
        Self::Write {
            table: table.into(),
            message: err.to_string(),
            cause,
        }
    }
}

impl Diagnose for SinkError {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Refused(d) => (**d).clone(),
            Self::Write { table, cause, .. } => {
                let mut d =
                    Diagnostic::error("store.write_failed", Category::STORE, self.to_string())
                        .with_table(table.clone());
                d.cause.clone_from(cause);
                d
            }
            Self::Arrow { table, .. } => {
                Diagnostic::error("internal.arrow", Category::INTERNAL, self.to_string())
                    .with_table(table.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(v: ValueError) -> RowError {
        RowError::Value(v)
    }

    #[test]
    fn row_error_codes() {
        let e: RowError = RowError::FieldCount {
            declared: 7,
            observed: 8,
        };
        let d = e.diagnostic();
        assert_eq!(d.code, "csv.field_count_mismatch");
        assert_eq!(d.expected.as_deref(), Some("7 fields"));
        let e = value(ValueError::Timestamp {
            column: "T".into(),
            value: "x".into(),
            source: TimestampError::Shape,
        });
        assert_eq!(e.diagnostic().code, "parse.timestamp");
        assert!(e.to_string().contains("Column T"));
        for (src, code) in [
            (NumberError::Float, "parse.float"),
            (NumberError::Integer, "parse.integer"),
            (NumberError::DecimalShape, "parse.decimal"),
        ] {
            assert_eq!(
                value(ValueError::Number {
                    column: "N".into(),
                    value: "x".into(),
                    source: src
                })
                .diagnostic()
                .code,
                code
            );
        }
        assert_eq!(
            value(ValueError::Utf8 { column: "U".into() })
                .diagnostic()
                .code,
            "parse.utf8"
        );
        assert_eq!(
            value(ValueError::Boolean {
                column: "B".into(),
                value: "x".into()
            })
            .diagnostic()
            .code,
            "parse.boolean"
        );
        assert_eq!(
            value(ValueError::NullNotAllowed { column: "K".into() })
                .diagnostic()
                .column
                .as_deref(),
            Some("K")
        );
        let short: RowError = RowError::TooShort(2);
        assert_eq!(short.diagnostic().code, "csv.row_too_short");
        let unknown: RowError = RowError::UnknownRecord('x');
        assert_eq!(unknown.diagnostic().code, "csv.unknown_record");
        let e: RowError = RowError::BlockMismatch {
            report_type: "A".into(),
            report_subtype: "B".into(),
            version: "1".into(),
            block_type: "C".into(),
            block_subtype: "D".into(),
            block_version: 2,
        };
        assert_eq!(e.diagnostic().code, "csv.block_mismatch");
    }

    #[test]
    fn a_number_fault_says_what_was_declared_and_what_was_found() {
        let fault = |value: &str, source: NumberError| {
            let e = ValueError::Number {
                column: "N".into(),
                value: value.into(),
                source,
            };
            (e.to_string(), e.diagnostic())
        };
        let (text, d) = fault("13.55", NumberError::Integer);
        assert_eq!(
            text,
            "Column N: the value 13.55 is a number with a fraction or exponent, but the column is declared a whole number (Int64)."
        );
        assert_eq!(d.code, "parse.integer");
        assert_eq!(d.expected.as_deref(), Some("a whole number (Int64)"));
        assert_eq!(d.observed.as_deref(), Some("13.55"));
        assert!(
            fault("1e3", NumberError::Integer)
                .0
                .contains("fraction or exponent")
        );
        assert!(
            fault("99999999999999999999", NumberError::Integer)
                .0
                .contains("too large for the column's 64 bits")
        );
        assert!(
            fault("abc", NumberError::Integer)
                .0
                .contains("is not a number, but the column is declared a whole number")
        );
        let (text, d) = fault("abc", NumberError::Float);
        assert!(text.contains("declared a number (Float64)"), "{text}");
        assert_eq!(d.expected.as_deref(), Some("a number (Float64)"));
        let (_, d) = fault("1.234", NumberError::DecimalScale { scale: 2, seen: 3 });
        assert_eq!(d.code, "parse.decimal");
        assert_eq!(
            d.expected.as_deref(),
            Some("a decimal with at most 2 fractional digit(s)")
        );
        let (_, d) = fault("x", NumberError::DecimalShape);
        assert_eq!(d.expected.as_deref(), Some("a decimal number"));
    }

    #[test]
    fn scan_error_codes_carry_lines() {
        let long: ScanError = ScanError::LineTooLong { line: 3, limit: 1 };
        assert_eq!(long.diagnostic().line, Some(3));
        let before: ScanError = ScanError::DataBeforeHeader { line: 2 };
        assert_eq!(before.diagnostic().code, "csv.data_before_header");
        let bad: ScanError = ScanError::BadHeader {
            line: 2,
            message: "m".into(),
        };
        assert_eq!(bad.diagnostic().code, "csv.header_malformed");
        // A refusal says exactly what its first bad row had wrong: which
        // column, what was declared, what the file has.
        let refused: ScanError = ScanError::TooManyBadRows {
            bad: 1,
            limit: 0,
            line: 133,
            first: Box::new(value(ValueError::Number {
                column: "REGISTEREDCAPACITY".into(),
                value: "13.55".into(),
                source: NumberError::Integer,
            })),
        };
        let d = refused.diagnostic();
        assert_eq!(d.code, "csv.too_many_bad_rows");
        assert_eq!(d.line, Some(133));
        assert_eq!(d.column.as_deref(), Some("REGISTEREDCAPACITY"));
        assert_eq!(d.expected.as_deref(), Some("a whole number (Int64)"));
        assert_eq!(d.observed.as_deref(), Some("13.55"));
        assert!(
            d.message.contains("line 133")
                && d.message.contains("REGISTEREDCAPACITY")
                && d.message.contains("13.55 is a number with a fraction")
                && d.message.contains("declared a whole number (Int64)"),
            "{}",
            d.message
        );
        assert_eq!(d.cause.len(), 1);
        assert!(d.cause[0].ends_with("[parse.integer]"), "{:?}", d.cause);
        let stopped: ScanError = ScanError::Stopped { line: 9 };
        assert_eq!(stopped.diagnostic().code, "csv.stopped_fail_fast");
        let e: ScanError = ScanError::Io {
            bytes: 10,
            source: std::io::Error::other("boom"),
        };
        assert_eq!(e.diagnostic().cause, vec!["boom".to_owned()]);
        assert_eq!(e.line(), None);
        // A failed write keeps what the operating system said.
        #[derive(Debug, thiserror::Error)]
        #[error("The lake path /x could not be created or written.")]
        struct Outer(#[source] std::io::Error);
        let os = std::io::Error::from_raw_os_error(24);
        let os_text = os.to_string();
        let e: ScanError = ScanError::Sink(SinkError::write("T", &Outer(os)));
        let d = e.diagnostic();
        assert_eq!(d.code, "store.write_failed");
        assert_eq!(d.table.as_deref(), Some("T"));
        assert_eq!(d.cause, vec![os_text.clone()]);
        assert!(d.render().contains(&os_text), "{}", d.render());
        assert!(d.message.contains("Writing a batch of table T failed"));
        let e: ScanError = ScanError::Sink(SinkError::Arrow {
            table: "T".into(),
            message: "m".into(),
        });
        assert_eq!(e.diagnostic().code, "internal.arrow");
        let e = SinkError::Refused(Box::new(Diagnostic::error(
            "schema.unknown_table",
            Category::SCHEMA,
            "No table.",
        )));
        assert_eq!(e.diagnostic().code, "schema.unknown_table");
        assert_eq!(e.to_string(), "No table.");
    }
}
