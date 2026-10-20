//! The type of a column, chosen once from everything observed of it.

use crate::types::{IcebergType, MAX_PRECISION};

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
}

impl Evidence {
    /// Evidence that holds both.
    pub fn join(self, other: Self) -> Self {
        Self {
            kind: self.kind.join(other.kind),
            int_digits: self.int_digits.max(other.int_digits),
            scale: self.scale.max(other.scale),
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

/// The most significant digits a double holds exactly.
const DOUBLE_DIGITS: u8 = 15;
/// The most digits a long holds in every case.
const LONG_DIGITS: u8 = 18;

/// The column's type from all of its evidence, so that no later value needs
/// more than a promotion:
///
/// - integers: `long`, or an exact decimal past 18 digits;
/// - decimals of up to 15 significant digits: `double`, exact for them;
/// - longer decimals: `decimal(P, S)` with S the largest scale seen, fixed
///   for the life of the column; past 38 digits, text;
/// - days `date`, times `timestamptz_ns`, anything else `string`;
/// - never a value: the declared type, else `string`.
///
/// A 32-bit float is never chosen: it rounds AEMO's prices and energies.
pub fn type_for(e: Evidence, declared: Option<IcebergType>) -> Choice {
    let digits = e.int_digits.saturating_add(e.scale);
    let ty = match e.kind {
        Kind::Empty => {
            return Choice {
                ty: declared.unwrap_or(IcebergType::String),
                observed: false,
            };
        }
        Kind::Integer if e.int_digits <= LONG_DIGITS => IcebergType::Long,
        Kind::Decimal if digits <= DOUBLE_DIGITS => IcebergType::Double,
        Kind::Integer | Kind::Decimal if digits <= MAX_PRECISION => IcebergType::Decimal {
            precision: digits,
            scale: e.scale,
        },
        Kind::Date => IcebergType::Date,
        Kind::Timestamp => IcebergType::TimestamptzNs,
        Kind::Integer | Kind::Decimal | Kind::Text => IcebergType::String,
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
        assert_eq!(ev(Integer, 4, 0).join(ev(Decimal, 2, 5)), ev(Decimal, 4, 5));
    }

    #[test]
    fn chooses_once_from_everything_seen() {
        let t = |e| type_for(e, None).ty;
        assert_eq!(t(ev(Kind::Integer, 10, 0)), IcebergType::Long);
        assert_eq!(
            t(ev(Kind::Integer, 20, 0)),
            IcebergType::decimal(20, 0).unwrap()
        );
        // RRP: 5 digits before the point, 5 after: a double holds it exactly.
        assert_eq!(t(ev(Kind::Decimal, 5, 5)), IcebergType::Double);
        assert_eq!(
            t(ev(Kind::Decimal, 12, 6)),
            IcebergType::decimal(18, 6).unwrap()
        );
        assert_eq!(t(ev(Kind::Decimal, 30, 10)), IcebergType::String);
        assert_eq!(t(ev(Kind::Date, 0, 0)), IcebergType::Date);
        assert_eq!(t(ev(Kind::Timestamp, 0, 0)), IcebergType::TimestamptzNs);
        assert_eq!(t(ev(Kind::Text, 0, 0)), IcebergType::String);
        assert!(type_for(ev(Kind::Text, 0, 0), None).observed);
    }

    #[test]
    fn a_column_never_seen_keeps_its_declared_type() {
        let c = type_for(Evidence::default(), Some(IcebergType::Double));
        assert_eq!(
            c,
            Choice {
                ty: IcebergType::Double,
                observed: false
            }
        );
        assert_eq!(type_for(Evidence::default(), None).ty, IcebergType::String);
    }
}
