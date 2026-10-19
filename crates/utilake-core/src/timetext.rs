//! Time as text outside the lake: RFC 3339 with an offset, and the forms it
//! is read from.
//!
//! Inside the lake an instant is UTC nanoseconds in a column that carries
//! the market offset (`nemtime`). Outside it, in a sidecar, the manifest or
//! a listing, an instant is text, and the text always says which offset it
//! is in: `2024-02-13T08:27:00Z` for the tool's own clock and HTTP dates,
//! `2024-02-13T18:27:00+10:00` for a time nemweb lists in market time. A
//! time without an offset is never written; one that is read (an older
//! mirror record, an IIS listing) is read in the offset its writer used.
//!
//! Not on the hot path.

use std::fmt;

use crate::nemtime::{civil_from_days, days_from_civil, days_in_month};

/// An instant with the offset it was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stamp {
    /// Whole seconds since the Unix epoch, UTC.
    pub secs_utc: i64,
    /// Nanoseconds within that second.
    pub nanos: u32,
    /// Seconds east of UTC the text gave the time in.
    pub offset_secs: i32,
}

impl Stamp {
    /// An instant in whole seconds, to be written in `offset_secs`.
    #[must_use]
    pub const fn new(secs_utc: i64, offset_secs: i32) -> Self {
        Self {
            secs_utc,
            nanos: 0,
            offset_secs,
        }
    }

    /// The RFC 3339 text: `YYYY-MM-DDTHH:MM:SS[.fraction]` followed by `Z`
    /// or `+HH:MM`.
    #[must_use]
    pub fn rfc3339(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let local = self.secs_utc + i64::from(self.offset_secs);
        let rem = local.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(local.div_euclid(86_400));
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
            rem / 3600,
            (rem / 60) % 60,
            rem % 60
        )?;
        if self.nanos != 0 {
            let digits = format!("{:09}", self.nanos);
            write!(f, ".{}", digits.trim_end_matches('0'))?;
        }
        if self.offset_secs == 0 {
            return f.write_str("Z");
        }
        let sign = if self.offset_secs < 0 { '-' } else { '+' };
        let abs = self.offset_secs.unsigned_abs();
        write!(f, "{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60)
    }
}

/// Written as its RFC 3339 text.
impl serde::Serialize for Stamp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Read from RFC 3339 text; text without an offset is refused.
impl<'de> serde::Deserialize<'de> for Stamp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        parse_rfc3339(&text).ok_or_else(|| {
            serde::de::Error::custom(format!("{text:?} is not RFC 3339 with an offset"))
        })
    }
}

/// A UTC instant in whole seconds as RFC 3339 in `offset_secs`.
#[must_use]
pub fn format_rfc3339(secs_utc: i64, offset_secs: i32) -> String {
    Stamp::new(secs_utc, offset_secs).rfc3339()
}

/// Reads time text in any form utilake meets. A form that carries its
/// offset (RFC 3339, an HTTP date, AEMO's WEM `generatedAt`) is read in
/// it; one that does not (an IIS listing, `YYYY-MM-DD HH:MM:SS`) is read in
/// `naive_offset_secs`, the offset its writer used.
#[must_use]
pub fn parse(s: &str, naive_offset_secs: i32) -> Option<Stamp> {
    let s = s.trim();
    parse_rfc3339(s)
        .or_else(|| parse_http_date(s))
        .or_else(|| parse_wem_generated(s))
        .or_else(|| parse_iis_listing(s, naive_offset_secs))
        .or_else(|| parse_naive(s, naive_offset_secs))
}

/// `s` as RFC 3339 with an offset, read from any form; `None` when it is
/// not a time.
#[must_use]
pub fn normalise(s: &str, naive_offset_secs: i32) -> Option<String> {
    parse(s, naive_offset_secs).map(|t| t.rfc3339())
}

/// Whether `s` is RFC 3339 text that says its offset.
#[must_use]
pub fn is_rfc3339(s: &str) -> bool {
    parse_rfc3339(s).is_some()
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)`. The `T` may be a
/// `t` or a space, as RFC 3339 allows.
#[must_use]
pub fn parse_rfc3339(s: &str) -> Option<Stamp> {
    let b = s.as_bytes();
    let (y, m, d) = civil_date(b.get(..10)?, b'-')?;
    if !matches!(b.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(&b[11..])?;
    let offset_secs = match rest {
        b"Z" | b"z" => 0,
        [sign @ (b'+' | b'-'), oh0, oh1, b':', om0, om1] => {
            let oh = two(&[*oh0, *oh1])?;
            let om = two(&[*om0, *om1])?;
            if oh > 23 || om > 59 {
                return None;
            }
            let secs = (oh * 3600 + om * 60) as i32;
            if *sign == b'-' { -secs } else { secs }
        }
        _ => return None,
    };
    let local = local_seconds(y, m, d, hh, mm, ss)?;
    Some(Stamp {
        secs_utc: local - i64::from(offset_secs),
        nanos,
        offset_secs,
    })
}

/// The HTTP date a server sends in `Last-Modified`:
/// `Tue, 13 Feb 2024 08:27:00 GMT`.
#[must_use]
pub fn parse_http_date(s: &str) -> Option<Stamp> {
    let mut t = tokens(s);
    let (wday, dd, mon, yyyy, hms, zone) = (
        t.next()?,
        t.next()?,
        t.next()?,
        t.next()?,
        t.next()?,
        t.next()?,
    );
    if t.next().is_some() || !is_weekday(wday) || !matches!(zone, "GMT" | "UTC") {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(hms.as_bytes())?;
    if !rest.is_empty() {
        return None;
    }
    let local = local_seconds(
        number(yyyy)?.try_into().ok()?,
        month_number(mon)?,
        number(dd)?,
        hh,
        mm,
        ss,
    )?;
    Some(Stamp {
        secs_utc: local,
        nanos,
        offset_secs: 0,
    })
}

/// AEMO's WEM manifest `generatedAt`: `Thu Nov 02 2023 11:35:00Z`.
#[must_use]
pub fn parse_wem_generated(s: &str) -> Option<Stamp> {
    let mut t = tokens(s);
    let (wday, mon, dd, yyyy, hms) = (t.next()?, t.next()?, t.next()?, t.next()?, t.next()?);
    if t.next().is_some() || !is_weekday(wday) {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(hms.as_bytes())?;
    if !matches!(rest, b"Z" | b"z") {
        return None;
    }
    let local = local_seconds(
        number(yyyy)?.try_into().ok()?,
        month_number(mon)?,
        number(dd)?,
        hh,
        mm,
        ss,
    )?;
    Some(Stamp {
        secs_utc: local,
        nanos,
        offset_secs: 0,
    })
}

/// The time an IIS directory listing shows, in the server's local time
/// (`offset_secs`): `2/13/2024  8:27 AM` or `Monday, October 12, 2026
/// 10:45 AM`.
#[must_use]
pub fn parse_iis_listing(s: &str, offset_secs: i32) -> Option<Stamp> {
    if let Some(local) = iis_short_local(s.as_bytes()) {
        return Some(Stamp::new(local - i64::from(offset_secs), offset_secs));
    }
    let mut t = tokens(s);
    if !is_weekday(t.next()?) {
        return None;
    }
    let m = month_number(t.next()?)?;
    let d = number(t.next()?)?;
    let y: i32 = number(t.next()?)?.try_into().ok()?;
    let (hh, mm, ss, nanos, rest) = iis_time_of_day(t.next()?.as_bytes())?;
    let hh = twelve_hour(hh, rest, t.next()?)?;
    if t.next().is_some() {
        return None;
    }
    let local = local_seconds(y, m, d, hh, mm, ss)?;
    Some(Stamp {
        secs_utc: local - i64::from(offset_secs),
        nanos,
        offset_secs,
    })
}

/// The short IIS form, `M/d/yyyy h:mm AM`, as local seconds. One pass over
/// the bytes: a listing has thousands of these, one per line, and its
/// parser is measured.
fn iis_short_local(b: &[u8]) -> Option<i64> {
    /// One to `max` digits at `*i`, advancing past them.
    fn num(b: &[u8], i: &mut usize, max: usize) -> Option<u32> {
        let start = *i;
        let mut n = 0u32;
        while *i < b.len() && *i - start < max {
            let Some(d) = digit(b[*i]) else { break };
            n = n * 10 + d;
            *i += 1;
        }
        (*i > start).then_some(n)
    }
    /// The byte `c` at `*i`, advancing past it.
    fn expect(b: &[u8], i: &mut usize, c: u8) -> Option<()> {
        (b.get(*i) == Some(&c)).then(|| *i += 1)
    }
    /// One or more spaces at `*i`, advancing past them.
    fn spaces(b: &[u8], i: &mut usize) -> Option<()> {
        let n = b[*i..].iter().take_while(|c| **c == b' ').count();
        *i += n;
        (n > 0).then_some(())
    }
    let i = &mut 0usize;
    let m = num(b, i, 2)?;
    expect(b, i, b'/')?;
    let d = num(b, i, 2)?;
    expect(b, i, b'/')?;
    let y = num(b, i, 4)?;
    spaces(b, i)?;
    let hh = num(b, i, 2)?;
    expect(b, i, b':')?;
    let start = *i;
    let mm = num(b, i, 2)?;
    (*i - start == 2).then_some(())?;
    spaces(b, i)?;
    let meridiem = std::str::from_utf8(&b[*i..]).ok()?;
    let hh = twelve_hour(hh, b"", meridiem)?;
    local_seconds(y as i32, m, d, hh, mm, 0)
}

/// `YYYY-MM-DD HH:MM:SS` or `YYYY/MM/DD HH:MM:SS` (a `T` between the parts
/// too) with no offset, as a zip entry's time is written: read in
/// `offset_secs`.
#[must_use]
pub fn parse_naive(s: &str, offset_secs: i32) -> Option<Stamp> {
    let b = s.as_bytes();
    let date = b.get(..10)?;
    let sep = if date[4] == b'/' { b'/' } else { b'-' };
    let (y, m, d) = civil_date(date, sep)?;
    if !matches!(b.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(&b[11..])?;
    if !rest.is_empty() {
        return None;
    }
    let local = local_seconds(y, m, d, hh, mm, ss)?;
    Some(Stamp {
        secs_utc: local - i64::from(offset_secs),
        nanos,
        offset_secs,
    })
}

/// Words separated by spaces or commas.
fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split([' ', ',', '\t']).filter(|t| !t.is_empty())
}

fn digit(b: u8) -> Option<u32> {
    let d = b.wrapping_sub(b'0');
    (d < 10).then_some(u32::from(d))
}

fn two(s: &[u8]) -> Option<u32> {
    Some(digit(*s.first()?)? * 10 + digit(*s.get(1)?)?)
}

/// One to six decimal digits.
fn number(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 6 {
        return None;
    }
    s.bytes().try_fold(0u32, |n, b| Some(n * 10 + digit(b)?))
}

/// `YYYY<sep>MM<sep>DD`, exactly ten bytes.
fn civil_date(b: &[u8], sep: u8) -> Option<(i32, u32, u32)> {
    if b.len() != 10 || b[4] != sep || b[7] != sep {
        return None;
    }
    let y = two(&b[0..2])? * 100 + two(&b[2..4])?;
    Some((y as i32, two(&b[5..7])?, two(&b[8..10])?))
}

/// `HH:MM[:SS[.fraction]]` at the start of `b`: the parts, the fraction as
/// nanoseconds, and what follows.
fn time_of_day(b: &[u8]) -> Option<(u32, u32, u32, u32, &[u8])> {
    from_minutes(two(b)?, b.get(2..)?)
}

/// The same with a one-digit hour allowed, as IIS writes `8:27 AM`.
fn iis_time_of_day(b: &[u8]) -> Option<(u32, u32, u32, u32, &[u8])> {
    if b.get(1) == Some(&b':') {
        return from_minutes(digit(b[0])?, &b[1..]);
    }
    time_of_day(b)
}

/// `:MM[:SS[.fraction]]` at the start of `b`, after the hour `hh`.
fn from_minutes(hh: u32, b: &[u8]) -> Option<(u32, u32, u32, u32, &[u8])> {
    if b.first() != Some(&b':') {
        return None;
    }
    let mm = two(&b[1..])?;
    let mut rest = &b[3..];
    let mut ss = 0;
    let mut nanos = 0;
    if rest.first() == Some(&b':') {
        ss = two(&rest[1..])?;
        rest = &rest[3..];
        if rest.first() == Some(&b'.') {
            let frac = &rest[1..];
            let digits = frac.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 || digits > 9 {
                return None;
            }
            for d in &frac[..digits] {
                nanos = nanos * 10 + digit(*d)?;
            }
            nanos *= 10u32.pow(9 - digits as u32);
            rest = &frac[digits..];
        }
    }
    Some((hh, mm, ss, nanos, rest))
}

/// A twelve-hour clock reading with its `AM`/`PM`.
fn twelve_hour(hh: u32, rest: &[u8], meridiem: &str) -> Option<u32> {
    if !rest.is_empty() || !(1..=12).contains(&hh) {
        return None;
    }
    match meridiem {
        "AM" | "am" => Some(hh % 12),
        "PM" | "pm" => Some(hh % 12 + 12),
        _ => None,
    }
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = name.to_ascii_lowercase();
    let full = [
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
    MONTHS
        .iter()
        .zip(full)
        .position(|(short, long)| lower == *short || lower == long)
        .map(|i| i as u32 + 1)
}

fn is_weekday(name: &str) -> bool {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    const FULL: [&str; 7] = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let lower = name.to_ascii_lowercase();
    DAYS.contains(&lower.as_str()) || FULL.contains(&lower.as_str())
}

/// Seconds since the epoch of a civil date and time, checked.
fn local_seconds(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) || hh > 23 || mm > 59 || ss > 60
    {
        return None;
    }
    Some(
        days_from_civil(y, m, d) * 86_400
            + i64::from(hh) * 3600
            + i64::from(mm) * 60
            + i64::from(ss),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nemtime::{NEM_OFFSET_SECS, WEM_OFFSET_SECS};

    const T: i64 = 1_707_812_820; // 2024-02-13T08:27:00Z

    #[test]
    fn formats_with_the_offset() {
        assert_eq!(format_rfc3339(T, 0), "2024-02-13T08:27:00Z");
        assert_eq!(
            format_rfc3339(T, NEM_OFFSET_SECS),
            "2024-02-13T18:27:00+10:00"
        );
        assert_eq!(
            format_rfc3339(T, WEM_OFFSET_SECS),
            "2024-02-13T16:27:00+08:00"
        );
        assert_eq!(
            format_rfc3339(T, -3600 * 5 - 1800),
            "2024-02-13T02:57:00-05:30"
        );
        assert_eq!(format_rfc3339(0, 0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(-1, 0), "1969-12-31T23:59:59Z");
        let frac = Stamp {
            secs_utc: T,
            nanos: 500_000,
            offset_secs: 0,
        };
        assert_eq!(frac.rfc3339(), "2024-02-13T08:27:00.0005Z");
        assert_eq!(frac.to_string(), frac.rfc3339());
    }

    #[test]
    fn serde_is_the_rfc3339_text() {
        let s = Stamp::new(T, NEM_OFFSET_SECS);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"2024-02-13T18:27:00+10:00\"");
        assert_eq!(serde_json::from_str::<Stamp>(&json).unwrap(), s);
        let err = serde_json::from_str::<Stamp>("\"2024-02-13T18:27:00\"").unwrap_err();
        assert!(err.to_string().contains("offset"), "{err}");
        assert!(serde_json::from_str::<Stamp>("7").is_err());
    }

    #[test]
    fn reads_rfc3339_only_with_an_offset() {
        assert_eq!(
            parse_rfc3339("2024-02-13T08:27:00Z"),
            Some(Stamp::new(T, 0))
        );
        assert_eq!(
            parse_rfc3339("2024-02-13T18:27:00+10:00"),
            Some(Stamp::new(T, NEM_OFFSET_SECS))
        );
        assert_eq!(
            parse_rfc3339("2024-02-13t02:57:00-05:30"),
            Some(Stamp::new(T, -19_800))
        );
        assert_eq!(
            parse_rfc3339("2024-02-13 08:27:00.25z"),
            Some(Stamp {
                secs_utc: T,
                nanos: 250_000_000,
                offset_secs: 0
            })
        );
        // The round trip of every offset utilake writes.
        for off in [0, NEM_OFFSET_SECS, WEM_OFFSET_SECS] {
            let text = format_rfc3339(T, off);
            assert!(is_rfc3339(&text), "{text}");
            assert_eq!(parse_rfc3339(&text), Some(Stamp::new(T, off)));
        }
        for naive in [
            "2024-02-13T08:27:00",
            "2024-02-13 08:27:00",
            "2024-02-13",
            "2024-02-13T08:27",
            "2024-02-13T08:27:00+10",
            "2024-02-13T08:27:00+25:00",
            "2024-02-13T08:27:00+10:60",
            "2024-02-13T08:27:00.Z",
            "2024-02-13T08:27:00.1234567890Z",
            "2024-02-30T08:27:00Z",
            "2024-13-01T08:27:00Z",
            "2024-02-13T24:00:00Z",
            "2024-02-13T08:60:00Z",
            "2024-02-13T08:27:61Z",
            "",
            "Z",
        ] {
            assert_eq!(parse_rfc3339(naive), None, "{naive}");
            assert!(!is_rfc3339(naive));
        }
    }

    #[test]
    fn reads_http_dates() {
        assert_eq!(
            parse_http_date("Tue, 13 Feb 2024 08:27:00 GMT"),
            Some(Stamp::new(T, 0))
        );
        assert_eq!(
            parse_http_date("Mon, 18 May 2026 07:27:50 UTC").map(|t| t.rfc3339()),
            Some("2026-05-18T07:27:50Z".into())
        );
        for bad in [
            "Tue, 13 Feb 2024 08:27:00",
            "Tue, 13 Feb 2024 08:27:00 AEST",
            "Xyz, 13 Feb 2024 08:27:00 GMT",
            "Tue, 13 Foo 2024 08:27:00 GMT",
            "Tue, 30 Feb 2024 08:27:00 GMT",
            "Tue, 13 Feb 2024 08:27:00 GMT extra",
            "Tue, 13 Feb 2024 08:27:00Z GMT",
        ] {
            assert_eq!(parse_http_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn reads_wem_generated_at() {
        assert_eq!(
            parse_wem_generated("Thu Nov 02 2023 11:35:00Z").map(|t| t.rfc3339()),
            Some("2023-11-02T11:35:00Z".into())
        );
        assert_eq!(parse_wem_generated("Thu Nov 02 2023 11:35:00"), None);
        assert_eq!(parse_wem_generated("Nov 02 2023 11:35:00Z"), None);
        assert_eq!(parse_wem_generated("Thu Nov 02 2023 11:35:00Z x"), None);
    }

    #[test]
    fn reads_iis_listings_in_the_offset_given() {
        // nemweb lists in AEST: 8:27 AM there is 22:27 the day before in UTC.
        let short = parse_iis_listing("2/13/2024  8:27 AM", NEM_OFFSET_SECS).unwrap();
        assert_eq!(short.rfc3339(), "2024-02-13T08:27:00+10:00");
        assert_eq!(short.secs_utc, T - i64::from(NEM_OFFSET_SECS));
        let long = parse_iis_listing("Monday, October 12, 2026 10:45 AM", NEM_OFFSET_SECS).unwrap();
        assert_eq!(long.rfc3339(), "2026-10-12T10:45:00+10:00");
        assert_eq!(
            parse_iis_listing("Monday, May 18, 2026 05:27 PM", NEM_OFFSET_SECS)
                .unwrap()
                .rfc3339(),
            "2026-05-18T17:27:00+10:00"
        );
        assert_eq!(
            parse_iis_listing("12/31/2024 12:00 AM", 0)
                .unwrap()
                .rfc3339(),
            "2024-12-31T00:00:00Z"
        );
        assert_eq!(
            parse_iis_listing("12/31/2024 12:30 PM", 0)
                .unwrap()
                .rfc3339(),
            "2024-12-31T12:30:00Z"
        );
        assert_eq!(
            parse_iis_listing("02/03/2024 08:05 am", 0)
                .unwrap()
                .rfc3339(),
            "2024-02-03T08:05:00Z"
        );
        for bad in [
            "2/13/2024 8:27",
            "2/13/2024 8:27 XM",
            "2/13/2024 13:27 PM",
            "2/13/2024 0:27 AM",
            "2/13/2024/1 8:27 AM",
            "2/13/2024 8:27 AM 1",
            "2/13/2024 8:7 AM",
            "2/13/20245 8:27 AM",
            "2/13/2024  8:27AM",
            "2/13/20248:27 AM",
            "2/13 8:27 AM",
            "Funday, October 12, 2026 10:45 AM",
            "Monday, Octember 12, 2026 10:45 AM",
            "Monday, October 12, 2026 10:45 AM 1",
            "13/2/2024 8:27 AM",
            "",
        ] {
            assert_eq!(parse_iis_listing(bad, 0), None, "{bad}");
        }
    }

    #[test]
    fn reads_a_naive_time_in_the_offset_given() {
        assert_eq!(
            parse_naive("2017-01-12 12:24:16", NEM_OFFSET_SECS)
                .unwrap()
                .rfc3339(),
            "2017-01-12T12:24:16+10:00"
        );
        assert_eq!(
            parse_naive("2017/01/12T12:24:16.5", 0).unwrap().rfc3339(),
            "2017-01-12T12:24:16.5Z"
        );
        assert_eq!(parse_naive("2017-01-12T12:24:16Z", 0), None);
        assert_eq!(parse_naive("2017-01-12", 0), None);
        assert_eq!(parse_naive("2017-01-12 12:24:16 x", 0), None);
    }

    #[test]
    fn reads_any_form_and_normalises() {
        let nem = NEM_OFFSET_SECS;
        assert_eq!(
            normalise("Tue, 13 Feb 2024 08:27:00 GMT", nem).as_deref(),
            Some("2024-02-13T08:27:00Z")
        );
        assert_eq!(
            normalise(" 2024-02-13T08:27:00Z ", nem).as_deref(),
            Some("2024-02-13T08:27:00Z")
        );
        assert_eq!(
            normalise("2024-02-13T18:27:00+10:00", nem).as_deref(),
            Some("2024-02-13T18:27:00+10:00")
        );
        assert_eq!(
            normalise("Thu Nov 02 2023 11:35:00Z", nem).as_deref(),
            Some("2023-11-02T11:35:00Z")
        );
        assert_eq!(
            normalise("Monday, May 18, 2026 05:27 PM", nem).as_deref(),
            Some("2026-05-18T17:27:00+10:00")
        );
        assert_eq!(
            normalise("2017-01-12 12:24:16", nem).as_deref(),
            Some("2017-01-12T12:24:16+10:00")
        );
        assert_eq!(normalise("yesterday", nem), None);
        assert_eq!(normalise("", nem), None);
        // The same instant, whichever offset it was written in.
        let a = parse("2024-02-13T08:27:00Z", nem).unwrap();
        let b = parse("2024-02-13T18:27:00+10:00", nem).unwrap();
        assert_eq!(a.secs_utc, b.secs_utc);
        assert_ne!(a.offset_secs, b.offset_secs);
    }

    #[test]
    fn helpers_reject_what_is_not_theirs() {
        assert_eq!(number(""), None);
        assert_eq!(number("1234567"), None);
        assert_eq!(number("12a"), None);
        assert_eq!(month_number("Sept"), None);
        assert_eq!(month_number("DEC"), Some(12));
        assert!(is_weekday("SUNDAY"));
        assert!(!is_weekday("Sun."));
        assert_eq!(time_of_day(b"8:27"), None);
        assert_eq!(time_of_day(b"08-27"), None);
        assert_eq!(time_of_day(b"08:27:0"), None);
        assert_eq!(time_of_day(b"08"), None);
        assert_eq!(iis_time_of_day(b"8:27"), Some((8, 27, 0, 0, &b""[..])));
        assert_eq!(iis_time_of_day(b"08:27"), Some((8, 27, 0, 0, &b""[..])));
        assert_eq!(iis_time_of_day(b":27"), None);
        assert_eq!(iis_time_of_day(b"x:27"), None);
        assert_eq!(
            local_seconds(2024, 2, 29, 0, 0, 0),
            Some(days_from_civil(2024, 2, 29) * 86_400)
        );
        assert_eq!(local_seconds(2023, 2, 29, 0, 0, 0), None);
        // A leap second is read as the last second of the minute.
        assert!(local_seconds(2016, 12, 31, 23, 59, 60).is_some());
    }
}
