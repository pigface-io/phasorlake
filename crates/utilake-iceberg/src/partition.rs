//! The partition spec: by month of the table's valid time.

use serde_json::{Value, json};

use crate::schema::Field;
use crate::types::IcebergType;

/// The first partition field id, as Iceberg numbers them.
const FIRST_PARTITION_FIELD_ID: u32 = 1000;

/// One partition field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionField {
    /// The column it is computed from.
    pub source_id: u32,
    /// Its own id, from 1000.
    pub field_id: u32,
    /// Its name.
    pub name: String,
    /// The transform, as Iceberg names it.
    pub transform: &'static str,
}

/// How a table's files are split.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PartitionSpec {
    /// The spec's id.
    pub spec_id: u32,
    /// The fields; none for a table without a time.
    pub fields: Vec<PartitionField>,
}

impl PartitionSpec {
    /// Partitioned by month of `time`, which must be a date or a time; none
    /// for any other column, or no column.
    pub fn by_month(time: Option<&Field>) -> Self {
        let fields = time
            .filter(|f| matches!(f.ty, IcebergType::Date | IcebergType::Timestamptz))
            .map(|f| PartitionField {
                source_id: f.id,
                field_id: FIRST_PARTITION_FIELD_ID,
                name: format!("{}_month", f.name.to_lowercase()),
                transform: "month",
            })
            .into_iter()
            .collect();
        Self { spec_id: 0, fields }
    }

    /// The spec as Iceberg table metadata writes it.
    pub fn to_json(&self) -> Value {
        json!({
            "spec-id": self.spec_id,
            "fields": self.fields.iter().map(|f| json!({
                "source-id": f.source_id,
                "field-id": f.field_id,
                "name": f.name,
                "transform": f.transform,
            })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(ty: IcebergType) -> Field {
        Field {
            id: 4,
            name: "SETTLEMENTDATE".into(),
            ty,
            required: true,
            doc: None,
        }
    }

    #[test]
    fn months_of_the_valid_time() {
        let spec = PartitionSpec::by_month(Some(&field(IcebergType::Timestamptz)));
        assert_eq!(
            spec.to_json(),
            json!({"spec-id": 0, "fields": [
                {"source-id": 4, "field-id": 1000, "name": "settlementdate_month", "transform": "month"}
            ]})
        );
        assert_eq!(
            PartitionSpec::by_month(Some(&field(IcebergType::Date)))
                .fields
                .len(),
            1
        );
        assert!(
            PartitionSpec::by_month(Some(&field(IcebergType::String)))
                .fields
                .is_empty()
        );
        assert_eq!(PartitionSpec::by_month(None), PartitionSpec::default());
    }
}
