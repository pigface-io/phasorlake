//! Resolving an I row to a conformance plan (see [`crate::plan`]).
//!
//! A block no run of the history fits deviates: unknown columns are kept as
//! text and missing ones are null; strict mode refuses it.

use std::sync::Arc;

use phasor_core::{Category, Diagnostic, Fingerprint, YearMonth};

use phasor_mms_decode::IRow;

use crate::error::ResolveError;
use crate::file_model::Market;
use crate::plan::{ConformancePlan, PlanHash};
use crate::registry::{Registry, Resolution, SchemaSource, TableId};

/// Strictness of resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// A deviation or an unknown table is an error.
    Strict,
    /// A deviation is a warning and an unknown table is derived from the file.
    #[default]
    Permissive,
}

/// Options for one resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResolveOptions {
    /// Strictness.
    pub mode: Mode,
    /// The market the file belongs to, where its pairs are looked up.
    pub market: Market,
}

/// How the I row matched the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// A run of the table's history fits the block.
    Conforms,
    /// No run fits: the columns differ from the schema in force for the pair.
    Deviates {
        /// Columns of that schema the I row lacks (written null).
        missing: Vec<String>,
        /// I-row columns the schema does not list.
        extra: Vec<String>,
        /// The columns both have come in another order.
        reordered: bool,
    },
    /// No table claims the report pair; one was derived from the header.
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
    /// The schema the block was read against (for a deviation, the one in force).
    pub version: u16,
    /// The Iceberg schema the block resolved to; none when it deviates.
    pub schema: Option<u32>,
    /// How it matched.
    pub status: Status,
    /// The plan to conform with.
    pub plan: Arc<ConformancePlan>,
    /// Where the table's definition came from.
    pub source: SchemaSource,
    /// Warnings raised while resolving.
    pub warnings: Vec<Diagnostic>,
    /// The fingerprint of the I row's columns.
    pub fingerprint: Fingerprint,
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

/// Columns missing from each side, and whether the shared ones are reordered.
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

/// The release that stands for "not known": after every run's.
pub(crate) const LATEST: &str = "9999-12";

impl Registry {
    /// Resolves an I row of a source whose month is not known, against the latest fitting run.
    pub fn resolve(&self, irow: &IRow<'_>, opts: ResolveOptions) -> Result<Resolved, ResolveError> {
        self.resolve_in(irow, opts, None)
    }

    /// Resolves a block given by name, as a person types it, normalised as a header.
    ///
    /// `None` when the type or subtype is empty or has control characters.
    pub fn resolve_names(
        &self,
        report_type: &str,
        report_subtype: &str,
        version: u16,
        columns: &[String],
        period: Option<YearMonth>,
    ) -> Option<Result<Resolved, ResolveError>> {
        let norm = |s: &str| phasor_core::normalize_name(s, &mut String::new()).map(str::to_owned);
        let (rt, st) = (norm(report_type)?, norm(report_subtype)?);
        let names: Vec<String> = columns.iter().filter_map(|c| norm(c)).collect();
        let cols: Vec<&str> = names.iter().map(String::as_str).collect();
        let irow = IRow {
            report_type: &rt,
            report_subtype: &st,
            version,
            columns: &cols,
        };
        Some(self.resolve_in(&irow, ResolveOptions::default(), period))
    }

    /// Resolves an I row of a source released in month `period`.
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
        let fp = irow.fingerprint();
        let declared = self.pair(
            opts.market,
            irow.report_type,
            irow.report_subtype,
            Some(irow.columns),
        );
        // Hot path: a column list only one schema has is read with it.
        if let Some((id, k)) = declared {
            let shapes = self.shapes(id.0 as usize);
            if let Some(&schema) = shapes.exact.get(&(k, fp))
                && shapes.versions.contains(&(k, irow.version))
                && let Some(plan) = shapes.plan(schema)
            {
                return Ok(Resolved {
                    table: id,
                    version: schema,
                    schema: Some(u32::from(schema)),
                    status: Status::Conforms,
                    plan: plan.clone(),
                    source: self.schema_source(id),
                    warnings: Vec::new(),
                    fingerprint: fp,
                });
            }
        }
        let id = match declared
            .map(|(id, _)| id)
            .or_else(|| self.derived_by_pair(opts.market, irow.report_type, irow.report_subtype))
        {
            Some(id) => id,
            None if opts.mode == Mode::Strict => return Err(unknown()),
            // No table claims the pair: derive one from the header, skip nothing.
            None => self.derive(opts.market, irow).ok_or_else(unknown)?,
        };
        if self.is_derived(id) {
            if opts.mode == Mode::Strict {
                return Err(unknown());
            }
            return Ok(self.resolve_derived(id, irow, fp));
        }
        let key = (id, fp, irow.version, period);
        let r = self.resolution(key, || self.resolve_block(id, irow, period));
        if opts.mode == Mode::Strict
            && let Status::Deviates {
                missing,
                extra,
                reordered,
            } = &r.status
        {
            let t = self.table(id);
            return Err(ResolveError::Deviates {
                table: t.name.clone(),
                schema: r.version,
                expected: self.expected(id, r.version).join(","),
                observed: irow.columns.join(","),
                missing: missing.clone(),
                extra: extra.clone(),
                reordered: *reordered,
            });
        }
        Ok(Resolved {
            table: id,
            version: r.version,
            schema: r.schema,
            status: r.status.clone(),
            plan: r.plan.clone(),
            source: self.schema_source(id),
            warnings: r.warnings.clone(),
            fingerprint: fp,
        })
    }

    /// The columns of schema `schema` of table `id`, in file order.
    fn expected(&self, id: TableId, schema: u16) -> Vec<String> {
        let t = self.table(id);
        t.schema(schema)
            .map(|s| t.schema_columns(s).into_iter().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// What a block of a registry table resolves to, the first time its shape is seen.
    fn resolve_block(&self, id: TableId, irow: &IRow<'_>, period: Option<YearMonth>) -> Resolution {
        let t = self.table(id);
        let shapes = self.shapes(id.0 as usize);
        let pair = (irow.report_type, irow.report_subtype);
        let release = period.map_or_else(|| LATEST.to_owned(), |m| m.to_string());
        let fits = shapes
            .history
            .resolve([pair.0, pair.1], irow.version, irow.columns, &release)
            .and_then(|s| u16::try_from(s).ok())
            .and_then(|s| shapes.plan(s).map(|p| (s, p)));
        let mut warnings = Vec::new();
        if !t.has_version(pair, irow.version) {
            warnings.push(
                Diagnostic::warning(
                    "schema.unseen_version",
                    Category::SCHEMA,
                    format!(
                        "Table {} has no run of AEMO version {} of ({}, {}); the block is read by its columns all the same.",
                        t.name, irow.version, pair.0, pair.1
                    ),
                )
                .with_table(t.name.clone())
                .with_version(irow.version),
            );
        }
        if let Some((schema, plan)) = fits {
            return Resolution {
                plan: plan.clone(),
                status: Status::Conforms,
                version: schema,
                schema: Some(u32::from(schema)),
                warnings,
            };
        }
        // Permissive: nothing the file carries is lost.
        let reference = t.reference_schema(pair, period).unwrap_or(0);
        let expected = self.expected(id, reference);
        let (missing, extra, reordered) = compare(&expected, irow.columns);
        let plan = Arc::new(ConformancePlan::new(
            t,
            self.market_of(id),
            &shapes.lake,
            reference,
            irow.columns,
            &self.table_dir(id),
        ));
        let mut d = Diagnostic::warning(
            "schema.deviates",
            Category::SCHEMA,
            format!(
                "No run of table {}'s history has this file's {} columns{}. Against schema {}, the one in force, with {} columns: {} missing (written null), {} not listed (a column of the table feeds its own, any other is kept as text under its own name). If the file is right, the census has not seen it; a table of your own can read it.",
                t.name,
                irow.columns.len(),
                if reordered { " in this order" } else { "" },
                reference,
                expected.len(),
                missing.len(),
                extra.len(),
            ),
        )
        .with_table(t.name.clone())
        .with_version(irow.version);
        d.expected = Some(expected.join(","));
        d.observed = Some(irow.columns.join(","));
        warnings.push(d);
        Resolution {
            plan,
            status: Status::Deviates {
                missing,
                extra,
                reordered,
            },
            version: reference,
            schema: None,
            warnings,
        }
    }

    /// The plan of an I row onto a derived table: every column as text.
    fn resolve_derived(&self, id: TableId, irow: &IRow<'_>, fp: Fingerprint) -> Resolved {
        let t = self.table(id);
        let market = self.market_of(id);
        let d = self.resolution((id, fp, irow.version, None), || {
            let plan = Arc::new(ConformancePlan::new(
                t,
                market,
                self.lake_schema(id),
                0,
                irow.columns,
                &self.table_dir(id),
            ));
            let warning = Diagnostic::warning(
                "schema.unknown_table",
                Category::SCHEMA,
                format!(
                    "No table in the registry claims report pair ({}, {}). Its rows are kept as the file gives them, every column as text, under {}. Once phasor-schemas publishes a table for it, a phasor built with it reads the pair typed; ingest again then.",
                    irow.report_type,
                    irow.report_subtype,
                    self.table_dir(id)
                ),
            )
            .with_table(t.name.clone())
            .with_version(irow.version);
            Resolution {
                plan,
                status: Status::Derived,
                version: 0,
                schema: None,
                warnings: vec![warning],
            }
        });
        Resolved {
            table: id,
            version: 0,
            schema: None,
            status: Status::Derived,
            plan: d.plan.clone(),
            source: self.schema_source(id),
            warnings: d.warnings.clone(),
            fingerprint: fp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::blob::SchemaSet;
    use crate::file_model::model::tests::{run, unit_scada};
    use crate::plan::{Cast, ColumnRoute, Origin};

    fn route(r: &ColumnRoute) -> (Option<u16>, Cast, bool) {
        (r.target, r.cast, r.nullable)
    }

    /// A registry of DISPATCH_UNIT_SCADA alone: three columns, then four from 2026-08.
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
    fn a_block_conforms_to_the_schema_its_history_names() {
        let r = reg();
        let opts = ResolveOptions::default();
        let res = r.resolve_in(&scada(&FOUR), opts, month("2026-08")).unwrap();
        assert_eq!(
            (res.status.clone(), res.version, res.schema),
            (Status::Conforms, 1, Some(1))
        );
        assert_eq!(res.label(), "conforms");
        assert!(res.warnings.is_empty());
        let p = &res.plan;
        assert_eq!(p.schema, 1);
        assert_eq!(
            route(&p.routes[0]),
            (
                Some(0),
                Cast::TimestampNanos {
                    offset_secs: 36_000
                },
                true
            )
        );
        assert_eq!(route(&p.routes[1]), (Some(1), Cast::Utf8, true));
        assert_eq!(route(&p.routes[2]), (Some(2), Cast::Float64, true));
        assert!(p.missing.is_empty());
        assert_eq!(p.time_column, Some(0));
        // Strict mode takes it too, with the same plan.
        let strict = ResolveOptions {
            mode: Mode::Strict,
            ..Default::default()
        };
        let again = r
            .resolve_in(&scada(&FOUR), strict, month("2030-01"))
            .unwrap();
        assert!(Arc::ptr_eq(&again.plan, &res.plan));
        // The columns decide, whatever the month: the older schema in its
        // months, before any and after.
        for m in ["2024-01", "2026-07", "2009-07", "2030-01"] {
            let old = r.resolve_in(&scada(&THREE), strict, month(m)).unwrap();
            assert_eq!((old.status, old.version), (Status::Conforms, 0), "{m}");
            assert_eq!(old.plan.missing, [3]);
        }
        // The 2026 columns in a 2024 source are the 2026 schema.
        let early = r
            .resolve_in(&scada(&FOUR), strict, month("2024-06"))
            .unwrap();
        assert_eq!(early.version, 1);
        // With no month known, the latest run that fits.
        assert_eq!(r.resolve(&scada(&THREE), opts).unwrap().version, 0);
        assert_eq!(r.resolve(&scada(&FOUR), opts).unwrap().version, 1);
        // Two schemas, two plans.
        assert_ne!(
            r.resolve(&scada(&THREE), opts).unwrap().plan_hash(),
            res.plan_hash()
        );
    }

    /// Two runs of one column list: the last released by the month.
    #[test]
    fn the_month_picks_among_runs_of_one_column_list() {
        let mut t = unit_scada();
        let mut again = t.schemas[0].clone();
        again.id = 2;
        t.schemas.push(again);
        t.runs.push(run(2, 1, "2026-10", "2026-12"));
        let r = Registry::from_set(SchemaSet {
            tables: vec![t],
            ..Default::default()
        });
        let opts = ResolveOptions::default();
        let at = |m| {
            r.resolve_in(&scada(&THREE), opts, month(m))
                .unwrap()
                .version
        };
        assert_eq!((at("2025-01"), at("2026-10")), (0, 2));
        assert_eq!(r.resolve(&scada(&THREE), opts).unwrap().version, 2);
    }

    #[test]
    fn a_block_that_deviates_is_kept_unless_strict() {
        let r = reg();
        let opts = ResolveOptions::default();
        let strict = ResolveOptions {
            mode: Mode::Strict,
            ..Default::default()
        };
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
        assert_eq!((res.version, res.schema), (1, None));
        assert_eq!(res.warnings[0].code, "schema.deviates");
        assert!(res.warnings[0].message.contains("in this order"));
        let p = &res.plan;
        assert_eq!(p.schema, 1);
        assert_eq!(p.routes[2].origin, Origin::FileExtra);
        assert_eq!(p.routes[2].target_field_id, None);
        assert_eq!(p.fields[4].origin, Origin::FileExtra);
        assert_eq!(p.missing, [2, 3]);
        // The same shape again is the same plan.
        let again = r.resolve_in(&scada(&odd), opts, month("2026-08")).unwrap();
        assert!(Arc::ptr_eq(&again.plan, &res.plan));
        let e = r
            .resolve_in(&scada(&odd), strict, month("2026-08"))
            .unwrap_err();
        assert!(matches!(e, ResolveError::Deviates { schema: 1, .. }), "{e}");
        // Against the schema in force in its month.
        let res = r.resolve_in(&scada(&odd), opts, month("2024-06")).unwrap();
        assert_eq!(res.version, 0);
        // The columns of a schema, but a column more: a column of the
        // table feeds its own.
        let five = [
            "SETTLEMENTDATE",
            "DUID",
            "SCADAVALUE",
            "LASTCHANGED",
            "LASTCHANGED",
        ];
        let res = r.resolve(&scada(&five), opts).unwrap();
        assert_eq!(res.plan.routes[3].origin, Origin::Declared);
        // A block without one of the table's columns: kept, its rows null
        // there, or refused as deviating in strict mode. No column is a key.
        let fewer = ["SETTLEMENTDATE", "SCADAVALUE"];
        let res = r.resolve(&scada(&fewer), opts).unwrap();
        assert!(res.warnings.iter().all(|w| w.code != "schema.missing_key"));
        assert!(res.plan.arrow.field(1).is_nullable());
        assert!(!res.plan.fields.is_empty());
        let e = r.resolve(&scada(&fewer), strict).unwrap_err();
        assert!(matches!(e, ResolveError::Deviates { .. }), "{e}");
        // Another report pair of the table fits none of its runs.
        let mut t = unit_scada();
        t.report_pairs
            .push(("DISPATCH".into(), "UNIT_SCADA2".into()));
        let r = Registry::from_set(SchemaSet {
            tables: vec![t],
            ..Default::default()
        });
        let other = IRow {
            report_subtype: "UNIT_SCADA2",
            ..scada(&THREE)
        };
        let res = r.resolve(&other, opts).unwrap();
        assert_eq!((res.label(), res.version), ("deviates", 1));
    }

    #[test]
    fn an_unseen_aemo_version_is_a_warning() {
        let r = reg();
        let v2 = IRow {
            version: 2,
            ..scada(&FOUR)
        };
        let res = r
            .resolve_in(
                &v2,
                ResolveOptions {
                    mode: Mode::Strict,
                    ..Default::default()
                },
                month("2026-08"),
            )
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
            .resolve(
                &irow,
                ResolveOptions {
                    mode: Mode::Strict,
                    ..Default::default()
                },
            )
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
        assert_eq!(
            r.table_by_pair(Market::Nem, "NOBODY", "CLAIMS"),
            Some(res.table)
        );
        // Derived in one market, unknown to another.
        assert_eq!(r.table_by_pair(Market::Gas, "NOBODY", "CLAIMS"), None);
        assert!(r.is_derived(res.table));
        assert!(
            r.resolve(
                &irow,
                ResolveOptions {
                    mode: Mode::Strict,
                    ..Default::default()
                }
            )
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

    /// Every run of every embedded table resolves, in its own month, to its own schema.
    #[test]
    fn every_run_of_the_embedded_histories_conforms() {
        let r = Registry::load_embedded();
        let strict = ResolveOptions {
            mode: Mode::Strict,
            ..Default::default()
        };
        let mut runs = 0;
        for (id, t) in r.tables() {
            // Each table's pairs are looked up in its own market.
            let strict = ResolveOptions {
                market: t.market,
                ..strict
            };
            for run in &t.runs {
                let s = t.schema(run.schema).unwrap();
                let cols = t.schema_columns(s);
                let irow = IRow {
                    report_type: &run.report.0,
                    report_subtype: &run.report.1,
                    version: run.version,
                    columns: &cols,
                };
                let res = r.resolve_in(&irow, strict, Some(run.from)).unwrap();
                assert_eq!(res.status, Status::Conforms, "{} {}", t.name, run.from);
                assert_eq!(res.table, id);
                // Two schemas of one column list are one plan's columns.
                assert_eq!(t.schema_columns(t.schema(res.version).unwrap()), cols);
                runs += 1;
            }
        }
        assert!(runs > 900, "{runs}");
        let res = r
            .resolve_in(&scada(&THREE), strict, month("2024-08"))
            .unwrap();
        assert_eq!(res.source, SchemaSource::Registry);
    }
}
