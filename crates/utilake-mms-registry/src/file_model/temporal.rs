//! A table's two time axes: what each fact is about, when it applies
//! (valid time), and which of its revisions is the newest (knowledge).
//!
//! Dedupe, "as of" reads, effective-date windows and forecast vintages are
//! one question asked of these roles: which version of a fact is the
//! truth, as known at some time. The roles are derived from the data
//! model's primary key and the table's columns by the one rule every
//! reader and the publisher share ([`utilake_iceberg::derive`]), so no
//! table has a hand-kept list; a table file may override them, marked
//! [`RoleOrigin::Hand`].
//!
//! The roles name columns by `C`: a [`Temporal<String>`] is what a table
//! file says; [`Temporal::check`] proves every name against the table and
//! turns it into a [`Temporal<ColumnId>`], the only form the rest of
//! utilake reads.

use serde::{Deserialize, Serialize};

use crate::file_model::model::{Column, ColumnId, LakeType, Table};

/// Column names reserved for what utilake adds to a read (`_ul_source`,
/// `_ul_valid_to`, …); no table may declare one.
pub const RESERVED_PREFIX: &str = "_UL_";

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
    /// The time labels the trading day the row is for, written at
    /// midnight (the bid and settlement tables' SETTLEMENTDATE): the row
    /// covers `[label + start, label + start + 1 day)` where `start` is
    /// the market's trading day start, and a window keeps the labels whose
    /// day overlaps it.
    Day(C),
    /// The row is in force from the time until the next row of the same
    /// entity (EFFECTIVEDATE), or until the end column says, whichever is
    /// first: a window keeps each version whose span overlaps it.
    From {
        /// The column the version is in force from.
        column: C,
        /// The column that ends it early (END_DATE), when the table has
        /// one; AEMO's "no end" dates are open ends.
        until: Option<C>,
    },
}

impl<C> ValidAxis<C> {
    /// The column, if the axis has one.
    pub const fn column(&self) -> Option<&C> {
        match self {
            Self::Timeless => None,
            Self::IntervalEnd(c) | Self::Instant(c) | Self::Day(c) => Some(c),
            Self::From { column, .. } => Some(column),
        }
    }

    /// The column that ends a version, on a `from` axis that has one.
    pub const fn until(&self) -> Option<&C> {
        match self {
            Self::From { until: Some(u), .. } => Some(u),
            _ => None,
        }
    }

    /// The same axis over another column reference.
    pub fn map<D>(self, mut f: impl FnMut(C) -> D) -> ValidAxis<D> {
        match self {
            Self::Timeless => ValidAxis::Timeless,
            Self::IntervalEnd(c) => ValidAxis::IntervalEnd(f(c)),
            Self::Instant(c) => ValidAxis::Instant(f(c)),
            Self::Day(c) => ValidAxis::Day(f(c)),
            Self::From { column, until } => ValidAxis::From {
                column: f(column),
                until: until.map(f),
            },
        }
    }

    /// The name in a table file.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Timeless => "timeless",
            Self::IntervalEnd(_) => "interval_end",
            Self::Instant(_) => "instant",
            Self::Day(_) => "day",
            Self::From { .. } => "from",
        }
    }
}

/// A table's roles. Every primary-key column has exactly one of entity,
/// valid or order; a column outside the key may be an order, the cutoff,
/// or the end of a version.
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
            .chain(self.valid.until())
            .chain(&self.order)
            .chain(&self.cutoff)
    }
}

fn is_time(c: &Column) -> bool {
    matches!(c.ty, LakeType::Timestamp | LakeType::Date)
}

/// The roles as the publisher states them, as utilake's; `None` for a
/// valid kind utilake does not know.
pub fn from_published(t: &utilake_iceberg::Temporal) -> Option<Temporal<String>> {
    let v = &t.valid;
    let valid = match (v.kind.as_str(), v.column.clone()) {
        ("timeless", None) => ValidAxis::Timeless,
        ("interval_end", Some(c)) => ValidAxis::IntervalEnd(c),
        ("instant", Some(c)) => ValidAxis::Instant(c),
        ("day", Some(c)) => ValidAxis::Day(c),
        ("from", Some(c)) => ValidAxis::From {
            column: c,
            until: v.until.clone(),
        },
        _ => return None,
    };
    Some(Temporal {
        origin: match t.origin.as_str() {
            "hand" => RoleOrigin::Hand,
            _ => RoleOrigin::Derived,
        },
        entity: t.entity.clone(),
        valid,
        order: t.order.clone(),
        cutoff: t.cutoff.clone(),
    })
}

/// The roles of a table, from its primary key and columns: the publisher's
/// rule ([`utilake_iceberg::derive`]) over the columns as Iceberg fields.
pub fn derive(primary_key: &[String], columns: &[Column]) -> Temporal<String> {
    let fields: Vec<utilake_iceberg::Field> = columns
        .iter()
        .map(|c| utilake_iceberg::Field {
            id: c.id.0,
            name: c.name.clone(),
            ty: c.ty.iceberg(),
            required: !c.nullable,
            doc: None,
        })
        .collect();
    from_published(&utilake_iceberg::derive(primary_key, &fields))
        .expect("the publisher derives only kinds utilake knows")
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
            .chain(self.valid.until().map(|n| ("until", n)))
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
            .chain(self.valid.until())
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
    fn the_publishers_rule_over_the_tables_columns() {
        let c = cols(&[
            ("DUID", S),
            ("START_DATE", TS),
            ("END_DATE", TS),
            ("VERSIONNO", D),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(&names(&["DUID", "START_DATE", "VERSIONNO"]), &c);
        assert_eq!(t.entity, names(&["DUID"]));
        assert_eq!(
            t.valid,
            ValidAxis::From {
                column: "START_DATE".into(),
                until: Some("END_DATE".into())
            }
        );
        assert_eq!(t.order, names(&["VERSIONNO", "LASTCHANGED"]));
        assert_eq!(t.cutoff.as_deref(), Some("LASTCHANGED"));
        assert_eq!(t.origin, RoleOrigin::Derived);
        assert_eq!(t.columns().count(), 6, "the end column is a role column");
        let c = cols(&[("SETTLEMENTDATE", TS), ("REGIONID", S)]);
        let t = derive(&names(&["SETTLEMENTDATE", "REGIONID"]), &c);
        assert_eq!(t.valid, ValidAxis::IntervalEnd("SETTLEMENTDATE".into()));
        assert!(t.valid.until().is_none());
    }

    #[test]
    fn published_roles_of_every_kind_read_back() {
        let published =
            |kind: &str, column: Option<&str>, until: Option<&str>| utilake_iceberg::Temporal {
                valid: utilake_iceberg::Valid {
                    kind: kind.into(),
                    column: column.map(str::to_owned),
                    until: until.map(str::to_owned),
                },
                entity: vec!["X".into()],
                order: vec![],
                cutoff: None,
                origin: "hand".into(),
            };
        let day = from_published(&published("day", Some("SETTLEMENTDATE"), None)).unwrap();
        assert_eq!(day.valid, ValidAxis::Day("SETTLEMENTDATE".into()));
        assert_eq!(day.origin, RoleOrigin::Hand);
        let from = from_published(&published("from", Some("A"), Some("B"))).unwrap();
        assert_eq!(from.valid.until().map(String::as_str), Some("B"));
        assert_eq!(
            from_published(&published("timeless", None, None))
                .unwrap()
                .valid,
            ValidAxis::Timeless
        );
        assert!(from_published(&published("instant", None, None)).is_none());
        assert!(from_published(&published("epoch", Some("A"), None)).is_none());
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
        hand.valid = ValidAxis::From {
            column: "DUID".into(),
            until: Some("SCADAVALUE".into()),
        };
        hand.cutoff = Some("DUID".into());
        let p = hand.check(&t).unwrap_err().join("\n");
        assert!(p.contains("entity names NOPE"), "{p}");
        assert!(p.contains("valid DUID is string, not a time"), "{p}");
        assert!(p.contains("until SCADAVALUE is "), "{p}");
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
        assert_eq!(ValidAxis::IntervalEnd(1).kind(), "interval_end");
        assert_eq!(ValidAxis::Day(1).kind(), "day");
        let f = ValidAxis::From {
            column: 1,
            until: Some(2),
        };
        assert_eq!(f.kind(), "from");
        assert_eq!((f.column(), f.until()), (Some(&1), Some(&2)));
        assert_eq!(
            f.map(|n| n * 10),
            ValidAxis::From {
                column: 10,
                until: Some(20)
            }
        );
        assert!(ValidAxis::<u8>::Timeless.column().is_none());
        assert!(ValidAxis::Day(1).until().is_none());
    }
}
