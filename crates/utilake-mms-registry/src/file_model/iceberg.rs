//! A table as an Iceberg schema and partition spec (`utilake_iceberg`).

use utilake_iceberg::{Field, PartitionSpec, Schema, Update, UpdateError};

use super::model::Table;

impl Table {
    /// The table's Iceberg schema: each column a field with its id, type
    /// and nullability, the primary key as identifier fields. A key Iceberg
    /// refuses (a nullable or float column) is left out and its reason
    /// returned beside the schema, so the table still reads.
    pub fn iceberg_schema(&self) -> (Schema, Option<UpdateError>) {
        let fields: Vec<Field> = self
            .columns
            .iter()
            .map(|c| Field {
                id: c.id.0,
                name: c.name.clone(),
                ty: c.ty.iceberg(),
                required: !c.nullable,
                doc: c.note.clone(),
            })
            .collect();
        let schema = Schema {
            schema_id: 0,
            last_column_id: self.next_column_id.saturating_sub(1),
            fields,
            identifier_field_ids: Vec::new(),
        };
        let ids = self
            .primary_key
            .iter()
            .filter_map(|k| self.column(k).map(|c| c.id.0))
            .collect();
        match schema.apply(&Update::SetIdentifierFields { ids }) {
            Ok(mut keyed) => {
                keyed.schema_id = 0;
                (keyed, None)
            }
            Err(e) => (schema, Some(e)),
        }
    }

    /// Partitioned by month of the table's time column.
    pub fn partition_spec(&self) -> PartitionSpec {
        let (schema, _) = self.iceberg_schema();
        PartitionSpec::by_month(self.time_column.as_deref().and_then(|t| schema.by_name(t)))
    }
}

#[cfg(test)]
mod tests {
    use crate::file_model::model::tests::unit_scada;

    #[test]
    fn a_table_is_an_iceberg_schema_with_its_key_and_month_partitions() {
        let mut t = unit_scada();
        t.columns[2].note = Some("MW".into());
        let (s, issue) = t.iceberg_schema();
        assert!(issue.is_none(), "{issue:?}");
        assert_eq!((s.schema_id, s.last_column_id), (0, 4));
        assert_eq!(s.identifier_field_ids, [1, 2]);
        assert_eq!(s.field(3).unwrap().doc.as_deref(), Some("MW"));
        assert!(!s.field(3).unwrap().required);
        let spec = t.partition_spec().to_json();
        assert_eq!(spec["fields"][0]["source-id"], 1);
        assert_eq!(spec["fields"][0]["transform"], "month");
    }

    #[test]
    fn a_key_iceberg_refuses_is_reported_and_left_out() {
        let mut t = unit_scada();
        t.primary_key = vec!["SETTLEMENTDATE".into(), "SCADAVALUE".into()];
        t.columns[2].nullable = false;
        let (s, issue) = t.iceberg_schema();
        assert!(s.identifier_field_ids.is_empty());
        assert!(issue.unwrap().to_string().contains("float"));
    }
}
