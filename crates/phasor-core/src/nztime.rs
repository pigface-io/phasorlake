//! New Zealand's clock: standard time (NZST, +12:00) and daylight time
//! (NZDT, +13:00), and the trading periods the Electricity Authority keys
//! its data by. Integer arithmetic on the family's calendar: this runs on
//! the hot path, with no calendar library.
//!
//! Daylight time, since the 2007 change: from the last Sunday of September
//! (02:00 NZST becomes 03:00 NZDT) to the first Sunday of April (03:00 NZDT
//! becomes 02:00 NZST). From 1990 to 2007: from the first Sunday of October
//! to the third Sunday of March. Before 1990 every date is read as standard
//! time: the data begins in 1996.
//!
//! A trading period is the n-th half hour of elapsed time from the trading
//! date's local midnight: 48 a day, 46 on the day daylight time starts, 50
//! on the day it ends. So its start needs only the offset at midnight.

use phasor_provenance::civil::{days_from_civil, days_in_month, weekday};

/// Seconds east of UTC of New Zealand Standard Time.
pub const NZST_OFFSET_SECS: i32 = 12 * 3600;
/// Seconds east of UTC of New Zealand Daylight Time.
pub const NZDT_OFFSET_SECS: i32 = 13 * 3600;

/// A trading period, in seconds.
const PERIOD_SECS: i64 = 30 * 60;

/// Sunday, as [`weekday`] counts it (0 is Monday).
const SUNDAY: u32 = 6;

/// The day of the `n`-th Sunday of month `m` of year `y`.
fn nth_sunday(y: i32, m: u32, n: u32) -> u32 {
    let first = weekday(days_from_civil(y, m, 1));
    1 + (SUNDAY + 7 - first) % 7 + 7 * (n - 1)
}

/// The day of the last Sunday of month `m` of year `y`.
fn last_sunday(y: i32, m: u32) -> u32 {
    let last = days_in_month(y, m);
    last - (weekday(days_from_civil(y, m, last)) + 7 - SUNDAY) % 7
}

/// The month and day daylight time starts in year `y`; none before 1990.
fn start(y: i32) -> Option<(u32, u32)> {
    match y {
        ..1990 => None,
        1990..2007 => Some((10, nth_sunday(y, 10, 1))),
        _ => Some((9, last_sunday(y, 9))),
    }
}

/// The month and day daylight time ends in year `y`; none before 1990.
fn end(y: i32) -> Option<(u32, u32)> {
    match y {
        ..1990 => None,
        1990..2008 => Some((3, nth_sunday(y, 3, 3))),
        _ => Some((4, nth_sunday(y, 4, 1))),
    }
}

/// Whether daylight time is in force at the local midnight that begins the
/// day `(y, m, d)`: after the day it starts (that day begins in standard
/// time) and up to the day it ends (that day begins in daylight time).
#[must_use]
pub fn daylight_at_midnight(y: i32, m: u32, d: u32) -> bool {
    let (Some(start), Some(end)) = (start(y), end(y)) else {
        return false;
    };
    (m, d) > start || (m, d) <= end
}

/// The offset from UTC, in seconds, at the local midnight that begins the
/// day `(y, m, d)`.
#[must_use]
pub fn offset_at_midnight(y: i32, m: u32, d: u32) -> i32 {
    if daylight_at_midnight(y, m, d) {
        NZDT_OFFSET_SECS
    } else {
        NZST_OFFSET_SECS
    }
}

/// How many trading periods the day `(y, m, d)` has: 48, 46 when daylight
/// time starts in it, 50 when it ends in it.
#[must_use]
pub fn periods_in_day(y: i32, m: u32, d: u32) -> u32 {
    let days = days_from_civil(y, m, d);
    let (ny, nm, nd) = phasor_provenance::civil::civil_from_days(days + 1);
    match (
        daylight_at_midnight(y, m, d),
        daylight_at_midnight(ny, nm, nd),
    ) {
        (false, true) => 46,
        (true, false) => 50,
        _ => 48,
    }
}

/// The start of trading period `period` of the trading date `(y, m, d)`,
/// as microseconds since 1970 UTC; `None` for a period the day does not
/// have.
#[must_use]
pub fn trading_period_start_us(y: i32, m: u32, d: u32, period: u32) -> Option<i64> {
    if period == 0 || period > periods_in_day(y, m, d) {
        return None;
    }
    let midnight = days_from_civil(y, m, d) * 86_400 - i64::from(offset_at_midnight(y, m, d));
    Some((midnight + i64::from(period - 1) * PERIOD_SECS) * 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Microseconds since 1970 of a UTC date and time.
    fn utc(y: i32, m: u32, d: u32, hh: i64, mm: i64) -> i64 {
        (days_from_civil(y, m, d) * 86_400 + hh * 3_600 + mm * 60) * 1_000_000
    }

    #[test]
    fn the_changeover_sundays_since_the_rules_changed_and_before() {
        // Since 2007: the last Sunday of September to the first of April.
        assert_eq!(start(2026), Some((9, 27)));
        assert_eq!(end(2026), Some((4, 5)));
        assert_eq!(start(2025), Some((9, 28)));
        assert_eq!(end(2025), Some((4, 6)));
        assert_eq!(
            start(2007),
            Some((9, 30)),
            "the first start under the new rule"
        );
        assert_eq!(end(2008), Some((4, 6)), "the first end under the new rule");
        // Before: the first Sunday of October to the third of March.
        assert_eq!(start(2006), Some((10, 1)));
        assert_eq!(end(2007), Some((3, 18)));
        assert_eq!(start(1996), Some((10, 6)));
        assert_eq!(end(1997), Some((3, 16)));
        assert_eq!(start(1989), None);
        assert_eq!(end(1989), None);
    }

    #[test]
    fn midnight_is_daylight_time_after_the_start_and_up_to_the_end() {
        assert!(
            !daylight_at_midnight(2026, 9, 27),
            "the start day begins in NZST"
        );
        assert!(daylight_at_midnight(2026, 9, 28));
        assert!(daylight_at_midnight(2026, 12, 31));
        assert!(daylight_at_midnight(2026, 1, 1));
        assert!(
            daylight_at_midnight(2026, 4, 5),
            "the end day begins in NZDT"
        );
        assert!(!daylight_at_midnight(2026, 4, 6));
        assert!(!daylight_at_midnight(2026, 7, 1));
        assert!(
            !daylight_at_midnight(1989, 1, 1),
            "standard time before 1990"
        );
        assert_eq!(offset_at_midnight(2026, 1, 5), NZDT_OFFSET_SECS);
        assert_eq!(offset_at_midnight(2026, 7, 1), NZST_OFFSET_SECS);
    }

    #[test]
    fn a_day_has_48_periods_46_when_daylight_time_starts_and_50_when_it_ends() {
        assert_eq!(periods_in_day(2026, 1, 5), 48);
        assert_eq!(periods_in_day(2026, 9, 27), 46);
        assert_eq!(periods_in_day(2026, 4, 5), 50);
        assert_eq!(periods_in_day(2026, 9, 26), 48);
        assert_eq!(periods_in_day(2026, 4, 4), 48);
        assert_eq!(periods_in_day(2026, 12, 31), 48, "across the year's end");
        assert_eq!(periods_in_day(2007, 9, 30), 46);
        assert_eq!(periods_in_day(2007, 3, 18), 50);
    }

    #[test]
    fn a_trading_periods_start_is_midnight_in_the_days_clock_plus_the_half_hours_before_it() {
        // 2026-01-05 is in daylight time: midnight is 11:00 UTC the day before.
        assert_eq!(
            trading_period_start_us(2026, 1, 5, 1),
            Some(utc(2026, 1, 4, 11, 0))
        );
        assert_eq!(
            trading_period_start_us(2026, 1, 5, 48),
            Some(utc(2026, 1, 5, 10, 30))
        );
        assert_eq!(trading_period_start_us(2026, 1, 5, 49), None);
        assert_eq!(trading_period_start_us(2026, 1, 5, 0), None);
        // Winter: midnight is 12:00 UTC the day before.
        assert_eq!(
            trading_period_start_us(2026, 7, 1, 1),
            Some(utc(2026, 6, 30, 12, 0))
        );
        // The day daylight time ends: 50 periods, 1,800 s apart, the last
        // ending at the next midnight (NZST, 12:00 UTC).
        let ends = trading_period_start_us(2026, 4, 5, 50).unwrap() + PERIOD_SECS * 1_000_000;
        assert_eq!(ends, utc(2026, 4, 5, 12, 0));
        assert_eq!(trading_period_start_us(2026, 4, 5, 51), None);
        // The day it starts: 46 periods, the last ending at the next
        // midnight (NZDT, 11:00 UTC).
        let ends = trading_period_start_us(2026, 9, 27, 46).unwrap() + PERIOD_SECS * 1_000_000;
        assert_eq!(ends, utc(2026, 9, 27, 11, 0));
        assert_eq!(trading_period_start_us(2026, 9, 27, 47), None);
        // Every period of a year is 1,800 s after the one before it, across
        // every day, and the years before 1990 run on standard time.
        let mut last = trading_period_start_us(2025, 12, 31, 48).unwrap();
        for m in 1..=12 {
            for d in 1..=days_in_month(2026, m) {
                for p in 1..=periods_in_day(2026, m, d) {
                    let t = trading_period_start_us(2026, m, d, p).unwrap();
                    assert_eq!(t - last, PERIOD_SECS * 1_000_000, "{m}-{d} {p}");
                    last = t;
                }
            }
        }
        assert_eq!(
            trading_period_start_us(1989, 1, 1, 1),
            Some(utc(1988, 12, 31, 12, 0))
        );
    }
}
