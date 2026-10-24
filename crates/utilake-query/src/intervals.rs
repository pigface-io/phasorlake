//! What a window should hold, measured from what it holds: the distinct
//! valid times a read returned, their step, and the gaps between them.
//!
//! The registry records no interval length, and a table's can change
//! (TRADINGPRICE went from 30 to 5 minutes in October 2021), so the step
//! is not declared but read off the frame: the most common gap between
//! consecutive distinct valid times. Every gap seen is reported, so a
//! window across such a change shows both.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::time::{MarketOffset, ValidWindow, rfc3339};

/// How many gaps are listed; the rest are counted.
const LISTED_GAPS: usize = 20;

/// A run of missing intervals between two present ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gap {
    /// The last valid time before the gap, in the market's clock.
    pub after: String,
    /// The first valid time after it.
    pub before: String,
    /// Intervals of the step that lie between them.
    pub missing: u64,
}

/// The intervals of a window, measured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Intervals {
    /// The most common gap between consecutive distinct valid times, as
    /// an ISO 8601 duration (`PT5M`, `PT30M`, `P1D`).
    pub step: String,
    /// Every gap seen, shortest first, at most eight.
    pub steps_seen: Vec<String>,
    /// Distinct valid times in the frame.
    pub present: u64,
    /// Intervals of the step the window holds; none for a window that is
    /// one instant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<u64>,
    /// The first gaps, in time order.
    pub gaps: Vec<Gap>,
    /// Gaps beyond the ones listed.
    pub more_gaps: u64,
}

/// A span of microseconds as an ISO 8601 duration.
pub fn iso_duration(us: i64) -> String {
    const S: i64 = 1_000_000;
    if us <= 0 {
        return "PT0S".to_owned();
    }
    if us % (86_400 * S) == 0 {
        return format!("P{}D", us / (86_400 * S));
    }
    if us % (3_600 * S) == 0 {
        return format!("PT{}H", us / (3_600 * S));
    }
    if us % (60 * S) == 0 {
        return format!("PT{}M", us / (60 * S));
    }
    if us % S == 0 {
        return format!("PT{}S", us / S);
    }
    format!("PT{}.{:06}S", us / S, us % S)
}

/// Measures `times`, the valid times of a read's rows (any order, with
/// repeats), against `window`. None with fewer than three distinct times:
/// no step can be read off them.
pub fn measure(times: &mut Vec<i64>, window: &ValidWindow, off: MarketOffset) -> Option<Intervals> {
    if !times.is_sorted() {
        times.sort_unstable();
    }
    times.dedup();
    if times.len() < 3 {
        return None;
    }
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for w in times.windows(2) {
        *counts.entry(w[1] - w[0]).or_default() += 1;
    }
    let step = counts
        .iter()
        .max_by_key(|(d, n)| (**n, std::cmp::Reverse(**d)))
        .map(|(d, _)| *d)
        .expect("two or more times give a gap");
    let steps_seen = counts.keys().take(8).map(|d| iso_duration(*d)).collect();
    let mut gaps = Vec::new();
    let mut more_gaps = 0;
    for w in times.windows(2) {
        let d = w[1] - w[0];
        if d > step {
            if gaps.len() < LISTED_GAPS {
                gaps.push(Gap {
                    after: rfc3339(w[0], off.0),
                    before: rfc3339(w[1], off.0),
                    missing: u64::try_from(d / step - 1).unwrap_or(u64::MAX),
                });
            } else {
                more_gaps += 1;
            }
        }
    }
    let expected = match window {
        ValidWindow::Range { after, upto } => {
            Some(u64::try_from((upto.get() - after.get()) / step).unwrap_or(u64::MAX))
        }
        ValidWindow::At(_) => None,
    };
    Some(Intervals {
        step: iso_duration(step),
        steps_seen,
        present: times.len() as u64,
        expected,
        gaps,
        more_gaps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::ValidTime;

    const M: i64 = 60_000_000;
    const NEM: MarketOffset = MarketOffset(36_000);

    #[test]
    fn durations() {
        assert_eq!(iso_duration(5 * M), "PT5M");
        assert_eq!(iso_duration(30 * M), "PT30M");
        assert_eq!(iso_duration(60 * M), "PT1H");
        assert_eq!(iso_duration(1440 * M), "P1D");
        assert_eq!(iso_duration(90_000_000), "PT90S");
        assert_eq!(iso_duration(1_500_000), "PT1.500000S");
        assert_eq!(iso_duration(0), "PT0S");
    }

    #[test]
    fn a_day_of_five_minute_intervals_with_one_gap_and_a_change_of_step() {
        // Midnight 2024-01-01 in +10:00.
        let t0 = 1_704_031_200_000_000;
        let mut times: Vec<i64> = (1..=288).map(|i| t0 + i * 5 * M).collect();
        // One interval missing, every row twice, out of order.
        times.retain(|t| *t != t0 + 100 * 5 * M);
        let mut twice = times.clone();
        twice.reverse();
        times.extend(twice);
        let window = ValidWindow::Range {
            after: ValidTime::new(t0),
            upto: ValidTime::new(t0 + 288 * 5 * M),
        };
        let i = measure(&mut times, &window, NEM).unwrap();
        assert_eq!(i.step, "PT5M");
        assert_eq!(i.steps_seen, ["PT5M", "PT10M"]);
        assert_eq!((i.present, i.expected), (287, Some(288)));
        assert_eq!(
            i.gaps,
            [Gap {
                after: "2024-01-01T08:15:00+10:00".into(),
                before: "2024-01-01T08:25:00+10:00".into(),
                missing: 1,
            }]
        );
        assert_eq!(i.more_gaps, 0);

        // Thirty-minute rows followed by five-minute ones: the step is the
        // common one, both are seen, and the coarser half reads as gaps.
        let mut mixed: Vec<i64> = (1..=12).map(|i| t0 + i * 30 * M).collect();
        mixed.extend((73..=144).map(|i| t0 + i * 5 * M));
        let i = measure(&mut mixed, &window, NEM).unwrap();
        assert_eq!(i.step, "PT5M");
        assert_eq!(i.steps_seen, ["PT5M", "PT30M"]);
        assert_eq!(i.gaps.len(), 11);
        assert_eq!(i.gaps[0].missing, 5);

        // An instant has no expectation; too few times, no measure.
        let at = ValidWindow::At(ValidTime::new(t0));
        let mut three = vec![t0, t0 + M, t0 + 2 * M];
        assert_eq!(measure(&mut three, &at, NEM).unwrap().expected, None);
        let mut two = vec![t0, t0 + M];
        assert!(measure(&mut two, &at, NEM).is_none());
        // Many gaps: the first twenty listed, the rest counted.
        let mut sparse: Vec<i64> = (0..100).map(|i| t0 + i * 5 * M).collect();
        let mut t = t0 + 99 * 5 * M;
        for _ in 0..40 {
            t += 10 * M;
            sparse.push(t);
        }
        let i = measure(&mut sparse, &window, NEM).unwrap();
        assert_eq!(i.step, "PT5M", "{:?}", i.steps_seen);
        assert_eq!((i.gaps.len(), i.more_gaps), (20, 20));
    }
}
