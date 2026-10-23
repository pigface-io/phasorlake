//! Proving a query against a table: the schema it reads as, every role as
//! a position in it, the projection of each file, and the filters split
//! into those that may run before resolution and those that may not.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use utilake_core::{Category, Diagnostic};
use utilake_iceberg::{Column, DataFile, IcebergType, Target};
use utilake_mms_registry::file_model::ValidAxis;
use utilake_mms_registry::{LoadMode, TableId};

use crate::error::QueryError;
use crate::query::{
    Bound, BoundFilter, BoundSpec, FieldIx, FileRead, Projection, Query, Resolution, Unit,
};
use crate::time::{MarketOffset, ValidWindow};
use crate::window::plan_bounds;
use crate::{FIELD_ID_KEY, Lake};

/// The Arrow type of an Iceberg type, timestamps in the market's offset.
pub fn arrow_type(ty: IcebergType, tz: &str) -> DataType {
    match ty {
        IcebergType::Boolean => DataType::Boolean,
        IcebergType::Long => DataType::Int64,
        IcebergType::Double => DataType::Float64,
        IcebergType::Decimal { precision, scale } => {
            DataType::Decimal128(precision, i8::try_from(scale).unwrap_or(i8::MAX))
        }
        IcebergType::Date => DataType::Date32,
        IcebergType::Timestamptz => DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from(tz))),
        IcebergType::String => DataType::Utf8,
    }
}

/// Whether a column of this type can rank revisions.
const fn orderable(ty: &DataType) -> bool {
    matches!(
        ty,
        DataType::Int64
            | DataType::Float64
            | DataType::Decimal128(_, _)
            | DataType::Date32
            | DataType::Timestamp(_, _)
            | DataType::Utf8
    )
}

/// Binds a query to the table it names and the lake's files of it.
pub fn bind(query: Query, lake: &Lake<'_>) -> Result<Bound, QueryError> {
    let r = lake.registry;
    // A declared table by name; a table derived from a file by its report
    // pair, `TYPE.SUBTYPE`, as the lake's `_unregistered/` names it.
    let id = r
        .table_by_name(&query.table)
        .or_else(|| {
            let (rt, st) = query.table.split_once('.')?;
            r.table_by_pair(rt, st)
        })
        .ok_or_else(|| QueryError::UnknownTable(query.table.clone()))?;
    let table = r.table(id);
    let offset = MarketOffset(r.market_of(id).utc_offset_secs);
    let tz = offset.tz();
    let mut warnings = Vec::new();
    let files: Vec<(usize, &crate::LakeFile)> = lake
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.table == table.name)
        .collect();

    let Some(history) = r.history(id) else {
        return bind_raw(query, id, lake, &files, offset, warnings);
    };

    // The target schema.
    let schema_id = match query.target {
        Target::Latest => history.current_schema_id,
        Target::AsOf(s) => s,
    };
    let target = history.schema(schema_id).ok_or_else(|| QueryError::Plan {
        table: table.name.clone(),
        source: utilake_iceberg::PlanError::UnknownSchema(schema_id),
    })?;
    let fields: Vec<Field> = target
        .fields
        .iter()
        .map(|f| {
            let md: HashMap<String, String> = [(FIELD_ID_KEY.to_owned(), f.id.to_string())].into();
            Field::new(&f.name, arrow_type(f.ty, &tz), !f.required).with_metadata(md)
        })
        .collect();
    let schema: SchemaRef = Arc::new(Schema::new(fields));
    let position = |field_id: u32| {
        target
            .fields
            .iter()
            .position(|f| f.id == field_id)
            .map(FieldIx)
    };

    // The roles, as positions in the target.
    let roles = table
        .temporal
        .check(table)
        .map_err(|problems| QueryError::Roles {
            table: table.name.clone(),
            problems,
        })?;
    let role_ix = |role: &'static str, cid: utilake_mms_registry::file_model::ColumnId| {
        position(cid.0).ok_or_else(|| QueryError::RoleMissing {
            table: table.name.clone(),
            schema: schema_id,
            role,
            column: table
                .column_by_id(cid)
                .map_or_else(|| cid.to_string(), |c| c.name.clone()),
        })
    };
    let entity = roles
        .entity
        .iter()
        .map(|c| role_ix("entity", *c))
        .collect::<Result<Vec<_>, _>>()?;
    let valid = match &roles.valid {
        ValidAxis::Timeless => ValidAxis::Timeless,
        ValidAxis::IntervalEnd(c) => ValidAxis::IntervalEnd(role_ix("valid", *c)?),
        ValidAxis::Instant(c) => ValidAxis::Instant(role_ix("valid", *c)?),
        ValidAxis::From(c) => ValidAxis::From(role_ix("valid", *c)?),
    };
    let order = roles
        .order
        .iter()
        .map(|c| role_ix("order", *c))
        .collect::<Result<Vec<_>, _>>()?;
    let cutoff = roles.cutoff.map(|c| role_ix("cutoff", c)).transpose()?;
    for (role, ix) in order
        .iter()
        .map(|i| ("order", *i))
        .chain(cutoff.map(|i| ("cutoff", i)))
    {
        let f = schema.field(ix.0);
        if !orderable(f.data_type()) {
            return Err(QueryError::RoleType {
                table: table.name.clone(),
                role,
                column: f.name().clone(),
                ty: f.data_type().to_string(),
            });
        }
    }

    // The window on this axis.
    if let (ValidAxis::IntervalEnd(c), ValidWindow::At(_)) = (&valid, &query.window) {
        return Err(QueryError::AtOnIntervalTable {
            table: table.name.clone(),
            column: schema.field(c.0).name().clone(),
        });
    }
    if matches!(valid, ValidAxis::Timeless) {
        warnings.push(
            Diagnostic::warning(
                "query.window_ignored",
                Category::Query,
                format!(
                    "Table {} has no valid time, so every row is in every window.",
                    table.name
                ),
            )
            .with_table(table.name.clone()),
        );
    }
    let time = table
        .time_column
        .as_deref()
        .and_then(|n| schema.index_of(n).ok())
        .map(FieldIx);
    let unit = if table.load_mode == LoadMode::Append
        && valid.column().is_some_and(|c| Some(*c) == time)
    {
        Unit::Partition
    } else {
        Unit::Table
    };
    let collapse_all = entity.is_empty() && matches!(valid, ValidAxis::Timeless);
    let spec = BoundSpec {
        entity,
        valid,
        order,
        cutoff,
        unit,
        collapse_all,
    };

    // The files, each projected onto the target.
    let bounds = plan_bounds(Some(&spec.valid), &query.window);
    let mut reads = Vec::new();
    for (ix, f) in &files {
        let df = DataFile {
            path: f.path.clone(),
            schema_id: f.schema_id.unwrap_or(history.current_schema_id),
            min_us: f.min_us,
            max_us: f.max_us,
        };
        let planned =
            utilake_iceberg::plan(history, std::slice::from_ref(&df), bounds, query.target)
                .map_err(|source| QueryError::Plan {
                    table: table.name.clone(),
                    source,
                })?;
        if let Some(read) = planned.into_iter().next() {
            reads.push(FileRead {
                file: *ix,
                columns: read.columns,
            });
        }
    }

    let (entity_filters, value_filters, keep) =
        filters_and_keep(&query, &table.name, &schema, &spec, &mut warnings)?;
    Ok(narrow(Bound {
        query,
        table: table.name.clone(),
        schema_id: Some(schema_id),
        schema,
        spec: Some(spec),
        time,
        files: reads,
        entity_filters,
        value_filters,
        keep,
        offset,
        warnings,
    }))
}

/// A read that returns some columns reads only those, the roles, the
/// time and the filtered columns: the schema and every position in it
/// shrink to that set, in the target's order.
fn narrow(b: Bound) -> Bound {
    let Some(keep) = &b.keep else { return b };
    let mut needed: Vec<usize> = keep.iter().map(|i| i.0).collect();
    if let Some(s) = &b.spec {
        needed.extend(s.entity.iter().map(|i| i.0));
        needed.extend(s.valid.column().map(|i| i.0));
        needed.extend(s.order.iter().map(|i| i.0));
        needed.extend(s.cutoff.map(|i| i.0));
    }
    needed.extend(b.time.map(|i| i.0));
    needed.extend(
        b.entity_filters
            .iter()
            .chain(&b.value_filters)
            .map(|f| f.column.0),
    );
    needed.sort_unstable();
    needed.dedup();
    if needed.len() == b.schema.fields().len() {
        return b;
    }
    let at = |i: FieldIx| FieldIx(needed.binary_search(&i.0).expect("every role is needed"));
    let fields: Vec<Field> = needed.iter().map(|i| b.schema.field(*i).clone()).collect();
    let remap = |f: &BoundFilter| BoundFilter {
        column: at(f.column),
        values: f.values.clone(),
    };
    Bound {
        schema: Arc::new(Schema::new(fields)),
        spec: b.spec.as_ref().map(|s| BoundSpec {
            entity: s.entity.iter().map(|i| at(*i)).collect(),
            valid: s.valid.clone().map(at),
            order: s.order.iter().map(|i| at(*i)).collect(),
            cutoff: s.cutoff.map(at),
            unit: s.unit,
            collapse_all: s.collapse_all,
        }),
        time: b.time.map(at),
        files: b
            .files
            .iter()
            .map(|fr| FileRead {
                file: fr.file,
                columns: needed.iter().map(|i| fr.columns[*i].clone()).collect(),
            })
            .collect(),
        entity_filters: b.entity_filters.iter().map(remap).collect(),
        value_filters: b.value_filters.iter().map(remap).collect(),
        keep: Some(keep.iter().map(|i| at(*i)).collect()),
        ..b
    }
}

/// A table without a history: every column as the registry derived it
/// from the files, read raw, bounded on its time column if it has one.
fn bind_raw(
    query: Query,
    id: TableId,
    lake: &Lake<'_>,
    files: &[(usize, &crate::LakeFile)],
    offset: MarketOffset,
    warnings: Vec<Diagnostic>,
) -> Result<Bound, QueryError> {
    let table = lake.registry.table(id);
    if query.resolution != Resolution::Raw {
        return Err(QueryError::RawOnly(table.name.clone()));
    }
    let schema = lake.registry.lake_schema(id).clone();
    let time = table
        .time_column
        .as_deref()
        .and_then(|n| schema.index_of(n).ok())
        .map(FieldIx);
    if time.is_none() && matches!(query.window, ValidWindow::At(_)) {
        return Err(QueryError::AtOnIntervalTable {
            table: table.name.clone(),
            column: String::new(),
        });
    }
    let bounds = plan_bounds(None, &query.window);
    let columns: Vec<Column> = schema
        .fields()
        .iter()
        .map(|f| Column::Read {
            source: f.name().clone(),
            name: f.name().clone(),
            cast: None,
        })
        .collect();
    let reads = files
        .iter()
        .filter(|(_, f)| {
            time.is_none()
                || (f.max_us.is_none_or(|m| m >= bounds.0) && f.min_us.is_none_or(|m| m < bounds.1))
        })
        .map(|(ix, _)| FileRead {
            file: *ix,
            columns: columns.clone(),
        })
        .collect();
    let mut warnings = warnings;
    let spec = BoundSpec {
        entity: Vec::new(),
        valid: ValidAxis::Timeless,
        order: Vec::new(),
        cutoff: None,
        unit: Unit::Table,
        collapse_all: true,
    };
    let (entity_filters, value_filters, keep) =
        filters_and_keep(&query, &table.name, &schema, &spec, &mut warnings)?;
    Ok(narrow(Bound {
        query,
        table: table.name.clone(),
        schema_id: None,
        schema,
        spec: None,
        time,
        files: reads,
        entity_filters,
        value_filters,
        keep,
        offset,
        warnings,
    }))
}

/// Filters by column, split by whether the column is an entity, and the
/// projection as positions.
#[allow(clippy::type_complexity)]
fn filters_and_keep(
    query: &Query,
    table: &str,
    schema: &SchemaRef,
    spec: &BoundSpec,
    warnings: &mut Vec<Diagnostic>,
) -> Result<(Vec<BoundFilter>, Vec<BoundFilter>, Option<Vec<FieldIx>>), QueryError> {
    let find = |name: &str| {
        schema
            .index_of(name)
            .map(FieldIx)
            .map_err(|_| QueryError::UnknownColumn {
                table: table.to_owned(),
                column: name.to_owned(),
            })
    };
    let mut entity_filters = Vec::new();
    let mut value_filters = Vec::new();
    for f in &query.filters {
        let column = find(&f.column)?;
        let ty = schema.field(column.0).data_type();
        for v in &f.values {
            if !crate::scan::reads_as(v, ty) {
                return Err(QueryError::BadFilterValue {
                    column: f.column.clone(),
                    ty: ty.to_string(),
                    value: v.clone(),
                });
            }
        }
        let bound = BoundFilter {
            column,
            values: f.values.clone(),
        };
        if spec.entity.contains(&column) || query.resolution == Resolution::Raw {
            entity_filters.push(bound);
        } else {
            warnings.push(
                Diagnostic::warning(
                    "query.filter_after_resolution",
                    Category::Query,
                    format!(
                        "The filter on {} runs after resolution: it is not an entity column, so filtering first could change which revision wins.",
                        f.column
                    ),
                )
                .with_table(table.to_owned())
                .with_column(f.column.clone()),
            );
            value_filters.push(bound);
        }
    }
    let keep = match &query.columns {
        Projection::All => None,
        Projection::Columns(names) => Some(
            names
                .iter()
                .map(|n| find(n))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    Ok((entity_filters, value_filters, keep))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_maps_and_only_some_order() {
        let tz = "+10:00";
        assert_eq!(arrow_type(IcebergType::Boolean, tz), DataType::Boolean);
        assert_eq!(arrow_type(IcebergType::Long, tz), DataType::Int64);
        assert_eq!(arrow_type(IcebergType::Double, tz), DataType::Float64);
        assert_eq!(
            arrow_type(
                IcebergType::Decimal {
                    precision: 5,
                    scale: 2
                },
                tz
            ),
            DataType::Decimal128(5, 2)
        );
        assert_eq!(arrow_type(IcebergType::Date, tz), DataType::Date32);
        assert_eq!(arrow_type(IcebergType::String, tz), DataType::Utf8);
        assert!(matches!(
            arrow_type(IcebergType::Timestamptz, tz),
            DataType::Timestamp(TimeUnit::Microsecond, Some(_))
        ));
        assert!(orderable(&DataType::Int64));
        assert!(orderable(&DataType::Date32));
        assert!(!orderable(&DataType::Boolean));
        assert!(!orderable(&DataType::Binary));
    }
}
