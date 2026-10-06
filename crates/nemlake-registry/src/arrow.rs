//! Lake types as Arrow types, and a table's lake schema.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};

use crate::file_model::{Column, LakeType, MarketDef, Table};

/// Parquet's field-id metadata key; Iceberg reads the same key.
pub const FIELD_ID_KEY: &str = "PARQUET:field_id";
/// On a column of values kept as text: the id (or, for a column of the
/// file only, the name) of the column whose values it holds.
pub const RAW_OF_KEY: &str = "nemlake:raw-of";

impl LakeType {
    /// The Arrow type, with timestamps in `market`'s offset.
    pub fn arrow(self, market: &MarketDef) -> DataType {
        match self {
            Self::String => DataType::Utf8,
            Self::Double => DataType::Float64,
            Self::Long => DataType::Int64,
            Self::Boolean => DataType::Boolean,
            Self::Date => DataType::Date32,
            Self::TimestampNs => DataType::Timestamp(
                arrow_schema::TimeUnit::Nanosecond,
                Some(Arc::from(market.arrow_tz())),
            ),
        }
    }
}

/// The Arrow field for a column, with its stable id in the metadata.
pub fn field(c: &Column, market: &MarketDef) -> Field {
    let md: HashMap<String, String> = [(FIELD_ID_KEY.to_owned(), c.id.0.to_string())].into();
    Field::new(&c.name, c.ty.arrow(market), c.nullable).with_metadata(md)
}

/// The lake schema of a table: every column it ever had, in lake order,
/// each with its id as `PARQUET:field_id`. Files of every increment share
/// it; a file that lacks a column writes nulls.
pub fn lake_schema(t: &Table, market: &MarketDef) -> SchemaRef {
    let fields: Vec<Field> = t.columns.iter().map(|c| field(c, market)).collect();
    let md: HashMap<String, String> = [
        ("nemlake:table".to_owned(), t.name.clone()),
        ("nemlake:market".to_owned(), t.market.to_string()),
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
                arrow_schema::TimeUnit::Nanosecond,
                Some(Arc::from("+10:00")),
            ),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn the_lake_schema_carries_ids() {
        let s = lake_schema(&unit_scada(), &nem());
        assert_eq!(s.fields().len(), 4);
        assert_eq!(s.field(2).metadata()[FIELD_ID_KEY], "3");
        assert!(!s.field(0).is_nullable());
        assert_eq!(s.metadata()["nemlake:table"], "DISPATCH_UNIT_SCADA");
        assert_eq!(s.metadata()["nemlake:market"], "nem");
    }
}
