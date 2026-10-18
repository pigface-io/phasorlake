//! A point in time with its UTC offset, as RFC 3339 text: every time field
//! the mirror writes.
//!
//! A time taken from outside as text stays as it came (`modified` of a
//! listing line, `modified` of a zip entry, `last_modified` of a response),
//! and a field named after it with `_at` holds the same time as a
//! [`Stamp`]: `2026-10-12T10:45:00+10:00`, or `2026-10-12T00:45:00Z` for
//! UTC. nemweb's listing times and the DOS times in AEMO's zips are AEST
//! (UTC+10, no daylight saving); an HTTP date and the mirror's own clock
//! are UTC. A record written before the `_at` fields were kept gets them
//! derived from its text fields when it is read.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Seconds east of UTC of Australian Eastern Standard Time.
pub const AEST: i32 = 10 * 3600;

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// A point in time and the UTC offset it is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Stamp {
    /// Seconds since 1970-01-01T00:00:00Z.
    secs: i64,
    /// Seconds east of UTC.
    offset: i32,
}

/// Text that is not RFC 3339 with an offset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a time in RFC 3339 form with an offset")]
pub struct BadStamp(pub String);

impl Stamp {
    /// Now, UTC.
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self::from_unix(i64::try_from(secs).unwrap_or(i64::MAX))
    }

    /// A Unix time, UTC.
    pub fn from_unix(secs: i64) -> Self {
        Self { secs, offset: 0 }
    }

    /// A civil date and time at `offset` seconds east of UTC; none when a
    /// field is out of range.
    pub fn at(
        year: i64,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        offset: i32,
    ) -> Option<Self> {
        let in_range = (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60
            && offset.abs() < 86_400;
        if !in_range {
            return None;
        }
        let days = days_from_civil(year, i64::from(month), i64::from(day));
        let local =
            days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
        Some(Self {
            secs: local - i64::from(offset),
            offset,
        })
    }

    /// Seconds since 1970-01-01T00:00:00Z.
    pub fn unix(self) -> i64 {
        self.secs
    }

    /// Seconds east of UTC.
    pub fn offset(self) -> i32 {
        self.offset
    }

    /// RFC 3339 text with an offset: `YYYY-MM-DDTHH:MM:SS[.f]Z` or
    /// `…+HH:MM`; nothing else.
    pub fn parse(text: &str) -> Result<Self, BadStamp> {
        rfc3339(text).ok_or_else(|| BadStamp(bounded(text)))
    }

    /// A time as nemweb's IIS listings write it, AEST:
    /// `Monday, October 12, 2026 10:45 AM` or `2/13/2024  8:27 AM`.
    pub fn from_listing(text: &str) -> Option<Self> {
        let mut w = words(text);
        let first = w.next()?;
        let (year, month, day) = if first.contains('/') {
            let mut d = first.split('/');
            let (month, day, year) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
            if d.next().is_some() {
                return None;
            }
            (year, small(month)?, day)
        } else {
            let (month, day, year) = (w.next()?, w.next()?, w.next()?);
            (num(year)?, month_number(month)?, num(day)?)
        };
        let (clock, half) = (w.next()?, w.next()?);
        if w.next().is_some() {
            return None;
        }
        let (hour, minute) = hour_minute(clock)?;
        let hour = match (hour, half) {
            (1..=12, "AM") => hour % 12,
            (1..=12, "PM") => hour % 12 + 12,
            _ => return None,
        };
        Self::at(year, month, small(day)?, hour, minute, 0, AEST)
    }

    /// A time as a zip entry's DOS time is written, AEST:
    /// `2017-01-12 12:24:16`.
    pub fn from_dos_text(text: &str) -> Option<Self> {
        let (date, clock) = text.split_once(' ')?;
        let (year, month, day) = ymd(date)?;
        let (hour, minute, second) = hms(clock)?;
        Self::at(year, month, day, hour, minute, second, AEST)
    }

    /// An HTTP date (RFC 7231, `Mon, 12 Oct 2026 00:32:30 GMT`), UTC.
    pub fn from_http_date(text: &str) -> Option<Self> {
        let mut w = words(text);
        let (_, day, month, year, clock, zone) = (
            w.next()?,
            w.next()?,
            w.next()?,
            w.next()?,
            w.next()?,
            w.next()?,
        );
        if zone != "GMT" || w.next().is_some() {
            return None;
        }
        let (hour, minute, second) = hms(clock)?;
        Self::at(
            num(year)?,
            month_number(month)?,
            small(num(day)?)?,
            hour,
            minute,
            second,
            0,
        )
    }

    fn civil(self) -> (i64, i64, i64, i64, i64, i64) {
        let local = self.secs + i64::from(self.offset);
        let days = local.div_euclid(86_400);
        let rem = local.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d, h, mi, s) = self.civil();
        write!(f, "{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}")?;
        if self.offset == 0 {
            return f.write_str("Z");
        }
        let sign = if self.offset < 0 { '-' } else { '+' };
        let off = self.offset.abs();
        write!(f, "{sign}{:02}:{:02}", off / 3600, off % 3600 / 60)
    }
}

impl Serialize for Stamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Stamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// RFC 3339: date, `T`, time, an optional fraction, then `Z` or `±HH:MM`.
fn rfc3339(text: &str) -> Option<Stamp> {
    let (date, rest) = text.split_at_checked(10)?;
    let (year, month, day) = ymd(date)?;
    let rest = rest.strip_prefix(['T', 't'])?;
    let (clock, rest) = rest.split_at_checked(8)?;
    let (hour, minute, second) = hms(clock)?;
    let rest = match rest.strip_prefix('.') {
        Some(frac) => {
            let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            &frac[digits..]
        }
        None => rest,
    };
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (h, m) = rest[1..].split_once(':')?;
            if h.len() != 2 || m.len() != 2 {
                return None;
            }
            let (h, m) = (i32::try_from(num(h)?).ok()?, i32::try_from(num(m)?).ok()?);
            if h > 23 || m > 59 {
                return None;
            }
            sign * (h * 3600 + m * 60)
        }
    };
    Stamp::at(year, month, day, hour, minute, second, offset)
}

/// The words of `text`, split on whitespace and commas.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
}

/// A run of one to nine ASCII digits as a number.
fn num(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 9 {
        return None;
    }
    s.bytes().try_fold(0i64, |n, b| {
        b.is_ascii_digit().then(|| n * 10 + i64::from(b - b'0'))
    })
}

fn small(n: i64) -> Option<u8> {
    u8::try_from(n).ok()
}

/// `YYYY-MM-DD`.
fn ymd(s: &str) -> Option<(i64, u8, u8)> {
    let mut p = s.split('-');
    let (y, m, d) = (p.next()?, p.next()?, p.next()?);
    if p.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    Some((num(y)?, small(num(m)?)?, small(num(d)?)?))
}

/// `HH:MM:SS`.
fn hms(s: &str) -> Option<(u8, u8, u8)> {
    let mut p = s.split(':');
    let (h, m, sec) = (p.next()?, p.next()?, p.next()?);
    if p.next().is_some() || h.len() != 2 || m.len() != 2 || sec.len() != 2 {
        return None;
    }
    Some((small(num(h)?)?, small(num(m)?)?, small(num(sec)?)?))
}

/// `H:MM` or `HH:MM`.
fn hour_minute(s: &str) -> Option<(u8, u8)> {
    let (h, m) = s.split_once(':')?;
    if m.len() != 2 || h.is_empty() || h.len() > 2 {
        return None;
    }
    Some((small(num(h)?)?, small(num(m)?)?))
}

/// A month by its English name or three-letter abbreviation, 1 to 12.
fn month_number(name: &str) -> Option<u8> {
    let at = MONTHS.iter().position(|m| {
        m.eq_ignore_ascii_case(name) || (name.len() == 3 && m[..3].eq_ignore_ascii_case(name))
    })?;
    small(at as i64 + 1)
}

fn bounded(text: &str) -> String {
    text.chars().take(64).collect()
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Checks of written records: every time field is aware, or has its aware
/// twin beside it.
#[cfg(test)]
pub mod check {
    use super::Stamp;

    /// Whether a key names a time field.
    fn time_key(key: &str) -> bool {
        key.ends_with("_at") || key.contains("modified") || key == "as_of"
    }

    /// Whether a string reads as a time in any form the mirror meets.
    fn looks_like_a_time(s: &str) -> bool {
        let dated = s.as_bytes().windows(10).any(|w| {
            w[4] == b'-' && w[7] == b'-' && w.iter().filter(|b| b.is_ascii_digit()).count() == 8
        });
        dated || s.ends_with(" AM") || s.ends_with(" PM") || s.ends_with(" GMT")
    }

    fn aware(v: &serde_json::Value) -> bool {
        v.as_str().is_some_and(|s| Stamp::parse(s).is_ok())
    }

    /// Fails unless every time field of every object in `v` is RFC 3339
    /// with an offset, or has a `{name}_at` field beside it that is.
    pub fn assert_aware(v: &serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    if let Some(s) = value.as_str() {
                        if time_key(key) {
                            let twin = map.get(&format!("{key}_at"));
                            assert!(
                                aware(value) || twin.is_some_and(aware),
                                "`{key}`: `{s}` is naive and has no aware `{key}_at` beside it"
                            );
                        } else {
                            assert!(!looks_like_a_time(s), "`{key}` holds a time: `{s}`");
                        }
                    } else {
                        assert_aware(value);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(assert_aware),
            _ => {}
        }
    }

    /// `assert_aware` over every line of an NDJSON text.
    pub fn assert_aware_lines(ndjson: &str) {
        for line in ndjson.lines().filter(|l| !l.is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
            assert_aware(&v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> String {
        Stamp::parse(text).unwrap().to_string()
    }

    #[test]
    fn rfc3339_round_trips_and_rejects_naive_text() {
        assert_eq!(s("1970-01-01T00:00:00Z"), "1970-01-01T00:00:00Z");
        assert_eq!(s("2024-02-29T12:34:56.123z"), "2024-02-29T12:34:56Z");
        assert_eq!(s("2026-10-12T10:45:00+10:00"), "2026-10-12T10:45:00+10:00");
        assert_eq!(s("2026-10-12T10:45:00-03:30"), "2026-10-12T10:45:00-03:30");
        assert_eq!(s("2026-10-12t10:45:00+00:00"), "2026-10-12T10:45:00Z");
        let t = Stamp::parse("2026-10-12T10:45:00+10:00").unwrap();
        assert_eq!((t.unix(), t.offset()), (1_791_765_900, AEST));
        assert_eq!(
            Stamp::from_unix(1_791_765_900).to_string(),
            "2026-10-12T00:45:00Z"
        );
        assert_eq!(Stamp::from_unix(-1).to_string(), "1969-12-31T23:59:59Z");
        assert_eq!(Stamp::now().to_string().len(), 20);
        for bad in [
            "2026-10-12 10:45:00",
            "2026-10-12T10:45:00",
            "2026-10-12T10:45:00.",
            "2026-10-12T10:45:00+10",
            "2026-10-12T10:45:00+1000",
            "2026-10-12T10:45:00+1:00",
            "2026-10-12T10:45:00+10:0",
            "2026-10-12T10:45:00+24:00",
            "2026-10-12T10:45:00*10:00",
            "2026-13-12T10:45:00Z",
            "2026-10-12T25:45:00Z",
            "2026-1-12T10:45:00Z",
            "2026-10-12T10:45Z",
            "Monday, October 12, 2026 10:45 AM",
            "t",
            "",
        ] {
            let e = Stamp::parse(bad).unwrap_err();
            assert_eq!(e.0, bad, "{bad}");
        }
        assert_eq!(
            Stamp::parse(&"x".repeat(100))
                .unwrap_err()
                .to_string()
                .len(),
            64 + "`` is not a time in RFC 3339 form with an offset".len()
        );
    }

    #[test]
    fn listing_times_are_aest() {
        let at = |t: &str| Stamp::from_listing(t).unwrap().to_string();
        assert_eq!(
            at("Monday, October 12, 2026 10:45 AM"),
            "2026-10-12T10:45:00+10:00"
        );
        assert_eq!(
            at("Tuesday, December 9, 2025 09:05 PM"),
            "2025-12-09T21:05:00+10:00"
        );
        assert_eq!(
            at("Monday, February 6, 2017 12:23 AM"),
            "2017-02-06T00:23:00+10:00"
        );
        assert_eq!(
            at("Monday, February 6, 2017 12:23 PM"),
            "2017-02-06T12:23:00+10:00"
        );
        assert_eq!(at("2/13/2024  8:27 AM"), "2024-02-13T08:27:00+10:00");
        assert_eq!(at("12/1/2024 12:00 PM"), "2024-12-01T12:00:00+10:00");
        assert_eq!(
            Stamp::from_listing("Monday, October 12, 2026 10:45 AM").unwrap(),
            Stamp::at(2026, 10, 12, 10, 45, 0, AEST).unwrap()
        );
        for bad in [
            "",
            "m",
            "Monday, Octember 12, 2026 10:45 AM",
            "Monday, October 12, 2026 10:45 XM",
            "Monday, October 12, 2026 13:45 PM",
            "Monday, October 12, 2026 0:45 AM",
            "Monday, October 12, 2026 10:45",
            "Monday, October 12, 2026 10:4 AM",
            "Monday, October 12, 2026 100:45 AM",
            "Monday, October x, 2026 10:45 AM",
            "Monday, October 12, 2026 10:45 AM extra",
            "13/40/2024 8:27 AM",
            "2/13/2024/1 8:27 AM",
            "2/13 8:27 AM",
            "300/13/2024 8:27 AM",
            "2/13/2024000000 8:27 AM",
            "2026-10-12T10:45:00+10:00",
        ] {
            assert!(Stamp::from_listing(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn dos_times_are_aest_and_http_dates_utc() {
        assert_eq!(
            Stamp::from_dos_text("2017-01-12 12:24:16")
                .unwrap()
                .to_string(),
            "2017-01-12T12:24:16+10:00"
        );
        for bad in ["2017-01-12", "2017-00-12 12:24:16", "2017-01-12 12:24", "m"] {
            assert!(Stamp::from_dos_text(bad).is_none(), "{bad}");
        }
        assert_eq!(
            Stamp::from_http_date("Mon, 12 Oct 2026 00:32:30 GMT")
                .unwrap()
                .to_string(),
            "2026-10-12T00:32:30Z"
        );
        assert_eq!(
            Stamp::from_http_date("Tue, 13 Feb 2024 08:27:00 GMT").unwrap(),
            Stamp::at(2024, 2, 13, 8, 27, 0, 0).unwrap()
        );
        for bad in [
            "Mon, 12 Oct 2026 00:32:30 UTC",
            "Mon, 12 Oct 2026 00:32:30",
            "Mon, 12 Oct 2026 00:32 GMT",
            "Mon, 12 Oct 2026 0:32:30 GMT",
            "Mon, 12 Oct 2026 00:32:30:00 GMT",
            "Mon, 999 Oct 2026 00:32:30 GMT",
            "Monday, 12-Oct-26 00:32:30 GMT",
            "",
        ] {
            assert!(Stamp::from_http_date(bad).is_none(), "{bad}");
        }
        assert!(Stamp::at(2026, 10, 12, 0, 0, 0, 86_400).is_none());
        assert!(Stamp::at(2026, 10, 32, 0, 0, 0, 0).is_none());
    }

    #[test]
    fn serde_is_the_text_form() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct R {
            at: Stamp,
        }
        let r = R {
            at: Stamp::at(2026, 10, 12, 10, 45, 0, AEST).unwrap(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"at":"2026-10-12T10:45:00+10:00"}"#);
        assert_eq!(serde_json::from_str::<R>(&json).unwrap(), r);
        let e = serde_json::from_str::<R>(r#"{"at":"2026-10-12 10:45:00"}"#).unwrap_err();
        assert!(e.to_string().contains("RFC 3339"), "{e}");
        assert!(serde_json::from_str::<R>(r#"{"at":1}"#).is_err());
    }

    #[test]
    fn the_check_accepts_aware_fields_and_twins_and_refuses_the_rest() {
        let ok = serde_json::json!({
            "fetched_at": "2026-10-15T09:12:44Z",
            "modified": "Monday, October 12, 2026 10:45 AM",
            "modified_at": "2026-10-12T10:45:00+10:00",
            "name": "MMSDM_2026_09.zip",
            "entries": [{"modified": "2017-01-12 12:24:16", "modified_at": "2017-01-12T12:24:16+10:00"}],
            "size": 1,
        });
        check::assert_aware(&ok);
        check::assert_aware_lines("{\"as_of\":\"2026-10-15T09:12:44Z\"}\n\n");
        for bad in [
            serde_json::json!({"modified": "Monday, October 12, 2026 10:45 AM"}),
            serde_json::json!({"modified": "m", "modified_at": "t"}),
            serde_json::json!({"peeked_at": "2026-10-15 09:12:44"}),
            serde_json::json!({"as_of": "2026-10-15T09:12:44"}),
            serde_json::json!({"name": "Mon, 12 Oct 2026 00:32:30 GMT"}),
            serde_json::json!({"name": "2017-01-12 12:24:16"}),
            serde_json::json!({"name": "x 8:27 PM"}),
            serde_json::json!({"deep": [{"inner": {"modified": "m"}}]}),
        ] {
            assert!(
                std::panic::catch_unwind(|| check::assert_aware(&bad)).is_err(),
                "{bad}"
            );
        }
    }
}
