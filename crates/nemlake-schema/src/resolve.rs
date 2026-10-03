//! Resolving an I row to a projection: which lake column each I-row field
//! feeds and how to cast it.

use std::sync::Arc;

use arrow_schema::SchemaRef;
use nemlake_core::{Category, Diagnostic, IRow};
use nemlake_schema_model::{ColumnType, MarketDef, SchemaVersion, TableDef};

use crate::arrow::{data_type, decimal_fallback};
use crate::error::ResolveError;
use crate::registry::{Registry, TableId, TypeOptions};

/// Strictness of resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Only exact and aliased matches; drift and unknown tables are errors.
    Strict,
    /// Compatible drift is a warning, unknown columns are dropped with a
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

/// How a field is parsed into its lake column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cast {
    /// Bytes as UTF-8 text.
    Utf8,
    /// `f64`.
    Float64,
    /// `i64`.
    Int64,
    /// `Decimal128(precision, scale)`, unscaled `i128`.
    Decimal128 {
        /// Precision.
        precision: u8,
        /// Scale.
        scale: u8,
    },
    /// MMS timestamp in market time to UTC microseconds.
    TimestampMicros {
        /// Market offset east of UTC, seconds.
        offset_secs: i32,
    },
    /// Date to days since the epoch.
    Date32,
    /// `true`/`false`, `Y`/`N`, `1`/`0`.
    Boolean,
}

impl Cast {
    fn for_type(ty: ColumnType, market: &MarketDef, opts: TypeOptions) -> Self {
        match ty {
            ColumnType::Utf8 { .. } => Self::Utf8,
            ColumnType::Number {
                precision: Some(p),
                scale: Some(s),
            } if opts.decimal => Self::Decimal128 {
                precision: p,
                scale: s,
            },
            ColumnType::Number { .. } => Self::Float64,
            ColumnType::Integer => Self::Int64,
            ColumnType::Timestamp => Self::TimestampMicros {
                offset_secs: market.utc_offset_secs,
            },
            ColumnType::Date => Self::Date32,
            ColumnType::Boolean => Self::Boolean,
        }
    }
}

/// What to do with the I-row field at one position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnCast {
    /// Index of the lake column this field feeds, or `None` to drop it.
    pub target: Option<u16>,
    /// How to parse it.
    pub cast: Cast,
    /// Whether an empty field is allowed (null) or an error.
    pub nullable: bool,
}

/// A resolved shape: the lake schema plus one cast per I-row position.
#[derive(Debug)]
pub struct Projection {
    /// The unified lake schema (all columns of the table).
    pub arrow: SchemaRef,
    /// One entry per I-row column, in I-row order.
    pub casts: Vec<ColumnCast>,
    /// Lake column indexes the I row does not supply; they are written null.
    pub missing: Vec<u16>,
    /// Lake column indexes of the primary key.
    pub primary_key: Vec<u16>,
    /// Lake column index of the partition column.
    pub time_column: Option<u16>,
}

impl Projection {
    /// Builds the projection of `names` (normalised I-row columns) onto `t`.
    /// Names that are not columns or aliases of `t` are dropped (`target:
    /// None`); the caller decides whether that is allowed.
    pub fn build(
        t: &TableDef,
        market: &MarketDef,
        opts: TypeOptions,
        arrow: &SchemaRef,
        names: &[&str],
    ) -> Result<Self, ResolveError> {
        let mut casts = Vec::with_capacity(names.len());
        let mut supplied = vec![false; t.columns.len()];
        for n in names {
            match t.column_index(n) {
                Some(i) => {
                    let c = &t.columns[i];
                    supplied[i] = true;
                    casts.push(ColumnCast {
                        target: Some(i as u16),
                        cast: Cast::for_type(c.ty, market, opts),
                        nullable: c.nullable,
                    });
                }
                None => casts.push(ColumnCast {
                    target: None,
                    cast: Cast::Utf8,
                    nullable: true,
                }),
            }
        }
        let missing = supplied
            .iter()
            .enumerate()
            .filter(|(_, s)| !**s)
            .map(|(i, _)| i as u16)
            .collect();
        let primary_key = t
            .primary_key
            .iter()
            .filter_map(|k| t.column_index(k))
            .map(|i| i as u16)
            .collect();
        let time_column = t
            .time_column
            .as_deref()
            .and_then(|c| t.column_index(c))
            .map(|i| i as u16);
        Ok(Self {
            arrow: arrow.clone(),
            casts,
            missing,
            primary_key,
            time_column,
        })
    }

    /// Number of lake columns.
    pub fn lake_width(&self) -> usize {
        self.arrow.fields().len()
    }
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
        /// I-row columns the registry does not know (dropped).
        extra: Vec<String>,
        /// Same set, different order.
        reordered: bool,
    },
}

impl Status {
    /// One word for events and summaries.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Aliased { .. } => "aliased",
            Self::Drift { .. } => "drift",
        }
    }
}

/// The result of resolving an I row.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The table.
    pub table: TableId,
    /// The version the projection is based on.
    pub version: SchemaVersion,
    /// How it matched.
    pub status: Status,
    /// The projection to parse with.
    pub projection: Arc<Projection>,
    /// Warnings raised while resolving (drift, decimal fallback).
    pub warnings: Vec<Diagnostic>,
}

impl Registry {
    /// Resolves an I row.
    pub fn resolve(&self, irow: &IRow<'_>, opts: ResolveOptions) -> Result<Resolved, ResolveError> {
        if irow.columns.is_empty() {
            return Err(ResolveError::NoColumns {
                report_type: irow.report_type.to_owned(),
                report_subtype: irow.report_subtype.to_owned(),
                version: irow.version,
            });
        }
        let Some(id) = self.table_by_pair(irow.report_type, irow.report_subtype) else {
            return Err(ResolveError::UnknownTable {
                report_type: irow.report_type.to_owned(),
                report_subtype: irow.report_subtype.to_owned(),
            });
        };
        let t = self.table(id);
        let fp = irow.fingerprint();
        let version = SchemaVersion(irow.version);
        let mut warnings = Vec::new();

        // 1. Exact: declared version with the same fingerprint, or one of
        //    the other shapes AEMO published under that version number.
        if let Some(v) = t.version(version) {
            let projection = if v.fingerprint == fp {
                self.projection(id, version)
            } else {
                self.variant_projection(id, version, fp)
            };
            if let Some(projection) = projection {
                let projection = projection.clone();
                self.decimal_warnings(t, &mut warnings);
                return Ok(Resolved {
                    table: id,
                    version,
                    status: Status::Exact,
                    projection,
                    warnings,
                });
            }
        }

        // 2. Aliased: undeclared version, but some declared version has these columns.
        if t.version(version).is_none()
            && let Some((other_id, matched)) = self.by_fingerprint(fp)
            && other_id == id
        {
            let projection = self
                .projection(id, matched)
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
                projection,
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

        for k in &t.primary_key {
            if !canonical.contains(&k.as_str()) {
                return Err(ResolveError::MissingKey {
                    table: t.name.clone(),
                    version: base.n.0,
                    column: k.clone(),
                });
            }
        }
        if opts.mode == Mode::Strict {
            return Err(ResolveError::VersionDrift {
                table: t.name.clone(),
                version: base.n.0,
                declared: base.columns.len(),
                observed: observed.len(),
                missing,
                extra,
            });
        }
        let market = self.market_of(id);
        let projection = self.dynamic_projection(id, fp, || {
            Arc::new(
                Projection::build(t, market, self.options(), self.lake_schema(id), &observed)
                    .expect("drift projection builds from known columns"),
            )
        });
        let mut d = Diagnostic::warning(
            if extra.is_empty() { "schema.compatible_drift" } else { "schema.unknown_columns" },
            Category::Schema,
            format!(
                "Table {} version {} declares {} columns but this file has {}: {} missing (written null), {} unknown (dropped){}.",
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
        self.decimal_warnings(t, &mut warnings);
        Ok(Resolved {
            table: id,
            version: base.n,
            status: Status::Drift {
                missing,
                extra,
                reordered,
            },
            projection,
            warnings,
        })
    }

    /// Resolves a plain-CSV header row against a dataset's table.
    pub fn resolve_header(
        &self,
        dataset: &str,
        header: &[&str],
        opts: ResolveOptions,
    ) -> Result<Resolved, ResolveError> {
        let Some(id) = self.table_by_dataset(dataset) else {
            return Err(ResolveError::UnknownTable {
                report_type: "dataset".into(),
                report_subtype: dataset.to_owned(),
            });
        };
        let t = self.table(id);
        let v = t.latest_version().expect("a validated table has a version");
        let irow = IRow {
            report_type: "dataset",
            report_subtype: dataset,
            version: v.n.0,
            columns: header,
        };
        // Share the MMS path: a dataset's table claims no report pair, so
        // resolve by hand with the same drift rules.
        let fp = irow.fingerprint();
        if fp == v.fingerprint {
            let projection = self
                .projection(id, v.n)
                .expect("declared versions are precomputed")
                .clone();
            return Ok(Resolved {
                table: id,
                version: v.n,
                status: Status::Exact,
                projection,
                warnings: vec![],
            });
        }
        if opts.mode == Mode::Strict {
            let missing = v
                .columns
                .iter()
                .filter(|c| !header.contains(&c.as_str()))
                .cloned()
                .collect();
            let extra = header
                .iter()
                .filter(|c| t.column_index(c).is_none())
                .map(|c| (*c).to_owned())
                .collect();
            return Err(ResolveError::VersionDrift {
                table: t.name.clone(),
                version: v.n.0,
                declared: v.columns.len(),
                observed: header.len(),
                missing,
                extra,
            });
        }
        let market = self.market_of(id);
        let projection = self.dynamic_projection(id, fp, || {
            Arc::new(
                Projection::build(t, market, self.options(), self.lake_schema(id), header)
                    .expect("header projection builds"),
            )
        });
        let missing = v
            .columns
            .iter()
            .filter(|c| !header.contains(&c.as_str()))
            .cloned()
            .collect();
        let extra = header
            .iter()
            .filter(|c| t.column_index(c).is_none())
            .map(|c| (*c).to_owned())
            .collect();
        Ok(Resolved {
            table: id,
            version: v.n,
            status: Status::Drift {
                missing,
                extra,
                reordered: false,
            },
            projection,
            warnings: vec![],
        })
    }

    fn decimal_warnings(&self, t: &TableDef, warnings: &mut Vec<Diagnostic>) {
        if !self.options().decimal {
            return;
        }
        for c in &t.columns {
            if decimal_fallback(c.ty, self.options()) {
                warnings.push(
                    Diagnostic::warning(
                        "schema.decimal_fallback",
                        Category::Schema,
                        format!("Column {} of table {} has no declared precision and scale, so --decimal falls back to Float64 for it.", c.name, t.name),
                    )
                    .with_table(t.name.clone())
                    .with_column(c.name.clone()),
                );
            }
        }
    }
}

/// The Arrow type a cast produces, for tests and tools.
pub fn cast_data_type(
    ty: ColumnType,
    market: &MarketDef,
    opts: TypeOptions,
) -> arrow_schema::DataType {
    data_type(ty, market, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Arc<Registry> {
        Registry::load_embedded(TypeOptions::default())
    }

    fn scada_cols() -> Vec<&'static str> {
        vec!["SETTLEMENTDATE", "DUID", "SCADAVALUE"]
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
        let p = &res.projection;
        assert_eq!(p.casts.len(), 3);
        assert_eq!(
            p.casts[0],
            ColumnCast {
                target: Some(0),
                cast: Cast::TimestampMicros {
                    offset_secs: 36_000
                },
                nullable: false
            }
        );
        assert_eq!(
            p.casts[1],
            ColumnCast {
                target: Some(1),
                cast: Cast::Utf8,
                nullable: false
            }
        );
        assert_eq!(
            p.casts[2],
            ColumnCast {
                target: Some(2),
                cast: Cast::Float64,
                nullable: true
            }
        );
        assert!(p.missing.is_empty());
        assert_eq!(p.primary_key, [0, 1]);
        assert_eq!(p.time_column, Some(0));
        assert_eq!(p.lake_width(), 3);
        // Strict mode accepts exact matches too, and the same Arc is returned.
        let again = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap();
        assert!(Arc::ptr_eq(&again.projection, &res.projection));
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
            res.projection.casts[2].cast,
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
                missing: vec!["SCADAVALUE".into()],
                extra: vec![],
                reordered: false
            }
        );
        assert_eq!(res.status.label(), "drift");
        assert_eq!(res.projection.missing, [2]);
        assert_eq!(res.projection.casts.len(), 2);
        assert_eq!(res.warnings[0].code, "schema.compatible_drift");
        assert_eq!(
            res.warnings[0].expected.as_deref(),
            Some("SETTLEMENTDATE,DUID,SCADAVALUE")
        );
        // The dynamic projection is cached.
        let again = r.resolve(&row, ResolveOptions::default()).unwrap();
        assert!(Arc::ptr_eq(&again.projection, &res.projection));
        let e = r
            .resolve(&row, ResolveOptions { mode: Mode::Strict })
            .unwrap_err();
        assert!(
            matches!(
                e,
                ResolveError::VersionDrift {
                    declared: 3,
                    observed: 2,
                    ..
                }
            ),
            "{e}"
        );
    }

    #[test]
    fn unknown_column_is_dropped_with_warning_and_reorder_detected() {
        let r = reg();
        let cols = vec!["DUID", "SETTLEMENTDATE", "SCADAVALUE", "QUALITY"];
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
        assert_eq!(res.projection.casts[0].target, Some(1));
        assert_eq!(res.projection.casts[1].target, Some(0));
        assert_eq!(res.projection.casts[3].target, None);
        assert_eq!(res.warnings[0].code, "schema.unknown_columns");
        assert!(res.warnings[0].message.contains("order differs"));
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
    fn missing_primary_key_is_always_an_error() {
        let r = reg();
        let cols = vec!["SETTLEMENTDATE", "SCADAVALUE"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let e = r.resolve(&row, ResolveOptions::default()).unwrap_err();
        assert_eq!(
            e,
            ResolveError::MissingKey {
                table: "DISPATCH_UNIT_SCADA".into(),
                version: 1,
                column: "DUID".into()
            }
        );
    }

    #[test]
    fn unknown_table_and_no_columns() {
        let r = reg();
        let cols = scada_cols();
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "NOPE",
            version: 1,
            columns: &cols,
        };
        assert!(matches!(
            r.resolve(&row, ResolveOptions::default()),
            Err(ResolveError::UnknownTable { .. })
        ));
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
        // A rename AEMO made in place: the archive says CONFIDENTIAL_TO where
        // the live feed says DUID, both as version 5.
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
            assert!(res.projection.casts.iter().all(|c| c.target.is_some()));
            assert!(res.projection.missing.is_empty());
        }
        // The alias column feeds the same lake column as the canonical name.
        let main = r.projection(id, v.n).unwrap();
        let res = r.resolve(&row, ResolveOptions::default()).unwrap();
        let targets = |p: &Projection| p.casts.iter().map(|c| c.target).collect::<Vec<_>>();
        assert_eq!(targets(&res.projection), targets(main));
        assert!(Arc::ptr_eq(
            &res.projection,
            r.variant_projection(id, v.n, variant.fingerprint).unwrap()
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
            res.projection.missing.len(),
            v.columns.len() - short.columns.len()
        );
    }

    #[test]
    fn alias_in_an_undeclared_shape_is_compatible_drift() {
        let r = reg();
        let id = r.table_by_name("DISPATCHCONSTRAINT").unwrap();
        let t = r.table(id);
        let v = t.versions.last().unwrap();
        // The alias shape minus its last column is not declared anywhere.
        let variant = v.variants.first().unwrap();
        let mut cols = variant.columns.clone();
        let dropped = cols.pop().unwrap();
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
                missing: vec![dropped],
                extra: vec![],
                reordered: false
            }
        );
        assert!(res.projection.casts.iter().all(|c| c.target.is_some()));
        assert!(matches!(
            r.resolve(&row, ResolveOptions { mode: Mode::Strict }),
            Err(ResolveError::VersionDrift { .. })
        ));
    }

    #[test]
    fn header_resolution_for_datasets() {
        let r = reg();
        assert!(matches!(
            r.resolve_header("facility-scada", &["A"], ResolveOptions::default()),
            Err(ResolveError::UnknownTable { .. })
        ));
        let market = r.market(nemlake_schema_model::Market::Nem).unwrap();
        assert_eq!(
            cast_data_type(ColumnType::Integer, market, TypeOptions::default()),
            arrow_schema::DataType::Int64
        );
    }
}
