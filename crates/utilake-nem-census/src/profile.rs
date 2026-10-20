//! What a column's values are, read from every D row: how many are null,
//! what kind they are on a widening lattice, how long and how precise.
//!
//! Nothing is sampled and nothing is taken from AEMO's declarations: this is
//! the evidence the column's Iceberg type is chosen from
//! (`utilake_iceberg::type_for`) and the declarations are audited against.

use std::cmp::Ordering;

use utilake_iceberg::{Evidence, Kind};

/// One column of one block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColumnProfile {
    /// Rows without a value: an empty field, or no field at all.
    pub nulls: u64,
    /// The kind of every value.
    pub kind: Kind,
    /// The most digits before the point.
    pub int_digits: u8,
    /// The most digits after it, as written.
    pub scale: u8,
    /// The longest value, in bytes.
    pub max_len: u32,
    /// The least value of a number or time column, as written.
    pub min: Option<Vec<u8>>,
    /// The greatest value of a number or time column, as written.
    pub max: Option<Vec<u8>>,
}

impl ColumnProfile {
    /// Takes one field; `None` when the row has no field there.
    pub fn observe(&mut self, v: Option<&[u8]>) {
        let Some(v) = v.filter(|v| !v.is_empty()) else {
            self.nulls += 1;
            return;
        };
        let (kind, int_digits, scale) = classify(v);
        self.kind = self.kind.join(kind);
        self.int_digits = self.int_digits.max(int_digits);
        self.scale = self.scale.max(scale);
        self.max_len = self.max_len.max(u32::try_from(v.len()).unwrap_or(u32::MAX));
        if self.kind == Kind::Text {
            self.min = None;
            self.max = None;
            return;
        }
        let numeric = matches!(self.kind, Kind::Integer | Kind::Decimal);
        let below = |a: &[u8], b: &[u8]| order(a, b, numeric) == Ordering::Less;
        match &mut self.min {
            Some(m) if !below(v, m) => {}
            m => *m = Some(v.to_vec()),
        }
        match &mut self.max {
            Some(m) if !below(m, v) => {}
            m => *m = Some(v.to_vec()),
        }
    }

    /// What the profile says of the column's type.
    pub fn evidence(&self) -> Evidence {
        Evidence {
            kind: self.kind,
            int_digits: self.int_digits,
            scale: self.scale,
        }
    }
}

/// Orders two values: numbers by value, times as written (AEMO's
/// `YYYY/MM/DD HH:MM:SS` orders as text).
fn order(a: &[u8], b: &[u8], numeric: bool) -> Ordering {
    if numeric {
        let n = |v: &[u8]| {
            std::str::from_utf8(v)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(f64::NAN)
        };
        n(a).total_cmp(&n(b))
    } else {
        a.cmp(b)
    }
}

/// The kind of one value, with its digits before and after the point.
pub fn classify(v: &[u8]) -> (Kind, u8, u8) {
    if let Some(k) = time_kind(v) {
        return (k, 0, 0);
    }
    number(v).unwrap_or((Kind::Text, 0, 0))
}

/// `YYYY/MM/DD` is a date; with ` HH:MM:SS` and an optional fraction, a
/// time.
fn time_kind(v: &[u8]) -> Option<Kind> {
    let digits =
        |r: std::ops::Range<usize>| v.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    let date = v.len() >= 10
        && digits(0..4)
        && v[4] == b'/'
        && digits(5..7)
        && v[7] == b'/'
        && digits(8..10);
    if !date {
        return None;
    }
    if v.len() == 10 {
        return Some(Kind::Date);
    }
    let time = v.len() >= 19
        && v[10] == b' '
        && digits(11..13)
        && v[13] == b':'
        && digits(14..16)
        && v[16] == b':'
        && digits(17..19);
    let fraction = || v.len() == 19 || (v[19] == b'.' && v.len() > 20 && digits(20..v.len()));
    (time && fraction()).then_some(Kind::Timestamp)
}

/// A number: an optional sign, digits with at most one point, and an
/// optional exponent. Its digits before and after the point are those of
/// its value written out in full; leading zeros do not count.
fn number(v: &[u8]) -> Option<(Kind, u8, u8)> {
    let s = std::str::from_utf8(v).ok()?;
    let s = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (mantissa, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], s[i + 1..].parse::<i32>().ok()?),
        None => (s, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all_digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if (int.is_empty() && frac.is_empty()) || !all_digits(int) || !all_digits(frac) {
        return None;
    }
    let int_len = i32::try_from(int.trim_start_matches('0').len()).ok()?;
    let frac_len = i32::try_from(frac.len()).ok()?;
    let int_digits = (int_len + exp).max(0);
    let scale = (frac_len - exp).max(0);
    let kind = if scale == 0 && !s.contains(['.', 'e', 'E']) {
        Kind::Integer
    } else {
        Kind::Decimal
    };
    let cap = |n: i32| u8::try_from(n).unwrap_or(u8::MAX);
    Some((kind, cap(int_digits), cap(scale)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_aemo_values() {
        assert_eq!(classify(b"2024/08/01 00:05:00"), (Kind::Timestamp, 0, 0));
        assert_eq!(
            classify(b"2024/08/01 00:05:00.123"),
            (Kind::Timestamp, 0, 0)
        );
        assert_eq!(classify(b"2024/08/01"), (Kind::Date, 0, 0));
        assert_eq!(classify(b"2024/08/01 00:05"), (Kind::Text, 0, 0));
        assert_eq!(classify(b"2024/08/01 00:05:00."), (Kind::Text, 0, 0));
        assert_eq!(classify(b"2024/08/01X"), (Kind::Text, 0, 0));
        assert_eq!(classify(b"123"), (Kind::Integer, 3, 0));
        assert_eq!(classify(b"-0042"), (Kind::Integer, 2, 0));
        assert_eq!(classify(b"0"), (Kind::Integer, 0, 0));
        assert_eq!(classify(b"15500.12345"), (Kind::Decimal, 5, 5));
        assert_eq!(classify(b".5"), (Kind::Decimal, 0, 1));
        assert_eq!(classify(b"12."), (Kind::Decimal, 2, 0));
        assert_eq!(classify(b"1.5E-05"), (Kind::Decimal, 0, 6));
        assert_eq!(classify(b"2E+3"), (Kind::Decimal, 4, 0));
        assert_eq!(classify(b"2E+x"), (Kind::Text, 0, 0));
        assert_eq!(classify(b"."), (Kind::Text, 0, 0));
        assert_eq!(classify(b"1.2.3"), (Kind::Text, 0, 0));
        assert_eq!(classify(b"NSW1"), (Kind::Text, 0, 0));
        assert_eq!(classify(&[0xff, b'1']), (Kind::Text, 0, 0));
    }

    #[test]
    fn a_column_widens_counts_nulls_and_bounds_its_values() {
        let mut p = ColumnProfile::default();
        for v in [
            Some(&b"5"[..]),
            None,
            Some(b""),
            Some(b"-12.5"),
            Some(b"300"),
        ] {
            p.observe(v);
        }
        assert_eq!(p.nulls, 2);
        assert_eq!(p.kind, Kind::Decimal);
        assert_eq!((p.int_digits, p.scale, p.max_len), (3, 1, 5));
        assert_eq!(p.min.as_deref(), Some(&b"-12.5"[..]));
        assert_eq!(p.max.as_deref(), Some(&b"300"[..]));
        assert_eq!(
            p.evidence(),
            Evidence {
                kind: Kind::Decimal,
                int_digits: 3,
                scale: 1
            }
        );

        let mut t = ColumnProfile::default();
        t.observe(Some(b"2024/08/02 00:00:00"));
        t.observe(Some(b"2024/08/01"));
        assert_eq!(t.kind, Kind::Timestamp);
        assert_eq!(t.min.as_deref(), Some(&b"2024/08/01"[..]));
        assert_eq!(t.max.as_deref(), Some(&b"2024/08/02 00:00:00"[..]));
        // Text has no bounds.
        t.observe(Some(b"N/A"));
        assert_eq!(
            (t.kind, t.min.clone(), t.max.clone()),
            (Kind::Text, None, None)
        );
        t.observe(Some(b"OTHER"));
        assert_eq!(t.max_len, 19);

        let never = ColumnProfile::default();
        assert_eq!(never.evidence(), Evidence::default());
    }

    #[test]
    fn numbers_order_by_value_and_bad_ones_last() {
        assert_eq!(order(b"9", b"10", true), Ordering::Less);
        assert_eq!(order(b"9", b"10", false), Ordering::Greater);
        assert_eq!(order(b"x", b"1", true), Ordering::Greater);
    }
}
