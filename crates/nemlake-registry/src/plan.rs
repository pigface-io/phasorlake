//! The conformance plan: how files of one shape become rows of the lake.
//!
//! Resolution ends in a [`ConformancePlan`], a value that says, for every
//! column of the file, which lake column it feeds and how its text is
//! parsed, and for the output, which columns it has, which of them form
//! the key and which one partitions it. `nemlake-conform` applies it; the
//! registry is the only place that builds one. It serialises (`nemlake
//! schema plan` prints it as JSON); [`PlanHash`] names what of it decides
//! the output.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use serde::Serialize;

use crate::arrow::FIELD_ID_KEY;
use crate::file_model::{ColumnType, LoadMode, Market, MarketDef, TableDef};
use crate::registry::TypeOptions;

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

    /// The Arrow type this cast produces, in `market`'s time zone.
    pub(crate) fn data_type(self, market: &MarketDef) -> DataType {
        match self {
            Self::Utf8 => DataType::Utf8,
            Self::Float64 => DataType::Float64,
            Self::Int64 => DataType::Int64,
            Self::Decimal128 { precision, scale } => DataType::Decimal128(precision, scale as i8),
            Self::TimestampNanos { .. } => DataType::Timestamp(
                arrow_schema::TimeUnit::Nanosecond,
                Some(Arc::from(market.arrow_tz())),
            ),
            Self::Date32 => DataType::Date32,
            Self::Boolean => DataType::Boolean,
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
}

/// The generation of the conversions the code itself performs: how a
/// timestamp, a number or a date is read, how a partition is chosen, how a
/// value that does not fit is kept. Raise it when the same plan would be
/// written differently by a newer nemlake, so that lakes written by the
/// older one are known to be behind.
pub const PLAN_FORMAT: u32 = 2;

/// The identity of a plan: everything that decides what files of its shape
/// are written as, and nothing else. It is recorded with every Parquet file
/// and every ingest, so a file says which plan made it, and an ingest whose
/// plan has changed since can be found and done again from the archived
/// source.
///
/// It covers the table, market, directory and load mode, the key and time
/// columns by name, and each route: the file column, the lake column it
/// feeds with that column's id, type and nullability, the cast and why it
/// goes there. It does not cover the lake columns the shape does not carry,
/// so declaring a column restales only the shapes that carry it, nor a
/// description or provenance note.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanHash(pub u128);

impl std::fmt::Debug for PlanHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl std::fmt::Display for PlanHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "plan:{:032x}", self.0)
    }
}

/// What a route contributes to its plan's identity.
#[derive(Serialize)]
struct RouteIdentity<'a> {
    source: &'a str,
    target: Option<&'a str>,
    field_id: Option<u32>,
    data_type: Option<&'a str>,
    nullable_out: Option<bool>,
    raw: Option<&'a str>,
    cast: Cast,
    nullable: bool,
    origin: Origin,
}

/// What [`PlanHash`] is the hash of.
#[derive(Serialize)]
struct PlanIdentity<'a> {
    format: u32,
    table: &'a str,
    market: Market,
    directory: &'a str,
    load_mode: LoadMode,
    key: Vec<&'a str>,
    time: Option<&'a str>,
    routes: Vec<RouteIdentity<'a>>,
}

impl PlanHash {
    fn of(plan: &ConformancePlan) -> Self {
        let name = |i: u16| plan.fields[usize::from(i)].name.as_str();
        let identity = PlanIdentity {
            format: PLAN_FORMAT,
            table: &plan.table,
            market: plan.market,
            directory: &plan.directory,
            load_mode: plan.load_mode,
            key: plan.key_columns.iter().map(|&k| name(k)).collect(),
            time: plan.time_column.map(name),
            routes: plan
                .routes
                .iter()
                .map(|r| {
                    let field = r.target.map(|t| &plan.fields[usize::from(t)]);
                    RouteIdentity {
                        source: &r.source_name,
                        target: field.map(|f| f.name.as_str()),
                        field_id: r.target_field_id,
                        data_type: field.map(|f| f.data_type.as_str()),
                        nullable_out: field.map(|f| f.nullable),
                        raw: r.raw_target.map(name),
                        cast: r.cast,
                        nullable: r.nullable,
                        origin: r.origin,
                    }
                })
                .collect(),
        };
        let bytes = postcard::to_stdvec(&identity).expect("a plan's identity serialises");
        Self(xxhash_rust::xxh3::xxh3_128(&bytes))
    }
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
    /// The output field that keeps, as text, a value of this column that
    /// does not fit its type: the table's declared `{column}__raw`. `None`
    /// when the table declares none; such a value then goes to a raw
    /// column of the batch alone.
    pub raw_target: Option<u16>,
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
    /// The identity of the plan: see [`PlanHash`].
    #[serde(skip)]
    pub hash: PlanHash,
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
        let t = self.t;
        let c = &t.columns[i];
        self.supplied[i] = true;
        let raw_target = t
            .columns
            .iter()
            .position(|r| r.raw_of() == Some(c.name.as_str()))
            .map(|r| r as u16);
        self.routes.push(ColumnRoute {
            source_name: name.to_owned(),
            source_index: self.routes.len() as u16,
            target: Some(i as u16),
            target_name: Some(c.name.clone()),
            target_field_id: Some(c.id.0),
            raw_target,
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
            raw_target: None,
            cast: Cast::Utf8,
            nullable: true,
            origin: Origin::FileExtra,
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
        let mut plan = ConformancePlan {
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
            hash: PlanHash(0),
        };
        plan.hash = PlanHash::of(&plan);
        plan
    }
}

impl ConformancePlan {
    /// The plan of a declared column list (normalised I-row columns) onto
    /// `t`, with the table's lake schema. Every name of a declared list is
    /// a column or alias of `t` (validation holds it). An undeclared list
    /// goes through
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
            let i = t
                .column_index(n)
                .expect("a declared column list names columns of its table");
            d.route_to_column(n, i, Origin::Declared);
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
    #[cfg(test)]
    pub(crate) fn lake_width(&self) -> usize {
        self.fields.len()
    }

    /// The name of the output column at `index`.
    pub fn column_name(&self, index: usize) -> &str {
        &self.fields[index].name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use crate::resolve::ResolveOptions;

    fn scada<'a>(columns: &'a [&'a str]) -> nemlake_core::IRow<'a> {
        nemlake_core::IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns,
        }
    }

    /// The hash is the same for the same shape and plan, and differs when
    /// the shape, the type mode or the table differs.
    #[test]
    fn the_hash_follows_what_decides_the_output() {
        let reg = Registry::load_embedded(TypeOptions::default());
        let opts = ResolveOptions::default();
        let declared = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
        let a = reg.resolve(&scada(&declared), opts).unwrap();
        let again = reg.resolve(&scada(&declared), opts).unwrap();
        assert_eq!(a.plan_hash(), again.plan_hash());
        let h = a.plan_hash();
        assert_eq!(h.to_string(), format!("plan:{:032x}", h.0));
        assert_eq!(format!("{h:?}"), h.to_string());
        // Another column list of the same table is another plan.
        let fewer = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let b = reg.resolve(&scada(&fewer), opts).unwrap();
        assert_ne!(a.plan_hash(), b.plan_hash());
        let extra = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "NOBODY_KNOWS"];
        let c = reg.resolve(&scada(&extra), opts).unwrap();
        assert_ne!(a.plan_hash(), c.plan_hash());
        assert_ne!(b.plan_hash(), c.plan_hash());
        // Decimal mode writes other types.
        let dec = Registry::load_embedded(TypeOptions { decimal: true });
        let d = dec.resolve(&scada(&declared), opts).unwrap();
        assert_ne!(a.plan_hash(), d.plan_hash());
        // A table derived from the file has a plan of its own, and the same
        // one in another registry that derives it again.
        let cols = ["A", "B"];
        let unknown = nemlake_core::IRow {
            report_type: "PLAN_TEST",
            report_subtype: "NOBODY",
            version: 1,
            columns: &cols,
        };
        let e = reg.resolve(&unknown, opts).unwrap();
        assert_ne!(e.plan_hash(), a.plan_hash());
        let f = dec.resolve(&unknown, opts).unwrap();
        assert_eq!(e.plan_hash(), f.plan_hash());
    }

    /// Declaring a column restales only the shapes that carry it.
    #[test]
    fn a_new_column_restales_only_the_shapes_that_carry_it() {
        use crate::file_model::blob::SchemaSet;
        use crate::file_model::{ColumnDef, ColumnId, ColumnType, SchemaVersion};
        let reg = Registry::load_embedded(TypeOptions::default());
        let mut t = reg
            .table(reg.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone();
        t.columns.push(ColumnDef {
            id: ColumnId(t.next_column_id),
            name: "QUALITY".into(),
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
        let wider = Registry::from_set(
            SchemaSet {
                markets: vec![],
                tables: vec![t],
                ..Default::default()
            },
            TypeOptions::default(),
        );
        let opts = ResolveOptions::default();
        let old = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
        let carrying = [
            "SETTLEMENTDATE",
            "DUID",
            "SCADAVALUE",
            "LASTCHANGED",
            "QUALITY",
        ];
        let hash = |r: &Registry, cols: &[&str]| r.resolve(&scada(cols), opts).unwrap().plan_hash();
        assert_eq!(hash(&reg, &old), hash(&wider, &old), "fresh");
        assert_ne!(hash(&reg, &carrying), hash(&wider, &carrying), "stale");
        // The plan of the old shape has the new column all the same,
        // written null; it is not part of what the shape is read by.
        let plan = wider.resolve(&scada(&old), opts).unwrap().plan;
        assert_eq!(plan.lake_width(), 5);
        assert_eq!(plan.missing, [4]);
    }

    /// A widening, Date to Timestamp say, is not something a later
    /// Iceberg table promotes in its metadata: the plan of every shape that
    /// carries the column changes, so its files are written again.
    #[test]
    fn a_widening_restales_the_shapes_that_carry_the_column() {
        use crate::file_model::blob::SchemaSet;
        use crate::file_model::{ColumnType, compat::is_widening};
        let reg = Registry::load_embedded(TypeOptions::default());
        let mut t = reg
            .table(reg.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
            .clone();
        let with = |t: &crate::file_model::TableDef| {
            Registry::from_set(
                SchemaSet {
                    markets: vec![],
                    tables: vec![t.clone()],
                    ..Default::default()
                },
                TypeOptions::default(),
            )
        };
        let last = t.column_index("LASTCHANGED").unwrap();
        t.columns[last].ty = ColumnType::Date;
        let dated = with(&t);
        t.columns[last].ty = ColumnType::Timestamp;
        let timed = with(&t);
        assert!(is_widening(ColumnType::Date, ColumnType::Timestamp));
        let opts = ResolveOptions::default();
        let carrying = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
        let without = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let hash = |r: &Registry, cols: &[&str]| r.resolve(&scada(cols), opts).unwrap().plan_hash();
        assert_ne!(hash(&dated, &carrying), hash(&timed, &carrying));
        assert_eq!(hash(&dated, &without), hash(&timed, &without));
    }

    #[test]
    fn a_declared_shape_plans_every_column() {
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
        let json = serde_json::to_value(&*p).unwrap();
        assert_eq!(json["routes"][0]["cast"]["kind"], "timestamp_nanos");
        assert_eq!(json["routes"][0]["origin"], "declared");
        assert!(json.get("arrow").is_none());

        // A file column beyond the lake's is routed to an extra column.
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
        assert_eq!(q.routes[3].origin, Origin::FileExtra);
        assert_eq!(q.routes[3].target_field_id, None);
        assert_eq!(q.fields[4].origin, Origin::FileExtra);
        assert_eq!(q.missing, [3]);
    }
}
