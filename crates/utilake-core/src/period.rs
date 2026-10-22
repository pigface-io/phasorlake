//! Periods: the calendar month AEMO cuts its archives in, the day other
//! sources publish by, and a range of either.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub use utilake_mms_decode::month::{YearMonth, YearMonthError};

/// A calendar day such as `2024-01-15`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CivilDay {
    year: i32,
    month: u8,
    day: u8,
}

impl CivilDay {
    /// Builds a day, rejecting a month outside 1..=12 or a day the month
    /// does not have.
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        let ym = YearMonth::new(year, month)?;
        let days = crate::calendar::days_in_month(ym.year(), u32::from(ym.month()));
        (day >= 1 && u32::from(day) <= days).then_some(Self { year, month, day })
    }

    /// The month the day is in.
    pub fn month(self) -> YearMonth {
        YearMonth::new(self.year, self.month).expect("a day's month is valid")
    }

    /// The day of the month, 1..=31.
    pub const fn day(self) -> u8 {
        self.day
    }
}

impl fmt::Display for CivilDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl FromStr for CivilDay {
    type Err = YearMonthError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let shape = || YearMonthError::Shape(s.to_owned());
        let (ym, d) = s.rsplit_once('-').ok_or_else(shape)?;
        if d.len() != 2 || !d.bytes().all(|b| b.is_ascii_digit()) || ym.len() != 7 {
            return Err(shape());
        }
        let ym: YearMonth = ym.parse()?;
        let day: u8 = d.parse().map_err(|_| shape())?;
        Self::new(ym.year(), ym.month(), day).ok_or_else(shape)
    }
}

impl TryFrom<String> for CivilDay {
    type Error = YearMonthError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<CivilDay> for String {
    fn from(d: CivilDay) -> Self {
        d.to_string()
    }
}

/// The span of time one published file covers: a month for AEMO's monthly
/// archives, a day for sources published daily. Written as its text,
/// `YYYY-MM` or `YYYY-MM-DD`, so a month reads back as it always did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Period {
    /// A calendar month.
    Month(YearMonth),
    /// A calendar day.
    Day(CivilDay),
}

impl Period {
    /// The month the period is in, or is.
    pub fn month(self) -> YearMonth {
        match self {
            Self::Month(m) => m,
            Self::Day(d) => d.month(),
        }
    }
}

impl From<YearMonth> for Period {
    fn from(m: YearMonth) -> Self {
        Self::Month(m)
    }
}

impl From<CivilDay> for Period {
    fn from(d: CivilDay) -> Self {
        Self::Day(d)
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Month(m) => m.fmt(f),
            Self::Day(d) => d.fmt(f),
        }
    }
}

impl FromStr for Period {
    type Err = YearMonthError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 10 {
            s.parse().map(Self::Day)
        } else {
            s.parse().map(Self::Month)
        }
    }
}

impl TryFrom<String> for Period {
    type Error = YearMonthError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Period> for String {
    fn from(p: Period) -> Self {
        p.to_string()
    }
}

/// An inclusive span of periods: what a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeRange {
    /// The first period.
    pub from: Period,
    /// The last period, inclusive.
    pub to: Period,
}

impl TimeRange {
    /// The months `from` to `to`, inclusive.
    pub fn months(from: YearMonth, to: YearMonth) -> Self {
        Self {
            from: Period::Month(from),
            to: Period::Month(to),
        }
    }

    /// One period.
    pub fn one(p: impl Into<Period>) -> Self {
        let p = p.into();
        Self { from: p, to: p }
    }

    /// The first month the range touches.
    pub fn first_month(&self) -> YearMonth {
        self.from.month()
    }

    /// The last month the range touches.
    pub fn last_month(&self) -> YearMonth {
        self.to.month()
    }

    /// Every month the range touches, in order; empty when `to` is earlier
    /// than `from`.
    pub fn months_iter(&self) -> impl Iterator<Item = YearMonth> {
        self.first_month().range_inclusive(self.last_month())
    }

    /// Whether a period lies within the range.
    pub fn contains(&self, p: impl Into<Period>) -> bool {
        let p = p.into();
        match (self.from, self.to, p) {
            // A month contains its days, and a day lies within its month.
            (Period::Month(a), Period::Month(b), Period::Day(d)) => {
                a <= d.month() && d.month() <= b
            }
            (Period::Day(a), Period::Day(b), Period::Month(m)) => a.month() <= m && m <= b.month(),
            (a, b, p) => a <= p && p <= b,
        }
    }
}

/// `YYYY-MM` for one period, `from..to` otherwise.
impl fmt::Display for TimeRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.from == self.to {
            self.from.fmt(f)
        } else {
            write!(f, "{}..{}", self.from, self.to)
        }
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

    #[test]
    fn days_and_periods() {
        let d: CivilDay = "2024-02-29".parse().unwrap();
        assert_eq!((d.month(), d.day()), (YearMonth::new(2024, 2).unwrap(), 29));
        assert_eq!(d.to_string(), "2024-02-29");
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-01-00",
            "2024-01-1",
            "2024-1-01",
            "x",
            "",
        ] {
            assert!(bad.parse::<CivilDay>().is_err(), "{bad}");
        }
        assert_eq!(CivilDay::new(2024, 4, 31), None);
        let m: Period = "2024-01".parse().unwrap();
        let day: Period = "2024-01-15".parse().unwrap();
        assert_eq!(m, Period::Month(YearMonth::new(2024, 1).unwrap()));
        assert_eq!(day.month(), m.month());
        assert!(m < day);
        assert_eq!(m.to_string(), "2024-01");
        assert_eq!(day.to_string(), "2024-01-15");
        assert_eq!(serde_json::to_string(&m).unwrap(), "\"2024-01\"");
        assert_eq!(
            serde_json::from_str::<Period>("\"2024-01-15\"").unwrap(),
            day
        );
        assert!("2024-1".parse::<Period>().is_err());
        assert_eq!(Period::from(CivilDay::new(2024, 1, 15).unwrap()), day);
    }

    #[test]
    fn ranges_of_months_and_days() {
        let a = YearMonth::new(2023, 11).unwrap();
        let b = YearMonth::new(2024, 2).unwrap();
        let r = TimeRange::months(a, b);
        assert_eq!(r.months_iter().count(), 4);
        assert_eq!((r.first_month(), r.last_month()), (a, b));
        assert_eq!(r.to_string(), "2023-11..2024-02");
        assert_eq!(TimeRange::one(a).to_string(), "2023-11");
        assert!(r.contains(a) && r.contains(b));
        assert!(!r.contains(YearMonth::new(2024, 3).unwrap()));
        assert!(r.contains(CivilDay::new(2024, 1, 15).unwrap()));
        let days = TimeRange {
            from: Period::Day(CivilDay::new(2024, 1, 10).unwrap()),
            to: Period::Day(CivilDay::new(2024, 1, 20).unwrap()),
        };
        assert_eq!(days.months_iter().count(), 1);
        assert!(days.contains(YearMonth::new(2024, 1).unwrap()));
        assert!(days.contains(CivilDay::new(2024, 1, 15).unwrap()));
        assert!(!days.contains(CivilDay::new(2024, 1, 21).unwrap()));
        assert_eq!(days.to_string(), "2024-01-10..2024-01-20");
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"from":"2023-11","to":"2024-02"}"#);
        assert_eq!(serde_json::from_str::<TimeRange>(&json).unwrap(), r);
    }
}
