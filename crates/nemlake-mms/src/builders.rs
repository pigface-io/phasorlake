//! Typed column builders fed from raw bytes.

use arrow_array::ArrayRef;
use arrow_array::builder::{
    ArrayBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Float64Builder, Int64Builder,
    StringBuilder, TimestampMicrosecondBuilder,
};
use arrow_schema::DataType;
use nemlake_core::{nemtime, num};
use nemlake_schema::Cast;

use crate::error::RowError;

/// A value parsed from a field, before it is appended. Holding parsed values
/// for a whole row before appending any means a bad field never leaves a
/// half-appended row behind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Parsed {
    /// Empty field.
    Null,
    /// A float.
    F64(f64),
    /// An integer or timestamp micros or days.
    I64(i64),
    /// An unscaled decimal.
    I128(i128),
    /// Text: the field index to fetch from the row again.
    Str(u16),
    /// A boolean.
    Bool(bool),
}

/// Parses one field according to its cast.
#[inline]
pub fn parse_field(
    cast: Cast,
    nullable: bool,
    index: u16,
    column: &str,
    bytes: &[u8],
) -> Result<Parsed, RowError> {
    if bytes.is_empty() {
        return if nullable {
            Ok(Parsed::Null)
        } else {
            Err(RowError::NullNotAllowed {
                column: column.to_owned(),
            })
        };
    }
    let text = || String::from_utf8_lossy(bytes).into_owned();
    match cast {
        Cast::Utf8 => {
            if std::str::from_utf8(bytes).is_err() {
                return Err(RowError::Utf8 {
                    column: column.to_owned(),
                });
            }
            Ok(Parsed::Str(index))
        }
        Cast::Float64 => {
            num::parse_f64(bytes)
                .map(Parsed::F64)
                .map_err(|source| RowError::Number {
                    column: column.to_owned(),
                    value: text(),
                    source,
                })
        }
        Cast::Int64 => num::parse_i64(bytes)
            .map(Parsed::I64)
            .map_err(|source| RowError::Number {
                column: column.to_owned(),
                value: text(),
                source,
            }),
        Cast::Decimal128 { precision, scale } => num::parse_decimal(bytes, precision, scale)
            .map(Parsed::I128)
            .map_err(|source| RowError::Number {
                column: column.to_owned(),
                value: text(),
                source,
            }),
        Cast::TimestampMicros { offset_secs } => nemtime::parse_micros(bytes, offset_secs)
            .map(Parsed::I64)
            .map_err(|source| RowError::Timestamp {
                column: column.to_owned(),
                value: text(),
                source,
            }),
        Cast::Date32 => nemtime::parse_days(bytes)
            .map(|d| Parsed::I64(i64::from(d)))
            .map_err(|source| RowError::Timestamp {
                column: column.to_owned(),
                value: text(),
                source,
            }),
        Cast::Boolean => match bytes {
            b"true" | b"TRUE" | b"True" | b"Y" | b"y" | b"1" | b"T" => Ok(Parsed::Bool(true)),
            b"false" | b"FALSE" | b"False" | b"N" | b"n" | b"0" | b"F" => Ok(Parsed::Bool(false)),
            _ => Err(RowError::Boolean {
                column: column.to_owned(),
                value: text(),
            }),
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
    /// `Decimal128`.
    Dec(Decimal128Builder),
    /// Timestamp micros with a zone.
    Ts(TimestampMicrosecondBuilder),
    /// Days.
    Date(Date32Builder),
    /// Booleans.
    Bool(BooleanBuilder),
}

impl ColumnBuilder {
    /// A builder for an Arrow type, with room for `capacity` rows.
    pub fn new(ty: &DataType, capacity: usize) -> Self {
        match ty {
            DataType::Utf8 => Self::Utf8(StringBuilder::with_capacity(capacity, capacity * 8)),
            DataType::Float64 => Self::F64(Float64Builder::with_capacity(capacity)),
            DataType::Int64 => Self::I64(Int64Builder::with_capacity(capacity)),
            DataType::Decimal128(p, s) => Self::Dec(
                Decimal128Builder::with_capacity(capacity)
                    .with_data_type(DataType::Decimal128(*p, *s)),
            ),
            DataType::Timestamp(_, tz) => Self::Ts(
                TimestampMicrosecondBuilder::with_capacity(capacity).with_timezone_opt(tz.clone()),
            ),
            DataType::Date32 => Self::Date(Date32Builder::with_capacity(capacity)),
            DataType::Boolean => Self::Bool(BooleanBuilder::with_capacity(capacity)),
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
            Self::Dec(b) => b.append_null(),
            Self::Ts(b) => b.append_null(),
            Self::Date(b) => b.append_null(),
            Self::Bool(b) => b.append_null(),
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
            (Self::Dec(b), Parsed::I128(v)) => b.append_value(v),
            (Self::Ts(b), Parsed::I64(v)) => b.append_value(v),
            (Self::Date(b), Parsed::I64(v)) => b.append_value(v as i32),
            (Self::Bool(b), Parsed::Bool(v)) => b.append_value(v),
            (b, v) => unreachable!("parsed {v:?} does not match builder {b:?}"),
        }
    }

    /// Rows appended so far.
    pub fn len(&self) -> usize {
        match self {
            Self::Utf8(b) => b.len(),
            Self::F64(b) => b.len(),
            Self::I64(b) => b.len(),
            Self::Dec(b) => b.len(),
            Self::Ts(b) => b.len(),
            Self::Date(b) => b.len(),
            Self::Bool(b) => b.len(),
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
            Self::Dec(b) => std::sync::Arc::new(b.finish()),
            Self::Ts(b) => std::sync::Arc::new(b.finish()),
            Self::Date(b) => std::sync::Arc::new(b.finish()),
            Self::Bool(b) => std::sync::Arc::new(b.finish()),
        }
    }
}

#[cfg(test)]
mod tests {
    use arrow_array::Array;

    use super::*;

    #[test]
    fn parses_every_cast() {
        let ts = Cast::TimestampMicros {
            offset_secs: 36_000,
        };
        assert!(matches!(
            parse_field(ts, true, 0, "T", b"2024/01/01 00:05:00"),
            Ok(Parsed::I64(_))
        ));
        assert!(matches!(
            parse_field(ts, true, 0, "T", b"nope"),
            Err(RowError::Timestamp { .. })
        ));
        assert_eq!(parse_field(ts, true, 0, "T", b""), Ok(Parsed::Null));
        assert!(matches!(
            parse_field(ts, false, 0, "T", b""),
            Err(RowError::NullNotAllowed { .. })
        ));
        assert_eq!(
            parse_field(Cast::Float64, true, 0, "F", b"1.5"),
            Ok(Parsed::F64(1.5))
        );
        assert!(matches!(
            parse_field(Cast::Float64, true, 0, "F", b"x"),
            Err(RowError::Number {
                source: num::NumberError::Float,
                ..
            })
        ));
        assert_eq!(
            parse_field(Cast::Int64, true, 0, "I", b"42"),
            Ok(Parsed::I64(42))
        );
        assert!(parse_field(Cast::Int64, true, 0, "I", b"4.2").is_err());
        assert_eq!(
            parse_field(
                Cast::Decimal128 {
                    precision: 10,
                    scale: 2
                },
                true,
                0,
                "D",
                b"1.5"
            ),
            Ok(Parsed::I128(150))
        );
        assert!(
            parse_field(
                Cast::Decimal128 {
                    precision: 10,
                    scale: 2
                },
                true,
                0,
                "D",
                b"1.555"
            )
            .is_err()
        );
        assert_eq!(
            parse_field(Cast::Date32, true, 0, "D", b"1970/01/03"),
            Ok(Parsed::I64(2))
        );
        assert!(parse_field(Cast::Date32, true, 0, "D", b"x").is_err());
        assert_eq!(
            parse_field(Cast::Utf8, true, 3, "S", b"abc"),
            Ok(Parsed::Str(3))
        );
        assert!(matches!(
            parse_field(Cast::Utf8, true, 3, "S", b"\xff"),
            Err(RowError::Utf8 { .. })
        ));
        for t in [&b"true"[..], b"Y", b"1", b"T"] {
            assert_eq!(
                parse_field(Cast::Boolean, true, 0, "B", t),
                Ok(Parsed::Bool(true))
            );
        }
        for f in [&b"false"[..], b"N", b"0", b"F"] {
            assert_eq!(
                parse_field(Cast::Boolean, true, 0, "B", f),
                Ok(Parsed::Bool(false))
            );
        }
        assert!(parse_field(Cast::Boolean, true, 0, "B", b"maybe").is_err());
    }

    #[test]
    fn builders_round_trip() {
        let tz = DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some("+10:00".into()));
        let types = [
            DataType::Utf8,
            DataType::Float64,
            DataType::Int64,
            DataType::Decimal128(10, 2),
            tz.clone(),
            DataType::Date32,
            DataType::Boolean,
        ];
        let values = [
            Parsed::Str(0),
            Parsed::F64(1.5),
            Parsed::I64(7),
            Parsed::I128(150),
            Parsed::I64(1_000_000),
            Parsed::I64(3),
            Parsed::Bool(true),
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
