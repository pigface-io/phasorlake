//! A table's columns as `phasor list TABLE` and the Python `table()` name
//! them: the table's own, each with the data model's words for it, then the
//! columns the lake derives from each row, in the order of the lake schema
//! ([`crate::arrow::lake_schema`]), so a listing and the schema agree.

use std::collections::BTreeMap;

use crate::arrow::DERIVED_FIELD_ID_BASE;
use crate::file_model::{DeriveRule, Derived, Table};

/// What a column is to a reader, as far as the registry says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnRole {
    /// The table's time column, which partitions its files.
    Time,
    /// A column the lake derives from each row, never in the file.
    Derived,
    /// Any other column.
    Plain,
}

impl ColumnRole {
    /// The word a listing prints: `time`, `derived`, or nothing.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Derived => "derived",
            Self::Plain => "",
        }
    }
}

/// One column of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedColumn {
    /// Its name.
    pub name: String,
    /// Its lake type, as the registry writes it (`decimal(15,5)`, `long`).
    pub ty: String,
    /// What it is to a reader.
    pub role: ColumnRole,
    /// Its Parquet field id.
    pub field_id: u32,
    /// Whether it may be null (every column may).
    pub nullable: bool,
    /// The data model's words for it, or what a derived column holds.
    pub description: Option<String>,
}

/// Every column of `t`, its own then the derived, with `docs` (the data
/// model's words by field id) on its own.
#[must_use]
pub fn listed_columns(t: &Table, docs: &BTreeMap<u32, String>) -> Vec<ListedColumn> {
    let own = t.columns.iter().map(|c| ListedColumn {
        name: c.name.clone(),
        ty: c.ty.to_string(),
        role: if t.time_column.as_deref() == Some(c.name.as_str()) {
            ColumnRole::Time
        } else {
            ColumnRole::Plain
        },
        field_id: c.id.0,
        nullable: c.nullable,
        description: docs.get(&c.id.0).cloned(),
    });
    let derived = t.derived.iter().enumerate().map(|(i, d)| ListedColumn {
        name: d.column.clone(),
        ty: "long".to_owned(),
        role: ColumnRole::Derived,
        field_id: DERIVED_FIELD_ID_BASE
            + u32::try_from(i).expect("fewer derived columns than that"),
        nullable: true,
        description: Some(describe(d)),
    });
    own.chain(derived).collect()
}

/// What a derived column holds, in a sentence.
fn describe(d: &Derived) -> String {
    match d.rule {
        DeriveRule::UtcUs => format!(
            "{} as microseconds since 1970 UTC, the same instant whatever clock it is displayed in",
            d.from.join(", ")
        ),
        DeriveRule::NzTradingPeriodStart => format!(
            "the start of the trading period {} name, as microseconds since 1970 UTC, through New Zealand daylight saving",
            d.from.join(" and ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::model::tests::unit_scada;

    #[test]
    fn a_listing_is_the_lake_schema_in_order_with_roles_and_words() {
        let mut t = unit_scada();
        t.derived = vec![
            Derived {
                column: "SETTLEMENTDATE_UTC_US".into(),
                rule: DeriveRule::UtcUs,
                from: vec!["SETTLEMENTDATE".into()],
            },
            Derived {
                column: "INTERVAL_START_UTC_US".into(),
                rule: DeriveRule::NzTradingPeriodStart,
                from: vec!["TRADINGDATE".into(), "TRADINGPERIOD".into()],
            },
        ];
        let docs: BTreeMap<u32, String> = [(3, "the reading".to_owned())].into();
        let cols = listed_columns(&t, &docs);
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names.len(), t.columns.len() + 2);
        assert_eq!(names[0], "SETTLEMENTDATE");
        assert_eq!(cols[0].role, ColumnRole::Time);
        assert_eq!(cols[0].role.as_str(), "time");
        assert_eq!(cols[1].role.as_str(), "");
        assert_eq!(cols[2].description.as_deref(), Some("the reading"));
        let d = &cols[names.len() - 2];
        assert_eq!(
            (
                d.name.as_str(),
                d.ty.as_str(),
                d.role,
                d.field_id,
                d.nullable
            ),
            (
                "SETTLEMENTDATE_UTC_US",
                "long",
                ColumnRole::Derived,
                DERIVED_FIELD_ID_BASE,
                true
            )
        );
        assert!(
            d.description
                .as_deref()
                .unwrap()
                .starts_with("SETTLEMENTDATE as microseconds")
        );
        let last = cols.last().unwrap();
        assert_eq!(last.field_id, DERIVED_FIELD_ID_BASE + 1);
        assert!(
            last.description
                .as_deref()
                .unwrap()
                .contains("TRADINGDATE and TRADINGPERIOD")
        );
        // The listing's names are the lake schema's.
        let schema = crate::arrow::lake_schema(
            &t,
            &crate::file_model::MarketDef {
                market: crate::file_model::Market::Nem,
                utc_offset_secs: 36_000,
                timestamp_format: crate::file_model::TimestampFormat::AemoSlash,
                trading_day_start_secs: 14_400,
            },
        );
        let schema_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(schema_names, names);
    }
}
