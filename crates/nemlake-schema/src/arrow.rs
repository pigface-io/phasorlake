//! Mapping logical column types to Arrow.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use nemlake_schema_model::{ColumnDef, ColumnType, MarketDef, TableDef};

use crate::registry::TypeOptions;

/// Parquet's field-id metadata key; Iceberg reads the same key.
pub const FIELD_ID_KEY: &str = "PARQUET:field_id";
/// The Oracle type the column was declared with, for provenance.
pub const ORACLE_KEY: &str = "nemlake:oracle";

/// The Arrow type for a column under the given options.
///
/// `Number` becomes `Float64` unless decimals are requested and the schema
/// knows precision and scale; a decimal request for a column without them
/// falls back to `Float64` and the caller may warn.
pub fn data_type(ty: ColumnType, market: &MarketDef, opts: TypeOptions) -> DataType {
    match ty {
        ColumnType::Utf8 { .. } => DataType::Utf8,
        ColumnType::Number {
            precision: Some(p),
            scale: Some(s),
        } if opts.decimal => DataType::Decimal128(p, s as i8),
        ColumnType::Number { .. } => DataType::Float64,
        ColumnType::Integer => DataType::Int64,
        ColumnType::Timestamp => {
            DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from(market.arrow_tz())))
        }
        ColumnType::Date => DataType::Date32,
        ColumnType::Boolean => DataType::Boolean,
    }
}

/// Whether a decimal was requested but this column has to fall back.
pub fn decimal_fallback(ty: ColumnType, opts: TypeOptions) -> bool {
    opts.decimal
        && matches!(ty, ColumnType::Number { precision, scale } if precision.is_none() || scale.is_none())
}

/// The Arrow field for a column, with its stable id in the metadata.
pub fn field(c: &ColumnDef, market: &MarketDef, opts: TypeOptions) -> Field {
    let mut md = HashMap::with_capacity(2);
    md.insert(FIELD_ID_KEY.to_owned(), c.id.0.to_string());
    if let Some(o) = &c.oracle {
        md.insert(ORACLE_KEY.to_owned(), o.clone());
    }
    Field::new(&c.name, data_type(c.ty, market, opts), c.nullable).with_metadata(md)
}

/// The unified lake schema of a table: every column ever declared, in lake
/// order. Files of every version share it; a version that lacks a column
/// writes nulls.
pub fn lake_schema(t: &TableDef, market: &MarketDef, opts: TypeOptions) -> SchemaRef {
    let fields: Vec<Field> = t.columns.iter().map(|c| field(c, market, opts)).collect();
    let mut md = HashMap::new();
    md.insert("nemlake:table".to_owned(), t.name.clone());
    md.insert("nemlake:market".to_owned(), t.market.to_string());
    Arc::new(Schema::new_with_metadata(fields, md))
}

#[cfg(test)]
mod tests {
    use nemlake_schema_model::{Market, TimestampFormat};

    use super::*;

    fn nem() -> MarketDef {
        MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
        }
    }

    #[test]
    fn maps_types() {
        let f64_opts = TypeOptions { decimal: false };
        let dec = TypeOptions { decimal: true };
        let num = ColumnType::Number {
            precision: Some(15),
            scale: Some(5),
        };
        assert_eq!(data_type(num, &nem(), f64_opts), DataType::Float64);
        assert_eq!(data_type(num, &nem(), dec), DataType::Decimal128(15, 5));
        let bare = ColumnType::Number {
            precision: None,
            scale: None,
        };
        assert_eq!(data_type(bare, &nem(), dec), DataType::Float64);
        assert!(decimal_fallback(bare, dec));
        assert!(!decimal_fallback(num, dec));
        assert!(!decimal_fallback(bare, f64_opts));
        assert_eq!(data_type(ColumnType::Integer, &nem(), dec), DataType::Int64);
        assert_eq!(
            data_type(
                ColumnType::Utf8 {
                    max_len: Some(3),
                    dict: true
                },
                &nem(),
                dec
            ),
            DataType::Utf8
        );
        assert_eq!(data_type(ColumnType::Date, &nem(), dec), DataType::Date32);
        assert_eq!(
            data_type(ColumnType::Boolean, &nem(), dec),
            DataType::Boolean
        );
        assert_eq!(
            data_type(ColumnType::Timestamp, &nem(), dec),
            DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from("+10:00")))
        );
    }
}
