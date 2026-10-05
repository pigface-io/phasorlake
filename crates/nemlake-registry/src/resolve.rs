//! Resolving an I row to a conformance plan: which lake column each I-row
//! field feeds and how to cast it (see [`crate::plan`]).

use std::sync::Arc;

use crate::file_model::{SchemaVersion, TableDef};
use nemlake_core::{Category, Diagnostic, Fingerprint, IRow, YearMonth};

use crate::arrow::decimal_fallback;
use crate::error::ResolveError;
use crate::plan::ConformancePlan;
use crate::plan::PlanHash;
use crate::registry::{Registry, SchemaSource, TableId};

/// Strictness of resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Only exact and aliased matches; drift and unknown tables are errors.
    Strict,
    /// Compatible drift is a warning, unknown columns are kept as text with a
    /// warning, unknown tables are inferred.
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
    /// Declared version, identical columns (the version's own list or one
    /// of its declared variants).
    Exact,
    /// Undeclared version whose columns match declared version `matched`.
    Aliased {
        /// The declared version with the same columns.
        matched: SchemaVersion,
    },
    /// Known table, columns differ from the declared version.
    Drift {
        /// Declared columns the I row lacks (written null).
        missing: Vec<String>,
        /// I-row columns the registry does not know (kept as text).
        extra: Vec<String>,
        /// Same set, different order.
        reordered: bool,
    },
    /// No table claims the report pair; the table was derived from the
    /// file's own header, every column as text.
    Derived,
}

impl Status {
    /// One word for events and summaries.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Aliased { .. } => "aliased",
            Self::Drift { .. } => "drift",
            Self::Derived => "derived",
        }
    }
}

/// The result of resolving an I row.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The table.
    pub table: TableId,
    /// The version the plan is based on.
    pub version: SchemaVersion,
    /// How it matched.
    pub status: Status,
    /// The plan to conform with.
    pub plan: Arc<ConformancePlan>,
    /// Where the table's definition came from.
    pub source: SchemaSource,
    /// The routes of hand-written rules that were applied: file column,
    /// lake column.
    pub routed: Vec<(String, String)>,
    /// Warnings raised while resolving (drift, decimal fallback).
    pub warnings: Vec<Diagnostic>,
}

impl Resolved {
    /// The identity of the plan this shape is read with.
    pub fn plan_hash(&self) -> PlanHash {
        self.plan.hash
    }

    /// How the shape matched, in one word, with `+rule` when a
    /// hand-written rule changed where a column goes.
    pub fn label(&self) -> String {
        if self.routed.is_empty() {
            self.status.label().to_owned()
        } else {
            format!("{}+rule", self.status.label())
        }
    }
}

impl Registry {
    /// Resolves an I row of a source whose month is not known. Rules
    /// bounded by months do not apply; see [`Registry::resolve_in`].
    pub fn resolve(&self, irow: &IRow<'_>, opts: ResolveOptions) -> Result<Resolved, ResolveError> {
        self.resolve_in(irow, opts, None)
    }

    /// Resolves an I row of a source of month `period`: by the registry's
    /// declarations first, then by the table's hand-written rules for that
    /// month, version and column list.
    ///
    /// Strictness is decided before the rules: a rule changes where a
    /// column goes, not whether its shape is declared.
    pub fn resolve_in(
        &self,
        irow: &IRow<'_>,
        opts: ResolveOptions,
        period: Option<YearMonth>,
    ) -> Result<Resolved, ResolveError> {
        let plain = self.resolve_declared(irow, opts)?;
        if self.is_derived(plain.table) || self.table(plain.table).rules.is_empty() {
            return Ok(plain);
        }
        Ok(self.apply_rules(plain, irow, period))
    }

    /// Applies the table's rules that are for this block.
    fn apply_rules(
        &self,
        mut res: Resolved,
        irow: &IRow<'_>,
        period: Option<YearMonth>,
    ) -> Resolved {
        let id = res.table;
        let t = self.table(id);
        let fp = irow.fingerprint();
        let version = SchemaVersion(irow.version);
        let shapes = if t.rules.iter().any(|r| r.shape.is_some()) {
            self.shape_ids(irow, period)
        } else {
            Vec::new()
        };
        // Routes in the order the rules are written; the first route of a
        // file column, and the first into a lake column, stands.
        let mut routes: Vec<(&str, usize)> = Vec::new();
        for (n, rule) in t.rules.iter().enumerate() {
            if !rule.applies(period, version, fp, &shapes) {
                continue;
            }
            for c in &rule.columns {
                let Some(target) = t.columns.iter().position(|d| d.name == c.column) else {
                    continue;
                };
                if !irow.columns.contains(&c.source.as_str())
                    || routes.iter().any(|(s, i)| *s == c.source || *i == target)
                {
                    continue;
                }
                // Two fields cannot feed one column: where the file also
                // carries the column under its own name, that one stands
                // and the routed one is kept under its own.
                if let Some(other) = irow
                    .columns
                    .iter()
                    .find(|o| **o != c.source && t.column_index(o) == Some(target))
                {
                    res.warnings.push(
                        Diagnostic::warning(
                            "schema.rule_conflict",
                            Category::Schema,
                            format!(
                                "Rule {} of table {} routes {} to {}, but this file also carries {other}, which feeds that column. The rule is not applied to this file: {} is kept under its own name.",
                                n + 1,
                                t.name,
                                c.source,
                                c.column,
                                c.source
                            ),
                        )
                        .with_table(t.name.clone())
                        .with_version(irow.version),
                    );
                    continue;
                }
                routes.push((c.source.as_str(), target));
            }
        }
        if routes.is_empty() {
            return res;
        }
        // The same list read by other routes is another shape to the cache.
        let key = Fingerprint::of_names(
            irow.columns
                .iter()
                .copied()
                .chain(std::iter::once("\u{1e}routes"))
                .chain(
                    routes
                        .iter()
                        .flat_map(|(s, i)| [*s, t.columns[*i].name.as_str()]),
                ),
        );
        let market = self.market_of(id);
        res.plan = self.dynamic_plan(id, key, || {
            Arc::new(ConformancePlan::file_shape(
                t,
                market,
                self.options(),
                self.lake_schema(id),
                irow.columns,
                &self.table_dir(id),
                &routes,
            ))
        });
        res.routed = routes
            .iter()
            .map(|(s, i)| ((*s).to_owned(), t.columns[*i].name.clone()))
            .collect();
        res
    }

    /// Resolves an I row by what the registry declares, rules aside.
    fn resolve_declared(
        &self,
        irow: &IRow<'_>,
        opts: ResolveOptions,
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
        let fp = irow.fingerprint();
        let version = SchemaVersion(irow.version);
        let mut warnings = Vec::new();

        // 1. Exact: declared version with the same fingerprint, or one of
        //    the other shapes AEMO published under that version number.
        if let Some(v) = t.version(version) {
            let plan = if v.fingerprint == fp {
                self.plan(id, version)
            } else {
                self.variant_plan(id, version, fp)
            };
            if let Some(plan) = plan {
                let plan = plan.clone();
                self.decimal_warnings(t, &mut warnings);
                return Ok(Resolved {
                    table: id,
                    version,
                    status: Status::Exact,
                    plan,
                    source: self.schema_source(id),
                    routed: Vec::new(),
                    warnings,
                });
            }
        }

        // 2. Aliased: undeclared version, but some declared version has these columns.
        if t.version(version).is_none()
            && let Some((other_id, matched)) = self.by_fingerprint(fp)
            && other_id == id
        {
            let plan = self
                .variant_plan(id, matched, fp)
                .or_else(|| self.plan(id, matched))
                .expect("declared versions are precomputed")
                .clone();
            warnings.push(
                Diagnostic::warning(
                    "schema.version_aliased",
                    Category::Schema,
                    format!(
                        "Table {} was published as version {} with exactly the columns of declared version {matched}; read as version {matched}. Add version {} to the schema file.",
                        t.name, irow.version, irow.version
                    ),
                )
                .with_table(t.name.clone())
                .with_version(irow.version),
            );
            self.decimal_warnings(t, &mut warnings);
            return Ok(Resolved {
                table: id,
                version: matched,
                status: Status::Aliased { matched },
                plan,
                source: self.schema_source(id),
                routed: Vec::new(),
                warnings,
            });
        }

        // 3. Drift against the declared version, or the nearest one.
        let base = match t.version(version) {
            Some(v) => v,
            None => {
                let nearest = t
                    .nearest_version(version)
                    .expect("a validated table has a version");
                if opts.mode == Mode::Strict {
                    return Err(ResolveError::UnknownVersion {
                        table: t.name.clone(),
                        version: irow.version,
                        nearest: nearest.n.0,
                    });
                }
                nearest
            }
        };
        let observed: Vec<&str> = irow.columns.to_vec();
        let known: Vec<&str> = observed
            .iter()
            .copied()
            .filter(|c| t.column_index(c).is_some())
            .collect();
        let extra: Vec<String> = observed
            .iter()
            .filter(|c| t.column_index(c).is_none())
            .map(|c| (*c).to_owned())
            .collect();
        let canonical: Vec<&str> = known
            .iter()
            .map(|c| t.column(c).map_or(*c, |d| d.name.as_str()))
            .collect();
        let missing: Vec<String> = base
            .columns
            .iter()
            .filter(|c| !canonical.contains(&c.as_str()))
            .cloned()
            .collect();
        let base_present: Vec<&str> = base
            .columns
            .iter()
            .map(String::as_str)
            .filter(|c| canonical.contains(c))
            .collect();
        let canon_present: Vec<&str> = canonical
            .iter()
            .copied()
            .filter(|c| base.columns.iter().any(|b| b == c))
            .collect();
        let reordered = base_present != canon_present;

        let missing_keys: Vec<&String> = t
            .primary_key
            .iter()
            .filter(|k| !canonical.contains(&k.as_str()))
            .collect();
        if opts.mode == Mode::Strict {
            if let Some(k) = missing_keys.first() {
                return Err(ResolveError::MissingKey {
                    table: t.name.clone(),
                    version: base.n.0,
                    column: (*k).clone(),
                });
            }
            return Err(ResolveError::VersionDrift {
                table: t.name.clone(),
                version: base.n.0,
                declared: base.columns.len(),
                observed: observed.len(),
                missing,
                extra,
            });
        }
        // Permissive: nothing the file carries is lost. Declared columns go
        // to their lake columns, the rest are kept as text under their own
        // names, and what the file lacks is null.
        let market = self.market_of(id);
        let plan = self.dynamic_plan(id, fp, || {
            Arc::new(ConformancePlan::file_shape(
                t,
                market,
                self.options(),
                self.lake_schema(id),
                &observed,
                &self.table_dir(id),
                &[],
            ))
        });
        let mut d = Diagnostic::warning(
            if extra.is_empty() { "schema.compatible_drift" } else { "schema.unknown_columns" },
            Category::Schema,
            format!(
                "Table {} version {} declares {} columns but this file has {}: {} missing (written null), {} unknown (kept as text under their own names){}. `nemlake schema infer` drafts a table file that declares this shape.",
                t.name,
                base.n,
                base.columns.len(),
                observed.len(),
                missing.len(),
                extra.len(),
                if reordered { ", order differs" } else { "" }
            ),
        )
        .with_table(t.name.clone())
        .with_version(irow.version);
        d.expected = Some(base.columns.join(","));
        d.observed = Some(observed.join(","));
        warnings.push(d);
        if !missing_keys.is_empty() {
            warnings.push(
                Diagnostic::warning(
                    "schema.missing_key",
                    Category::Schema,
                    format!(
                        "This file of table {} lacks primary-key column(s) {}; its rows are kept with those columns null, and cannot be told apart by key.",
                        t.name,
                        missing_keys
                            .iter()
                            .map(|k| k.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .with_table(t.name.clone())
                .with_version(irow.version),
            );
        }
        self.decimal_warnings(t, &mut warnings);
        Ok(Resolved {
            table: id,
            version: base.n,
            status: Status::Drift {
                missing,
                extra,
                reordered,
            },
            plan,
            source: self.schema_source(id),
            routed: Vec::new(),
            warnings,
        })
    }

    /// The plan of an I row onto a table derived from the file: every
    /// column as text. A later block of the same pair with another column
    /// list keeps its extra columns under their own names.
    fn resolve_derived(&self, id: TableId, irow: &IRow<'_>) -> Resolved {
        let t = self.table(id);
        let market = self.market_of(id);
        let fp = irow.fingerprint();
        let names: Vec<&str> = irow.columns.to_vec();
        let plan = self.dynamic_plan(id, fp, || {
            Arc::new(ConformancePlan::file_shape(
                t,
                market,
                self.options(),
                self.lake_schema(id),
                &names,
                &self.table_dir(id),
                &[],
            ))
        });
        let warning = Diagnostic::warning(
            "schema.unknown_table",
            Category::Schema,
            format!(
                "No table in the registry claims report pair ({}, {}). Its rows are kept as the file gives them, every column as text, under {}. `nemlake schema infer <file> --write` drafts a table for it; ingest again once the draft is adopted.",
                irow.report_type,
                irow.report_subtype,
                self.table_dir(id)
            ),
        )
        .with_table(t.name.clone())
        .with_version(irow.version);
        Resolved {
            table: id,
            version: SchemaVersion(irow.version),
            status: Status::Derived,
            plan,
            source: self.schema_source(id),
            routed: Vec::new(),
            warnings: vec![warning],
        }
    }

    fn decimal_warnings(&self, t: &TableDef, warnings: &mut Vec<Diagnostic>) {
        if !self.options().decimal {
            return;
        }
        // One warning per table, not one per column per file.
        let floats: Vec<&str> = t
            .columns
            .iter()
            .filter(|c| decimal_fallback(c.ty, self.options()))
            .map(|c| c.name.as_str())
            .collect();
        if floats.is_empty() {
            return;
        }
        let shown = floats
            .iter()
            .take(8)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let more = floats.len().saturating_sub(8);
        warnings.push(
            Diagnostic::warning(
                "schema.decimal_fallback",
                Category::Schema,
                format!(
                    "{} column(s) of table {} have no precision and scale that their values fit, so --decimal reads them as Float64: {shown}{}.",
                    floats.len(),
                    t.name,
                    if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    }
                ),
            )
            .with_table(t.name.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::ColumnType;
    use crate::plan::{Cast, ColumnRoute};
    use crate::registry::TypeOptions;

    fn route(r: &ColumnRoute) -> (Option<u16>, Cast, bool) {
        (r.target, r.cast, r.nullable)
    }

    fn reg() -> Arc<Registry> {
        Registry::load_embedded(TypeOptions::default())
    }

    fn scada_cols() -> Vec<&'static str> {
        vec!["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"]
    }

    #[test]
    fn exact_match() {
        let r = reg();
        let cols = scada_cols();
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert_eq!(res.status, Status::Exact);
        assert_eq!(res.status.label(), "exact");
        assert!(res.warnings.is_empty());
        let p = &res.plan;
        assert_eq!(p.routes.len(), 4);
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
        assert_eq!(
            route(&p.routes[3]),
            (
                Some(3),
                Cast::TimestampNanos {
                    offset_secs: 36_000
                },
                true
            )
        );
        assert!(p.missing.is_empty());
        assert_eq!(p.key_columns, [0, 1]);
        assert_eq!(p.time_column, Some(0));
        assert_eq!(p.lake_width(), 4);
        // Strict mode accepts exact matches too, and the same Arc is returned.
        let again = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap();
        assert!(Arc::ptr_eq(&again.plan, &res.plan));
    }

    #[test]
    fn decimal_mode_casts_and_warns_on_fallback() {
        let r = Registry::load_embedded(TypeOptions { decimal: true });
        let cols = scada_cols();
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert_eq!(
            res.plan.routes[2].cast,
            Cast::Decimal128 {
                precision: 16,
                scale: 6
            }
        );
        // A table with a bare NUMBER column warns once per such column.
        let (id, t) = r
            .tables()
            .find(|(_, t)| {
                t.columns.iter().any(|c| {
                    matches!(
                        c.ty,
                        ColumnType::Number {
                            precision: None,
                            ..
                        }
                    )
                })
            })
            .unwrap();
        let v = &t.versions[0];
        let cols: Vec<&str> = v.columns.iter().map(String::as_str).collect();
        let (rt, st) = &t.report_pairs[0];
        let row = IRow {
            report_type: rt,
            report_subtype: st,
            version: v.n.0,
            columns: &cols,
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert_eq!(res.table, id);
        assert!(
            res.warnings
                .iter()
                .any(|w| w.code == "schema.decimal_fallback"),
            "{:?}",
            res.warnings
        );
    }

    #[test]
    fn aliased_version() {
        let r = reg();
        let cols = scada_cols();
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 9,
            columns: &cols,
        };
        let res = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap();
        assert_eq!(
            res.status,
            Status::Aliased {
                matched: SchemaVersion(1)
            }
        );
        assert_eq!(res.version, SchemaVersion(1));
        assert_eq!(res.warnings[0].code, "schema.version_aliased");
        assert_eq!(res.warnings[0].version, Some(9));
        assert!(Arc::ptr_eq(
            &res.plan,
            r.plan(res.table, SchemaVersion(1)).unwrap()
        ));
        // The columns of a declared variant, under an undeclared version
        // number, are that variant.
        let short = &cols[..3];
        let row = IRow {
            columns: short,
            ..row
        };
        let res = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap();
        assert_eq!(
            res.status,
            Status::Aliased {
                matched: SchemaVersion(1)
            }
        );
        assert_eq!(res.plan.routes.len(), 3);
        assert_eq!(res.plan.missing, [3]);
        assert!(Arc::ptr_eq(
            &res.plan,
            r.variant_plan(res.table, SchemaVersion(1), row.fingerprint())
                .unwrap()
        ));
    }

    #[test]
    fn compatible_drift_missing_column_is_null_in_permissive_and_error_in_strict() {
        let r = reg();
        let cols = vec!["SETTLEMENTDATE", "DUID"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert_eq!(
            res.status,
            Status::Drift {
                missing: vec!["SCADAVALUE".into(), "LASTCHANGED".into()],
                extra: vec![],
                reordered: false
            }
        );
        assert_eq!(res.status.label(), "drift");
        assert_eq!(res.plan.missing, [2, 3]);
        assert_eq!(res.plan.routes.len(), 2);
        assert_eq!(res.warnings[0].code, "schema.compatible_drift");
        assert_eq!(
            res.warnings[0].expected.as_deref(),
            Some("SETTLEMENTDATE,DUID,SCADAVALUE,LASTCHANGED")
        );
        // The dynamic plan is cached.
        let again = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert!(Arc::ptr_eq(&again.plan, &res.plan));
        let e = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap_err();
        assert!(
            matches!(
                e,
                ResolveError::VersionDrift {
                    declared: 4,
                    observed: 2,
                    ..
                }
            ),
            "{e}"
        );
    }

    #[test]
    fn unknown_column_is_kept_as_text_and_reorder_detected() {
        let r = reg();
        let cols = vec![
            "DUID",
            "SETTLEMENTDATE",
            "SCADAVALUE",
            "LASTCHANGED",
            "QUALITY",
        ];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        match &res.status {
            Status::Drift {
                missing,
                extra,
                reordered,
            } => {
                assert!(missing.is_empty());
                assert_eq!(extra, &["QUALITY".to_owned()]);
                assert!(reordered);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(res.plan.routes[0].target, Some(1));
        assert_eq!(res.plan.routes[1].target, Some(0));
        // The column the table does not have is kept: one more column, text,
        // nullable, under its own name and marked as coming from the file.
        assert_eq!(route(&res.plan.routes[4]), (Some(4), Cast::Utf8, true));
        assert_eq!(res.plan.lake_width(), 5);
        let extra = res.plan.arrow.field(4);
        assert_eq!(extra.name(), "QUALITY");
        assert_eq!(extra.data_type(), &arrow_schema::DataType::Utf8);
        assert!(extra.is_nullable());
        assert_eq!(
            extra.metadata().get("nemlake:source").map(String::as_str),
            Some("file")
        );
        assert_eq!(res.plan.column_name(4), "QUALITY");
        // The table's own columns are as the lake has them.
        let lake = r.lake_schema(res.table);
        assert_eq!(lake.fields().len(), 4);
        assert_eq!(res.plan.arrow.fields()[..4], lake.fields()[..]);
        assert_eq!(res.warnings[0].code, "schema.unknown_columns");
        assert!(res.warnings[0].message.contains("order differs"));
        assert!(res.warnings[0].message.contains("kept as text"));
        // A name the file repeats is kept too, told apart by a suffix.
        let twice = vec!["SETTLEMENTDATE", "DUID", "SCADAVALUE", "X", "X", "DUID"];
        let row = IRow {
            columns: &twice,
            ..row
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        let names: Vec<&str> = (0..res.plan.lake_width())
            .map(|i| res.plan.column_name(i))
            .collect();
        assert_eq!(
            names,
            [
                "SETTLEMENTDATE",
                "DUID",
                "SCADAVALUE",
                "LASTCHANGED",
                "X",
                "X__2",
                "DUID__2"
            ]
        );
        assert!(res.plan.routes.iter().all(|c| c.target.is_some()));
    }

    #[test]
    fn unknown_version_drifts_against_nearest_or_errors_in_strict() {
        let r = reg();
        let cols = vec!["SETTLEMENTDATE", "DUID", "SCADAVALUE", "NEW"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 2,
            columns: &cols,
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert_eq!(res.version, SchemaVersion(1));
        assert!(matches!(res.status, Status::Drift { .. }));
        let e = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap_err();
        assert_eq!(
            e,
            ResolveError::UnknownVersion {
                table: "DISPATCH_UNIT_SCADA".into(),
                version: 2,
                nearest: 1
            }
        );
    }

    #[test]
    fn a_file_without_a_key_column_is_kept_unless_strict() {
        let r = reg();
        let cols = vec!["SETTLEMENTDATE", "SCADAVALUE"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let e = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap_err();
        assert_eq!(
            e,
            ResolveError::MissingKey {
                table: "DISPATCH_UNIT_SCADA".into(),
                version: 1,
                column: "DUID".into()
            }
        );
        // Permissive: the rows are kept, the key column is null for them,
        // which this shape's schema allows though the lake's does not.
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert!(matches!(res.status, Status::Drift { .. }));
        assert_eq!(res.plan.missing, [1, 3]);
        assert!(res.plan.arrow.field(1).is_nullable());
        assert!(!r.lake_schema(res.table).field(1).is_nullable());
        assert!(!res.plan.arrow.field(0).is_nullable());
        assert!(
            res.warnings
                .iter()
                .any(|w| w.code == "schema.missing_key" && w.message.contains("DUID")),
            "{:?}",
            res.warnings
        );
    }

    #[test]
    fn unknown_table_and_no_columns() {
        let r = reg();
        let cols = vec!["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "NOPE",
            version: 1,
            columns: &cols,
        };
        assert!(matches!(
            r.resolve(&row, ResolveOptions { mode: Mode::Strict }),
            Err(ResolveError::UnknownTable { .. })
        ));
        assert!(r.table_by_pair("DISPATCH", "NOPE").is_none());
        // Permissive: the pair gets a table derived from the row itself.
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert_eq!(res.status, Status::Derived);
        assert_eq!(res.status.label(), "derived");
        assert!(r.is_derived(res.table));
        assert_eq!(res.version, SchemaVersion(1));
        let t = r.table(res.table);
        assert_eq!(t.name, "DISPATCH_NOPE");
        assert_eq!(r.table_dir(res.table), "_unregistered/dispatch.nope");
        assert_eq!(t.columns.len(), cols.len());
        assert!(
            t.columns
                .iter()
                .all(|c| c.nullable && matches!(c.ty, ColumnType::Utf8 { .. }))
        );
        assert_eq!(res.plan.lake_width(), 3);
        assert!(
            res.plan
                .routes
                .iter()
                .all(|c| c.cast == Cast::Utf8 && c.target.is_some())
        );
        assert_eq!(res.warnings.len(), 1);
        assert_eq!(res.warnings[0].code, "schema.unknown_table");
        assert_eq!(res.warnings[0].severity, nemlake_core::Severity::Warning);
        // The same pair is the same table for the rest of the run, also in
        // another shape, whose extra column is kept.
        assert_eq!(r.table_by_pair("DISPATCH", "NOPE"), Some(res.table));
        assert_eq!(r.declared_by_pair("DISPATCH", "NOPE"), None);
        let wider = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "MORE"];
        let again = r
            .resolve(
                &IRow {
                    columns: &wider,
                    version: 2,
                    ..row
                },
                ResolveOptions::default(),
            )
            .unwrap();
        assert_eq!(again.table, res.table);
        assert_eq!(again.version, SchemaVersion(2));
        assert_eq!(again.plan.lake_width(), 4);
        assert_eq!(again.plan.column_name(3), "MORE");
        // Strict still refuses the pair, derived earlier or not.
        assert!(matches!(
            r.resolve(&row, ResolveOptions { mode: Mode::Strict }),
            Err(ResolveError::UnknownTable { .. })
        ));
        // A table without a subtype is named by its type alone.
        let bare = IRow {
            report_type: "NOSUBTYPE_TEST",
            report_subtype: "",
            version: 1,
            columns: &cols,
        };
        let res = r.resolve(&bare, ResolveOptions::default()).unwrap();
        assert_eq!(r.table(res.table).name, "NOSUBTYPE_TEST");
        assert_eq!(r.table_dir(res.table), "_unregistered/nosubtype_test");
        // The registry's own tables are not derived and keep their directory.
        let scada = r.table_by_name("DISPATCH_UNIT_SCADA").unwrap();
        assert!(!r.is_derived(scada));
        assert_eq!(r.table_dir(scada), "dispatch_unit_scada");
        assert_eq!(
            r.tables().count(),
            r.tables().filter(|(id, _)| !r.is_derived(*id)).count()
        );
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &[],
        };
        assert!(matches!(
            r.resolve(&row, ResolveOptions::default()),
            Err(ResolveError::NoColumns { .. })
        ));
    }

    #[test]
    fn declared_variants_resolve_exactly() {
        let r = reg();
        // Two lists of one version that differ in a name: the archive says
        // CONFIDENTIAL_TO where the live feed says DUID, both as version 5.
        // Each list is declared and each name is a column of its own.
        let id = r.table_by_name("DISPATCHCONSTRAINT").unwrap();
        let t = r.table(id);
        let v = t.versions.last().unwrap();
        let variant = v.variants.first().expect("the table has a variant");
        assert_ne!(variant.columns, v.columns);
        let refs: Vec<&str> = variant.columns.iter().map(String::as_str).collect();
        let row = IRow {
            report_type: &t.report_pairs[0].0,
            report_subtype: &t.report_pairs[0].1,
            version: v.n.0,
            columns: &refs,
        };
        for mode in [Mode::Permissive, Mode::Strict] {
            let res = r.resolve(&row, ResolveOptions { mode }).unwrap();
            assert_eq!(res.status, Status::Exact);
            assert_eq!(res.version, v.n);
            assert!(res.warnings.is_empty(), "{:?}", res.warnings);
            assert!(res.plan.routes.iter().all(|c| c.target.is_some()));
            // What the other list has under its own name is null here.
            let lacking: Vec<&str> = res
                .plan
                .missing
                .iter()
                .map(|&i| t.columns[usize::from(i)].name.as_str())
                .collect();
            assert_eq!(lacking.len(), 1, "{lacking:?}");
            assert!(v.columns.iter().any(|c| c == lacking[0]));
            assert!(!variant.columns.iter().any(|c| c == lacking[0]));
        }
        // The two lists feed the same lake columns but for that one name.
        let main = r.plan(id, v.n).unwrap();
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        let targets = |p: &ConformancePlan| p.routes.iter().map(|c| c.target).collect::<Vec<_>>();
        let differing = targets(&res.plan)
            .iter()
            .zip(targets(main))
            .filter(|(a, b)| **a != *b)
            .count();
        assert_eq!(differing, 1);
        assert!(Arc::ptr_eq(
            &res.plan,
            r.variant_plan(id, v.n, variant.fingerprint).unwrap()
        ));

        // Columns appended without a version bump: the short list is a variant
        // and the columns it lacks are written null.
        let id = r.table_by_name("DISPATCHINTERCONNECTORRES").unwrap();
        let t = r.table(id);
        let v = t.versions.last().unwrap();
        let short = v
            .variants
            .iter()
            .find(|x| x.columns.len() < v.columns.len())
            .expect("a shorter variant");
        let refs: Vec<&str> = short.columns.iter().map(String::as_str).collect();
        let row = IRow {
            report_type: &t.report_pairs[0].0,
            report_subtype: &t.report_pairs[0].1,
            version: v.n.0,
            columns: &refs,
        };
        let res = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap();
        assert_eq!(res.status, Status::Exact);
        assert!(res.warnings.is_empty());
        assert_eq!(
            res.plan.missing.len(),
            v.columns.len() - short.columns.len()
        );
    }

    #[test]
    fn an_undeclared_cut_of_a_declared_list_is_compatible_drift() {
        let r = reg();
        let id = r.table_by_name("DISPATCHCONSTRAINT").unwrap();
        let t = r.table(id);
        let v = t.versions.last().unwrap();
        // A declared variant minus its last column is not declared anywhere.
        let variant = v.variants.first().unwrap();
        let mut cols = variant.columns.clone();
        cols.pop().unwrap();
        // Missing, against the version's own list: what was cut, and the
        // name only that list has.
        let missing: Vec<String> = v
            .columns
            .iter()
            .filter(|c| !cols.contains(c))
            .cloned()
            .collect();
        assert_eq!(missing.len(), 2);
        let refs: Vec<&str> = cols.iter().map(String::as_str).collect();
        let row = IRow {
            report_type: &t.report_pairs[0].0,
            report_subtype: &t.report_pairs[0].1,
            version: v.n.0,
            columns: &refs,
        };
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert_eq!(
            res.status,
            Status::Drift {
                missing,
                extra: vec![],
                reordered: false
            }
        );
        assert!(res.plan.routes.iter().all(|c| c.target.is_some()));
        assert!(matches!(
            r.resolve(&row, ResolveOptions { mode: Mode::Strict }),
            Err(ResolveError::VersionDrift { .. })
        ));
    }

    /// A registry of one table: DISPATCH_UNIT_SCADA with a hand-declared
    /// text column and rules that route SCADAVALUE to it for the files of
    /// 2009-07 to 2011-12, and CONFIDENTIAL_TO to DUID in version 7.
    fn ruled() -> Registry {
        use crate::file_model::blob::SchemaSet;
        use crate::file_model::{ColumnDef, ColumnId, RuleColumn, RuleDef};
        let embedded = reg();
        let mut t = embedded
            .table(embedded.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone();
        t.columns.push(ColumnDef {
            id: ColumnId(t.next_column_id),
            name: "SCADAVALUE_TEXT".into(),
            aliases: vec![],
            ty: ColumnType::Utf8 {
                max_len: None,
                dict: false,
            },
            nullable: true,
            oracle: None,
            loader: None,
            added_in: SchemaVersion(1),
            removed_in: None,
            description: None,
            src: vec!["manual".into()],
            curated: true,
        });
        t.next_column_id += 1;
        t.rules = vec![
            RuleDef {
                note: "the value was published as text until 2011".into(),
                from: Some("2009-07".parse().unwrap()),
                to: Some("2011-12".parse().unwrap()),
                version: None,
                fingerprint: None,
                shape: None,
                columns: vec![RuleColumn {
                    source: "SCADAVALUE".into(),
                    column: "SCADAVALUE_TEXT".into(),
                }],
            },
            RuleDef {
                note: "version 7 named the unit CONFIDENTIAL_TO".into(),
                from: None,
                to: None,
                version: Some(SchemaVersion(7)),
                fingerprint: None,
                shape: None,
                columns: vec![RuleColumn {
                    source: "CONFIDENTIAL_TO".into(),
                    column: "DUID".into(),
                }],
            },
        ];
        assert_eq!(crate::file_model::validate::validate_structure(&t), vec![]);
        Registry::from_set(
            SchemaSet {
                markets: vec![],
                tables: vec![t],
                ..Default::default()
            },
            TypeOptions::default(),
        )
    }

    #[test]
    fn a_rule_for_a_shape_routes_that_shape_only() {
        use crate::file_model::blob::SchemaSet;
        use crate::file_model::shapes::ShapeEntry;
        use crate::file_model::{RuleColumn, RuleDef};
        use nemlake_core::TypeClass;
        let base = ruled();
        let mut t = base
            .table(base.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone();
        let entry = |columns: &[(&str, TypeClass)], first: &str, last: &str| {
            let mut e = ShapeEntry {
                id: nemlake_core::ShapeId(0),
                report_type: "DISPATCH".into(),
                report_subtype: "UNIT_SCADA".into(),
                version: 1,
                first_seen: first.parse().unwrap(),
                last_seen: last.parse().unwrap(),
                columns: columns.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect(),
            };
            e.id = e.shape().id();
            e
        };
        let three = [
            ("SETTLEMENTDATE", TypeClass::Timestamp),
            ("DUID", TypeClass::Varchar),
            ("SCADAVALUE", TypeClass::Varchar),
        ];
        let mut four = three.to_vec();
        four.push(("LASTCHANGED", TypeClass::Timestamp));
        let ruled_shape = entry(&three, "2010-01", "2010-12");
        let sibling = entry(&four, "2010-01", "2010-12");
        t.rules = vec![RuleDef {
            note: "SCADAVALUE was loaded as text in this shape".into(),
            from: None,
            to: None,
            version: None,
            fingerprint: None,
            shape: Some(ruled_shape.id),
            columns: vec![RuleColumn {
                source: "SCADAVALUE".into(),
                column: "SCADAVALUE_TEXT".into(),
            }],
        }];
        assert_eq!(crate::file_model::validate::validate_structure(&t), vec![]);
        let mut unruled = t.clone();
        unruled.rules.clear();
        let unruled = Registry::from_set(
            SchemaSet {
                markets: vec![],
                tables: vec![unruled],
                ..Default::default()
            },
            TypeOptions::default(),
        );
        let r = Registry::from_set(
            SchemaSet {
                markets: vec![],
                tables: vec![t],
                shapes: [(
                    "DISPATCH_UNIT_SCADA".to_owned(),
                    vec![ruled_shape.clone(), sibling.clone()],
                )]
                .into(),
            },
            TypeOptions::default(),
        );
        let cols3: Vec<&str> = three.iter().map(|(n, _)| *n).collect();
        let cols4: Vec<&str> = four.iter().map(|(n, _)| *n).collect();
        let row = |columns| IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns,
        };
        let month = |s: &str| Some(s.parse::<YearMonth>().unwrap());
        assert_eq!(r.shape_ids(&row(&cols3), None), [ruled_shape.id]);
        assert_eq!(
            r.shape_ids(&row(&cols3), month("2010-05")),
            [ruled_shape.id]
        );
        assert!(r.shape_ids(&row(&cols3), month("2012-01")).is_empty());
        assert_eq!(r.shape_ids(&row(&cols4), None), [sibling.id]);
        let opts = ResolveOptions::default();
        // The shape the rule names, in a month the census saw it: routed.
        let res = r.resolve_in(&row(&cols3), opts, month("2010-05")).unwrap();
        assert_eq!(res.label(), "exact+rule");
        // A month the census did not see it in, or the sibling shape: not.
        let later = r.resolve_in(&row(&cols3), opts, month("2012-01")).unwrap();
        assert_eq!(later.label(), "exact");
        let other = r.resolve_in(&row(&cols4), opts, month("2010-05")).unwrap();
        assert_eq!(other.label(), "exact");
        // The rule makes only its shape's files stale.
        let before = |cols| {
            unruled
                .resolve_in(&row(cols), opts, month("2010-05"))
                .unwrap()
                .plan_hash()
        };
        assert_ne!(res.plan_hash(), before(&cols3));
        assert_eq!(other.plan_hash(), before(&cols4));
        assert_eq!(later.plan_hash(), before(&cols3));
    }

    #[test]
    fn a_rule_routes_a_column_for_the_files_of_its_interval() {
        let r = ruled();
        let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let month = |s: &str| Some(s.parse::<YearMonth>().unwrap());
        let opts = ResolveOptions::default();
        let id = r.table_by_name("DISPATCH_UNIT_SCADA").unwrap();
        let t = r.table(id);
        let text = t.column_index("SCADAVALUE_TEXT").unwrap() as u16;
        let value = t.column_index("SCADAVALUE").unwrap() as u16;

        // Outside the interval, and where the month is not known, the
        // file is read by the declarations alone.
        for period in [month("2012-01"), month("2009-06"), None] {
            let plain = r.resolve_in(&row, opts, period).unwrap();
            assert_eq!(plain.status, Status::Exact);
            assert!(plain.routed.is_empty());
            assert_eq!(plain.label(), "exact");
            assert_eq!(plain.plan.routes[2].target, Some(value));
            assert_eq!(plain.plan.routes[2].cast, Cast::Float64);
        }
        let plain = r.resolve(&row, opts).unwrap();

        // Inside it, SCADAVALUE goes to the text column, as text, and the
        // number column is null for these files.
        for period in [month("2009-07"), month("2010-05"), month("2011-12")] {
            let res = r.resolve_in(&row, opts, period).unwrap();
            assert_eq!(res.status, Status::Exact);
            assert_eq!(res.label(), "exact+rule");
            assert_eq!(
                res.routed,
                [("SCADAVALUE".to_owned(), "SCADAVALUE_TEXT".to_owned())]
            );
            assert_eq!(res.plan.routes[2].target, Some(text));
            assert_eq!(res.plan.routes[2].cast, Cast::Utf8);
            assert!(res.plan.missing.contains(&value));
            assert!(!res.plan.missing.contains(&text));
            assert_eq!(res.plan.routes[0], plain.plan.routes[0]);
            // Other rules, other hash: the lake knows which made a file.
            assert_ne!(res.plan_hash(), plain.plan_hash());
            assert!(res.warnings.is_empty());
            // Strict mode takes the same rule: the shape is declared.
            let strict = r
                .resolve_in(&row, ResolveOptions { mode: Mode::Strict }, period)
                .unwrap();
            assert!(Arc::ptr_eq(&strict.plan, &res.plan));
        }

        // A rule by version, whatever the month: an undeclared name is
        // routed to a declared column instead of being kept beside it.
        let v7 = ["SETTLEMENTDATE", "CONFIDENTIAL_TO", "SCADAVALUE"];
        let row7 = IRow {
            version: 7,
            columns: &v7,
            ..row
        };
        let res = r.resolve_in(&row7, opts, None).unwrap();
        assert!(matches!(res.status, Status::Drift { .. }));
        assert_eq!(res.label(), "drift+rule");
        assert_eq!(
            res.routed,
            [("CONFIDENTIAL_TO".to_owned(), "DUID".to_owned())]
        );
        assert_eq!(res.plan.routes[1].target, Some(1));
        assert_eq!(res.plan.lake_width(), t.columns.len());
        // Both rules at once: each routes its own column.
        let res = r.resolve_in(&row7, opts, month("2010-01")).unwrap();
        assert_eq!(
            res.routed,
            [
                ("SCADAVALUE".to_owned(), "SCADAVALUE_TEXT".to_owned()),
                ("CONFIDENTIAL_TO".to_owned(), "DUID".to_owned())
            ]
        );
        // Strictness comes first: a rule does not declare a shape.
        assert!(matches!(
            r.resolve_in(&row7, ResolveOptions { mode: Mode::Strict }, None),
            Err(ResolveError::UnknownVersion { .. })
        ));

        // Two fields cannot feed one column. A file that carries the
        // target under its own name keeps both, and says so.
        let both = ["SETTLEMENTDATE", "DUID", "CONFIDENTIAL_TO", "SCADAVALUE"];
        let row_both = IRow {
            version: 7,
            columns: &both,
            ..row
        };
        let res = r.resolve_in(&row_both, opts, None).unwrap();
        assert!(res.routed.is_empty());
        assert_eq!(res.label(), "drift");
        let conflict = res
            .warnings
            .iter()
            .find(|w| w.code == "schema.rule_conflict")
            .expect("the conflict is reported");
        assert!(conflict.message.contains("CONFIDENTIAL_TO") && conflict.message.contains("DUID"));
        assert_eq!(
            res.plan.column_name(res.plan.lake_width() - 1),
            "CONFIDENTIAL_TO"
        );
    }

    #[test]
    fn integer_columns_are_int64() {
        let r = reg();
        let market = r.market(crate::file_model::Market::Nem).unwrap();
        assert_eq!(
            crate::arrow::data_type(ColumnType::Integer, market, TypeOptions::default()),
            arrow_schema::DataType::Int64
        );
    }
}
