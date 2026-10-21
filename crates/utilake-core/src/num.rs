//! Number parsing for MMS fields: `FLOAT EXTERNAL`, `INTEGER EXTERNAL` and
//! `NUMBER(p,s)` as decimal, from bytes, without allocation.

/// Why a byte string is not a number of the requested kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberError {
    /// Not a floating-point literal (`fast-float2` rejected it or left bytes over).
    Float,
    /// Not an optionally signed run of digits, or outside `i64`.
    Integer,
    /// Not a decimal literal of digits with at most one `.`.
    DecimalShape,
    /// More fractional digits than the column's scale allows.
    DecimalScale {
        /// The column's scale.
        scale: u8,
        /// Fractional digits seen.
        seen: u8,
    },
    /// Too many digits for `Decimal128(precision, scale)`.
    DecimalOverflow,
}

impl std::fmt::Display for NumberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Float => write!(f, "is not a floating-point number"),
            Self::Integer => write!(f, "is not an integer that fits in 64 bits"),
            Self::DecimalShape => write!(
                f,
                "is not a decimal number of digits with at most one point"
            ),
            Self::DecimalScale { scale, seen } => {
                write!(
                    f,
                    "has {seen} fractional digits but the column allows {scale}"
                )
            }
            Self::DecimalOverflow => {
                write!(f, "has more digits than the column's precision allows")
            }
        }
    }
}

impl std::error::Error for NumberError {}

/// Parses an `f64`. AEMO writes plain decimals and occasionally exponent
/// forms such as `6E-05`; both are accepted. Leading `+` is accepted.
#[inline]
pub fn parse_f64(s: &[u8]) -> Result<f64, NumberError> {
    match fast_float2::parse_partial::<f64, _>(s) {
        Ok((v, n)) if n == s.len() && n > 0 => Ok(v),
        _ => Err(NumberError::Float),
    }
}

/// Parses an `i64` from an optionally signed run of ASCII digits.
///
/// A trailing `.0`-style fraction is accepted when it is all zeros, because an
/// Oracle `NUMBER(n,0)` is sometimes exported with a decimal point; any other
/// fraction is an error so a true decimal is never truncated silently.
pub fn parse_i64(s: &[u8]) -> Result<i64, NumberError> {
    let (neg, body) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (int, frac) = match memchr::memchr(b'.', body) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    if int.is_empty() {
        return Err(NumberError::Integer);
    }
    let mut v: i64 = 0;
    for &b in int {
        let d = b.wrapping_sub(b'0');
        if d >= 10 {
            return Err(NumberError::Integer);
        }
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add(i64::from(d)))
            .ok_or(NumberError::Integer)?;
    }
    if let Some(frac) = frac
        && !frac.iter().all(|&b| b == b'0')
    {
        return Err(NumberError::Integer);
    }
    Ok(if neg { -v } else { v })
}

/// Parses a decimal literal into the unscaled `i128` of `Decimal128(precision, scale)`.
///
/// `123.45` with scale 5 becomes `12_345_000`. Fewer fractional digits than
/// the scale are padded; more are an error (never rounded, so the lake holds
/// exactly what AEMO published or refuses the row). AEMO writes a few small
/// numbers in exponent form (`-3E-05`); such a value is read exactly too,
/// when the column's scale holds it.
pub fn parse_decimal(s: &[u8], precision: u8, scale: u8) -> Result<i128, NumberError> {
    let (neg, body) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (int, frac) = match memchr::memchr(b'.', body) {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, &body[body.len()..]),
    };
    if int.is_empty() && frac.is_empty() {
        return Err(NumberError::DecimalShape);
    }
    if frac.len() > usize::from(scale) {
        // Trailing zeros beyond the scale carry no information; only
        // significant extra digits are an error.
        if frac[usize::from(scale)..].iter().any(|&b| b != b'0') {
            if frac.iter().any(|&b| b == b'e' || b == b'E') {
                return exponent_form(s, precision, scale);
            }
            return Err(NumberError::DecimalScale {
                scale,
                seen: frac.len().min(255) as u8,
            });
        }
    }
    let frac = &frac[..frac.len().min(usize::from(scale))];
    let mut v: i128 = 0;
    let mut digits = 0u32;
    for &b in int.iter().chain(frac) {
        let d = b.wrapping_sub(b'0');
        if d >= 10 {
            return exponent_form(s, precision, scale);
        }
        if v != 0 || d != 0 {
            digits += 1;
        }
        v = v * 10 + i128::from(d);
    }
    let pad = u32::from(scale) - frac.len() as u32;
    digits += pad;
    v *= 10i128.pow(pad);
    if digits > u32::from(precision) {
        return Err(NumberError::DecimalOverflow);
    }
    Ok(if neg { -v } else { v })
}

/// The digits of `s` as one integer, and how many of them there are.
fn digits_of(s: &[u8]) -> Option<(i128, u32)> {
    let mut v: i128 = 0;
    for &b in s {
        let d = b.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(i128::from(d))?;
    }
    Some((v, s.len() as u32))
}

/// A decimal written `<mantissa>E<exponent>`, as `-3E-05`: exact, or an
/// error as [`parse_decimal`] gives one. Off the hot path: only a field
/// that is not plain digits comes here.
#[cold]
fn exponent_form(s: &[u8], precision: u8, scale: u8) -> Result<i128, NumberError> {
    let Some(e) = s.iter().position(|&b| b == b'e' || b == b'E') else {
        return Err(NumberError::DecimalShape);
    };
    let (mantissa, exponent) = (&s[..e], &s[e + 1..]);
    let (neg, body) = match mantissa.first() {
        Some(b'-') => (true, &mantissa[1..]),
        Some(b'+') => (false, &mantissa[1..]),
        _ => (false, mantissa),
    };
    let (int, frac) = match memchr::memchr(b'.', body) {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, &body[body.len()..]),
    };
    let exp: i32 = match exponent.first() {
        Some(b'-' | b'+') if exponent.len() > 1 => std::str::from_utf8(exponent)
            .ok()
            .and_then(|t| t.parse().ok()),
        Some(b'0'..=b'9') => std::str::from_utf8(exponent)
            .ok()
            .and_then(|t| t.parse().ok()),
        _ => None,
    }
    .filter(|e: &i32| e.abs() <= 76)
    .ok_or(NumberError::DecimalShape)?;
    if int.is_empty() && frac.is_empty() {
        return Err(NumberError::DecimalShape);
    }
    let ((i, _), (f, n)) = (
        digits_of(int).ok_or(NumberError::DecimalShape)?,
        digits_of(frac).ok_or(NumberError::DecimalShape)?,
    );
    let ten = |k: u32| 10i128.checked_pow(k).ok_or(NumberError::DecimalOverflow);
    let mut v = i
        .checked_mul(ten(n)?)
        .and_then(|v| v.checked_add(f))
        .ok_or(NumberError::DecimalOverflow)?;
    // `v` has `n - exp` fractional digits; the column has `scale`.
    let shift = i64::from(scale) - (i64::from(n) - i64::from(exp));
    if shift >= 0 {
        v = v
            .checked_mul(ten(
                u32::try_from(shift).map_err(|_| NumberError::DecimalOverflow)?
            )?)
            .ok_or(NumberError::DecimalOverflow)?;
    } else {
        let d = ten(u32::try_from(-shift).map_err(|_| NumberError::DecimalOverflow)?);
        match d {
            Ok(d) if v % d == 0 => v /= d,
            _ if v == 0 => {}
            _ => {
                return Err(NumberError::DecimalScale {
                    scale,
                    seen: (i64::from(scale) - shift).clamp(0, 255) as u8,
                });
            }
        }
    }
    let mut digits = 0u32;
    let mut rest = v;
    while rest != 0 {
        digits += 1;
        rest /= 10;
    }
    if digits > u32::from(precision) {
        return Err(NumberError::DecimalOverflow);
    }
    Ok(if neg { -v } else { v })
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        assert_eq!(parse_f64(b"57.98").unwrap(), 57.98);
        assert_eq!(parse_f64(b"6E-05").unwrap(), 6e-5);
        assert_eq!(parse_f64(b"-0.5").unwrap(), -0.5);
        assert_eq!(parse_f64(b"+1").unwrap(), 1.0);
        assert_eq!(
            parse_f64(b"13878.2169050000000000000000").unwrap(),
            13_878.216_905
        );
        assert_eq!(parse_f64(b""), Err(NumberError::Float));
        assert_eq!(parse_f64(b"abc"), Err(NumberError::Float));
        assert_eq!(parse_f64(b"1.0x"), Err(NumberError::Float));
    }

    #[test]
    fn integers() {
        assert_eq!(parse_i64(b"20231231241").unwrap(), 20_231_231_241);
        assert_eq!(parse_i64(b"-7").unwrap(), -7);
        assert_eq!(parse_i64(b"+7").unwrap(), 7);
        assert_eq!(parse_i64(b"7.00").unwrap(), 7);
        assert_eq!(parse_i64(b"7.5"), Err(NumberError::Integer));
        assert_eq!(parse_i64(b""), Err(NumberError::Integer));
        assert_eq!(parse_i64(b"-"), Err(NumberError::Integer));
        assert_eq!(parse_i64(b"1e3"), Err(NumberError::Integer));
        assert_eq!(
            parse_i64(b"99999999999999999999"),
            Err(NumberError::Integer)
        );
    }

    #[test]
    fn decimals() {
        assert_eq!(parse_decimal(b"123.45", 15, 5).unwrap(), 12_345_000);
        assert_eq!(parse_decimal(b"-0.5", 15, 5).unwrap(), -50_000);
        assert_eq!(parse_decimal(b"42", 15, 2).unwrap(), 4200);
        assert_eq!(parse_decimal(b".5", 15, 1).unwrap(), 5);
        assert_eq!(parse_decimal(b"5.", 15, 1).unwrap(), 50);
        assert_eq!(parse_decimal(b"1.2300", 15, 2).unwrap(), 123);
        assert_eq!(
            parse_decimal(b"1.234", 15, 2),
            Err(NumberError::DecimalScale { scale: 2, seen: 3 })
        );
        assert_eq!(parse_decimal(b"", 15, 2), Err(NumberError::DecimalShape));
        assert_eq!(parse_decimal(b".", 15, 2), Err(NumberError::DecimalShape));
        // Exponent forms, exactly, when the scale holds them.
        assert_eq!(parse_decimal(b"1e5", 15, 2).unwrap(), 10_000_000);
        assert_eq!(parse_decimal(b"-3E-05", 15, 5).unwrap(), -3);
        assert_eq!(parse_decimal(b"+1.5E-04", 15, 5).unwrap(), 15);
        assert_eq!(parse_decimal(b"2.50E+1", 15, 2).unwrap(), 2_500);
        assert_eq!(parse_decimal(b"0E-30", 15, 2).unwrap(), 0);
        assert_eq!(
            parse_decimal(b"1E-06", 15, 5),
            Err(NumberError::DecimalScale { scale: 5, seen: 6 })
        );
        assert_eq!(
            parse_decimal(b"1E20", 15, 2),
            Err(NumberError::DecimalOverflow)
        );
        assert_eq!(
            parse_decimal(b"1E80", 15, 2),
            Err(NumberError::DecimalShape)
        );
        assert_eq!(
            parse_decimal(b"1E-90", 15, 2),
            Err(NumberError::DecimalShape)
        );
        assert_eq!(
            parse_decimal(b"9E70", 38, 0),
            Err(NumberError::DecimalOverflow)
        );
        assert_eq!(
            parse_decimal(b"1E-70", 38, 0),
            Err(NumberError::DecimalScale { scale: 0, seen: 70 })
        );
        for bad in [&b"E5"[..], b"1E", b"1E-", b"1Ex", b"1xE5", b"1.xE5", b"abc"] {
            assert_eq!(
                parse_decimal(bad, 15, 2),
                Err(NumberError::DecimalShape),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_decimal(b"1.11111111111111111111111111111111111111111E0", 38, 2),
            Err(NumberError::DecimalShape)
        );
        assert_eq!(
            parse_decimal(b"1234", 5, 2),
            Err(NumberError::DecimalOverflow)
        );
        assert_eq!(parse_decimal(b"123", 5, 2).unwrap(), 12_300);
        assert_eq!(parse_decimal(b"0.00", 5, 2).unwrap(), 0);
        for e in [
            NumberError::Float,
            NumberError::Integer,
            NumberError::DecimalShape,
            NumberError::DecimalScale { scale: 1, seen: 2 },
            NumberError::DecimalOverflow,
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}
