//! What a window means on each valid axis, stated once and used by the
//! scan (which files and rows to read), the kernel (which versions to
//! keep) and the SQL (the same read for another engine).
//!
//! | axis | `Range { after, upto }` | `At(t)` |
//! |---|---|---|
//! | interval end | `after < t <= upto` | refused |
//! | instant | `after <= t < upto` | `t == at` |
//! | day (label `L`, trading day start `s`) | `L + s < upto && L + s + 1 day > after` | `L + s <= at < L + s + 1 day` |
//! | from | in force at `after`, and every later version until `upto`: `valid <= upto && valid_to > after` | `valid <= at < valid_to` |
//! | timeless | every row | every row |
//! | no roles | `after <= t < upto` on the time column | refused |
//!
//! An interval-ending time is the end of the period the row covers: the
//! row at `00:05` is the first of the day, the row at `24:00` its last,
//! so a day is `(midnight, next midnight]`. A day label is written at
//! midnight but covers the market's trading day, which starts at `s`
//! (04:00 in the NEM): the window keeps the labels whose day overlaps it.

use utilake_mms_registry::file_model::ValidAxis;

use crate::query::FieldIx;
use crate::time::{ValidEnd, ValidTime, ValidWindow};

/// A day, in microseconds.
pub const DAY_US: i64 = 86_400_000_000;

/// The microseconds `[t0, t1)` the planner prunes files by, for a window
/// on an axis (`None`: a table without roles, bounded on its time column);
/// `day_start` is the trading day's start after midnight, in microseconds.
pub fn plan_bounds(
    axis: Option<&ValidAxis<FieldIx>>,
    w: &ValidWindow,
    day_start: i64,
) -> (i64, i64) {
    match (axis, w) {
        (Some(ValidAxis::Timeless), _) => (i64::MIN, i64::MAX),
        (Some(ValidAxis::IntervalEnd(_)), ValidWindow::Range { after, upto }) => {
            (after.get().saturating_add(1), upto.get().saturating_add(1))
        }
        // Labels L with L + s < upto and L + s + 1 day > after.
        (Some(ValidAxis::Day(_)), ValidWindow::Range { after, upto }) => (
            after
                .get()
                .saturating_sub(day_start)
                .saturating_sub(DAY_US)
                .saturating_add(1),
            upto.get().saturating_sub(day_start),
        ),
        // Labels L with L + s <= t < L + s + 1 day.
        (Some(ValidAxis::Day(_)), ValidWindow::At(t)) => (
            t.get()
                .saturating_sub(day_start)
                .saturating_sub(DAY_US)
                .saturating_add(1),
            t.get().saturating_sub(day_start).saturating_add(1),
        ),
        (Some(ValidAxis::From { .. }), ValidWindow::Range { upto, .. }) => {
            (i64::MIN, upto.get().saturating_add(1))
        }
        (Some(ValidAxis::From { .. }), ValidWindow::At(t)) => (i64::MIN, t.get().saturating_add(1)),
        (_, ValidWindow::Range { after, upto }) => (after.get(), upto.get()),
        (_, ValidWindow::At(t)) => (t.get(), t.get().saturating_add(1)),
    }
}

/// Whether a row is in the window. `valid` is the row's valid time (none
/// for a null); `valid_to` is where the version stops, known only after
/// resolution and only on a `from` axis; `day_start` as in
/// [`plan_bounds`].
pub fn keeps(
    axis: Option<&ValidAxis<FieldIx>>,
    w: &ValidWindow,
    valid: Option<i64>,
    valid_to: Option<ValidEnd>,
    day_start: i64,
) -> bool {
    if matches!(axis, Some(ValidAxis::Timeless)) {
        return true;
    }
    let Some(v) = valid else { return false };
    let v = ValidTime::new(v);
    match (axis, w) {
        (Some(ValidAxis::IntervalEnd(_)), ValidWindow::Range { after, upto }) => {
            *after < v && v <= *upto
        }
        (Some(ValidAxis::Day(_)), ValidWindow::Range { after, upto }) => {
            let start = v.get().saturating_add(day_start);
            start < upto.get() && start.saturating_add(DAY_US) > after.get()
        }
        (Some(ValidAxis::Day(_)), ValidWindow::At(t)) => {
            let start = v.get().saturating_add(day_start);
            start <= t.get() && t.get() < start.saturating_add(DAY_US)
        }
        (Some(ValidAxis::From { .. }), ValidWindow::Range { after, upto }) => {
            v <= *upto && valid_to.is_none_or(|e| e.is_after(*after))
        }
        (Some(ValidAxis::From { .. }), ValidWindow::At(t)) => {
            v <= *t && valid_to.is_none_or(|e| e.is_after(*t))
        }
        (_, ValidWindow::Range { after, upto }) => *after <= v && v < *upto,
        (_, ValidWindow::At(t)) => v == *t,
    }
}

/// Whether the scan can apply the window before resolution: on every axis
/// but `from`, where the window needs `valid_to`.
pub const fn before_resolution(axis: Option<&ValidAxis<FieldIx>>) -> bool {
    !matches!(axis, Some(ValidAxis::From { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: FieldIx = FieldIx(0);
    const S: i64 = 14_400_000_000;

    fn range(a: i64, b: i64) -> ValidWindow {
        ValidWindow::Range {
            after: ValidTime::new(a),
            upto: ValidTime::new(b),
        }
    }

    fn from() -> ValidAxis<FieldIx> {
        ValidAxis::From {
            column: C,
            until: None,
        }
    }

    #[test]
    fn bounds_per_axis() {
        let w = range(10, 20);
        assert_eq!(
            plan_bounds(Some(&ValidAxis::IntervalEnd(C)), &w, 0),
            (11, 21)
        );
        assert_eq!(plan_bounds(Some(&ValidAxis::Instant(C)), &w, 0), (10, 20));
        assert_eq!(plan_bounds(Some(&from()), &w, 0), (i64::MIN, 21));
        assert_eq!(
            plan_bounds(Some(&ValidAxis::Timeless), &w, 0),
            (i64::MIN, i64::MAX)
        );
        assert_eq!(plan_bounds(None, &w, 0), (10, 20));
        let at = ValidWindow::At(ValidTime::new(15));
        assert_eq!(plan_bounds(Some(&ValidAxis::Instant(C)), &at, 0), (15, 16));
        assert_eq!(plan_bounds(Some(&from()), &at, 0), (i64::MIN, 16));
        assert_eq!(plan_bounds(None, &at, 0), (15, 16));
        assert_eq!(
            plan_bounds(
                Some(&ValidAxis::IntervalEnd(C)),
                &range(i64::MAX - 1, i64::MAX),
                0
            ),
            (i64::MAX, i64::MAX)
        );
        // A trading day 2024-01-01 in +10:00 asked for by its own span
        // [04:00, 04:00 next day) is the one label, midnight 2024-01-01.
        let midnight = 1_704_031_200_000_000;
        let day = Some(ValidAxis::Day(C));
        let w = range(midnight + S, midnight + S + DAY_US);
        assert_eq!(
            plan_bounds(day.as_ref(), &w, S),
            (midnight - DAY_US + 1, midnight + DAY_US)
        );
        let two = midnight + 2 * 3_600_000_000;
        let at = ValidWindow::At(ValidTime::new(two));
        assert_eq!(
            plan_bounds(day.as_ref(), &at, S),
            (two - S - DAY_US + 1, two - S + 1)
        );
    }

    #[test]
    fn rows_per_axis() {
        let w = range(10, 20);
        let ie = Some(ValidAxis::IntervalEnd(C));
        assert!(!keeps(ie.as_ref(), &w, Some(10), None, 0));
        assert!(keeps(ie.as_ref(), &w, Some(11), None, 0));
        assert!(keeps(ie.as_ref(), &w, Some(20), None, 0));
        assert!(!keeps(ie.as_ref(), &w, Some(21), None, 0));
        assert!(!keeps(ie.as_ref(), &w, None, None, 0));
        let inst = Some(ValidAxis::Instant(C));
        assert!(keeps(inst.as_ref(), &w, Some(10), None, 0));
        assert!(!keeps(inst.as_ref(), &w, Some(20), None, 0));
        let at = ValidWindow::At(ValidTime::new(15));
        assert!(keeps(inst.as_ref(), &at, Some(15), None, 0));
        assert!(!keeps(inst.as_ref(), &at, Some(16), None, 0));
        assert!(keeps(Some(&ValidAxis::Timeless), &at, None, None, 0));
        assert!(keeps(None, &w, Some(19), None, 0));
        assert!(!keeps(None, &at, Some(14), None, 0));
        // From: in force at the start, or starting inside; an open end is
        // always after; At is the version in force then.
        let f = Some(from());
        let open = Some(ValidEnd::Open);
        let ends = |e: i64| Some(ValidEnd::At(ValidTime::new(e)));
        assert!(keeps(f.as_ref(), &w, Some(5), open, 0));
        assert!(keeps(f.as_ref(), &w, Some(5), ends(11), 0));
        assert!(!keeps(f.as_ref(), &w, Some(5), ends(10), 0));
        assert!(keeps(f.as_ref(), &w, Some(20), open, 0));
        assert!(!keeps(f.as_ref(), &w, Some(21), open, 0));
        assert!(keeps(f.as_ref(), &at, Some(15), ends(16), 0));
        assert!(!keeps(f.as_ref(), &at, Some(15), ends(15), 0));
        assert!(!keeps(f.as_ref(), &at, Some(16), open, 0));
        assert!(before_resolution(ie.as_ref()) && !before_resolution(f.as_ref()));
        // Day: the label covers [L + s, L + s + 1 day).
        let midnight = 1_704_031_200_000_000;
        let d = Some(ValidAxis::Day(C));
        let day = range(midnight + S, midnight + S + DAY_US);
        assert!(keeps(d.as_ref(), &day, Some(midnight), None, S));
        assert!(!keeps(d.as_ref(), &day, Some(midnight - DAY_US), None, S));
        assert!(!keeps(d.as_ref(), &day, Some(midnight + DAY_US), None, S));
        // A calendar day overlaps two trading days: the one before ends
        // at 04:00, the day's own starts then.
        let calendar = range(midnight, midnight + DAY_US);
        assert!(keeps(
            d.as_ref(),
            &calendar,
            Some(midnight - DAY_US),
            None,
            S
        ));
        assert!(keeps(d.as_ref(), &calendar, Some(midnight), None, S));
        assert!(!keeps(
            d.as_ref(),
            &calendar,
            Some(midnight + DAY_US),
            None,
            S
        ));
        // 02:00 on the 1st is still the trading day of the 31st.
        let two = ValidWindow::At(ValidTime::new(midnight + 2 * 3_600_000_000));
        assert!(keeps(d.as_ref(), &two, Some(midnight - DAY_US), None, S));
        assert!(!keeps(d.as_ref(), &two, Some(midnight), None, S));
        let six = ValidWindow::At(ValidTime::new(midnight + 6 * 3_600_000_000));
        assert!(keeps(d.as_ref(), &six, Some(midnight), None, S));
        assert!(before_resolution(d.as_ref()));
    }
}
