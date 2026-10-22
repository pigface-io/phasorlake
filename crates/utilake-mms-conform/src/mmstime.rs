//! The MMS timestamp format, `yyyy/mm/dd hh24:mi:ss`, parsed without a
//! calendar library.
//!
//! AEMO writes every timestamp in market time. The lake stores every
//! instant as nanoseconds since the Unix epoch in UTC, with the market
//! offset recorded in the Arrow type. One unit everywhere means no value
//! has to be asked which unit it is in; the price is a range that ends in
//! 2262, so the far-future dates AEMO uses for "no end" are clamped and
//! reported.
//!
//! This module is on the hot path: no allocation, no `chrono`, no `jiff`.

use utilake_core::calendar::{NANOS_PER_SECOND, days_from_civil, days_in_month};

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
    /// A fractional second that is not one to nine digits.
    Fraction,
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
            Self::Fraction => write!(f, "has a fractional second that is not one to nine digits"),
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
/// whole seconds since the Unix epoch, UTC, and the nanoseconds within
/// that second.
///
/// This is the full reading of the text: a fractional second of one to
/// nine digits is padded with zeros to nanoseconds, so nothing published
/// is lost here. The two parts are kept apart because a single `i64` of
/// nanoseconds ends in April 2262 and AEMO's data carries dates such as
/// `9999/12/31 00:00:00` as "no end".
#[inline]
pub fn parse_seconds_nanos(s: &[u8], offset_secs: i32) -> Result<(i64, u32), TimestampError> {
    // The plain 19-byte form first: it is nearly every value.
    if s.len() > 19 && s[19] == b'.' {
        let local = parse_local_seconds(&s[..19])?;
        let nanos = fraction_nanos(&s[20..])?;
        return Ok((local - i64::from(offset_secs), nanos));
    }
    Ok((parse_local_seconds(s)? - i64::from(offset_secs), 0))
}

/// Parses an MMS timestamp in a market with the given UTC offset into
/// nanoseconds since the Unix epoch, UTC: the one unit of every timestamp
/// in the lake. Every digit of a fractional second is kept.
///
/// An `i64` of nanoseconds spans 1677-09-21 to 2262-04-11. AEMO writes
/// "no end" as `9999/12/31` or `2999/12/31`; such a value is clamped to the
/// nearest end of the range and the flag returned is `true`, so the caller
/// can report it. Clamping keeps `end > now` true, which a null would not.
/// The clamped value is an instant like any other in its column, UTC
/// nanoseconds under the market offset the column's type carries.
#[inline]
pub fn parse_nanos(s: &[u8], offset_secs: i32) -> Result<(i64, bool), TimestampError> {
    let (secs, nanos) = parse_seconds_nanos(s, offset_secs)?;
    match secs
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|n| n.checked_add(i64::from(nanos)))
    {
        Some(n) => Ok((n, false)),
        None if secs < 0 => Ok((i64::MIN, true)),
        None => Ok((i64::MAX, true)),
    }
}

/// The digits after the decimal point of a second, padded with zeros to
/// nanoseconds.
fn fraction_nanos(digits: &[u8]) -> Result<u32, TimestampError> {
    if digits.is_empty() || digits.len() > 9 {
        return Err(TimestampError::Fraction);
    }
    let mut nanos: u32 = 0;
    for i in 0..9 {
        let d = match digits.get(i) {
            Some(&b) => digit(b).ok_or(TimestampError::Fraction)?,
            None => 0,
        };
        nanos = nanos * 10 + d;
    }
    Ok(nanos)
}

/// Whether `s` is an MMS timestamp or date, with or without a fractional
/// second: what a sampler asks of a value.
#[must_use]
pub fn is_timestamp(s: &[u8]) -> bool {
    parse_seconds_nanos(s, 0).is_ok()
}

/// Parses `yyyy/mm/dd` (or the date part of a timestamp) as days since the
/// epoch, for Arrow `Date32`.
pub fn parse_days(s: &[u8]) -> Result<i32, TimestampError> {
    let local = parse_local_seconds(s)?;
    Ok((local.div_euclid(86_400)) as i32)
}

/// Formats a UTC instant in nanoseconds as the MMS string in market time,
/// to the second.
#[cfg(test)]
pub(crate) fn format_local(nanos_utc: i64, offset_secs: i32) -> String {
    utilake_core::calendar::format_local_seconds(
        nanos_utc.div_euclid(NANOS_PER_SECOND),
        offset_secs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use utilake_core::calendar::{NEM_OFFSET_SECS, format_local_seconds, local_year_month};

    #[test]
    fn parses_mms_timestamp_in_nem_time() {
        // 2024/01/01 00:05:00 AEST = 2023/12/31 14:05:00 UTC
        let (nanos, clamped) = parse_nanos(b"2024/01/01 00:05:00", NEM_OFFSET_SECS).unwrap();
        assert!(!clamped);
        let utc_secs = days_from_civil(2023, 12, 31) * 86_400 + 14 * 3600 + 5 * 60;
        assert_eq!(nanos, utc_secs * NANOS_PER_SECOND);
        assert_eq!(format_local(nanos, NEM_OFFSET_SECS), "2024/01/01 00:05:00");
        assert_eq!(
            format_local_seconds(utc_secs, NEM_OFFSET_SECS),
            "2024/01/01 00:05:00"
        );
        assert_eq!(local_year_month(nanos, NEM_OFFSET_SECS), (2024, 1));
        // The same instant is still December in UTC, which is why partitions
        // are computed in market time.
        assert_eq!(local_year_month(nanos, 0), (2023, 12));
    }

    #[test]
    fn parses_fractional_seconds() {
        let nanos = |s: &[u8]| parse_nanos(s, NEM_OFFSET_SECS).map(|(n, _)| n);
        let whole = nanos(b"2026/07/28 09:29:54").unwrap();
        assert_eq!(nanos(b"2026/07/28 09:29:54.000"), Ok(whole));
        assert_eq!(nanos(b"2026/07/28 09:29:54.5"), Ok(whole + 500_000_000));
        assert_eq!(
            nanos(b"2026/07/28 09:29:54.123456"),
            Ok(whole + 123_456_000)
        );
        // Every digit is kept, down to the nanosecond.
        assert_eq!(
            nanos(b"2026/07/28 09:29:54.123456789"),
            Ok(whole + 123_456_789)
        );
        assert_eq!(nanos(b"2026/07/28 09:29:54.000000001"), Ok(whole + 1));
        // Before the epoch the fraction still moves time forward.
        assert_eq!(
            parse_nanos(b"1969/12/31 23:59:59", 0),
            Ok((-NANOS_PER_SECOND, false))
        );
        assert_eq!(
            parse_nanos(b"1969/12/31 23:59:59.25", 0),
            Ok((-750_000_000, false))
        );
        // No digits, more than nine, or not digits: refused.
        for bad in [
            &b"2026/07/28 09:29:54."[..],
            b"2026/07/28 09:29:54.1234567890",
            b"2026/07/28 09:29:54.12x",
        ] {
            assert_eq!(
                parse_nanos(bad, 0),
                Err(TimestampError::Fraction),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(TimestampError::Fraction.to_string().contains("nine digits"));
        // The parts: whole seconds, and nanoseconds padded with zeros.
        assert_eq!(
            parse_seconds_nanos(b"2026/07/28 09:29:54.5", 0).unwrap().1,
            500_000_000
        );
        assert_eq!(parse_seconds_nanos(b"2026/07/28 09:29:54", 0).unwrap().1, 0);
        // Anything else after the seconds is still a length error.
        assert_eq!(
            parse_nanos(b"2026/07/28 09:29:54Z", 0),
            Err(TimestampError::Length(20))
        );
        assert!(is_timestamp(b"2026/07/28 09:29:54.000"));
        assert!(is_timestamp(b"2026/07/28 09:29:54"));
        assert!(is_timestamp(b"2026/07/28"));
        assert!(is_timestamp(b"9999/12/31 00:00:00"));
        assert!(!is_timestamp(b"GOOD"));
        assert!(!is_timestamp(b""));
    }

    #[test]
    fn clamps_what_nanoseconds_cannot_hold() {
        // The ends of the range, to the second.
        assert_eq!(
            parse_nanos(b"2262/04/11 23:47:16", 0).map(|v| v.1),
            Ok(false)
        );
        assert_eq!(parse_nanos(b"2262/04/11 23:47:17", 0), Ok((i64::MAX, true)));
        assert_eq!(
            parse_nanos(b"1677/09/21 00:12:44", 0).map(|v| v.1),
            Ok(false)
        );
        assert_eq!(parse_nanos(b"1677/09/21 00:12:43", 0), Ok((i64::MIN, true)));
        // The values AEMO uses for "no end".
        for sentinel in [
            &b"9999/12/31 00:00:00"[..],
            b"9999/12/31 23:59:59.997",
            b"2999/12/31 00:00:00",
            b"9999/12/30 00:00:00",
        ] {
            assert_eq!(parse_nanos(sentinel, NEM_OFFSET_SECS), Ok((i64::MAX, true)));
        }
        assert_eq!(parse_nanos(b"0001/01/01 00:00:00", 0), Ok((i64::MIN, true)));
        // The text itself is read in full before it is clamped.
        let (far, nanos) = parse_seconds_nanos(b"9999/12/31 23:59:59.997", 0).unwrap();
        assert_eq!(nanos, 997_000_000);
        assert_eq!(format_local_seconds(far, 0), "9999/12/31 23:59:59");
        // A clamped value still has a month: the last, or the first.
        assert_eq!(local_year_month(i64::MAX, NEM_OFFSET_SECS), (2262, 4));
        assert_eq!(local_year_month(i64::MIN, NEM_OFFSET_SECS), (1677, 9));
        assert_eq!(format_local(i64::MAX, 0), "2262/04/11 23:47:16");
        // What is not a timestamp is an error, not a clamp.
        assert!(parse_nanos(b"9999/13/31 00:00:00", 0).is_err());
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
}
