//! The type of a column, chosen once from everything observed of it.

use crate::types::IcebergType;

/// What a column's values were seen to be, on a widening lattice: `Empty`
/// below everything, `Integer` below `Decimal`, `Date` below `Timestamp`,
/// and `Text` above everything.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// No value was ever seen.
    #[default]
    Empty,
    /// Whole numbers.
    Integer,
    /// Numbers with a fractional part.
    Decimal,
    /// Days.
    Date,
    /// Days with a time of day.
    Timestamp,
    /// Anything else.
    Text,
}

impl Kind {
    /// Every kind, bottom of the lattice first.
    pub const ALL: [Self; 6] = [
        Self::Empty,
        Self::Integer,
        Self::Decimal,
        Self::Date,
        Self::Timestamp,
        Self::Text,
    ];

    /// One letter per kind, for compact records: `-`, `i`, `d`, `D`, `t`, `s`.
    pub const fn letter(self) -> char {
        match self {
            Self::Empty => '-',
            Self::Integer => 'i',
            Self::Decimal => 'd',
            Self::Date => 'D',
            Self::Timestamp => 't',
            Self::Text => 's',
        }
    }

    /// The kind of a letter from [`Kind::letter`].
    pub fn of_letter(c: char) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.letter() == c)
    }

    /// The least kind that holds both.
    pub fn join(self, other: Self) -> Self {
        use Kind::{Date, Decimal, Empty, Integer, Text, Timestamp};
        match (self, other) {
            (Empty, k) | (k, Empty) => k,
            (a, b) if a == b => a,
            (Integer | Decimal, Integer | Decimal) => Decimal,
            (Date | Timestamp, Date | Timestamp) => Timestamp,
            _ => Text,
        }
    }
}

/// What was seen of a column over its whole history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Evidence {
    /// The kind of every value.
    pub kind: Kind,
    /// The most digits seen before the point.
    pub int_digits: u8,
    /// The most digits seen after it.
    pub scale: u8,
    /// The most significant digits of any one value: from its first
    /// non-zero digit to its last digit.
    pub sig_digits: u8,
}

impl Evidence {
    /// Evidence that holds both.
    pub fn join(self, other: Self) -> Self {
        Self {
            kind: self.kind.join(other.kind),
            int_digits: self.int_digits.max(other.int_digits),
            scale: self.scale.max(other.scale),
            sig_digits: self.sig_digits.max(other.sig_digits),
        }
    }
}

/// A column's type, and whether data decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    /// The type.
    pub ty: IcebergType,
    /// False when no value was ever seen and the type is the declared one.
    pub observed: bool,
}

/// What AEMO declares of a column (its control file, its data model), and
/// whether it is in the table's key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Declared {
    /// Declared whole: the model's `NUMERIC(p,0)`, or a control file's
    /// `INTEGER EXTERNAL`.
    pub whole: bool,
    /// Declared with a fraction: the model's scale above 0, or a control
    /// file's `FLOAT`/`DECIMAL EXTERNAL` with no model.
    pub fractional: bool,
    /// The model's precision, when it declares one.
    pub precision: Option<u8>,
    /// The model's scale, when it declares one.
    pub scale: Option<u8>,
    /// The type a column never seen with a value keeps.
    pub fallback: Option<IcebergType>,
    /// Part of the table's key: Iceberg refuses a float there.
    pub key: bool,
}

/// The most digits a decimal stored as a 64-bit integer holds.
const INT64_DIGITS: u8 = 18;
/// The most digits a long holds in every case.
const LONG_DIGITS: u8 = 18;
/// Significant digits from which a value is a float printed in full
/// (`239.99999999999997` has 17).
const FLOAT_PRINTED: u8 = 16;

/// The column's type from all of its evidence and its declaration, chosen
/// once for the life of the column:
///
/// - numbers declared whole, or in the key, that held only whole numbers:
///   `long`;
/// - floats printed in full (more digits after the point than declared,
///   16 or more significant): `double`;
/// - other numbers: `decimal(P, S)` with S the larger of the declared and
///   the largest observed scale, fixed for life, and P what the values and
///   the declaration need, while it fits a 64-bit integer (18 digits);
///   past that, `double`;
/// - days `date`, times `timestamptz`, anything else `string`;
/// - never a value: the declared type, else `string`.
///
/// A whole-number column with nothing declaring it whole is a
/// `decimal(P, 0)`, not a `long`: a later fraction is then a wider scale
/// to decide, not a type Iceberg cannot change. A 32-bit float is never
/// chosen.
pub fn choose(e: Evidence, d: Declared) -> Choice {
    let declared_scale = d.scale.unwrap_or(0);
    let scale = e.scale.max(declared_scale);
    let digits = e
        .int_digits
        .saturating_add(scale)
        .max(d.precision.unwrap_or(0));
    let ty = match e.kind {
        Kind::Empty => {
            return Choice {
                ty: d.fallback.unwrap_or(IcebergType::String),
                observed: false,
            };
        }
        Kind::Integer if (d.whole || d.key) && e.int_digits <= LONG_DIGITS => IcebergType::Long,
        Kind::Decimal
            if e.sig_digits >= FLOAT_PRINTED && d.scale.is_none_or(|s| e.scale > s) && !d.key =>
        {
            IcebergType::Double
        }
        Kind::Integer | Kind::Decimal if digits <= INT64_DIGITS => IcebergType::Decimal {
            precision: digits.max(1),
            scale,
        },
        Kind::Integer | Kind::Decimal if d.key => IcebergType::String,
        Kind::Integer | Kind::Decimal => IcebergType::Double,
        Kind::Date => IcebergType::Date,
        Kind::Timestamp => IcebergType::Timestamptz,
        Kind::Text => IcebergType::String,
    };
    Choice { ty, observed: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: Kind, int_digits: u8, scale: u8) -> Evidence {
        Evidence {
            kind,
            int_digits,
            scale,
            sig_digits: int_digits + scale,
        }
    }

    #[test]
    fn the_lattice_widens() {
        use Kind::*;
        assert_eq!(Empty.join(Integer), Integer);
        assert_eq!(Date.join(Empty), Date);
        assert_eq!(Integer.join(Decimal), Decimal);
        assert_eq!(Date.join(Timestamp), Timestamp);
        assert_eq!(Integer.join(Date), Text);
        assert_eq!(Text.join(Integer), Text);
        assert_eq!(Decimal.join(Decimal), Decimal);
        assert_eq!(Evidence::default().kind, Empty);
        for k in Kind::ALL {
            assert_eq!(Kind::of_letter(k.letter()), Some(k));
        }
        assert_eq!(Kind::of_letter('?'), None);
        assert_eq!(
            ev(Integer, 4, 0).join(ev(Decimal, 2, 5)),
            Evidence {
                kind: Decimal,
                int_digits: 4,
                scale: 5,
                sig_digits: 7
            }
        );
    }

    #[test]
    fn whole_numbers_are_long_only_where_declared_whole_or_keyed() {
        let whole = ev(Kind::Integer, 3, 0);
        let c = |d: Declared| choose(whole, d).ty;
        assert_eq!(
            c(Declared {
                whole: true,
                ..Declared::default()
            }),
            IcebergType::Long
        );
        assert_eq!(
            c(Declared {
                key: true,
                ..Declared::default()
            }),
            IcebergType::Long
        );
        // Nothing declares it whole: a decimal of scale 0, not a long.
        assert_eq!(c(Declared::default()), IcebergType::decimal(3, 0).unwrap());
        // Declared with a fraction: the declared scale, fixed for life.
        let d = Declared {
            fractional: true,
            precision: Some(15),
            scale: Some(5),
            ..Declared::default()
        };
        assert_eq!(c(d), IcebergType::decimal(15, 5).unwrap());
        // A long past 18 digits is no long.
        let huge = ev(Kind::Integer, 20, 0);
        assert_eq!(
            choose(
                huge,
                Declared {
                    whole: true,
                    ..Declared::default()
                }
            )
            .ty,
            IcebergType::Double
        );
        assert_eq!(
            choose(
                huge,
                Declared {
                    key: true,
                    ..Declared::default()
                }
            )
            .ty,
            IcebergType::String
        );
    }

    #[test]
    fn fractions_are_exact_decimals_and_printed_floats_doubles() {
        // RRP: declared NUMERIC(15,5), seen with 5 places.
        let rrp = ev(Kind::Decimal, 5, 5);
        let d = Declared {
            fractional: true,
            precision: Some(15),
            scale: Some(5),
            ..Declared::default()
        };
        assert_eq!(choose(rrp, d).ty, IcebergType::decimal(15, 5).unwrap());
        // Seen with more places than declared: the wider scale wins.
        assert_eq!(
            choose(ev(Kind::Decimal, 3, 7), d).ty,
            IcebergType::decimal(15, 7).unwrap()
        );
        // A float printed in full: 17 significant digits, more places than
        // declared.
        let float = Evidence {
            kind: Kind::Decimal,
            int_digits: 4,
            scale: 17,
            sig_digits: 17,
        };
        let six = Declared {
            fractional: true,
            precision: Some(6),
            scale: Some(0),
            ..Declared::default()
        };
        assert_eq!(choose(float, six).ty, IcebergType::Double);
        assert_eq!(choose(float, Declared::default()).ty, IcebergType::Double);
        // In a key it may not be a float: past 18 digits it is text.
        assert_eq!(
            choose(float, Declared { key: true, ..six }).ty,
            IcebergType::String
        );
        // Too many digits for a 64-bit decimal, and not a printed float.
        let long_dec = Evidence {
            kind: Kind::Decimal,
            int_digits: 12,
            scale: 9,
            sig_digits: 14,
        };
        assert_eq!(
            choose(long_dec, Declared::default()).ty,
            IcebergType::Double
        );
        assert_eq!(choose(ev(Kind::Date, 0, 0), d).ty, IcebergType::Date);
        assert_eq!(
            choose(ev(Kind::Timestamp, 0, 0), d).ty,
            IcebergType::Timestamptz
        );
        assert_eq!(choose(ev(Kind::Text, 0, 0), d).ty, IcebergType::String);
        assert!(choose(ev(Kind::Text, 0, 0), d).observed);
        // A zero is a decimal of one digit.
        assert_eq!(
            choose(ev(Kind::Integer, 0, 0), Declared::default()).ty,
            IcebergType::decimal(1, 0).unwrap()
        );
    }

    #[test]
    fn a_column_never_seen_keeps_its_declared_type() {
        let d = Declared {
            fallback: Some(IcebergType::Double),
            ..Declared::default()
        };
        assert_eq!(
            choose(Evidence::default(), d),
            Choice {
                ty: IcebergType::Double,
                observed: false
            }
        );
        assert_eq!(
            choose(Evidence::default(), Declared::default()).ty,
            IcebergType::String
        );
    }
}
