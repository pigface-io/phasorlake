//! The types a lake column may have, as Iceberg format version 3 names them.

use std::fmt;

/// The largest precision an Iceberg decimal holds.
pub const MAX_PRECISION: u8 = 38;

/// A column type. The set is what AEMO's files need: no `int` or `float`,
/// so no promotion from either is ever required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IcebergType {
    /// `true` or `false`.
    Boolean,
    /// A 64-bit integer.
    Long,
    /// A 64-bit float, exact for up to 15 significant digits.
    Double,
    /// An exact decimal; the scale is fixed for the life of the column.
    Decimal {
        /// Digits in all, at most 38.
        precision: u8,
        /// Digits after the point.
        scale: u8,
    },
    /// A day.
    Date,
    /// An instant in nanoseconds, adjusted to UTC.
    TimestamptzNs,
    /// Text.
    String,
}

impl IcebergType {
    /// Whether a column of this type may become `to` in metadata alone:
    /// the same type, or a decimal of larger precision and the same scale.
    pub fn promotes_to(self, to: Self) -> bool {
        match (self, to) {
            (
                Self::Decimal { precision, scale },
                Self::Decimal {
                    precision: p,
                    scale: s,
                },
            ) => s == scale && p >= precision,
            _ => self == to,
        }
    }

    /// Whether Iceberg accepts a field of this type as an identifier field:
    /// every type here but the float.
    pub const fn can_identify(self) -> bool {
        !matches!(self, Self::Double)
    }

    /// The type as written in Iceberg table metadata.
    pub fn name(self) -> String {
        match self {
            Self::Boolean => "boolean".into(),
            Self::Long => "long".into(),
            Self::Double => "double".into(),
            Self::Decimal { precision, scale } => format!("decimal({precision}, {scale})"),
            Self::Date => "date".into(),
            Self::TimestamptzNs => "timestamptz_ns".into(),
            Self::String => "string".into(),
        }
    }

    /// Reads a type as Iceberg table metadata writes it.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "boolean" => Self::Boolean,
            "long" => Self::Long,
            "double" => Self::Double,
            "date" => Self::Date,
            "timestamptz_ns" => Self::TimestamptzNs,
            "string" => Self::String,
            other => {
                let inner = other.strip_prefix("decimal(")?.strip_suffix(')')?;
                let (p, s) = inner.split_once(',')?;
                let (precision, scale) = (p.trim().parse().ok()?, s.trim().parse().ok()?);
                Self::decimal(precision, scale)?
            }
        })
    }

    /// A decimal, when Iceberg can hold it: precision 1 to 38, scale at
    /// most the precision.
    pub fn decimal(precision: u8, scale: u8) -> Option<Self> {
        ((1..=MAX_PRECISION).contains(&precision) && scale <= precision)
            .then_some(Self::Decimal { precision, scale })
    }
}

impl fmt::Display for IcebergType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_back() {
        for t in [
            IcebergType::Boolean,
            IcebergType::Long,
            IcebergType::Double,
            IcebergType::Date,
            IcebergType::TimestamptzNs,
            IcebergType::String,
            IcebergType::Decimal {
                precision: 18,
                scale: 5,
            },
        ] {
            assert_eq!(IcebergType::parse(&t.to_string()), Some(t), "{t}");
        }
        assert_eq!(
            IcebergType::parse("decimal(10,2)"),
            IcebergType::decimal(10, 2)
        );
        for bad in [
            "int",
            "float",
            "decimal(39, 0)",
            "decimal(5, 6)",
            "decimal(x, 1)",
            "decimal(5)",
            "decimal 5,1",
        ] {
            assert_eq!(IcebergType::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_a_wider_decimal_of_the_same_scale_promotes() {
        let d = |p, s| IcebergType::decimal(p, s).unwrap();
        assert!(d(10, 2).promotes_to(d(18, 2)));
        assert!(d(10, 2).promotes_to(d(10, 2)));
        assert!(!d(18, 2).promotes_to(d(10, 2)));
        assert!(!d(10, 2).promotes_to(d(18, 3)));
        assert!(IcebergType::Long.promotes_to(IcebergType::Long));
        assert!(!IcebergType::Long.promotes_to(IcebergType::Double));
        assert!(!IcebergType::Date.promotes_to(IcebergType::TimestamptzNs));
        assert!(!IcebergType::Double.promotes_to(d(18, 2)));
    }

    #[test]
    fn a_float_is_no_identifier() {
        assert!(!IcebergType::Double.can_identify());
        assert!(IcebergType::Long.can_identify());
        assert!(IcebergType::TimestamptzNs.can_identify());
    }
}
