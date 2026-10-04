//! Errors of scanning, parsing and sinking MMS rows.

use nemlake_core::nemtime::TimestampError;
use nemlake_core::num::NumberError;
use nemlake_core::{Category, Diagnose, Diagnostic};

/// Where in the file something happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    /// 1-based line.
    pub line: u64,
    /// 0-based byte offset of the line start.
    pub byte_offset: u64,
}

/// A row that could not be used; the row is quarantined and the scan goes on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    /// More non-empty fields than the I row declared.
    #[error("The D row has {observed} fields but the I row declared {declared}.")]
    FieldCount {
        /// Declared data columns.
        declared: usize,
        /// Observed data fields.
        observed: usize,
    },
    /// The D row's report prefix differs from its block's I row.
    #[error(
        "The D row belongs to ({report_type}, {report_subtype}) version {version} but the current block is ({block_type}, {block_subtype}) version {block_version}."
    )]
    BlockMismatch {
        /// Row's report type.
        report_type: String,
        /// Row's subtype.
        report_subtype: String,
        /// Row's version.
        version: String,
        /// Block's report type.
        block_type: String,
        /// Block's subtype.
        block_subtype: String,
        /// Block's version.
        block_version: u16,
    },
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
    /// Fewer than four fields: no report prefix.
    #[error(
        "The D row has only {0} fields; a row starts with D, report type, subtype and version."
    )]
    TooShort(usize),
}

/// What is wrong with `value` as a number of the declared kind, in the
/// words of what was expected and what was found: a whole number was
/// declared and the file has a fraction, not just "not an integer".
fn number_fault(value: &str, source: &NumberError) -> String {
    let is_number = nemlake_core::num::parse_f64(value.as_bytes()).is_ok();
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

impl Diagnose for RowError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::FieldCount { declared, observed } => {
                Diagnostic::error("csv.field_count_mismatch", Category::Csv, msg)
                    .expected_observed(format!("{declared} fields"), format!("{observed} fields"))
            }
            Self::BlockMismatch { .. } => {
                Diagnostic::error("csv.block_mismatch", Category::Csv, msg)
            }
            Self::TooShort(_) => Diagnostic::error("csv.row_too_short", Category::Csv, msg),
            Self::NullNotAllowed { column } => {
                Diagnostic::error("parse.null_not_allowed", Category::Parse, msg)
                    .with_column(column.clone())
            }
            Self::Timestamp { column, value, .. } => {
                Diagnostic::error("parse.timestamp", Category::Parse, msg)
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
                Diagnostic::error(code, Category::Parse, msg)
                    .with_column(column.clone())
                    .expected_observed(expected, value.clone())
            }
            Self::Utf8 { column } => {
                Diagnostic::error("parse.utf8", Category::Parse, msg).with_column(column.clone())
            }
            Self::Boolean { column, value } => {
                Diagnostic::error("parse.boolean", Category::Parse, msg)
                    .with_column(column.clone())
                    .expected_observed("a boolean", value.clone())
            }
        }
    }
}

/// What a sink returns from `row`: a bad row (quarantined, scan continues)
/// or a sink failure (the scan ends).
#[derive(Debug, thiserror::Error)]
pub enum RowFailure {
    /// The row is bad. Boxed: errors are rare and the hot path's `Result`
    /// must stay small.
    #[error(transparent)]
    Row(Box<RowError>),
    /// The sink failed, for example a batch could not be written.
    #[error(transparent)]
    Sink(Box<SinkError>),
}

impl From<RowError> for RowFailure {
    fn from(e: RowError) -> Self {
        Self::Row(Box::new(e))
    }
}

impl From<Box<RowError>> for RowFailure {
    fn from(e: Box<RowError>) -> Self {
        Self::Row(e)
    }
}

impl From<SinkError> for RowFailure {
    fn from(e: SinkError) -> Self {
        Self::Sink(Box::new(e))
    }
}

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
                    Diagnostic::error("store.write_failed", Category::Store, self.to_string())
                        .with_table(table.clone());
                d.cause.clone_from(cause);
                d
            }
            Self::Arrow { table, .. } => {
                Diagnostic::error("internal.arrow", Category::Internal, self.to_string())
                    .with_table(table.clone())
            }
        }
    }
}

/// A failure that ends the scan of one file.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// Reading the input failed.
    #[error("Reading the CSV failed after {bytes} bytes.")]
    Io {
        /// Bytes read before the failure.
        bytes: u64,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// One line exceeded the limit.
    #[error(
        "Line {line} is longer than {limit} bytes; an MMS row never is, so the file is not a CSV or is corrupt."
    )]
    LineTooLong {
        /// Line.
        line: u64,
        /// Limit.
        limit: usize,
    },
    /// A D row before any I row.
    #[error("A D row appears at line {line} before any I row has declared the columns.")]
    DataBeforeHeader {
        /// Line.
        line: u64,
    },
    /// An I row that could not be read.
    #[error("The I row at line {line} is malformed: {message}")]
    BadHeader {
        /// Line.
        line: u64,
        /// What is wrong.
        message: String,
    },
    /// Too many rows quarantined.
    #[error(
        "{bad} row(s) could not be read, more than the limit of {limit}, so the file is refused. The first is line {line}: {first}"
    )]
    TooManyBadRows {
        /// Bad rows seen.
        bad: u64,
        /// Limit.
        limit: u64,
        /// Line of the first bad row.
        line: u64,
        /// What was wrong with the first bad row.
        first: Box<RowError>,
    },
    /// The sink failed.
    #[error(transparent)]
    Sink(#[from] SinkError),
    /// The sink asked to stop (fail-fast).
    #[error("The scan stopped at line {line} because the run is failing fast.")]
    Stopped {
        /// Line.
        line: u64,
    },
}

impl ScanError {
    /// The line the error refers to, when it has one.
    pub fn line(&self) -> Option<u64> {
        match self {
            Self::LineTooLong { line, .. }
            | Self::DataBeforeHeader { line }
            | Self::BadHeader { line, .. }
            | Self::TooManyBadRows { line, .. }
            | Self::Stopped { line } => Some(*line),
            _ => None,
        }
    }
}

impl Diagnose for ScanError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        let mut d = match self {
            Self::Io { source, .. } => {
                Diagnostic::error("csv.read_failed", Category::Csv, msg).caused_by(source)
            }
            Self::LineTooLong { .. } => Diagnostic::error("csv.line_too_long", Category::Csv, msg),
            Self::DataBeforeHeader { .. } => {
                Diagnostic::error("csv.data_before_header", Category::Csv, msg)
            }
            Self::BadHeader { .. } => Diagnostic::error("csv.header_malformed", Category::Csv, msg),
            Self::TooManyBadRows { first, .. } => {
                // The refusal says exactly what the first bad row had
                // wrong: its column, what was expected and what was there.
                let row = first.diagnostic();
                let mut d = Diagnostic::error("csv.too_many_bad_rows", Category::Csv, msg);
                d.column = row.column;
                d.expected = row.expected;
                d.observed = row.observed;
                d.cause = vec![format!("{} [{}]", row.message, row.code)];
                d
            }
            Self::Sink(e) => e.diagnostic(),
            Self::Stopped { .. } => Diagnostic::error("csv.stopped_fail_fast", Category::Csv, msg),
        };
        if let Some(l) = self.line() {
            d.line = Some(l);
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_error_codes() {
        let e = RowError::FieldCount {
            declared: 7,
            observed: 8,
        };
        let d = e.diagnostic();
        assert_eq!(d.code, "csv.field_count_mismatch");
        assert_eq!(d.expected.as_deref(), Some("7 fields"));
        let e = RowError::Timestamp {
            column: "T".into(),
            value: "x".into(),
            source: TimestampError::Shape,
        };
        assert_eq!(e.diagnostic().code, "parse.timestamp");
        assert!(e.to_string().contains("Column T"));
        for (src, code) in [
            (NumberError::Float, "parse.float"),
            (NumberError::Integer, "parse.integer"),
            (NumberError::DecimalShape, "parse.decimal"),
        ] {
            assert_eq!(
                RowError::Number {
                    column: "N".into(),
                    value: "x".into(),
                    source: src
                }
                .diagnostic()
                .code,
                code
            );
        }
        assert_eq!(
            RowError::Utf8 { column: "U".into() }.diagnostic().code,
            "parse.utf8"
        );
        assert_eq!(
            RowError::Boolean {
                column: "B".into(),
                value: "x".into()
            }
            .diagnostic()
            .code,
            "parse.boolean"
        );
        assert_eq!(
            RowError::NullNotAllowed { column: "K".into() }
                .diagnostic()
                .column
                .as_deref(),
            Some("K")
        );
        assert_eq!(RowError::TooShort(2).diagnostic().code, "csv.row_too_short");
        let e = RowError::BlockMismatch {
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
            let e = RowError::Number {
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
        assert_eq!(
            ScanError::LineTooLong { line: 3, limit: 1 }
                .diagnostic()
                .line,
            Some(3)
        );
        assert_eq!(
            ScanError::DataBeforeHeader { line: 2 }.diagnostic().code,
            "csv.data_before_header"
        );
        assert_eq!(
            ScanError::BadHeader {
                line: 2,
                message: "m".into()
            }
            .diagnostic()
            .code,
            "csv.header_malformed"
        );
        // A refusal says exactly what its first bad row had wrong: which
        // column, what was declared, what the file has.
        let refused = ScanError::TooManyBadRows {
            bad: 1,
            limit: 0,
            line: 133,
            first: Box::new(RowError::Number {
                column: "REGISTEREDCAPACITY".into(),
                value: "13.55".into(),
                source: NumberError::Integer,
            }),
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
        assert_eq!(
            ScanError::Stopped { line: 9 }.diagnostic().code,
            "csv.stopped_fail_fast"
        );
        let e = ScanError::Io {
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
        let e = ScanError::Sink(SinkError::write("T", &Outer(os)));
        let d = e.diagnostic();
        assert_eq!(d.code, "store.write_failed");
        assert_eq!(d.table.as_deref(), Some("T"));
        assert_eq!(d.cause, vec![os_text.clone()]);
        assert!(d.render().contains(&os_text), "{}", d.render());
        assert!(d.message.contains("Writing a batch of table T failed"));
        let e = ScanError::Sink(SinkError::Arrow {
            table: "T".into(),
            message: "m".into(),
        });
        assert_eq!(e.diagnostic().code, "internal.arrow");
        let e = SinkError::Refused(Box::new(Diagnostic::error(
            "schema.unknown_table",
            Category::Schema,
            "No table.",
        )));
        assert_eq!(e.diagnostic().code, "schema.unknown_table");
        assert_eq!(e.to_string(), "No table.");
    }
}
