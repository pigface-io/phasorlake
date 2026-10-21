//! A table's two time axes: what each fact is about, when it applies
//! (valid time), and which of its revisions is the newest (knowledge).
//!
//! Dedupe, "as of" reads, effective-date windows and forecast vintages are
//! one question asked of these roles: which version of a fact is the
//! truth, as known at some time. The roles are derived from the data
//! model's primary key and the table's columns by [`derive()`], so no table
//! has a hand-kept list; a table file may override them, marked
//! [`RoleOrigin::Hand`].
//!
//! The roles name columns by `C`: a [`Temporal<String>`] is what a table
//! file says; [`Temporal::check`] proves every name against the table and
//! turns it into a [`Temporal<ColumnId>`], the only form the rest of
//! utilake reads.

use serde::{Deserialize, Serialize};

use crate::file_model::model::{Column, ColumnId, LakeType, Table};

/// Column names reserved for what utilake adds to a read (`_nl_source`,
/// `_nl_valid_to`, …); no table may declare one.
pub const RESERVED_PREFIX: &str = "_NL_";

/// Who chose a table's roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleOrigin {
    /// [`derive()`] chose them; a test re-derives every such table.
    Derived,
    /// A person chose them in the table file.
    Hand,
}

/// When a fact applies, and how a window of time selects it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValidAxis<C> {
    /// No column says when: every row applies always. A window does not
    /// select rows of such a table.
    Timeless,
    /// The time ends the interval the row covers (SETTLEMENTDATE): a window
    /// `(after, upto]` keeps `after < t <= upto`.
    IntervalEnd(C),
    /// The time labels the row: a day, the start of a period, a run
    /// (TRADINGDATE, DAY, RUN_DATETIME of a run's own result): a window
    /// keeps `after <= t < upto`.
    Instant(C),
    /// The row is in force from the time until the next row of the same
    /// entity (EFFECTIVEDATE): a window keeps each version whose span
    /// overlaps it.
    From(C),
}

impl<C> ValidAxis<C> {
    /// The column, if the axis has one.
    pub const fn column(&self) -> Option<&C> {
        match self {
            Self::Timeless => None,
            Self::IntervalEnd(c) | Self::Instant(c) | Self::From(c) => Some(c),
        }
    }

    /// The same axis over another column reference.
    pub fn map<D>(self, f: impl FnOnce(C) -> D) -> ValidAxis<D> {
        match self {
            Self::Timeless => ValidAxis::Timeless,
            Self::IntervalEnd(c) => ValidAxis::IntervalEnd(f(c)),
            Self::Instant(c) => ValidAxis::Instant(f(c)),
            Self::From(c) => ValidAxis::From(f(c)),
        }
    }

    /// The name in a table file.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Timeless => "timeless",
            Self::IntervalEnd(_) => "interval_end",
            Self::Instant(_) => "instant",
            Self::From(_) => "from",
        }
    }
}

/// A table's roles. Every primary-key column has exactly one of entity,
/// valid or order; a column outside the key may be an order or the cutoff.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Temporal<C> {
    /// Who chose the roles.
    pub origin: RoleOrigin,
    /// What a fact is about: two rows of the same entity and valid time
    /// are revisions of one fact. May be empty.
    pub entity: Vec<C>,
    /// When a fact applies.
    pub valid: ValidAxis<C>,
    /// Which revision is newer, most significant first. The source a row
    /// came from is always the last tiebreak, so it is not listed.
    pub order: Vec<C>,
    /// The time a revision became known, which a read "as known at" is
    /// held to. `None`: the time its source was published.
    pub cutoff: Option<C>,
}

impl<C> Temporal<C> {
    /// Every column a role names, entity first, each once at most.
    pub fn columns(&self) -> impl Iterator<Item = &C> {
        self.entity
            .iter()
            .chain(self.valid.column())
            .chain(&self.order)
            .chain(&self.cutoff)
    }
}

/// Valid-time columns of a period that ends at the time.
const INTERVAL_END: &[&str] = &[
    "SETTLEMENTDATE",
    "INTERVAL_DATETIME",
    "DATETIME",
    "PERIOD_ENDING",
];

/// Valid-time columns from which a row is in force.
const IN_FORCE_FROM: &[&str] = &["EFFECTIVEDATE", "VALIDFROM", "START_DATE", "STARTDATE"];

/// Columns that rank revisions. A time among them may also be a cutoff,
/// and is the valid time of a table that has no other.
const KNOWLEDGE: &[&str] = &[
    "RUN_DATETIME",
    "VERSION_DATETIME",
    "PUBLISH_DATETIME",
    "OFFERDATETIME",
    "OFFERDATE",
    "PREDISPATCHSEQNO",
    "VERSIONNO",
    "BILLRUNNO",
    "RUNNO",
    "RUN_NO",
];

/// A column that names a scenario, never a revision: AEMO publishes an
/// intervention run's rows beside the normal ones, and leaves it out of
/// some keys (the P5MIN and PREDISPATCH tables). Merging the two would
/// lose one, so it is always an entity where the table has it.
const SCENARIO: &str = "INTERVENTION";

/// The column every revision of most tables carries: when AEMO last
/// changed the row.
const LAST_CHANGED: &str = "LASTCHANGED";

fn is_time(c: &Column) -> bool {
    matches!(c.ty, LakeType::Timestamp | LakeType::Date)
}

/// The roles of a table, from its primary key and columns.
///
/// - The valid time is the first key column in `IN_FORCE_FROM` (`from`),
///   else in `INTERVAL_END` (`interval_end`), else the first other time
///   in the key that is not a knowledge column (`instant`), else the first
///   time among the key's knowledge columns (`instant`: a run's own
///   result is a fact of that run), else none (`timeless`).
/// - The order is the key's `KNOWLEDGE` columns in key order, then the
///   knowledge columns outside the key, then `LASTCHANGED`.
/// - The cutoff is the first time among the key's knowledge columns, else
///   `LASTCHANGED`, else none.
/// - Every other key column is the entity, and `INTERVENTION` wherever
///   the table has it (see `SCENARIO`).
pub fn derive(primary_key: &[String], columns: &[Column]) -> Temporal<String> {
    let col = |n: &str| columns.iter().find(|c| c.name == n);
    let timed = |n: &&String| col(n).is_some_and(is_time);
    let knowledge = |n: &str| KNOWLEDGE.contains(&n);
    let in_key = |list: &[&str]| {
        primary_key
            .iter()
            .find(|k| list.contains(&k.as_str()))
            .cloned()
    };
    let key_knowledge_time = primary_key
        .iter()
        .filter(timed)
        .find(|k| knowledge(k))
        .cloned();
    let valid = if let Some(c) = in_key(IN_FORCE_FROM).filter(|c| timed(&c)) {
        ValidAxis::From(c)
    } else if let Some(c) = in_key(INTERVAL_END).filter(|c| timed(&c)) {
        ValidAxis::IntervalEnd(c)
    } else if let Some(c) = primary_key
        .iter()
        .filter(timed)
        .find(|k| !knowledge(k))
        .cloned()
    {
        ValidAxis::Instant(c)
    } else if let Some(c) = key_knowledge_time.clone() {
        ValidAxis::Instant(c)
    } else {
        ValidAxis::Timeless
    };
    let is_valid = |n: &str| valid.column().is_some_and(|v| v == n);
    let mut order: Vec<String> = primary_key
        .iter()
        .filter(|k| knowledge(k) && !is_valid(k))
        .cloned()
        .collect();
    for k in KNOWLEDGE.iter().chain(&[LAST_CHANGED]) {
        if col(k).is_some() && !order.iter().any(|o| o == k) && !is_valid(k) {
            order.push((*k).to_owned());
        }
    }
    let cutoff = key_knowledge_time.filter(|c| !is_valid(c)).or_else(|| {
        col(LAST_CHANGED)
            .filter(|c| is_time(c))
            .map(|c| c.name.clone())
    });
    let mut entity: Vec<String> = primary_key
        .iter()
        .filter(|k| !is_valid(k) && !order.contains(k))
        .cloned()
        .collect();
    if col(SCENARIO).is_some() && !entity.iter().any(|e| e == SCENARIO) {
        entity.push(SCENARIO.to_owned());
    }
    Temporal {
        origin: RoleOrigin::Derived,
        entity,
        valid,
        order,
        cutoff,
    }
}

impl Temporal<String> {
    /// Proves every role against the table: each name is a column, a time
    /// role is a time, no column has two roles (the cutoff may also be an
    /// order), every key column has a role, and derived roles are what
    /// [`derive()`] gives today. Returns the roles by column id, or every
    /// problem found.
    pub fn check(&self, t: &Table) -> Result<Temporal<ColumnId>, Vec<String>> {
        let mut problems = Vec::new();
        let mut id = |role: &str, n: &String| match t.column(n) {
            Some(c) => Some(c.id),
            None => {
                problems.push(format!("temporal {role} names {n}, which is not a column"));
                None
            }
        };
        let entity: Vec<_> = self.entity.iter().filter_map(|n| id("entity", n)).collect();
        let valid = self.valid.clone().map(|n| id("valid", &n));
        let order: Vec<_> = self.order.iter().filter_map(|n| id("order", n)).collect();
        let cutoff = self.cutoff.as_ref().map(|n| id("cutoff", n));
        for (role, n) in self
            .valid
            .column()
            .map(|n| ("valid", n))
            .into_iter()
            .chain(self.cutoff.as_ref().map(|n| ("cutoff", n)))
        {
            if let Some(c) = t.column(n)
                && !is_time(c)
            {
                problems.push(format!(
                    "temporal {role} {n} is {}, not a time; a {role} column is timestamp or date",
                    c.ty
                ));
            }
        }
        let mut seen: Vec<&String> = Vec::new();
        for n in self
            .entity
            .iter()
            .chain(self.valid.column())
            .chain(&self.order)
        {
            if seen.contains(&n) {
                problems.push(format!("temporal names {n} in two roles"));
            }
            seen.push(n);
        }
        if let Some(c) = &self.cutoff
            && (self.entity.contains(c) || self.valid.column() == Some(c))
        {
            problems.push(format!(
                "temporal cutoff {c} is also an entity or the valid time; a cutoff is a knowledge column"
            ));
        }
        for k in &t.primary_key {
            if !seen.contains(&k) {
                problems.push(format!(
                    "primary-key column {k} has no temporal role; give it entity, valid or order"
                ));
            }
        }
        if self.origin == RoleOrigin::Derived {
            let fresh = derive(&t.primary_key, &t.columns);
            if fresh != *self {
                problems.push(format!(
                    "temporal says origin = \"derived\" but deriving gives {fresh:?}; regenerate it, or set origin = \"hand\""
                ));
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Temporal {
            origin: self.origin,
            entity: entity.into_iter().collect(),
            valid: valid.map(|c| c.expect("checked above")),
            order,
            cutoff: cutoff.map(|c| c.expect("checked above")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::model::tests::unit_scada;

    fn cols(spec: &[(&str, LakeType)]) -> Vec<Column> {
        spec.iter()
            .enumerate()
            .map(|(i, (n, ty))| Column {
                id: ColumnId(i as u32 + 1),
                name: (*n).to_owned(),
                ty: *ty,
                nullable: true,
                note: None,
            })
            .collect()
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| (*s).to_owned()).collect()
    }

    const TS: LakeType = LakeType::Timestamp;
    const S: LakeType = LakeType::String;
    const D: LakeType = LakeType::Double;

    #[test]
    fn effective_dated() {
        let c = cols(&[
            ("DUID", S),
            ("EFFECTIVEDATE", TS),
            ("VERSIONNO", D),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(&names(&["DUID", "EFFECTIVEDATE", "VERSIONNO"]), &c);
        assert_eq!(t.entity, names(&["DUID"]));
        assert_eq!(t.valid, ValidAxis::From("EFFECTIVEDATE".into()));
        assert_eq!(t.order, names(&["VERSIONNO", "LASTCHANGED"]));
        assert_eq!(t.cutoff.as_deref(), Some("LASTCHANGED"));
        assert_eq!(t.origin, RoleOrigin::Derived);
    }

    #[test]
    fn interval_with_runs() {
        let c = cols(&[
            ("SETTLEMENTDATE", TS),
            ("RUNNO", D),
            ("REGIONID", S),
            ("INTERVENTION", D),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(
            &names(&["SETTLEMENTDATE", "RUNNO", "REGIONID", "INTERVENTION"]),
            &c,
        );
        assert_eq!(t.valid, ValidAxis::IntervalEnd("SETTLEMENTDATE".into()));
        assert_eq!(t.entity, names(&["REGIONID", "INTERVENTION"]));
        assert_eq!(t.order, names(&["RUNNO", "LASTCHANGED"]));
    }

    #[test]
    fn forecast_runs_are_knowledge() {
        let c = cols(&[
            ("RUN_DATETIME", TS),
            ("INTERVAL_DATETIME", TS),
            ("REGIONID", S),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(
            &names(&["RUN_DATETIME", "INTERVAL_DATETIME", "REGIONID"]),
            &c,
        );
        assert_eq!(t.valid, ValidAxis::IntervalEnd("INTERVAL_DATETIME".into()));
        assert_eq!(t.order, names(&["RUN_DATETIME", "LASTCHANGED"]));
        assert_eq!(
            t.cutoff.as_deref(),
            Some("RUN_DATETIME"),
            "the run is when it was known"
        );
        assert_eq!(t.entity, names(&["REGIONID"]));
    }

    #[test]
    fn a_runs_own_result_is_valid_at_the_run() {
        let c = cols(&[("RUN_DATETIME", TS), ("LASTCHANGED", TS)]);
        let t = derive(&names(&["RUN_DATETIME"]), &c);
        assert_eq!(t.valid, ValidAxis::Instant("RUN_DATETIME".into()));
        assert_eq!(t.order, names(&["LASTCHANGED"]));
        assert_eq!(t.cutoff.as_deref(), Some("LASTCHANGED"));
        assert!(t.entity.is_empty());
    }

    #[test]
    fn a_day_label_with_a_run() {
        let c = cols(&[("RUN_DATETIME", TS), ("DAY", TS), ("REGIONID", S)]);
        let t = derive(&names(&["RUN_DATETIME", "DAY", "REGIONID"]), &c);
        assert_eq!(t.valid, ValidAxis::Instant("DAY".into()));
        assert_eq!(t.order, names(&["RUN_DATETIME"]));
        assert_eq!(t.cutoff.as_deref(), Some("RUN_DATETIME"));
    }

    #[test]
    fn a_knowledge_column_outside_the_key_ranks_revisions() {
        // PREDISPATCHPRICE: the run is not in AEMO's key.
        let c = cols(&[
            ("PREDISPATCHSEQNO", S),
            ("REGIONID", S),
            ("DATETIME", TS),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(&names(&["REGIONID", "DATETIME"]), &c);
        assert_eq!(t.order, names(&["PREDISPATCHSEQNO", "LASTCHANGED"]));
        // The intervention scenario is kept apart even where the key omits it.
        let mut with_iv = c.clone();
        with_iv.push(cols(&[("INTERVENTION", D)]).remove(0));
        let t = derive(&names(&["REGIONID", "DATETIME"]), &with_iv);
        assert_eq!(t.entity, names(&["REGIONID", "INTERVENTION"]));
        assert_eq!(t.cutoff.as_deref(), Some("LASTCHANGED"));
    }

    #[test]
    fn no_key_no_time() {
        let c = cols(&[
            ("CONTRACTYEAR", D),
            ("WEEKNO", D),
            ("BILLRUNNO", D),
            ("X", D),
        ]);
        let t = derive(&names(&["CONTRACTYEAR", "WEEKNO", "BILLRUNNO"]), &c);
        assert_eq!(t.valid, ValidAxis::Timeless);
        assert_eq!(t.order, names(&["BILLRUNNO"]));
        assert_eq!(t.entity, names(&["CONTRACTYEAR", "WEEKNO"]));
        assert_eq!(t.cutoff, None, "the source's publish time");
        let empty = derive(&[], &c);
        assert_eq!(empty.valid, ValidAxis::Timeless);
        assert!(empty.entity.is_empty());
    }

    #[test]
    fn a_time_named_like_a_valid_column_but_typed_as_text_is_not_one() {
        let c = cols(&[("EFFECTIVEDATE", S), ("DUID", S)]);
        let t = derive(&names(&["EFFECTIVEDATE", "DUID"]), &c);
        assert_eq!(t.valid, ValidAxis::Timeless);
        assert_eq!(t.entity, names(&["EFFECTIVEDATE", "DUID"]));
    }

    #[test]
    fn checks_against_the_table() {
        let t = unit_scada();
        let roles = derive(&t.primary_key, &t.columns);
        let ids = roles.check(&t).unwrap();
        assert_eq!(ids.valid, ValidAxis::IntervalEnd(ColumnId(1)));
        assert_eq!(ids.entity, vec![ColumnId(2)]);
        assert_eq!(ids.order, vec![ColumnId(4)]);
        assert_eq!(ids.cutoff, Some(ColumnId(4)));
        assert_eq!(roles.columns().count(), 4);

        let mut hand = roles.clone();
        hand.origin = RoleOrigin::Hand;
        hand.entity.push("NOPE".into());
        hand.valid = ValidAxis::From("DUID".into());
        hand.cutoff = Some("DUID".into());
        let p = hand.check(&t).unwrap_err().join("\n");
        assert!(p.contains("entity names NOPE"), "{p}");
        assert!(p.contains("valid DUID is string, not a time"), "{p}");
        assert!(p.contains("cutoff DUID is string"), "{p}");
        assert!(p.contains("names DUID in two roles"), "{p}");
        assert!(p.contains("is also an entity or the valid time"), "{p}");
        assert!(p.contains("SETTLEMENTDATE has no temporal role"), "{p}");

        let mut stale = roles;
        stale.order.clear();
        let p = stale.check(&t).unwrap_err().join("\n");
        assert!(p.contains("regenerate it"), "{p}");
    }

    #[test]
    fn axis_helpers() {
        let a = ValidAxis::Instant("DAY".to_owned());
        assert_eq!(a.kind(), "instant");
        assert_eq!(a.map(|s| s.len()), ValidAxis::Instant(3));
        assert_eq!(ValidAxis::<String>::Timeless.kind(), "timeless");
        assert_eq!(ValidAxis::From(1).kind(), "from");
        assert_eq!(ValidAxis::IntervalEnd(1).kind(), "interval_end");
        assert!(ValidAxis::<u8>::Timeless.column().is_none());
    }
}
