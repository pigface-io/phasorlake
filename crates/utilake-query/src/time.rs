//! The two time axes of a read, as types that cannot be mixed up.
//!
//! A [`ValidTime`] is when a fact applies; a [`KnownTime`] is when it was
//! known. Both are microseconds since 1970 in UTC, as the lake stores
//! every timestamp, and neither compares with the other:
//!
//! ```compile_fail
//! use utilake_query::{KnownTime, ValidTime};
//! let v = ValidTime::new(1);
//! let k = KnownTime::new(2);
//! let _ = v < k;
//! ```
//!
//! The only ways to make one are a number of microseconds or text read in
//! the market's clock ([`parse_local`]).

use std::marker::PhantomData;

use utilake_core::timetext;

use crate::error::QueryError;

mod sealed {
    pub trait Sealed {}
}

/// One of the two axes.
pub trait Axis:
    sealed::Sealed + Copy + Clone + Eq + Ord + std::hash::Hash + std::fmt::Debug
{
    /// The axis's name in messages.
    const NAME: &'static str;
}

/// When a fact applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Valid {}
/// When a fact was known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Known {}

impl sealed::Sealed for Valid {}
impl sealed::Sealed for Known {}
impl Axis for Valid {
    const NAME: &'static str = "valid";
}
impl Axis for Known {
    const NAME: &'static str = "known";
}

/// An instant on one axis: microseconds since 1970, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Micros<A: Axis>(i64, PhantomData<A>);

impl<A: Axis> Micros<A> {
    /// From microseconds since 1970.
    pub const fn new(us: i64) -> Self {
        Self(us, PhantomData)
    }

    /// Microseconds since 1970.
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<A: Axis> std::fmt::Display for Micros<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&rfc3339(self.0, 0))
    }
}

/// When a fact applies.
pub type ValidTime = Micros<Valid>;
/// When a fact was known.
pub type KnownTime = Micros<Known>;

/// The value the lake holds for AEMO's "no end" dates (31 December 2999
/// and 9999): nanoseconds clamped to `i64::MAX`, in microseconds.
pub const OPEN_US: i64 = i64::MAX / 1_000;

/// Where a version stops applying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidEnd {
    /// At this time.
    At(ValidTime),
    /// Never: the version is current, or its end is AEMO's "no end" date.
    Open,
}

impl ValidEnd {
    /// From a lake value: the clamped "no end" date reads as open.
    pub const fn from_us(us: i64) -> Self {
        if us >= OPEN_US {
            Self::Open
        } else {
            Self::At(ValidTime::new(us))
        }
    }

    /// Whether the end is after `t`, an open end being after everything.
    pub fn is_after(self, t: ValidTime) -> bool {
        match self {
            Self::Open => true,
            Self::At(e) => e > t,
        }
    }
}

/// The fixed offset of a market's clock, seconds east of UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MarketOffset(pub i32);

impl MarketOffset {
    /// The offset as Arrow and Iceberg write it, `+10:00`.
    pub fn tz(self) -> String {
        let sign = if self.0 < 0 { '-' } else { '+' };
        let abs = self.0.unsigned_abs();
        format!("{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60)
    }

    /// Microseconds since 1970 of a local day's midnight, from days since
    /// 1970 of the date (as Arrow's `Date32` counts them).
    pub const fn midnight_us(self, days: i32) -> i64 {
        days as i64 * 86_400_000_000 - self.0 as i64 * 1_000_000
    }
}

/// Reads text as an instant: RFC 3339 with an offset, or a date
/// (`YYYY-MM-DD`, that day's midnight) or a local date and time
/// (`YYYY-MM-DDTHH:MM[:SS]`, a space allowed for the `T`) in `off`.
pub fn parse_local<A: Axis>(text: &str, off: MarketOffset) -> Result<Micros<A>, QueryError> {
    let t = text.trim();
    if let Some(s) = timetext::parse_rfc3339(t) {
        return Ok(Micros::new(from_stamp(&s)));
    }
    let padded = match t.len() {
        10 => format!("{t}T00:00:00"),
        16 => format!("{t}:00"),
        _ => t.to_owned(),
    };
    timetext::parse_naive(&padded, off.0)
        .map(|s| Micros::new(from_stamp(&s)))
        .ok_or_else(|| QueryError::BadTime(text.to_owned()))
}

fn from_stamp(s: &timetext::Stamp) -> i64 {
    s.secs_utc * 1_000_000 + i64::from(s.nanos / 1_000)
}

/// An instant as RFC 3339 in an offset, to the second.
pub fn rfc3339(us: i64, offset_secs: i32) -> String {
    timetext::format_rfc3339(us.div_euclid(1_000_000), offset_secs)
}

/// The rows a read keeps on the valid axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidWindow {
    /// An interval. What "between" means depends on the table's axis: see
    /// [`crate::window`].
    Range {
        /// The start.
        after: ValidTime,
        /// The end.
        upto: ValidTime,
    },
    /// One instant: the version in force then.
    At(ValidTime),
}

impl ValidWindow {
    /// A range; `after` must be before `upto`.
    pub fn range(after: ValidTime, upto: ValidTime) -> Result<Self, QueryError> {
        if after < upto {
            Ok(Self::Range { after, upto })
        } else {
            Err(QueryError::WindowEmpty {
                after: after.to_string(),
                upto: upto.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEM: MarketOffset = MarketOffset(36_000);

    #[test]
    fn text_in_the_market_clock() {
        // 2024-01-01 00:00 in +10:00 is 2023-12-31 14:00 UTC.
        let midnight = 1_704_031_200_000_000;
        assert_eq!(
            parse_local::<Valid>("2024-01-01", NEM).unwrap().get(),
            midnight
        );
        assert_eq!(
            parse_local::<Valid>("2024-01-01T00:00", NEM).unwrap().get(),
            midnight
        );
        assert_eq!(
            parse_local::<Valid>("2024-01-01 00:00:05", NEM)
                .unwrap()
                .get(),
            midnight + 5_000_000
        );
        assert_eq!(
            parse_local::<Known>("2023-12-31T14:00:00Z", NEM)
                .unwrap()
                .get(),
            midnight
        );
        assert_eq!(
            parse_local::<Known>("2024-01-01T00:00:00.5+10:00", NEM)
                .unwrap()
                .get(),
            midnight + 500_000
        );
        let err = parse_local::<Valid>("soon", NEM).unwrap_err();
        assert!(matches!(err, QueryError::BadTime(t) if t == "soon"));
        assert_eq!(ValidTime::new(midnight).to_string(), "2023-12-31T14:00:00Z");
        assert_eq!(rfc3339(midnight, 36_000), "2024-01-01T00:00:00+10:00");
    }

    #[test]
    fn offsets_and_days() {
        assert_eq!(NEM.tz(), "+10:00");
        assert_eq!(MarketOffset(-19_800).tz(), "-05:30");
        // Day 19723 is 2024-01-01.
        assert_eq!(NEM.midnight_us(19_723), 1_704_031_200_000_000);
    }

    #[test]
    fn open_ends() {
        assert_eq!(ValidEnd::from_us(OPEN_US), ValidEnd::Open);
        assert_eq!(ValidEnd::from_us(OPEN_US + 1), ValidEnd::Open);
        assert_eq!(ValidEnd::from_us(5), ValidEnd::At(ValidTime::new(5)));
        assert!(ValidEnd::Open.is_after(ValidTime::new(i64::MAX)));
        assert!(ValidEnd::from_us(5).is_after(ValidTime::new(4)));
        assert!(!ValidEnd::from_us(5).is_after(ValidTime::new(5)));
    }

    #[test]
    fn windows() {
        let w = ValidWindow::range(ValidTime::new(1), ValidTime::new(2)).unwrap();
        assert!(matches!(w, ValidWindow::Range { .. }));
        let e = ValidWindow::range(ValidTime::new(2), ValidTime::new(2)).unwrap_err();
        assert!(matches!(e, QueryError::WindowEmpty { .. }));
        assert_eq!(Valid::NAME, "valid");
        assert_eq!(Known::NAME, "known");
    }
}
