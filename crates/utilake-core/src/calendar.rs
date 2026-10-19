//! The calendar, in whole days and seconds, and the fixed offsets of the
//! Australian markets: what every time in the lake is counted in.
//!
//! The lake stores every instant as nanoseconds since the Unix epoch in
//! UTC, with the market offset recorded in the Arrow type. The NEM runs on
//! Australian Eastern Standard Time all year (UTC+10, no daylight saving),
//! the WEM on Australian Western Standard Time (UTC+8). No calendar
//! library: these run on the hot path.

/// Seconds east of UTC for the National Electricity Market (AEST, fixed).
pub const NEM_OFFSET_SECS: i32 = 10 * 3600;
/// Seconds east of UTC for the Wholesale Electricity Market (AWST, fixed).
pub const WEM_OFFSET_SECS: i32 = 8 * 3600;

/// Nanoseconds in one second.
pub const NANOS_PER_SECOND: i64 = 1_000_000_000;

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

/// The days of month `m` (1..=12) in year `y`; 0 for a month that is not.
#[inline]
pub const fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// The civil `(year, month)` in the market's local time of a UTC instant in
/// nanoseconds, for partitioning.
pub fn local_year_month(nanos_utc: i64, offset_secs: i32) -> (i32, u32) {
    // Seconds first: adding the offset in nanoseconds could overflow at the
    // ends of the range, where clamped values sit.
    let local_secs = nanos_utc.div_euclid(NANOS_PER_SECOND) + i64::from(offset_secs);
    let (y, m, _) = civil_from_days(local_secs.div_euclid(86_400));
    (y, m)
}

/// Formats a UTC instant in whole seconds as the MMS string in market time.
pub fn format_local_seconds(secs_utc: i64, offset_secs: i32) -> String {
    let local = secs_utc + i64::from(offset_secs);
    let rem = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    format!(
        "{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// The UTC nanoseconds of a civil date and time written in `offset_secs`,
/// for tests and tools that name an instant.
#[must_use]
pub fn utc_nanos(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32, offset_secs: i32) -> i64 {
    let local = days_from_civil(y, m, d) * 86_400
        + i64::from(hh) * 3600
        + i64::from(mm) * 60
        + i64::from(ss);
    (local - i64::from(offset_secs)) * NANOS_PER_SECOND
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
    fn round_trips_every_day_of_four_centuries() {
        for day in days_from_civil(1900, 1, 1)..=days_from_civil(2300, 12, 31) {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
            assert!(d >= 1 && d <= days_in_month(y, m));
        }
    }

    #[test]
    fn negative_days_floor_correctly() {
        // 1969-12-31 23:00 UTC in nanoseconds, offset 0.
        let nanos = -3_600 * NANOS_PER_SECOND;
        assert_eq!(local_year_month(nanos, 0), (1969, 12));
        assert_eq!(
            format_local_seconds(nanos.div_euclid(NANOS_PER_SECOND), 0),
            "1969/12/31 23:00:00"
        );
        // One nanosecond before the epoch is still the last second of 1969.
        assert_eq!(
            format_local_seconds((-1i64).div_euclid(NANOS_PER_SECOND), 0),
            "1969/12/31 23:59:59"
        );
    }

    #[test]
    fn utc_nanos_names_an_instant() {
        // 2024/01/01 00:05:00 AEST = 2023/12/31 14:05:00 UTC
        let n = utc_nanos(2024, 1, 1, 0, 5, 0, NEM_OFFSET_SECS);
        assert_eq!(local_year_month(n, NEM_OFFSET_SECS), (2024, 1));
        assert_eq!(local_year_month(n, 0), (2023, 12));
        assert_eq!(
            format_local_seconds(n / NANOS_PER_SECOND, NEM_OFFSET_SECS),
            "2024/01/01 00:05:00"
        );
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2024, 13), 0);
    }
}
