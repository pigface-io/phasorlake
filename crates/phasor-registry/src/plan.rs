//! The conformance plan: how files of one shape become rows of the lake.
//!
//! Only the registry builds a [`ConformancePlan`]; `phasor-mms-conform` applies it.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use serde::Serialize;

use crate::arrow::FIELD_ID_KEY;
use crate::file_model::{LakeType, LoadMode, Market, MarketDef, Table};

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
    /// MMS timestamp in market time to UTC nanoseconds.
    TimestampNanos {
        /// Market offset east of UTC, seconds.
        offset_secs: i32,
    },
    /// Date to days since the epoch.
    Date32,
    /// `true`/`false`, `Y`/`N`, `1`/`0`.
    Boolean,
    /// An exact decimal of this precision and scale, never rounded.
    Decimal128 {
        /// Digits in all.
        precision: u8,
        /// Digits after the point.
        scale: u8,
    },
}

impl Cast {
    /// The cast that reads text into a column of `ty`.
    pub const fn for_type(ty: LakeType, market: &MarketDef) -> Self {
        match ty {
            LakeType::String => Self::Utf8,
            LakeType::Double => Self::Float64,
            LakeType::Long => Self::Int64,
            LakeType::Timestamp => Self::TimestampNanos {
                offset_secs: market.utc_offset_secs,
            },
            LakeType::Date => Self::Date32,
            LakeType::Boolean => Self::Boolean,
            LakeType::Decimal { precision, scale } => Self::Decimal128 { precision, scale },
        }
    }
}

/// Why a file column goes where it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Its name is a column of the table.
    Declared,
    /// The table has no column for it; it is kept as text in these files only.
    FileExtra,
}

/// The generation of the conversions the code itself performs.
///
/// Raise it when the same plan would be written differently by a newer phasor.
pub const PLAN_FORMAT: u32 = 10;

/// The identity of a plan: everything that decides what files of its shape are written as.
///
/// It leaves out lake columns the shape does not carry, so declaring a column
/// restales only the shapes that carry it, and descriptions and notes.
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
    schema: u16,
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
            schema: plan.schema,
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
    /// The schema of the table's history the shape was read against.
    pub schema: u16,
    /// Output field index of the partition column.
    pub time_column: Option<u16>,
    /// The output columns: the lake's, then any the file carries beyond them.
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

impl ConformancePlan {
    /// The plan of a file column list onto `t` against `schema`, losing nothing the file carries.
    ///
    /// Unknown names become trailing nullable text columns; lake columns the
    /// shape lacks are made nullable.
    pub fn new(
        t: &Table,
        market: &MarketDef,
        lake: &SchemaRef,
        schema: u16,
        names: &[&str],
        dir: &str,
    ) -> Self {
        let lake_width = lake.fields().len();
        let mut fields: Vec<Field> = Vec::new();
        let mut supplied = vec![false; lake_width];
        let mut routes = Vec::with_capacity(names.len());
        let mut extra: Vec<String> = Vec::new();
        for (k, n) in names.iter().enumerate() {
            let source_index = k as u16;
            match t.column_index(n).filter(|&i| !supplied[i]) {
                Some(i) => {
                    let c = &t.columns[i];
                    supplied[i] = true;
                    routes.push(ColumnRoute {
                        source_name: (*n).to_owned(),
                        source_index,
                        target: Some(i as u16),
                        target_name: Some(c.name.clone()),
                        target_field_id: Some(c.id.0),
                        cast: Cast::for_type(c.ty, market),
                        nullable: c.nullable,
                        origin: Origin::Declared,
                    });
                }
                None => {
                    // A repeated or colliding name gets a suffix.
                    let mut name = (*n).to_owned();
                    let mut j = 2;
                    while extra.contains(&name) || t.column_index(&name).is_some() {
                        name = format!("{n}__{j}");
                        j += 1;
                    }
                    routes.push(ColumnRoute {
                        source_name: (*n).to_owned(),
                        source_index,
                        target: Some((lake_width + extra.len()) as u16),
                        target_name: Some(name.clone()),
                        target_field_id: None,
                        cast: Cast::Utf8,
                        nullable: true,
                        origin: Origin::FileExtra,
                    });
                    let from_file: std::collections::HashMap<String, String> =
                        [("phasor:source".to_owned(), "file".to_owned())].into();
                    fields.push(Field::new(&name, DataType::Utf8, true).with_metadata(from_file));
                    extra.push(name);
                }
            }
        }
        let relaxed = supplied
            .iter()
            .enumerate()
            .any(|(i, s)| !s && !lake.field(i).is_nullable());
        let arrow = if fields.is_empty() && !relaxed {
            lake.clone()
        } else {
            let mut all: Vec<Field> = lake
                .fields()
                .iter()
                .zip(&supplied)
                .map(|(f, s)| {
                    let f = f.as_ref().clone();
                    if *s || f.is_nullable() {
                        f
                    } else {
                        f.with_nullable(true)
                    }
                })
                .collect();
            all.append(&mut fields);
            Arc::new(Schema::new_with_metadata(all, lake.metadata().clone()))
        };
        let missing: Vec<u16> = supplied
            .iter()
            .enumerate()
            .filter(|(_, s)| !**s)
            .map(|(i, _)| i as u16)
            .collect();
        let index = |c: &str| t.column_index(c).map(|i| i as u16);
        let mut plan = Self {
            table: t.name.clone(),
            market: t.market,
            directory: dir.to_owned(),
            load_mode: t.load_mode,
            schema,
            time_column: t.time_column.as_deref().and_then(index),
            fields: arrow
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
                .collect(),
            routes,
            missing,
            arrow,
            hash: PlanHash(0),
        };
        plan.hash = PlanHash::of(&plan);
        plan
    }

    /// Number of columns files of this shape are written with.
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
    use crate::file_model::blob::SchemaSet;
    use crate::file_model::model::tests::unit_scada;
    use crate::file_model::{Column, ColumnId};
    use crate::registry::Registry;
    use crate::resolve::ResolveOptions;

    fn scada<'a>(columns: &'a [&'a str]) -> phasor_mms_decode::IRow<'a> {
        phasor_mms_decode::IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns,
        }
    }

    fn with(t: Table) -> Registry {
        Registry::from_set(SchemaSet {
            tables: vec![t],
            ..Default::default()
        })
    }

    const FOUR: [&str; 4] = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
    const THREE: [&str; 3] = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];

    fn hash(r: &Registry, cols: &[&str]) -> PlanHash {
        r.resolve(&scada(cols), ResolveOptions::default())
            .unwrap()
            .plan_hash()
    }

    /// The hash differs exactly when the shape, the schema or the table differs.
    #[test]
    fn the_hash_follows_what_decides_the_output() {
        let reg = with(unit_scada());
        assert_eq!(hash(&reg, &FOUR), hash(&with(unit_scada()), &FOUR));
        let h = hash(&reg, &FOUR);
        assert_eq!(h.to_string(), format!("plan:{:032x}", h.0));
        assert_eq!(format!("{h:?}"), h.to_string());
        assert_ne!(hash(&reg, &THREE), h);
        let extra = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "NOBODY_KNOWS"];
        assert_ne!(hash(&reg, &extra), h);
        // The same columns read against another schema.
        let mut renumbered = unit_scada();
        renumbered.schemas[1].id = 3;
        renumbered.runs[1].schema = 3;
        assert_ne!(hash(&with(renumbered), &FOUR), h);
        // A description or a note is not part of it.
        let mut noted = unit_scada();
        noted.description = None;
        noted.columns[2].note = Some("why".into());
        assert_eq!(hash(&with(noted), &FOUR), h);
        // A table derived from the file has a plan of its own, and the same
        // one in another registry that derives it again.
        let cols = ["A", "B"];
        let unknown = phasor_mms_decode::IRow {
            report_type: "PLAN_TEST",
            report_subtype: "NOBODY",
            version: 1,
            columns: &cols,
        };
        let e = reg.resolve(&unknown, ResolveOptions::default()).unwrap();
        let f = with(unit_scada())
            .resolve(&unknown, ResolveOptions::default())
            .unwrap();
        assert_eq!(e.plan_hash(), f.plan_hash());
    }

    /// Declaring or retyping a column restales only the shapes that carry it.
    #[test]
    fn a_change_restales_only_the_shapes_it_touches() {
        let reg = with(unit_scada());
        let mut t = unit_scada();
        t.columns.push(Column {
            id: ColumnId(5),
            name: "QUALITY".into(),
            ty: LakeType::String,
            nullable: true,
            note: None,
        });
        let wider = with(t);
        let carrying = [
            "SETTLEMENTDATE",
            "DUID",
            "SCADAVALUE",
            "LASTCHANGED",
            "QUALITY",
        ];
        assert_eq!(hash(&reg, &FOUR), hash(&wider, &FOUR), "fresh");
        assert_ne!(hash(&reg, &carrying), hash(&wider, &carrying), "stale");
        // The plan of the old shape has the new column all the same,
        // written null.
        let plan = wider
            .resolve(&scada(&FOUR), ResolveOptions::default())
            .unwrap()
            .plan;
        assert_eq!(plan.lake_width(), 5);
        assert_eq!(plan.missing, [4]);
        let mut t = unit_scada();
        t.columns[3].ty = LakeType::Date;
        let dated = with(t);
        assert_ne!(hash(&reg, &FOUR), hash(&dated, &FOUR));
        assert_eq!(hash(&reg, &THREE), hash(&dated, &THREE));
    }

    #[test]
    fn a_conforming_shape_plans_every_column() {
        let r = with(unit_scada());
        let p = r
            .resolve(&scada(&FOUR), ResolveOptions::default())
            .unwrap()
            .plan;
        assert_eq!(p.table, "DISPATCH_UNIT_SCADA");
        assert_eq!(p.directory, "dispatch_unit_scada");
        assert_eq!(p.load_mode, LoadMode::Append);
        assert!(p.missing.is_empty());
        assert_eq!(p.lake_width(), 4);
        assert_eq!(p.column_name(2), "SCADAVALUE");
        assert!(p.routes.iter().all(|r| r.origin == Origin::Declared));
        assert_eq!(p.routes[1].source_index, 1);
        assert_eq!(p.routes[1].target_name.as_deref(), Some("DUID"));
        assert!(p.fields.iter().all(|f| f.field_id.is_some()));
        assert_eq!(p.routes[2].target_field_id, p.fields[2].field_id);
        let json = serde_json::to_value(&*p).unwrap();
        assert_eq!(json["routes"][0]["cast"]["kind"], "timestamp_nanos");
        assert_eq!(json["routes"][0]["origin"], "declared");
        assert_eq!(json["schema"], 1);
        assert!(json.get("arrow").is_none());
        // A repeated column is kept beside the first, under a suffix.
        let twice = ["SETTLEMENTDATE", "DUID", "DUID", "DUID__2"];
        let q = r
            .resolve(&scada(&twice), ResolveOptions::default())
            .unwrap()
            .plan;
        assert_eq!(q.routes[2].target_name.as_deref(), Some("DUID__2"));
        assert_eq!(q.routes[3].target_name.as_deref(), Some("DUID__2__2"));
        assert_eq!(q.fields[4].origin, Origin::FileExtra);
        let casts = [
            (LakeType::Long, Cast::Int64),
            (LakeType::Boolean, Cast::Boolean),
            (LakeType::Date, Cast::Date32),
        ];
        let market = r.market(Market::Nem).unwrap();
        for (ty, cast) in casts {
            assert_eq!(Cast::for_type(ty, market), cast);
        }
    }
}
