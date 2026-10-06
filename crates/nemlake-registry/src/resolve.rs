//! Resolving an I row to a conformance plan: which lake column each I-row
//! field feeds and how to cast it (see [`crate::plan`]).
//!
//! A block of report pair P in a source of month M is read against the
//! increment of P's table in force at M; with no month known, against the
//! increment whose columns are the block's, else the latest. It conforms
//! when its columns are the increment's, and deviates otherwise: a column
//! the table has not is kept as text, a lake column the file lacks is null.

use std::sync::Arc;

use nemlake_core::{Category, Diagnostic, IRow, YearMonth};

use crate::error::ResolveError;
use crate::plan::{ConformancePlan, PlanHash};
use crate::registry::{Deviation, Registry, SchemaSource, TableId};

/// Strictness of resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Only blocks that conform to their increment; a deviation or an
    /// unknown table is an error.
    Strict,
    /// A deviation is a warning and an unknown table is derived from the
    /// file; nothing the file carries is lost.
    #[default]
    Permissive,
}

/// Options for one resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResolveOptions {
    /// Strictness.
    pub mode: Mode,
}

/// How the I row matched the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The block's columns are its increment's.
    Conforms,
    /// The block's columns differ from its increment's.
    Deviates {
        /// Increment columns the I row lacks (written null).
        missing: Vec<String>,
        /// I-row columns the increment does not list: a column of the
        /// table feeds it, any other is kept as text.
        extra: Vec<String>,
        /// The columns both have come in another order.
        reordered: bool,
    },
    /// No table claims the report pair; the table was derived from the
    /// file's own header, every column as text.
    Derived,
}

impl Status {
    /// One word for events, manifests and footers.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Conforms => "conforms",
            Self::Deviates { .. } => "deviates",
            Self::Derived => "derived",
        }
    }
}

/// The result of resolving an I row.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The table.
    pub table: TableId,
    /// The increment of the table the block was read against: our version.
    pub version: u16,
    /// How it matched.
    pub status: Status,
    /// The plan to conform with.
    pub plan: Arc<ConformancePlan>,
    /// Where the table's definition came from.
    pub source: SchemaSource,
    /// Warnings raised while resolving.
    pub warnings: Vec<Diagnostic>,
}

impl Resolved {
    /// The identity of the plan this shape is read with.
    pub fn plan_hash(&self) -> PlanHash {
        self.plan.hash
    }

    /// How the shape matched, in one word.
    pub const fn label(&self) -> &'static str {
        self.status.label()
    }
}

/// The columns of `want` that `have` lacks, those of `have` that `want`
/// lacks, and whether the columns both have come in another order.
fn compare(want: &[String], have: &[&str]) -> (Vec<String>, Vec<String>, bool) {
    let missing = want
        .iter()
        .filter(|c| !have.contains(&c.as_str()))
        .cloned()
        .collect();
    let extra = have
        .iter()
        .filter(|c| !want.iter().any(|w| w == *c))
        .map(|c| (*c).to_owned())
        .collect();
    let shared = want.iter().filter(|c| have.contains(&c.as_str()));
    let reordered = !shared
        .map(String::as_str)
        .eq(have.iter().copied().filter(|c| want.iter().any(|w| w == c)));
    (missing, extra, reordered)
}

impl Registry {
    /// Resolves an I row of a source whose month is not known: against the
    /// increment whose columns are the block's, else the latest.
    pub fn resolve(&self, irow: &IRow<'_>, opts: ResolveOptions) -> Result<Resolved, ResolveError> {
        self.resolve_in(irow, opts, None)
    }

    /// Resolves an I row of a source of month `period`.
    pub fn resolve_in(
        &self,
        irow: &IRow<'_>,
        opts: ResolveOptions,
        period: Option<YearMonth>,
    ) -> Result<Resolved, ResolveError> {
        if irow.columns.is_empty() {
            return Err(ResolveError::NoColumns {
                report_type: irow.report_type.to_owned(),
                report_subtype: irow.report_subtype.to_owned(),
                version: irow.version,
            });
        }
        let unknown = || ResolveError::UnknownTable {
            report_type: irow.report_type.to_owned(),
            report_subtype: irow.report_subtype.to_owned(),
        };
        let id = match self.table_by_pair(irow.report_type, irow.report_subtype) {
            Some(id) => id,
            None if opts.mode == Mode::Strict => return Err(unknown()),
            // No table claims the pair. The rows are not skipped: the file's
            // own header says what they are, and that is what is kept.
            None => self
                .derive(crate::file_model::Market::Nem, irow)
                .ok_or_else(unknown)?,
        };
        if self.is_derived(id) {
            if opts.mode == Mode::Strict {
                return Err(unknown());
            }
            return Ok(self.resolve_derived(id, irow));
        }
        let t = self.table(id);
        let shapes = self.shapes(id.0 as usize);
        let fp = irow.fingerprint();
        let k = match period {
            Some(m) => t.increments.iter().rposition(|i| i.from <= m).unwrap_or(0),
            None => shapes
                .increments
                .iter()
                .rposition(|(f, _)| *f == fp)
                .unwrap_or(t.increments.len() - 1),
        };
        let inc = &t.increments[k];
        let (inc_fp, own) = &shapes.increments[k];
        let mut warnings = Vec::new();
        if !inc.aemo_versions.is_empty() && !inc.aemo_versions.contains(&irow.version) {
            warnings.push(
                Diagnostic::warning(
                    "schema.unseen_version",
                    Category::Schema,
                    format!(
                        "Table {}@{} has seen AEMO version(s) {:?}, not {}; the block is read by its columns all the same. `nemlake-census sync` records the versions the census sees.",
                        t.name, inc.n, inc.aemo_versions, irow.version
                    ),
                )
                .with_table(t.name.clone())
                .with_version(irow.version),
            );
        }
        if *inc_fp == fp {
            return Ok(Resolved {
                table: id,
                version: inc.n,
                status: Status::Conforms,
                plan: own.clone(),
                source: self.schema_source(id),
                warnings,
            });
        }
        let missing_keys = || -> Vec<&String> {
            t.primary_key
                .iter()
                .filter(|k| !irow.columns.contains(&k.as_str()))
                .collect()
        };
        if opts.mode == Mode::Strict {
            if let Some(k) = missing_keys().first() {
                return Err(ResolveError::MissingKey {
                    table: t.name.clone(),
                    increment: inc.n,
                    column: (*k).clone(),
                });
            }
            let (missing, extra, reordered) = compare(&inc.columns, irow.columns);
            return Err(ResolveError::Deviates {
                table: t.name.clone(),
                increment: inc.n,
                expected: inc.columns.join(","),
                observed: irow.columns.join(","),
                missing,
                extra,
                reordered,
            });
        }
        // Permissive: nothing the file carries is lost. What this shape is
        // and says is worked out the first time it is seen.
        let d = self.deviation((id, inc.n, fp, irow.version), || {
            let (missing, extra, reordered) = compare(&inc.columns, irow.columns);
            let plan = Arc::new(ConformancePlan::new(
                t,
                self.market_of(id),
                &shapes.lake,
                inc.n,
                irow.columns,
                &self.table_dir(id),
            ));
            let mut d = Diagnostic::warning(
                "schema.deviates",
                Category::Schema,
                format!(
                    "This file of table {} has {} columns where {}@{} lists {}: {} missing (written null), {} not listed (a column of the table feeds its own, any other is kept as text under its own name){}. If the file is right, fix the increment's columns in its table.toml.",
                    t.name,
                    irow.columns.len(),
                    t.name,
                    inc.n,
                    inc.columns.len(),
                    missing.len(),
                    extra.len(),
                    if reordered { ", order differs" } else { "" }
                ),
            )
            .with_table(t.name.clone())
            .with_version(irow.version);
            d.expected = Some(inc.columns.join(","));
            d.observed = Some(irow.columns.join(","));
            let mut warnings = vec![d];
            let keys = missing_keys();
            if !keys.is_empty() {
                warnings.push(
                    Diagnostic::warning(
                        "schema.missing_key",
                        Category::Schema,
                        format!(
                            "This file of table {} lacks primary-key column(s) {}; its rows are kept with those columns null, and cannot be told apart by key.",
                            t.name,
                            keys.iter()
                                .map(|k| k.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .with_table(t.name.clone())
                    .with_version(irow.version),
                );
            }
            Deviation {
                plan,
                status: Status::Deviates {
                    missing,
                    extra,
                    reordered,
                },
                warnings,
            }
        });
        warnings.extend(d.warnings.iter().cloned());
        Ok(Resolved {
            table: id,
            version: inc.n,
            status: d.status.clone(),
            plan: d.plan.clone(),
            source: self.schema_source(id),
            warnings,
        })
    }

    /// The plan of an I row onto a table derived from the file: every
    /// column as text. A later block of the same pair with another column
    /// list keeps its extra columns under their own names.
    fn resolve_derived(&self, id: TableId, irow: &IRow<'_>) -> Resolved {
        let t = self.table(id);
        let market = self.market_of(id);
        let d = self.deviation((id, 1, irow.fingerprint(), irow.version), || {
            let plan = Arc::new(ConformancePlan::new(
                t,
                market,
                self.lake_schema(id),
                1,
                irow.columns,
                &self.table_dir(id),
            ));
            let warning = Diagnostic::warning(
                "schema.unknown_table",
                Category::Schema,
                format!(
                    "No table in the registry claims report pair ({}, {}). Its rows are kept as the file gives them, every column as text, under {}. `nemlake-census sync` adds a table for it from the month's control file; ingest again once it is in.",
                    irow.report_type,
                    irow.report_subtype,
                    self.table_dir(id)
                ),
            )
            .with_table(t.name.clone())
            .with_version(irow.version);
            Deviation {
                plan,
                status: Status::Derived,
                warnings: vec![warning],
            }
        });
        Resolved {
            table: id,
            version: 1,
            status: Status::Derived,
            plan: d.plan.clone(),
            source: self.schema_source(id),
            warnings: d.warnings.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::blob::SchemaSet;
    use crate::file_model::model::tests::unit_scada;
    use crate::plan::{Cast, ColumnRoute, Origin};

    fn route(r: &ColumnRoute) -> (Option<u16>, Cast, bool) {
        (r.target, r.cast, r.nullable)
    }

    /// A registry of DISPATCH_UNIT_SCADA alone: three columns from
    /// 2024-01, four from 2026-08.
    fn reg() -> Registry {
        Registry::from_set(SchemaSet {
            tables: vec![unit_scada()],
            ..Default::default()
        })
    }

    fn scada<'a>(columns: &'a [&'a str]) -> IRow<'a> {
        IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns,
        }
    }

    const FOUR: [&str; 4] = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
    const THREE: [&str; 3] = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];

    fn month(s: &str) -> Option<YearMonth> {
        s.parse().ok()
    }

    #[test]
    fn a_block_conforms_to_the_increment_in_force() {
        let r = reg();
        let opts = ResolveOptions::default();
        let res = r.resolve_in(&scada(&FOUR), opts, month("2026-08")).unwrap();
        assert_eq!((res.status.clone(), res.version), (Status::Conforms, 2));
        assert_eq!(res.label(), "conforms");
        assert!(res.warnings.is_empty());
        let p = &res.plan;
        assert_eq!(p.increment, 2);
        assert_eq!(
            route(&p.routes[0]),
            (
                Some(0),
                Cast::TimestampNanos {
                    offset_secs: 36_000
                },
                false
            )
        );
        assert_eq!(route(&p.routes[1]), (Some(1), Cast::Utf8, false));
        assert_eq!(route(&p.routes[2]), (Some(2), Cast::Float64, true));
        assert!(p.missing.is_empty());
        assert_eq!(
            (p.key_columns.as_slice(), p.time_column),
            (&[0, 1][..], Some(0))
        );
        // Strict mode takes it too, with the same plan.
        let strict = ResolveOptions { mode: Mode::Strict };
        let again = r
            .resolve_in(&scada(&FOUR), strict, month("2030-01"))
            .unwrap();
        assert!(Arc::ptr_eq(&again.plan, &res.plan));
        // The older increment, in its months and before any.
        for m in ["2024-01", "2026-07", "2009-07"] {
            let old = r.resolve_in(&scada(&THREE), strict, month(m)).unwrap();
            assert_eq!((old.status, old.version), (Status::Conforms, 1), "{m}");
            assert_eq!(old.plan.missing, [3]);
        }
        // With no month known, the increment with the block's columns.
        assert_eq!(r.resolve(&scada(&THREE), opts).unwrap().version, 1);
        assert_eq!(r.resolve(&scada(&FOUR), opts).unwrap().version, 2);
        // Two increments, two plans.
        assert_ne!(
            r.resolve(&scada(&THREE), opts).unwrap().plan_hash(),
            res.plan_hash()
        );
    }

    #[test]
    fn a_block_that_deviates_is_kept_unless_strict() {
        let r = reg();
        let opts = ResolveOptions::default();
        let strict = ResolveOptions { mode: Mode::Strict };
        // The 2026 shape in a 2024 source: LASTCHANGED is not listed there,
        // but it is a column of the table, which feeds its own.
        let res = r.resolve_in(&scada(&FOUR), opts, month("2024-06")).unwrap();
        assert_eq!(
            res.status,
            Status::Deviates {
                missing: vec![],
                extra: vec!["LASTCHANGED".into()],
                reordered: false
            }
        );
        assert_eq!(res.version, 1);
        assert_eq!(res.warnings[0].code, "schema.deviates");
        assert_eq!(res.plan.routes[3].origin, Origin::Declared);
        assert_eq!(res.plan.increment, 1);
        let e = r
            .resolve_in(&scada(&FOUR), strict, month("2024-06"))
            .unwrap_err();
        assert!(
            matches!(e, ResolveError::Deviates { increment: 1, .. }),
            "{e}"
        );
        // A column nobody knows is kept as text; one the file lacks is null.
        let odd = ["DUID", "SETTLEMENTDATE", "NOBODY_KNOWS"];
        let res = r.resolve_in(&scada(&odd), opts, month("2026-08")).unwrap();
        assert_eq!(
            res.status,
            Status::Deviates {
                missing: vec!["SCADAVALUE".into(), "LASTCHANGED".into()],
                extra: vec!["NOBODY_KNOWS".into()],
                reordered: true
            }
        );
        let p = &res.plan;
        assert_eq!(p.routes[2].origin, Origin::FileExtra);
        assert_eq!(p.routes[2].target_field_id, None);
        assert_eq!(p.fields[4].origin, Origin::FileExtra);
        assert_eq!(p.missing, [2, 3]);
        // The same shape again is the same plan.
        let again = r.resolve_in(&scada(&odd), opts, month("2026-08")).unwrap();
        assert!(Arc::ptr_eq(&again.plan, &res.plan));
        // A block without a key column: kept with a warning, or refused.
        let keyless = ["SETTLEMENTDATE", "SCADAVALUE"];
        let res = r.resolve(&scada(&keyless), opts).unwrap();
        assert!(res.warnings.iter().any(|w| w.code == "schema.missing_key"));
        assert!(
            res.plan.arrow.field(1).is_nullable(),
            "the key takes nulls here"
        );
        assert!(!res.plan.fields.is_empty());
        let e = r.resolve(&scada(&keyless), strict).unwrap_err();
        assert!(matches!(e, ResolveError::MissingKey { ref column, .. } if column == "DUID"));
    }

    #[test]
    fn an_unseen_aemo_version_is_a_warning() {
        let r = reg();
        let v2 = IRow {
            version: 2,
            ..scada(&FOUR)
        };
        let res = r
            .resolve_in(&v2, ResolveOptions { mode: Mode::Strict }, month("2026-08"))
            .unwrap();
        assert_eq!(res.status, Status::Conforms);
        assert_eq!(res.warnings[0].code, "schema.unseen_version");
    }

    #[test]
    fn unknown_table_and_no_columns() {
        let r = reg();
        let cols = ["A", "B"];
        let irow = IRow {
            report_type: "NOBODY",
            report_subtype: "CLAIMS",
            version: 3,
            columns: &cols,
        };
        let e = r
            .resolve(&irow, ResolveOptions { mode: Mode::Strict })
            .unwrap_err();
        assert!(matches!(e, ResolveError::UnknownTable { .. }));
        let res = r.resolve(&irow, ResolveOptions::default()).unwrap();
        assert_eq!(res.status, Status::Derived);
        assert_eq!(res.source, SchemaSource::File);
        assert_eq!(res.warnings[0].code, "schema.unknown_table");
        assert_eq!(r.table_dir(res.table), "_unregistered/nobody.claims");
        assert_eq!(r.table(res.table).name, "NOBODY_CLAIMS");
        assert!(res.plan.routes.iter().all(|x| x.cast == Cast::Utf8));
        // Derived once; strict now refuses it, permissive reuses it.
        assert_eq!(r.table_by_pair("NOBODY", "CLAIMS"), Some(res.table));
        assert!(r.is_derived(res.table));
        assert!(
            r.resolve(&irow, ResolveOptions { mode: Mode::Strict })
                .is_err()
        );
        let again = r.resolve(&irow, ResolveOptions::default()).unwrap();
        assert!(Arc::ptr_eq(&again.plan, &res.plan));
        // Another column list of it keeps its own columns as text too.
        let wider = ["A", "B", "C"];
        let res = r
            .resolve(
                &IRow {
                    columns: &wider,
                    ..irow
                },
                ResolveOptions::default(),
            )
            .unwrap();
        assert_eq!(res.plan.fields.len(), 3);
        // No subtype: the type alone names it.
        let bare = IRow {
            report_type: "GENCONDATA",
            report_subtype: "",
            version: 1,
            columns: &cols,
        };
        let res = r.resolve(&bare, ResolveOptions::default()).unwrap();
        assert_eq!(r.table_dir(res.table), "_unregistered/gencondata");
        let none: [&str; 0] = [];
        let e = r
            .resolve(
                &IRow {
                    columns: &none,
                    ..irow
                },
                ResolveOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(e, ResolveError::NoColumns { .. }));
    }

    #[test]
    fn compare_names_what_differs() {
        let want: Vec<String> = ["A", "B", "C"].map(String::from).to_vec();
        assert_eq!(compare(&want, &["A", "B", "C"]), (vec![], vec![], false));
        assert_eq!(
            compare(&want, &["B", "A", "D"]),
            (vec!["C".to_owned()], vec!["D".to_owned()], true)
        );
        assert_eq!(
            compare(&want, &["A", "C"]),
            (vec!["B".to_owned()], vec![], false)
        );
    }

    #[test]
    fn the_embedded_registry_resolves_its_fixture_shapes() {
        let r = Registry::load_embedded();
        let res = r
            .resolve_in(
                &scada(&FOUR),
                ResolveOptions { mode: Mode::Strict },
                month("2026-08"),
            )
            .unwrap();
        assert_eq!(res.status, Status::Conforms);
        assert_eq!(res.source, SchemaSource::Registry);
    }
}
