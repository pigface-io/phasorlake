//! Lake types as Arrow types, and a table's lake schema.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};

use crate::file_model::{Column, LakeType, MarketDef, Table};

/// Parquet's field-id metadata key; Iceberg reads the same key.
pub const FIELD_ID_KEY: &str = "PARQUET:field_id";
/// On a raw text column: the id (or name) of the column whose values it holds.
pub const RAW_OF_KEY: &str = "phasor:raw-of";

impl LakeType {
    /// The Arrow type, with timestamps in `market`'s offset.
    pub fn arrow(self, market: &MarketDef) -> DataType {
        match self {
            Self::String => DataType::Utf8,
            Self::Double => DataType::Float64,
            Self::Long => DataType::Int64,
            Self::Boolean => DataType::Boolean,
            Self::Date => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(
                arrow_schema::TimeUnit::Microsecond,
                Some(Arc::from(market.arrow_tz())),
            ),
            Self::Decimal { precision, scale } => {
                DataType::Decimal128(precision, i8::try_from(scale).unwrap_or(i8::MAX))
            }
        }
    }
}

/// The Arrow field for a column, with its stable id in the metadata.
pub fn field(c: &Column, market: &MarketDef) -> Field {
    let md: HashMap<String, String> = [(FIELD_ID_KEY.to_owned(), c.id.0.to_string())].into();
    Field::new(&c.name, c.ty.arrow(market), c.nullable).with_metadata(md)
}

/// The lake schema of a table: every column it ever had, each with its `PARQUET:field_id`.
pub fn lake_schema(t: &Table, market: &MarketDef) -> SchemaRef {
    let fields: Vec<Field> = t.columns.iter().map(|c| field(c, market)).collect();
    let md: HashMap<String, String> = [
        ("phasor:table".to_owned(), t.name.clone()),
        ("phasor:market".to_owned(), t.market.to_string()),
    ]
    .into();
    Arc::new(Schema::new_with_metadata(fields, md))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::model::tests::unit_scada;
    use crate::file_model::{Market, TimestampFormat};

    fn nem() -> MarketDef {
        MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
            trading_day_start_secs: 14_400,
        }
    }

    #[test]
    fn lake_types_map_to_arrow() {
        let got: Vec<DataType> = LakeType::ALL.iter().map(|t| t.arrow(&nem())).collect();
        let want = [
            DataType::Utf8,
            DataType::Float64,
            DataType::Int64,
            DataType::Boolean,
            DataType::Date32,
            DataType::Timestamp(
                arrow_schema::TimeUnit::Microsecond,
                Some(Arc::from("+10:00")),
            ),
            DataType::Decimal128(15, 5),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn the_lake_schema_carries_ids() {
        let s = lake_schema(&unit_scada(), &nem());
        assert_eq!(s.fields().len(), 4);
        assert_eq!(s.field(2).metadata()[FIELD_ID_KEY], "3");
        assert!(s.field(0).is_nullable());
        assert_eq!(s.metadata()["phasor:table"], "DISPATCH_UNIT_SCADA");
        assert_eq!(s.metadata()["phasor:market"], "nem");
    }

    /// Every timestamp the registry produces is microseconds with its market's offset.
    #[test]
    fn every_timestamp_of_every_table_carries_its_market_offset() {
        let reg = crate::Registry::load_embedded();
        assert_eq!(reg.market(Market::Nem).unwrap().arrow_tz(), "+10:00");
        assert_eq!(reg.market(Market::Wem).unwrap().arrow_tz(), "+08:00");
        let mut timestamps = 0usize;
        let mut tables = 0usize;
        for (id, table) in reg.tables() {
            tables += 1;
            let tz = reg.market_of(id).arrow_tz();
            for f in reg.lake_schema(id).fields() {
                let at = format!("{}.{} is {}", table.name, f.name(), f.data_type());
                match f.data_type() {
                    DataType::Timestamp(unit, zone) => {
                        assert_eq!(*unit, arrow_schema::TimeUnit::Microsecond, "{at}");
                        assert_eq!(zone.as_deref(), Some(tz.as_str()), "{at}");
                        timestamps += 1;
                    }
                    DataType::Date64
                    | DataType::Time32(_)
                    | DataType::Time64(_)
                    | DataType::Duration(_)
                    | DataType::Interval(_) => panic!("{at}: not a lake type"),
                    _ => {}
                }
            }
        }
        assert!(
            tables > 100 && timestamps > tables,
            "{tables} tables, {timestamps} timestamps"
        );
        // The mapping itself, for every market the registry knows.
        for m in [Market::Nem, Market::Wem] {
            let def = reg.market(m).unwrap();
            let DataType::Timestamp(unit, Some(zone)) = LakeType::Timestamp.arrow(def) else {
                panic!("{m:?}: a timestamp without a zone");
            };
            assert_eq!(unit, arrow_schema::TimeUnit::Microsecond);
            assert_eq!(zone.as_ref(), def.arrow_tz());
        }
    }
}
