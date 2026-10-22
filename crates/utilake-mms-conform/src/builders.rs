//! Typed column builders fed from raw bytes.

use crate::mmstime as nemtime;
use arrow_array::ArrayRef;
use arrow_array::builder::{
    ArrayBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Float64Builder, Int64Builder,
    StringBuilder, TimestampMicrosecondBuilder,
};
use arrow_schema::DataType;
use utilake_core::num;
use utilake_mms_registry::Cast;

use crate::error::{RowError, ValueError};

/// A value parsed from a field, before it is appended. Holding parsed values
/// for a whole row before appending any means a bad field never leaves a
/// half-appended row behind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Parsed {
    /// Empty field.
    Null,
    /// A float.
    F64(f64),
    /// An integer, timestamp nanoseconds or days.
    I64(i64),
    /// A timestamp beyond what nanoseconds hold, clamped to the nearest end
    /// of the range.
    Clamped(i64),
    /// Text: the field index to fetch from the row again.
    Str(u16),
    /// A boolean.
    Bool(bool),
    /// An exact decimal, unscaled.
    Dec(i128),
}

/// Why a field could not be given its column's type. Small and `Copy`: a
/// fault is rare, but the parse result is checked for every field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The field is empty and the column does not allow nulls.
    Null,
    /// The text is not a timestamp or date.
    Timestamp(nemtime::TimestampError),
    /// The text is not a number of the column's kind.
    Number(num::NumberError),
    /// The bytes are not UTF-8.
    Utf8,
    /// The text is not a boolean.
    Boolean,
}

impl Fault {
    /// The fault as a row error, which says it in full: the column, the
    /// value, what was expected.
    pub fn row_error(self, column: &str, bytes: &[u8]) -> RowError {
        let column = column.to_owned();
        let value = || String::from_utf8_lossy(bytes).into_owned();
        RowError::Value(match self {
            Self::Null => ValueError::NullNotAllowed { column },
            Self::Timestamp(source) => ValueError::Timestamp {
                column,
                value: value(),
                source,
            },
            Self::Number(source) => ValueError::Number {
                column,
                value: value(),
                source,
            },
            Self::Utf8 => ValueError::Utf8 { column },
            Self::Boolean => ValueError::Boolean {
                column,
                value: value(),
            },
        })
    }
}

/// Parses one field according to its cast.
#[inline]
pub fn parse_field(cast: Cast, nullable: bool, index: u16, bytes: &[u8]) -> Result<Parsed, Fault> {
    if bytes.is_empty() {
        return if nullable {
            Ok(Parsed::Null)
        } else {
            Err(Fault::Null)
        };
    }
    match cast {
        Cast::Utf8 => {
            if std::str::from_utf8(bytes).is_err() {
                return Err(Fault::Utf8);
            }
            Ok(Parsed::Str(index))
        }
        Cast::Float64 => num::parse_f64(bytes)
            .map(Parsed::F64)
            .map_err(Fault::Number),
        Cast::Int64 => num::parse_i64(bytes)
            .map(Parsed::I64)
            .map_err(Fault::Number),
        Cast::TimestampNanos { offset_secs } => nemtime::parse_nanos(bytes, offset_secs)
            .map(|(nanos, clamped)| {
                if clamped {
                    Parsed::Clamped(nanos)
                } else {
                    Parsed::I64(nanos)
                }
            })
            .map_err(Fault::Timestamp),
        Cast::Date32 => nemtime::parse_days(bytes)
            .map(|d| Parsed::I64(i64::from(d)))
            .map_err(Fault::Timestamp),
        Cast::Decimal128 { precision, scale } => num::parse_decimal(bytes, precision, scale)
            .map(Parsed::Dec)
            .map_err(Fault::Number),
        Cast::Boolean => match bytes {
            b"true" | b"TRUE" | b"True" | b"Y" | b"y" | b"1" | b"T" => Ok(Parsed::Bool(true)),
            b"false" | b"FALSE" | b"False" | b"N" | b"n" | b"0" | b"F" => Ok(Parsed::Bool(false)),
            _ => Err(Fault::Boolean),
        },
    }
}

/// One lake column's builder.
#[derive(Debug)]
pub enum ColumnBuilder {
    /// Text.
    Utf8(StringBuilder),
    /// `f64`.
    F64(Float64Builder),
    /// `i64`.
    I64(Int64Builder),
    /// Timestamp microseconds with a zone: AEMO's times are whole seconds,
    /// and Iceberg's `timestamptz` is microseconds.
    Ts(TimestampMicrosecondBuilder),
    /// Days.
    Date(Date32Builder),
    /// Booleans.
    Bool(BooleanBuilder),
    /// Exact decimals.
    Dec(Decimal128Builder),
}

impl ColumnBuilder {
    /// A builder for an Arrow type, with room for `capacity` rows.
    pub fn new(ty: &DataType, capacity: usize) -> Self {
        match ty {
            DataType::Utf8 => Self::Utf8(StringBuilder::with_capacity(capacity, capacity * 8)),
            DataType::Float64 => Self::F64(Float64Builder::with_capacity(capacity)),
            DataType::Int64 => Self::I64(Int64Builder::with_capacity(capacity)),
            DataType::Timestamp(_, tz) => Self::Ts(
                TimestampMicrosecondBuilder::with_capacity(capacity).with_timezone_opt(tz.clone()),
            ),
            DataType::Date32 => Self::Date(Date32Builder::with_capacity(capacity)),
            DataType::Boolean => Self::Bool(BooleanBuilder::with_capacity(capacity)),
            DataType::Decimal128(p, s) => Self::Dec(
                Decimal128Builder::with_capacity(capacity)
                    .with_precision_and_scale(*p, *s)
                    .expect("the registry gives a decimal its precision and scale"),
            ),
            other => unreachable!("the registry never produces Arrow type {other}"),
        }
    }

    /// Appends a null.
    #[inline]
    pub fn append_null(&mut self) {
        match self {
            Self::Utf8(b) => b.append_null(),
            Self::F64(b) => b.append_null(),
            Self::I64(b) => b.append_null(),
            Self::Ts(b) => b.append_null(),
            Self::Date(b) => b.append_null(),
            Self::Bool(b) => b.append_null(),
            Self::Dec(b) => b.append_null(),
        }
    }

    /// Appends a parsed value. `text` resolves `Parsed::Str` back to bytes
    /// that [`parse_field`] already checked are UTF-8.
    #[inline]
    pub fn append(&mut self, v: Parsed, text: impl FnOnce(u16) -> &'static [u8]) {
        match (self, v) {
            (b, Parsed::Null) => b.append_null(),
            (Self::Utf8(b), Parsed::Str(i)) => {
                let bytes = text(i);
                // Checked in parse_field; the lossy path is unreachable but safe.
                b.append_value(std::str::from_utf8(bytes).unwrap_or(""));
            }
            (Self::F64(b), Parsed::F64(v)) => b.append_value(v),
            (Self::I64(b), Parsed::I64(v)) => b.append_value(v),
            // The parser gives nanoseconds.
            (Self::Ts(b), Parsed::I64(v) | Parsed::Clamped(v)) => {
                b.append_value(v.div_euclid(1_000));
            }
            (Self::Date(b), Parsed::I64(v)) => b.append_value(v as i32),
            (Self::Bool(b), Parsed::Bool(v)) => b.append_value(v),
            (Self::Dec(b), Parsed::Dec(v)) => b.append_value(v),
            (b, v) => unreachable!("parsed {v:?} does not match builder {b:?}"),
        }
    }

    /// Rows appended so far.
    pub fn len(&self) -> usize {
        match self {
            Self::Utf8(b) => b.len(),
            Self::F64(b) => b.len(),
            Self::I64(b) => b.len(),
            Self::Ts(b) => b.len(),
            Self::Date(b) => b.len(),
            Self::Bool(b) => b.len(),
            Self::Dec(b) => b.len(),
        }
    }

    /// Whether nothing has been appended.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Finishes the array and resets the builder.
    pub fn finish(&mut self) -> ArrayRef {
        match self {
            Self::Utf8(b) => std::sync::Arc::new(b.finish()),
            Self::F64(b) => std::sync::Arc::new(b.finish()),
            Self::I64(b) => std::sync::Arc::new(b.finish()),
            Self::Ts(b) => std::sync::Arc::new(b.finish()),
            Self::Date(b) => std::sync::Arc::new(b.finish()),
            Self::Bool(b) => std::sync::Arc::new(b.finish()),
            Self::Dec(b) => std::sync::Arc::new(b.finish()),
        }
    }
}

#[cfg(test)]
mod tests {
    use arrow_array::Array;

    use super::*;

    /// [`super::parse_field`] as the row error a fault becomes.
    fn parse_field(
        cast: Cast,
        nullable: bool,
        index: u16,
        column: &str,
        bytes: &[u8],
    ) -> Result<Parsed, Box<RowError>> {
        super::parse_field(cast, nullable, index, bytes)
            .map_err(|f| Box::new(f.row_error(column, bytes)))
    }

    /// The same, unboxed, to match on.
    macro_rules! parse_field {
        ($($arg:expr),* $(,)?) => {
            parse_field($($arg),*).map_err(|e| *e)
        };
    }

    #[test]
    fn parses_every_cast() {
        let ts = Cast::TimestampNanos {
            offset_secs: 36_000,
        };
        assert!(matches!(
            parse_field!(ts, true, 0, "T", b"2024/01/01 00:05:00"),
            Ok(Parsed::I64(_))
        ));
        assert!(matches!(
            parse_field!(ts, true, 0, "T", b"nope"),
            Err(RowError::Value(ValueError::Timestamp { .. }))
        ));
        // A far-future "no end" date is kept, clamped and marked as such.
        assert_eq!(
            parse_field!(ts, true, 0, "T", b"9999/12/31 00:00:00"),
            Ok(Parsed::Clamped(i64::MAX))
        );
        assert!(matches!(
            parse_field!(ts, true, 0, "T", b"2026/07/28 09:29:54.000000001"),
            Ok(Parsed::I64(n)) if n % 1_000_000_000 == 1
        ));
        assert_eq!(parse_field!(ts, true, 0, "T", b""), Ok(Parsed::Null));
        assert!(matches!(
            parse_field!(ts, false, 0, "T", b""),
            Err(RowError::Value(ValueError::NullNotAllowed { .. }))
        ));
        assert_eq!(
            parse_field!(Cast::Float64, true, 0, "F", b"1.5"),
            Ok(Parsed::F64(1.5))
        );
        assert!(matches!(
            parse_field!(Cast::Float64, true, 0, "F", b"x"),
            Err(RowError::Value(ValueError::Number {
                source: num::NumberError::Float,
                ..
            }))
        ));
        assert_eq!(
            parse_field!(Cast::Int64, true, 0, "I", b"42"),
            Ok(Parsed::I64(42))
        );
        assert!(parse_field!(Cast::Int64, true, 0, "I", b"4.2").is_err());
        // An exact decimal: padded to its scale, never rounded.
        let dec = Cast::Decimal128 {
            precision: 15,
            scale: 5,
        };
        assert_eq!(
            parse_field!(dec, true, 0, "N", b"-496.76722"),
            Ok(Parsed::Dec(-49_676_722))
        );
        assert_eq!(
            parse_field!(dec, true, 0, "N", b"4.9"),
            Ok(Parsed::Dec(490_000))
        );
        assert!(matches!(
            parse_field!(dec, true, 0, "N", b"1.123456"),
            Err(RowError::Value(ValueError::Number { .. }))
        ));
        assert_eq!(
            parse_field!(Cast::Date32, true, 0, "D", b"1970/01/03"),
            Ok(Parsed::I64(2))
        );
        assert!(parse_field!(Cast::Date32, true, 0, "D", b"x").is_err());
        assert_eq!(
            parse_field!(Cast::Utf8, true, 3, "S", b"abc"),
            Ok(Parsed::Str(3))
        );
        assert!(matches!(
            parse_field!(Cast::Utf8, true, 3, "S", b"\xff"),
            Err(RowError::Value(ValueError::Utf8 { .. }))
        ));
        for t in [&b"true"[..], b"Y", b"1", b"T"] {
            assert_eq!(
                parse_field!(Cast::Boolean, true, 0, "B", t),
                Ok(Parsed::Bool(true))
            );
        }
        for f in [&b"false"[..], b"N", b"0", b"F"] {
            assert_eq!(
                parse_field!(Cast::Boolean, true, 0, "B", f),
                Ok(Parsed::Bool(false))
            );
        }
        assert!(parse_field!(Cast::Boolean, true, 0, "B", b"maybe").is_err());
    }

    #[test]
    fn builders_round_trip() {
        let tz = DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some("+10:00".into()));
        let types = [
            DataType::Utf8,
            DataType::Float64,
            DataType::Int64,
            tz.clone(),
            DataType::Date32,
            DataType::Boolean,
            DataType::Decimal128(15, 5),
        ];
        let values = [
            Parsed::Str(0),
            Parsed::F64(1.5),
            Parsed::I64(7),
            Parsed::I64(1_000_000),
            Parsed::I64(3),
            Parsed::Bool(true),
            Parsed::Dec(1_234_500_000),
        ];
        for (ty, v) in types.iter().zip(values) {
            let mut b = ColumnBuilder::new(ty, 4);
            assert!(b.is_empty());
            b.append(v, |_| b"hello");
            b.append_null();
            b.append(Parsed::Null, |_| b"");
            assert_eq!(b.len(), 3);
            let arr = b.finish();
            assert_eq!(arr.len(), 3);
            assert_eq!(arr.null_count(), 2);
            assert_eq!(arr.data_type(), ty);
            assert!(b.is_empty(), "finish resets");
        }
    }

    #[test]
    #[should_panic(expected = "does not match builder")]
    fn mismatched_append_is_a_bug() {
        let mut b = ColumnBuilder::new(&DataType::Int64, 1);
        b.append(Parsed::F64(1.0), |_| b"");
    }

    #[test]
    #[should_panic(expected = "never produces")]
    fn unsupported_type_is_a_bug() {
        let _ = ColumnBuilder::new(&DataType::Binary, 1);
    }
}
