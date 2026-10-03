//! The MMS timestamp format, `yyyy/mm/dd hh24:mi:ss`, parsed without a
//! calendar library.
//!
//! AEMO writes every timestamp in market time: the NEM runs on Australian
//! Eastern Standard Time all year (UTC+10, no daylight saving) and the WEM on
//! Australian Western Standard Time (UTC+8). The lake stores instants as
//! microseconds since the Unix epoch in UTC, with the market offset recorded
//! in the Arrow type, so a value is `local_seconds - offset`.
//!
//! This module is on the hot path: no allocation, no `chrono`, no `jiff`.

/// Seconds east of UTC for the National Electricity Market (AEST, fixed).
pub const NEM_OFFSET_SECS: i32 = 10 * 3600;
/// Seconds east of UTC for the Wholesale Electricity Market (AWST, fixed).
pub const WEM_OFFSET_SECS: i32 = 8 * 3600;

/// Microseconds in one day.
pub const MICROS_PER_DAY: i64 = 86_400 * 1_000_000;

/// Why a byte string is not an MMS timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampError {
    /// Not 19 bytes (`yyyy/mm/dd hh:mm:ss`) or 10 bytes (`yyyy/mm/dd`).
    Length(usize),
    /// A separator or digit is in the wrong place.
    Shape,
    /// Month outside 1..=12.
    Month(u8),
    /// Day outside the month.
    Day(u8),
    /// Hour outside 0..=23.
    Hour(u8),
    /// Minute outside 0..=59.
    Minute(u8),
    /// Second outside 0..=59.
    Second(u8),
}

impl std::fmt::Display for TimestampError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Length(n) => write!(
                f,
                "has {n} bytes; a timestamp is 19 bytes (yyyy/mm/dd hh:mm:ss) or a date 10 bytes"
            ),
            Self::Shape => write!(
                f,
                "does not have digits and separators in the places of yyyy/mm/dd hh:mm:ss"
            ),
            Self::Month(m) => write!(f, "has month {m}, which is not between 1 and 12"),
            Self::Day(d) => write!(f, "has day {d}, which is not a day of that month"),
            Self::Hour(h) => write!(f, "has hour {h}, which is not between 0 and 23"),
            Self::Minute(m) => write!(f, "has minute {m}, which is not between 0 and 59"),
            Self::Second(s) => write!(f, "has second {s}, which is not between 0 and 59"),
        }
    }
}

impl std::error::Error for TimestampError {}

#[inline]
fn digit(b: u8) -> Option<u32> {
    let d = b.wrapping_sub(b'0');
    (d < 10).then_some(u32::from(d))
}

#[inline]
fn two(s: &[u8]) -> Option<u32> {
    Some(digit(s[0])? * 10 + digit(s[1])?)
}

#[inline]
fn four(s: &[u8]) -> Option<u32> {
    Some(two(&s[0..2])? * 100 + two(&s[2..4])?)
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian).
///
/// Howard Hinnant's `days_from_civil`, valid for every year an `i32` holds.
#[inline]
pub const fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count from 1970-01-01, as `(year, month, day)`.
pub const fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

#[inline]
const fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[inline]
const fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

/// Parses `yyyy/mm/dd` (10 bytes) or `yyyy/mm/dd hh:mm:ss` (19 bytes) as local
/// seconds since the epoch, before any offset is applied.
///
/// AEMO sometimes writes a bare date for a DATE column whose time is midnight;
/// both shapes are accepted. The `/` separators are what MMS uses; `-` is not
/// accepted so a mislabelled ISO column is caught rather than silently read.
pub fn parse_local_seconds(s: &[u8]) -> Result<i64, TimestampError> {
    let (date, time) = match s.len() {
        19 => {
            if s[10] != b' ' || s[13] != b':' || s[16] != b':' {
                return Err(TimestampError::Shape);
            }
            (&s[..10], Some(&s[11..]))
        }
        10 => (s, None),
        n => return Err(TimestampError::Length(n)),
    };
    if date[4] != b'/' || date[7] != b'/' {
        return Err(TimestampError::Shape);
    }
    let y = four(&date[0..4]).ok_or(TimestampError::Shape)?;
    let m = two(&date[5..7]).ok_or(TimestampError::Shape)?;
    let d = two(&date[8..10]).ok_or(TimestampError::Shape)?;
    if !(1..=12).contains(&m) {
        return Err(TimestampError::Month(m as u8));
    }
    let y = y as i32;
    if d == 0 || d > days_in_month(y, m) {
        return Err(TimestampError::Day(d as u8));
    }
    let mut secs = days_from_civil(y, m, d) * 86_400;
    if let Some(t) = time {
        let hh = two(&t[0..2]).ok_or(TimestampError::Shape)?;
        let mm = two(&t[3..5]).ok_or(TimestampError::Shape)?;
        let ss = two(&t[6..8]).ok_or(TimestampError::Shape)?;
        // AEMO writes "24:00:00" nowhere in the MMS feeds; it is rejected so a
        // bad row is reported rather than rolled into the next day silently.
        if hh > 23 {
            return Err(TimestampError::Hour(hh as u8));
        }
        if mm > 59 {
            return Err(TimestampError::Minute(mm as u8));
        }
        if ss > 59 {
            return Err(TimestampError::Second(ss as u8));
        }
        secs += i64::from(hh) * 3600 + i64::from(mm) * 60 + i64::from(ss);
    }
    Ok(secs)
}

/// Parses an MMS timestamp in a market with the given UTC offset into
/// microseconds since the Unix epoch, UTC.
#[inline]
pub fn parse_micros(s: &[u8], offset_secs: i32) -> Result<i64, TimestampError> {
    let local = parse_local_seconds(s)?;
    Ok((local - i64::from(offset_secs)) * 1_000_000)
}

/// Parses `yyyy/mm/dd` (or the date part of a timestamp) as days since the
/// epoch, for Arrow `Date32`.
pub fn parse_days(s: &[u8]) -> Result<i32, TimestampError> {
    let local = parse_local_seconds(s)?;
    Ok((local.div_euclid(86_400)) as i32)
}

/// The civil `(year, month)` in the market's local time of a UTC instant in
/// microseconds, for partitioning.
pub fn local_year_month(micros_utc: i64, offset_secs: i32) -> (i32, u32) {
    let local = micros_utc + i64::from(offset_secs) * 1_000_000;
    let days = local.div_euclid(MICROS_PER_DAY);
    let (y, m, _) = civil_from_days(days);
    (y, m)
}

/// Formats a UTC instant in microseconds as the MMS string in market time.
pub fn format_local(micros_utc: i64, offset_secs: i32) -> String {
    let local = micros_utc + i64::from(offset_secs) * 1_000_000;
    let days = local.div_euclid(MICROS_PER_DAY);
    let rem = local.rem_euclid(MICROS_PER_DAY) / 1_000_000;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 1, 1), 19_723);
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn parses_mms_timestamp_in_nem_time() {
        // 2024/01/01 00:05:00 AEST = 2023/12/31 14:05:00 UTC
        let micros = parse_micros(b"2024/01/01 00:05:00", NEM_OFFSET_SECS).unwrap();
        let utc_secs = days_from_civil(2023, 12, 31) * 86_400 + 14 * 3600 + 5 * 60;
        assert_eq!(micros, utc_secs * 1_000_000);
        assert_eq!(format_local(micros, NEM_OFFSET_SECS), "2024/01/01 00:05:00");
        assert_eq!(local_year_month(micros, NEM_OFFSET_SECS), (2024, 1));
        // The same instant is still December in UTC, which is why partitions
        // are computed in market time.
        assert_eq!(local_year_month(micros, 0), (2023, 12));
    }

    #[test]
    fn parses_bare_date() {
        let secs = parse_local_seconds(b"2024/02/29").unwrap();
        assert_eq!(secs, days_from_civil(2024, 2, 29) * 86_400);
        assert_eq!(parse_days(b"1970/01/02").unwrap(), 1);
    }

    #[test]
    fn rejects_invalid() {
        assert_eq!(
            parse_local_seconds(b"2023/02/29"),
            Err(TimestampError::Day(29))
        );
        assert_eq!(
            parse_local_seconds(b"2024/13/01 00:00:00"),
            Err(TimestampError::Month(13))
        );
        assert_eq!(
            parse_local_seconds(b"2024/01/00 00:00:00"),
            Err(TimestampError::Day(0))
        );
        assert_eq!(
            parse_local_seconds(b"2024/01/01 24:00:00"),
            Err(TimestampError::Hour(24))
        );
        assert_eq!(
            parse_local_seconds(b"2024/01/01 00:60:00"),
            Err(TimestampError::Minute(60))
        );
        assert_eq!(
            parse_local_seconds(b"2024/01/01 00:00:61"),
            Err(TimestampError::Second(61))
        );
        assert_eq!(
            parse_local_seconds(b"2024-01-01 00:00:00"),
            Err(TimestampError::Shape)
        );
        assert_eq!(
            parse_local_seconds(b"2024/01/01T00:00:00"),
            Err(TimestampError::Shape)
        );
        assert_eq!(
            parse_local_seconds(b"2024/01/01 00:00"),
            Err(TimestampError::Length(16))
        );
        assert_eq!(parse_local_seconds(b""), Err(TimestampError::Length(0)));
        assert_eq!(
            parse_local_seconds(b"2O24/01/01"),
            Err(TimestampError::Shape)
        );
        assert!(TimestampError::Day(31).to_string().contains("31"));
        for e in [
            TimestampError::Length(3),
            TimestampError::Shape,
            TimestampError::Month(0),
            TimestampError::Hour(25),
            TimestampError::Minute(61),
            TimestampError::Second(99),
        ] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn round_trips_every_day_of_four_centuries() {
        for day in days_from_civil(1900, 1, 1)..=days_from_civil(2300, 12, 31) {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
            assert!(d >= 1 && d <= days_in_month(y, m));
        }
    }

    #[test]
    fn negative_days_floor_correctly() {
        // 1969-12-31 23:00 UTC in micros, offset 0.
        let micros = -3_600_000_000;
        assert_eq!(local_year_month(micros, 0), (1969, 12));
        assert_eq!(format_local(micros, 0), "1969/12/31 23:00:00");
    }
}
