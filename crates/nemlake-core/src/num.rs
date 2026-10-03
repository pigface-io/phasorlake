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
/// exactly what AEMO published or refuses the row). Exponent forms are not
/// decimals; callers wanting them use `parse_f64`.
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
            return Err(NumberError::DecimalShape);
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
        assert_eq!(parse_decimal(b"1e5", 15, 2), Err(NumberError::DecimalShape));
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
