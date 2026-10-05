//! Calendar months, the unit AEMO publishes its archive in.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A calendar month such as `2024-01`.
///
/// Ordered chronologically. Serialises as the `YYYY-MM` string users type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct YearMonth {
    year: i32,
    month: u8,
}

/// Why a string is not a month.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum YearMonthError {
    /// The text is not of the form `YYYY-MM`.
    #[error("{0:?} is not a month of the form YYYY-MM, such as 2024-01")]
    Shape(String),
    /// The month number is outside 1..=12.
    #[error("{0:?} has month {1}, which is not between 01 and 12")]
    Month(String, u8),
}

impl YearMonth {
    /// Builds a month, rejecting a month number outside 1..=12.
    pub fn new(year: i32, month: u8) -> Option<Self> {
        (1..=12).contains(&month).then_some(Self { year, month })
    }

    /// The year.
    pub const fn year(self) -> i32 {
        self.year
    }

    /// The month number, 1..=12.
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The month after this one.
    pub const fn next(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                year: self.year,
                month: self.month + 1,
            }
        }
    }

    /// Every month from `self` to `to` inclusive; empty when `to` is earlier.
    pub fn range_inclusive(self, to: Self) -> impl Iterator<Item = Self> {
        let mut cur = self;
        let mut done = cur > to;
        std::iter::from_fn(move || {
            if done {
                return None;
            }
            let out = cur;
            if cur == to {
                done = true;
            } else {
                cur = cur.next();
            }
            Some(out)
        })
    }

    /// `YYYYMM`, as AEMO writes it in archive file names.
    #[cfg(test)]
    pub(crate) fn compact(self) -> String {
        format!("{:04}{:02}", self.year, self.month)
    }

    /// Months since year 0, for arithmetic such as "how many months old".
    pub const fn ordinal(self) -> i64 {
        self.year as i64 * 12 + (self.month as i64 - 1)
    }
}

impl fmt::Display for YearMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl FromStr for YearMonth {
    type Err = YearMonthError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let shape = || YearMonthError::Shape(s.to_owned());
        let (y, m) = s.split_once('-').ok_or_else(shape)?;
        if y.len() != 4
            || m.len() != 2
            || !y.bytes().all(|b| b.is_ascii_digit())
            || !m.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(shape());
        }
        let year: i32 = y.parse().map_err(|_| shape())?;
        let month: u8 = m.parse().map_err(|_| shape())?;
        Self::new(year, month).ok_or(YearMonthError::Month(s.to_owned(), month))
    }
}

impl TryFrom<String> for YearMonth {
    type Error = YearMonthError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<YearMonth> for String {
    fn from(ym: YearMonth) -> Self {
        ym.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        let ym: YearMonth = "2024-01".parse().unwrap();
        assert_eq!((ym.year(), ym.month()), (2024, 1));
        assert_eq!(ym.to_string(), "2024-01");
        assert_eq!(ym.compact(), "202401");
    }

    #[test]
    fn rejects_bad_shapes() {
        for bad in [
            "2024-1", "202401", "2024/01", "abcd-01", "2024-13", "2024-00", "",
        ] {
            assert!(
                bad.parse::<YearMonth>().is_err(),
                "{bad} should be rejected"
            );
        }
        assert_eq!(
            "2024-13".parse::<YearMonth>().unwrap_err(),
            YearMonthError::Month("2024-13".into(), 13)
        );
        assert_eq!(YearMonth::new(2024, 0), None);
    }

    #[test]
    fn ranges_cross_years() {
        let from = YearMonth::new(2023, 11).unwrap();
        let to = YearMonth::new(2024, 2).unwrap();
        let months: Vec<String> = from.range_inclusive(to).map(|m| m.to_string()).collect();
        assert_eq!(months, ["2023-11", "2023-12", "2024-01", "2024-02"]);
        assert_eq!(to.range_inclusive(from).count(), 0);
        assert_eq!(from.range_inclusive(from).count(), 1);
    }

    #[test]
    fn ordinal_and_order() {
        let a = YearMonth::new(2023, 12).unwrap();
        let b = YearMonth::new(2024, 1).unwrap();
        assert!(a < b);
        assert_eq!(b.ordinal() - a.ordinal(), 1);
        assert_eq!(a.next(), b);
    }

    #[test]
    fn serde_round_trip() {
        let ym = YearMonth::new(2024, 7).unwrap();
        let json = serde_json::to_string(&ym).unwrap();
        assert_eq!(json, "\"2024-07\"");
        let back: YearMonth = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ym);
        assert!(serde_json::from_str::<YearMonth>("\"2024-99\"").is_err());
    }
}
