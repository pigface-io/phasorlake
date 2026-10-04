//! The conformance plan: how files of one shape become rows of the lake.
//!
//! Resolution ends in a [`ConformancePlan`], a value that says, for every
//! column of the file, which lake column it feeds and how its text is
//! parsed, and for the output, which columns it has, which of them form
//! the key and which one partitions it. `nemlake-conform` applies it; the
//! registry is the only place that builds one. It serialises (`nemlake
//! schema plan` prints it as JSON) and has a deterministic byte encoding
//! ([`ConformancePlan::encode`]).

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use serde::Serialize;

use crate::arrow::FIELD_ID_KEY;
use crate::file_model::{ColumnType, LoadMode, Market, MarketDef, TableDef};
use crate::registry::TypeOptions;
use crate::rules::RulesHash;

/// How a field is parsed into its lake column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
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
    /// MMS timestamp in market time to UTC nanoseconds.
    TimestampNanos {
        /// Market offset east of UTC, seconds.
        offset_secs: i32,
    },
    /// Date to days since the epoch.
    Date32,
    /// `true`/`false`, `Y`/`N`, `1`/`0`.
    Boolean,
}

impl Cast {
    pub(crate) fn for_type(ty: ColumnType, market: &MarketDef, opts: TypeOptions) -> Self {
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
            ColumnType::Timestamp => Self::TimestampNanos {
                offset_secs: market.utc_offset_secs,
            },
            ColumnType::Date => Self::Date32,
            ColumnType::Boolean => Self::Boolean,
        }
    }
}

/// Why a file column goes where it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Its name is a column (or an alias of one) the table declares.
    Declared,
    /// A hand-written `[[rule]]` of the table routes it.
    Rule,
    /// The table has no column for it; it is kept as text under its own
    /// name, in a column of these files only.
    FileExtra,
    /// It is not kept: a plain-CSV header column its dataset's table does
    /// not declare.
    Dropped,
}

/// One column of the file and where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColumnRoute {
    /// The column's name in the file's header (normalised).
    pub source_name: String,
    /// Its 0-based position among the file's data columns.
    pub source_index: u16,
    /// Index of the output field it feeds, or `None` when it is dropped.
    pub target: Option<u16>,
    /// Name of that field.
    pub target_name: Option<String>,
    /// Stable id of that field; `None` for a column of the file only.
    pub target_field_id: Option<u32>,
    /// How its text is parsed.
    pub cast: Cast,
    /// Whether an empty field is allowed (null) or a fault.
    pub nullable: bool,
    /// Why it goes there.
    pub origin: Origin,
}

/// One column of the output, in output order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanField {
    /// Name.
    pub name: String,
    /// Stable id; `None` for a column of the file only.
    pub field_id: Option<u32>,
    /// Arrow type, as Arrow writes it.
    pub data_type: String,
    /// Whether it may be null in files of this shape.
    pub nullable: bool,
    /// `Declared` for a lake column, `FileExtra` for one of the file only.
    pub origin: Origin,
}

/// How files of one shape are conformed to the lake.
#[derive(Debug, Clone, Serialize)]
pub struct ConformancePlan {
    /// The table.
    pub table: String,
    /// Its market.
    pub market: Market,
    /// The directory its files go to, under the market.
    pub directory: String,
    /// Append or snapshot.
    pub load_mode: LoadMode,
    /// Output field indexes of the primary key.
    pub key_columns: Vec<u16>,
    /// Output field index of the partition column.
    pub time_column: Option<u16>,
    /// The output columns: the lake's, then any the file carries beyond
    /// them.
    pub fields: Vec<PlanField>,
    /// One route per file column, in file order.
    pub routes: Vec<ColumnRoute>,
    /// Output field indexes no file column feeds; they are written null.
    pub missing: Vec<u16>,
    /// The output schema, as Arrow builds batches with it.
    #[serde(skip)]
    pub arrow: SchemaRef,
    /// The identity of these rules: see [`crate::rules`].
    #[serde(skip)]
    pub rules: RulesHash,
}

/// The parts of a plan before its schema and hash are fixed.
struct Draft<'a> {
    t: &'a TableDef,
    market: &'a MarketDef,
    opts: TypeOptions,
    routes: Vec<ColumnRoute>,
    supplied: Vec<bool>,
}

impl<'a> Draft<'a> {
    fn new(t: &'a TableDef, market: &'a MarketDef, opts: TypeOptions, width: usize) -> Self {
        Self {
            t,
            market,
            opts,
            routes: Vec::new(),
            supplied: vec![false; width],
        }
    }

    /// File column `name` feeds lake column `i`.
    fn route_to_column(&mut self, name: &str, i: usize, origin: Origin) {
        let c = &self.t.columns[i];
        self.supplied[i] = true;
        self.routes.push(ColumnRoute {
            source_name: name.to_owned(),
            source_index: self.routes.len() as u16,
            target: Some(i as u16),
            target_name: Some(c.name.clone()),
            target_field_id: Some(c.id.0),
            cast: Cast::for_type(c.ty, self.market, self.opts),
            nullable: c.nullable,
            origin,
        });
    }

    /// File column `name` is kept as text in output field `i`, named `as_name`.
    fn route_to_extra(&mut self, name: &str, i: usize, as_name: &str) {
        self.routes.push(ColumnRoute {
            source_name: name.to_owned(),
            source_index: self.routes.len() as u16,
            target: Some(i as u16),
            target_name: Some(as_name.to_owned()),
            target_field_id: None,
            cast: Cast::Utf8,
            nullable: true,
            origin: Origin::FileExtra,
        });
    }

    fn route_dropped(&mut self, name: &str) {
        self.routes.push(ColumnRoute {
            source_name: name.to_owned(),
            source_index: self.routes.len() as u16,
            target: None,
            target_name: None,
            target_field_id: None,
            cast: Cast::Utf8,
            nullable: true,
            origin: Origin::Dropped,
        });
    }

    fn finish(self, dir: &str, arrow: SchemaRef) -> ConformancePlan {
        let t = self.t;
        let missing: Vec<u16> = self
            .supplied
            .iter()
            .enumerate()
            .filter(|(_, s)| !**s)
            .map(|(i, _)| i as u16)
            .collect();
        let key_columns: Vec<u16> = t
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
        let rules = crate::rules::hash(
            t,
            dir,
            &arrow,
            &self.routes,
            &missing,
            &key_columns,
            time_column,
        );
        let lake_width = t.columns.len();
        let fields = arrow
            .fields()
            .iter()
            .enumerate()
            .map(|(i, f)| PlanField {
                name: f.name().clone(),
                field_id: f.metadata().get(FIELD_ID_KEY).and_then(|v| v.parse().ok()),
                data_type: f.data_type().to_string(),
                nullable: f.is_nullable(),
                origin: if i < lake_width {
                    Origin::Declared
                } else {
                    Origin::FileExtra
                },
            })
            .collect();
        ConformancePlan {
            table: t.name.clone(),
            market: t.market,
            directory: dir.to_owned(),
            load_mode: t.load_mode,
            key_columns,
            time_column,
            fields,
            routes: self.routes,
            missing,
            arrow,
            rules,
        }
    }
}

impl ConformancePlan {
    /// The plan of a declared column list (normalised I-row columns) onto
    /// `t`, with the table's lake schema. Every name of a declared list is
    /// a column or alias of `t`; a name that is not (a plain-CSV header
    /// column) is dropped. An undeclared list goes through
    /// [`ConformancePlan::file_shape`], which keeps what this one would
    /// have no place for.
    pub fn declared(
        t: &TableDef,
        market: &MarketDef,
        opts: TypeOptions,
        lake: &SchemaRef,
        names: &[&str],
    ) -> Self {
        let mut d = Draft::new(t, market, opts, t.columns.len());
        for n in names {
            match t.column_index(n) {
                Some(i) => d.route_to_column(n, i, Origin::Declared),
                None => d.route_dropped(n),
            }
        }
        d.finish(&t.dir_name(), lake.clone())
    }

    /// The plan of a column list the registry has not declared, losing
    /// nothing the file carries, with the routes of the hand-written rules
    /// that apply: each names a file column and the index of the lake
    /// column it feeds in place of the one its name would give it.
    ///
    /// The schema is the table's lake schema with two changes that hold for
    /// files of this shape only: every name that is not a column of the
    /// table is added at the end as a nullable text column, marked as coming
    /// from the file, and a column the shape lacks is made nullable where
    /// the lake says it is not, since these files have no value for it.
    /// `dir` is the directory the table's files go to.
    pub fn file_shape(
        t: &TableDef,
        market: &MarketDef,
        opts: TypeOptions,
        lake: &SchemaRef,
        names: &[&str],
        dir: &str,
        routes: &[(&str, usize)],
    ) -> Self {
        let lake_width = lake.fields().len();
        let mut fields: Vec<Field> = lake.fields().iter().map(|f| f.as_ref().clone()).collect();
        let mut d = Draft::new(t, market, opts, lake_width);
        let mut extra: Vec<String> = Vec::new();
        for n in names {
            let routed = routes.iter().find(|(s, _)| s == n).map(|(_, i)| *i);
            let origin = if routed.is_some() {
                Origin::Rule
            } else {
                Origin::Declared
            };
            match routed
                .or_else(|| t.column_index(n))
                .filter(|&i| !d.supplied[i])
            {
                Some(i) => d.route_to_column(n, i, origin),
                None => {
                    // A name the file repeats, or one that collides with what
                    // is already there, is told apart by a suffix.
                    let mut name = (*n).to_owned();
                    let mut k = 2;
                    while extra.contains(&name) || t.column_index(&name).is_some() {
                        name = format!("{n}__{k}");
                        k += 1;
                    }
                    d.route_to_extra(n, lake_width + extra.len(), &name);
                    let from_file: std::collections::HashMap<String, String> =
                        [("nemlake:source".to_owned(), "file".to_owned())].into();
                    fields.push(Field::new(&name, DataType::Utf8, true).with_metadata(from_file));
                    extra.push(name);
                }
            }
        }
        for (m, supplied) in d.supplied.iter().enumerate() {
            let f = &mut fields[m];
            if !supplied && !f.is_nullable() {
                *f = f.clone().with_nullable(true);
            }
        }
        let arrow = Arc::new(Schema::new_with_metadata(fields, lake.metadata().clone()));
        d.finish(dir, arrow)
    }

    /// Number of columns files of this shape are written with: the lake's,
    /// plus any the file carries beyond them.
    pub fn lake_width(&self) -> usize {
        self.fields.len()
    }

    /// The name of the output column at `index`.
    pub fn column_name(&self, index: usize) -> &str {
        &self.fields[index].name
    }

    /// The plan's deterministic byte encoding: the same plan always
    /// encodes to the same bytes, and any change to what it says changes
    /// them.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("a plan always serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use crate::resolve::ResolveOptions;

    #[test]
    fn a_declared_shape_plans_every_column_and_encodes_stably() {
        let r = Registry::load_embedded(TypeOptions::default());
        let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
        let irow = nemlake_core::IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let p = r.resolve(&irow, ResolveOptions::default()).unwrap().plan;
        assert_eq!(p.table, "DISPATCH_UNIT_SCADA");
        assert_eq!(p.directory, "dispatch_unit_scada");
        assert_eq!(p.load_mode, LoadMode::Append);
        assert_eq!(p.key_columns, [0, 1]);
        assert_eq!(p.time_column, Some(0));
        assert!(p.missing.is_empty());
        assert_eq!(p.lake_width(), 4);
        assert_eq!(p.column_name(2), "SCADAVALUE");
        assert!(p.routes.iter().all(|r| r.origin == Origin::Declared));
        assert_eq!(p.routes[1].source_name, "DUID");
        assert_eq!(p.routes[1].source_index, 1);
        assert_eq!(p.routes[1].target_name.as_deref(), Some("DUID"));
        assert!(p.fields.iter().all(|f| f.field_id.is_some()));
        assert_eq!(p.routes[2].target_field_id, p.fields[2].field_id);
        assert_eq!(p.encode(), p.encode());
        let json = serde_json::to_value(&*p).unwrap();
        assert_eq!(json["routes"][0]["cast"]["kind"], "timestamp_nanos");
        assert_eq!(json["routes"][0]["origin"], "declared");
        assert!(json.get("arrow").is_none());

        // Another column list encodes to other bytes.
        let fewer = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "EXTRA"];
        let q = r
            .resolve(
                &nemlake_core::IRow {
                    columns: &fewer,
                    ..irow
                },
                ResolveOptions::default(),
            )
            .unwrap()
            .plan;
        assert_ne!(p.encode(), q.encode());
        assert_eq!(q.routes[3].origin, Origin::FileExtra);
        assert_eq!(q.routes[3].target_field_id, None);
        assert_eq!(q.fields[4].origin, Origin::FileExtra);
        assert_eq!(q.missing, [3]);
    }
}
