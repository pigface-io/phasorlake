//! The columns the lake computes from a row beside the table's own
//! ([`DerivedStep`]): each an instant as microseconds since 1970 UTC, from
//! fields the row carries. A field that is empty or does not read gives
//! null; nothing is refused, since the value it is computed from is kept
//! as the table's own column, misfit or not. Integer arithmetic only: this
//! runs for every row.

use phasor_core::nztime;
use phasor_registry::{DeriveRule, DerivedStep};

use crate::mmstime;

/// The value of `step` for a row whose data field `i` is `field(i)`.
pub fn compute<'a>(step: &DerivedStep, field: impl Fn(u16) -> &'a [u8]) -> Option<i64> {
    match step.rule {
        DeriveRule::UtcUs => {
            let bytes = field(*step.sources.first()?);
            let (nanos, _) = mmstime::parse_nanos(bytes, step.offset_secs).ok()?;
            Some(nanos.div_euclid(1_000))
        }
        DeriveRule::NzTradingPeriodStart => {
            let [date, period] = step.sources.as_slice() else {
                return None;
            };
            let days = mmstime::parse_days(field(*date)).ok()?;
            let (y, m, d) = phasor_core::calendar::civil_from_days(i64::from(days));
            let period = integer(field(*period))?;
            nztime::trading_period_start_us(y, m, d, period)
        }
    }
}

/// A field of digits alone as a number; `None` otherwise.
fn integer(bytes: &[u8]) -> Option<u32> {
    let bytes = bytes.trim_ascii();
    if bytes.is_empty() || bytes.len() > 9 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    bytes.iter().try_fold(0u32, |n, b| {
        n.checked_mul(10)?.checked_add(u32::from(b - b'0'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(rule: DeriveRule, sources: &[u16]) -> DerivedStep {
        DerivedStep {
            target: 9,
            rule,
            sources: sources.to_vec(),
            offset_secs: 12 * 3600,
        }
    }

    #[test]
    fn a_timestamps_instant_in_microseconds() {
        let fields: [&[u8]; 2] = [b"2026/01/04 22:59:22.250", b""];
        let at = |i: u16| fields[usize::from(i)];
        // 22:59:22.25 NZST is 10:59:22.25 UTC.
        let expected = (phasor_core::calendar::days_from_civil(2026, 1, 4) * 86_400
            + 10 * 3_600
            + 59 * 60
            + 22)
            * 1_000_000
            + 250_000;
        assert_eq!(compute(&step(DeriveRule::UtcUs, &[0]), at), Some(expected));
        assert_eq!(compute(&step(DeriveRule::UtcUs, &[1]), at), None, "empty");
        assert_eq!(
            compute(&step(DeriveRule::UtcUs, &[]), at),
            None,
            "no source"
        );
        let bad = |_: u16| &b"soon"[..];
        assert_eq!(compute(&step(DeriveRule::UtcUs, &[0]), bad), None);
    }

    #[test]
    fn a_trading_periods_start_from_its_date_and_number() {
        let fields: [&[u8]; 5] = [b"2026/01/05", b"1", b" 48 ", b"49", b"1.0"];
        let at = |i: u16| fields[usize::from(i)];
        let rule = DeriveRule::NzTradingPeriodStart;
        let start = nztime::trading_period_start_us(2026, 1, 5, 1).unwrap();
        assert_eq!(compute(&step(rule, &[0, 1]), at), Some(start));
        assert_eq!(
            compute(&step(rule, &[0, 2]), at),
            nztime::trading_period_start_us(2026, 1, 5, 48)
        );
        assert_eq!(
            compute(&step(rule, &[0, 3]), at),
            None,
            "a period the day lacks"
        );
        assert_eq!(
            compute(&step(rule, &[0, 4]), at),
            None,
            "not a whole number"
        );
        assert_eq!(compute(&step(rule, &[1, 1]), at), None, "not a date");
        assert_eq!(compute(&step(rule, &[0]), at), None, "one source short");
        assert_eq!(integer(b"4294967296"), None, "beyond u32, or ten digits");
        assert_eq!(integer(b"999999999"), Some(999_999_999));
    }
}
