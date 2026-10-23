//! What a window means on each valid axis, stated once and used by the
//! scan (which files and rows to read), the kernel (which versions to
//! keep) and the SQL (the same read for another engine).
//!
//! | axis | `Range { after, upto }` | `At(t)` |
//! |---|---|---|
//! | interval end | `after < t <= upto` | refused |
//! | instant | `after <= t < upto` | `t == at` |
//! | from | in force at `after`, and every later version until `upto`: `valid <= upto && valid_to > after` | `valid <= at < valid_to` |
//! | timeless | every row | every row |
//! | no roles | `after <= t < upto` on the time column | refused |
//!
//! An interval-ending time is the end of the period the row covers: the
//! row at `00:05` is the first of the day, the row at `24:00` its last,
//! so a day is `(midnight, next midnight]`.

use utilake_mms_registry::file_model::ValidAxis;

use crate::query::FieldIx;
use crate::time::{ValidEnd, ValidTime, ValidWindow};

/// The microseconds `[t0, t1)` the planner prunes files by, for a window
/// on an axis (`None`: a table without roles, bounded on its time column).
pub fn plan_bounds(axis: Option<&ValidAxis<FieldIx>>, w: &ValidWindow) -> (i64, i64) {
    match (axis, w) {
        (Some(ValidAxis::Timeless), _) => (i64::MIN, i64::MAX),
        (Some(ValidAxis::IntervalEnd(_)), ValidWindow::Range { after, upto }) => {
            (after.get().saturating_add(1), upto.get().saturating_add(1))
        }
        (Some(ValidAxis::From(_)), ValidWindow::Range { upto, .. }) => {
            (i64::MIN, upto.get().saturating_add(1))
        }
        (Some(ValidAxis::From(_)), ValidWindow::At(t)) => (i64::MIN, t.get().saturating_add(1)),
        (_, ValidWindow::Range { after, upto }) => (after.get(), upto.get()),
        (_, ValidWindow::At(t)) => (t.get(), t.get().saturating_add(1)),
    }
}

/// Whether a row is in the window. `valid` is the row's valid time (none
/// for a null); `valid_to` is where the version stops, known only after
/// resolution and only on a `from` axis.
pub fn keeps(
    axis: Option<&ValidAxis<FieldIx>>,
    w: &ValidWindow,
    valid: Option<i64>,
    valid_to: Option<ValidEnd>,
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
        (Some(ValidAxis::From(_)), ValidWindow::Range { after, upto }) => {
            v <= *upto && valid_to.is_none_or(|e| e.is_after(*after))
        }
        (Some(ValidAxis::From(_)), ValidWindow::At(t)) => {
            v <= *t && valid_to.is_none_or(|e| e.is_after(*t))
        }
        (_, ValidWindow::Range { after, upto }) => *after <= v && v < *upto,
        (_, ValidWindow::At(t)) => v == *t,
    }
}

/// Whether the scan can apply the window before resolution: on every axis
/// but `from`, where the window needs `valid_to`.
pub const fn before_resolution(axis: Option<&ValidAxis<FieldIx>>) -> bool {
    !matches!(axis, Some(ValidAxis::From(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: FieldIx = FieldIx(0);

    fn range(a: i64, b: i64) -> ValidWindow {
        ValidWindow::Range {
            after: ValidTime::new(a),
            upto: ValidTime::new(b),
        }
    }

    #[test]
    fn bounds_per_axis() {
        let w = range(10, 20);
        assert_eq!(plan_bounds(Some(&ValidAxis::IntervalEnd(C)), &w), (11, 21));
        assert_eq!(plan_bounds(Some(&ValidAxis::Instant(C)), &w), (10, 20));
        assert_eq!(plan_bounds(Some(&ValidAxis::From(C)), &w), (i64::MIN, 21));
        assert_eq!(
            plan_bounds(Some(&ValidAxis::Timeless), &w),
            (i64::MIN, i64::MAX)
        );
        assert_eq!(plan_bounds(None, &w), (10, 20));
        let at = ValidWindow::At(ValidTime::new(15));
        assert_eq!(plan_bounds(Some(&ValidAxis::Instant(C)), &at), (15, 16));
        assert_eq!(plan_bounds(Some(&ValidAxis::From(C)), &at), (i64::MIN, 16));
        assert_eq!(plan_bounds(None, &at), (15, 16));
        assert_eq!(
            plan_bounds(
                Some(&ValidAxis::IntervalEnd(C)),
                &range(i64::MAX - 1, i64::MAX)
            ),
            (i64::MAX, i64::MAX)
        );
    }

    #[test]
    fn rows_per_axis() {
        let w = range(10, 20);
        let ie = Some(ValidAxis::IntervalEnd(C));
        assert!(!keeps(ie.as_ref(), &w, Some(10), None));
        assert!(keeps(ie.as_ref(), &w, Some(11), None));
        assert!(keeps(ie.as_ref(), &w, Some(20), None));
        assert!(!keeps(ie.as_ref(), &w, Some(21), None));
        assert!(!keeps(ie.as_ref(), &w, None, None));
        let inst = Some(ValidAxis::Instant(C));
        assert!(keeps(inst.as_ref(), &w, Some(10), None));
        assert!(!keeps(inst.as_ref(), &w, Some(20), None));
        assert!(keeps(
            inst.as_ref(),
            &ValidWindow::At(ValidTime::new(10)),
            Some(10),
            None
        ));
        assert!(!keeps(
            inst.as_ref(),
            &ValidWindow::At(ValidTime::new(10)),
            Some(11),
            None
        ));
        assert!(keeps(None, &w, Some(19), None));
        assert!(!keeps(None, &w, Some(20), None));
        let tl = Some(ValidAxis::Timeless);
        assert!(keeps(tl.as_ref(), &w, None, None));
        assert!(before_resolution(ie.as_ref()));
        assert!(before_resolution(None));
    }

    #[test]
    fn versions_in_force() {
        let w = range(10, 20);
        let from = Some(ValidAxis::From(C));
        let at = |v| Some(ValidEnd::from_us(v));
        // In force at `after`: starts before, ends after.
        assert!(keeps(from.as_ref(), &w, Some(5), at(11)));
        // Ended at or before `after`: out.
        assert!(!keeps(from.as_ref(), &w, Some(5), at(10)));
        // The current version, open-ended.
        assert!(keeps(from.as_ref(), &w, Some(5), Some(ValidEnd::Open)));
        // Starts within, including at `upto`.
        assert!(keeps(from.as_ref(), &w, Some(20), Some(ValidEnd::Open)));
        assert!(!keeps(from.as_ref(), &w, Some(21), Some(ValidEnd::Open)));
        // No end known yet (before resolution): the start alone decides.
        assert!(keeps(from.as_ref(), &w, Some(5), None));
        assert!(!before_resolution(from.as_ref()));
        let t = ValidWindow::At(ValidTime::new(15));
        assert!(keeps(from.as_ref(), &t, Some(15), at(16)));
        assert!(!keeps(from.as_ref(), &t, Some(15), at(15)));
        assert!(!keeps(from.as_ref(), &t, Some(16), Some(ValidEnd::Open)));
        assert!(!keeps(from.as_ref(), &t, None, None));
    }
}
