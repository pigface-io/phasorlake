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
    /// The most significant digits of any one value.
    pub sig_digits: u8,
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
        let (kind, int_digits, scale, sig) = classify(v);
        self.kind = self.kind.join(kind);
        self.int_digits = self.int_digits.max(int_digits);
        self.scale = self.scale.max(scale);
        self.sig_digits = self.sig_digits.max(sig);
        self.max_len = self.max_len.max(u32::try_from(v.len()).unwrap_or(u32::MAX));
        if self.kind == Kind::Text {
            self.min = None;
            self.max = None;
            return;
        }
        let numeric = matches!(self.kind, Kind::Integer | Kind::Decimal);
        if self
            .min
            .as_deref()
            .is_none_or(|m| order(v, m, numeric) == Ordering::Less)
        {
            set(&mut self.min, v);
        }
        if self
            .max
            .as_deref()
            .is_none_or(|m| order(v, m, numeric) == Ordering::Greater)
        {
            set(&mut self.max, v);
        }
    }

    /// Takes in another profile of the same column.
    pub fn merge(&mut self, other: &Self) {
        self.nulls += other.nulls;
        self.kind = self.kind.join(other.kind);
        self.int_digits = self.int_digits.max(other.int_digits);
        self.scale = self.scale.max(other.scale);
        self.sig_digits = self.sig_digits.max(other.sig_digits);
        self.max_len = self.max_len.max(other.max_len);
        if self.kind == Kind::Text {
            self.min = None;
            self.max = None;
            return;
        }
        let numeric = matches!(self.kind, Kind::Integer | Kind::Decimal);
        let pick = |mine: &mut Option<Vec<u8>>, theirs: &Option<Vec<u8>>, want: Ordering| {
            if let Some(t) = theirs
                && mine.as_ref().is_none_or(|m| order(t, m, numeric) == want)
            {
                *mine = Some(t.clone());
            }
        };
        pick(&mut self.min, &other.min, Ordering::Less);
        pick(&mut self.max, &other.max, Ordering::Greater);
    }

    /// What the profile says of the column's type.
    pub fn evidence(&self) -> Evidence {
        Evidence {
            kind: self.kind,
            int_digits: self.int_digits,
            scale: self.scale,
            sig_digits: self.sig_digits,
        }
    }
}

/// Replaces a bound, reusing its buffer.
fn set(slot: &mut Option<Vec<u8>>, v: &[u8]) {
    let b = slot.get_or_insert_with(Vec::new);
    b.clear();
    b.extend_from_slice(v);
}

/// Orders two values: numbers by value, times as written (AEMO's
/// `YYYY/MM/DD HH:MM:SS` orders as text).
fn order(a: &[u8], b: &[u8], numeric: bool) -> Ordering {
    if !numeric {
        return a.cmp(b);
    }
    match (plain(a), plain(b)) {
        (Some(x), Some(y)) => cmp_plain(x, y),
        _ => {
            let n = |v: &[u8]| utilake_core::num::parse_f64(v).unwrap_or(f64::NAN);
            n(a).total_cmp(&n(b))
        }
    }
}

/// A plain decimal split into sign, whole digits without leading zeros,
/// and fraction digits without trailing zeros; `None` for anything else
/// (an exponent form).
fn plain(v: &[u8]) -> Option<(bool, &[u8], &[u8])> {
    let (neg, body) = match v.first() {
        Some(b'-') => (true, &v[1..]),
        Some(b'+') => (false, &v[1..]),
        _ => (false, v),
    };
    let (int, frac) = match body.iter().position(|&c| c == b'.') {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, &body[..0]),
    };
    if !int.iter().chain(frac).all(u8::is_ascii_digit) {
        return None;
    }
    let int = &int[int.iter().position(|&c| c != b'0').unwrap_or(int.len())..];
    let frac = &frac[..frac.iter().rposition(|&c| c != b'0').map_or(0, |i| i + 1)];
    // Minus zero is zero.
    Some((neg && !(int.is_empty() && frac.is_empty()), int, frac))
}

/// Orders two plain decimals by value, without parsing them.
fn cmp_plain(a: (bool, &[u8], &[u8]), b: (bool, &[u8], &[u8])) -> Ordering {
    let magnitude = |(_, ai, af): (bool, &[u8], &[u8]), (_, bi, bf): (bool, &[u8], &[u8])| {
        ai.len().cmp(&bi.len()).then(ai.cmp(bi)).then(af.cmp(bf))
    };
    match (a.0, b.0) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => magnitude(a, b),
        (true, true) => magnitude(b, a),
    }
}

/// The kind of one value, with its digits before and after the point and
/// its significant digits.
pub fn classify(v: &[u8]) -> (Kind, u8, u8, u8) {
    if let Some(k) = time_kind(v) {
        return (k, 0, 0, 0);
    }
    number(v).unwrap_or((Kind::Text, 0, 0, 0))
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
fn number(v: &[u8]) -> Option<(Kind, u8, u8, u8)> {
    let mut i = usize::from(matches!(v.first(), Some(b'-' | b'+')));
    let (mut int_len, mut frac_len, mut leading) = (0i32, 0i32, true);
    let mut sig = 0i32;
    let mut point = false;
    let mut any = false;
    while let Some(&c) = v.get(i) {
        match c {
            b'0'..=b'9' => {
                any = true;
                if point {
                    frac_len += 1;
                } else if !(leading && c == b'0') {
                    int_len += 1;
                }
                if !(leading && c == b'0') {
                    leading = false;
                    sig += 1;
                }
            }
            b'.' if !point => point = true,
            _ => break,
        }
        i += 1;
    }
    if !any {
        return None;
    }
    let mut exp = 0i32;
    let mut exponent = false;
    if let Some(b'e' | b'E') = v.get(i) {
        exponent = true;
        let rest = std::str::from_utf8(&v[i + 1..]).ok()?;
        exp = rest.parse().ok()?;
    } else if i != v.len() {
        return None;
    }
    let int_digits = (int_len + exp).max(0);
    let scale = (frac_len - exp).max(0);
    let kind = if point || exponent {
        Kind::Decimal
    } else {
        Kind::Integer
    };
    let cap = |n: i32| u8::try_from(n).unwrap_or(u8::MAX);
    Some((kind, cap(int_digits), cap(scale), cap(sig)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c3(v: &[u8]) -> (Kind, u8, u8) {
        let (k, i, s, _) = classify(v);
        (k, i, s)
    }

    #[test]
    fn classifies_aemo_values() {
        assert_eq!(classify(b"239.99999999999997").3, 17);
        assert_eq!(classify(b"0.000123").3, 3);
        assert_eq!(classify(b"-0042").3, 2);
        assert_eq!(classify(b"N").3, 0);
        assert_eq!(c3(b"2024/08/01 00:05:00"), (Kind::Timestamp, 0, 0));
        assert_eq!(c3(b"2024/08/01 00:05:00.123"), (Kind::Timestamp, 0, 0));
        assert_eq!(c3(b"2024/08/01"), (Kind::Date, 0, 0));
        assert_eq!(c3(b"2024/08/01 00:05"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"2024/08/01 00:05:00."), (Kind::Text, 0, 0));
        assert_eq!(c3(b"2024/08/01X"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"123"), (Kind::Integer, 3, 0));
        assert_eq!(c3(b"-0042"), (Kind::Integer, 2, 0));
        assert_eq!(c3(b"0"), (Kind::Integer, 0, 0));
        assert_eq!(c3(b"15500.12345"), (Kind::Decimal, 5, 5));
        assert_eq!(c3(b".5"), (Kind::Decimal, 0, 1));
        assert_eq!(c3(b"12."), (Kind::Decimal, 2, 0));
        assert_eq!(c3(b"1.5E-05"), (Kind::Decimal, 0, 6));
        assert_eq!(c3(b"2E+3"), (Kind::Decimal, 4, 0));
        assert_eq!(c3(b"2E+x"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"."), (Kind::Text, 0, 0));
        assert_eq!(c3(b"1.2.3"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"NSW1"), (Kind::Text, 0, 0));
        assert_eq!(c3(&[0xff, b'1']), (Kind::Text, 0, 0));
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
                scale: 1,
                sig_digits: 3
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
    fn profiles_merge_as_if_one() {
        let mut a = ColumnProfile::default();
        a.observe(Some(b"5"));
        a.observe(None);
        let mut b = ColumnProfile::default();
        b.observe(Some(b"-2.25"));
        b.observe(Some(b"40"));
        let mut both = a.clone();
        both.merge(&b);
        let mut once = ColumnProfile::default();
        for v in [Some(&b"5"[..]), None, Some(b"-2.25"), Some(b"40")] {
            once.observe(v);
        }
        assert_eq!(both, once);
        let mut text = ColumnProfile::default();
        text.observe(Some(b"N/A"));
        both.merge(&text);
        assert_eq!((both.kind, both.min, both.max), (Kind::Text, None, None));
        let mut empty = ColumnProfile::default();
        empty.merge(&b);
        assert_eq!(empty, b);
    }

    #[test]
    fn numbers_order_by_value_and_bad_ones_last() {
        assert_eq!(order(b"9", b"10", true), Ordering::Less);
        assert_eq!(order(b"9", b"10", false), Ordering::Greater);
        assert_eq!(order(b"x", b"1", true), Ordering::Greater);
        assert_eq!(order(b"-2.5", b"-10", true), Ordering::Greater);
        assert_eq!(order(b"-0.0", b"0", true), Ordering::Equal);
        assert_eq!(order(b"+007.50", b"7.5", true), Ordering::Equal);
        assert_eq!(order(b"0.25", b"0.3", true), Ordering::Less);
        assert_eq!(order(b"1E+3", b"999", true), Ordering::Greater);
        assert_eq!(order(b"-1", b"2", true), Ordering::Less);
    }

    #[test]
    fn a_column_that_changes_kind_starts_its_bounds_again() {
        let mut p = ColumnProfile::default();
        p.observe(Some(b"2024/08/02"));
        p.observe(Some(b"7"));
        assert_eq!(p.kind, Kind::Text);
        let mut q = ColumnProfile::default();
        q.observe(Some(b"7"));
        q.observe(Some(b"3.5"));
        assert_eq!(
            (q.min.as_deref(), q.max.as_deref()),
            (Some(&b"3.5"[..]), Some(&b"7"[..]))
        );
    }
}
