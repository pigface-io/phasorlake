//! Reading any interval of a table across its schemas.
//!
//! A table's files were written under different schemas. A read of a time
//! interval takes the files whose data overlaps it and projects each onto
//! one target schema by field id, as an Iceberg reader does: a field the
//! file's schema has is read under the name it has there and cast when the
//! target's type is a promotion of it; a field it lacks is null of the
//! target's type; a field the target lacks is left out. The target is the
//! table's latest schema, or the schema of a point in its history, so a
//! loader written against that schema reads every file the same way.
//!
//! [`duckdb_sql`] writes the plan as one DuckDB query over the Parquet
//! files: one `SELECT` per group of files with the same projection, joined
//! by `UNION ALL`, each bounded to the interval on the time column.

use crate::schema::Schema;
use crate::types::IcebergType;

/// A data file of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFile {
    /// Where it is.
    pub path: String,
    /// The schema it was written under.
    pub schema_id: u32,
    /// Its least time value, microseconds since 1970 (UTC).
    pub min_us: Option<i64>,
    /// Its greatest.
    pub max_us: Option<i64>,
}

/// The schema a read projects onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The table's latest schema.
    Latest,
    /// The schema with this id.
    AsOf(u32),
}

/// One column of a projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Column {
    /// Read from the file.
    Read {
        /// Its name in the file.
        source: String,
        /// Its name in the target.
        name: String,
        /// The target's type, when it is a promotion of the file's.
        cast: Option<IcebergType>,
    },
    /// Not in the file: null of the target's type.
    Null {
        /// Its name in the target.
        name: String,
        /// The target's type.
        ty: IcebergType,
    },
}

/// Files read with one projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The files.
    pub paths: Vec<String>,
    /// The target's columns, in its order.
    pub columns: Vec<Column>,
}

/// Why a read cannot be planned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The target or a file names a schema the table lacks.
    #[error("The table has no schema {0}.")]
    UnknownSchema(u32),
    /// A field's type in the target is not one Iceberg reaches from the file's.
    #[error(
        "Column {name} is {from} in schema {schema} and {to} in the target; Iceberg does not promote one to the other."
    )]
    NotAPromotion {
        /// The column.
        name: String,
        /// The file's schema.
        schema: u32,
        /// Its type there.
        from: IcebergType,
        /// The target's.
        to: IcebergType,
    },
}

fn schema(schemas: &[Schema], id: u32) -> Result<&Schema, PlanError> {
    schemas
        .iter()
        .find(|s| s.schema_id == id)
        .ok_or(PlanError::UnknownSchema(id))
}

/// The projection of a file of schema `from` onto `to`.
fn project(from: &Schema, to: &Schema) -> Result<Vec<Column>, PlanError> {
    to.fields
        .iter()
        .map(|f| match from.field(f.id) {
            None => Ok(Column::Null {
                name: f.name.clone(),
                ty: f.ty,
            }),
            Some(g) if g.ty == f.ty => Ok(Column::Read {
                source: g.name.clone(),
                name: f.name.clone(),
                cast: None,
            }),
            Some(g) if g.ty.promotes_to(f.ty) => Ok(Column::Read {
                source: g.name.clone(),
                name: f.name.clone(),
                cast: Some(f.ty),
            }),
            Some(g) => Err(PlanError::NotAPromotion {
                name: f.name.clone(),
                schema: from.schema_id,
                from: g.ty,
                to: f.ty,
            }),
        })
        .collect()
}

/// The reads of the files whose data overlaps `[t0, t1)` (microseconds),
/// projected onto `target`. A file with no time bounds is always read.
/// Files keep their order; consecutive files with the same projection share
/// a read.
pub fn plan(
    schemas: &[Schema],
    files: &[DataFile],
    (t0, t1): (i64, i64),
    target: Target,
) -> Result<Vec<Read>, PlanError> {
    let to = match target {
        Target::AsOf(id) => schema(schemas, id)?,
        Target::Latest => schemas
            .iter()
            .max_by_key(|s| s.schema_id)
            .ok_or(PlanError::UnknownSchema(0))?,
    };
    let mut out: Vec<Read> = Vec::new();
    for f in files {
        let overlaps = f.max_us.is_none_or(|m| m >= t0) && f.min_us.is_none_or(|m| m < t1);
        if !overlaps {
            continue;
        }
        let columns = project(schema(schemas, f.schema_id)?, to)?;
        match out.last_mut() {
            Some(r) if r.columns == columns => r.paths.push(f.path.clone()),
            _ => out.push(Read {
                paths: vec![f.path.clone()],
                columns,
            }),
        }
    }
    Ok(out)
}

/// DuckDB's name for a type.
fn duck_type(t: IcebergType) -> String {
    match t {
        IcebergType::Boolean => "BOOLEAN".into(),
        IcebergType::Long => "BIGINT".into(),
        IcebergType::Double => "DOUBLE".into(),
        IcebergType::Decimal { precision, scale } => format!("DECIMAL({precision}, {scale})"),
        IcebergType::Date => "DATE".into(),
        IcebergType::Timestamptz => "TIMESTAMPTZ".into(),
        IcebergType::String => "VARCHAR".into(),
    }
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// An instant as DuckDB reads it: microseconds since 1970 in UTC.
fn instant(us: i64) -> String {
    format!("make_timestamptz({us})")
}

/// The reads as one DuckDB query; `time` is the column the interval bounds
/// (named as in the target), none for a table without one.
pub fn duckdb_sql(reads: &[Read], time: Option<&str>, (t0, t1): (i64, i64)) -> String {
    let selects: Vec<String> = reads
        .iter()
        .map(|r| {
            let cols: Vec<String> = r
                .columns
                .iter()
                .map(|c| match c {
                    Column::Read { source, name, cast } => {
                        let v = match cast {
                            Some(t) => format!("CAST({} AS {})", ident(source), duck_type(*t)),
                            None => ident(source),
                        };
                        format!("{v} AS {}", ident(name))
                    }
                    Column::Null { name, ty } => {
                        format!("CAST(NULL AS {}) AS {}", duck_type(*ty), ident(name))
                    }
                })
                .collect();
            let files: Vec<String> = r.paths.iter().map(|p| literal(p)).collect();
            let source_time = time.and_then(|t| {
                r.columns.iter().find_map(|c| match c {
                    Column::Read { source, name, .. } if name == t => Some(source.clone()),
                    _ => None,
                })
            });
            let filter = source_time.map_or_else(String::new, |s| {
                format!(
                    "\nWHERE {} >= {} AND {} < {}",
                    ident(&s),
                    instant(t0),
                    ident(&s),
                    instant(t1)
                )
            });
            format!(
                "SELECT {}\nFROM read_parquet([{}]){filter}",
                cols.join(", "),
                files.join(", ")
            )
        })
        .collect();
    selects.join("\nUNION ALL\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Field;

    fn field(id: u32, name: &str, ty: IcebergType) -> Field {
        Field {
            id,
            name: name.into(),
            ty,
            required: false,
            doc: None,
        }
    }

    fn dec(p: u8) -> IcebergType {
        IcebergType::decimal(p, 5).unwrap()
    }

    /// A: time, rrp decimal(15, 5); B: rrp widened, raise added before it;
    /// C: raise left out again.
    fn schemas() -> Vec<Schema> {
        let t = field(1, "SETTLEMENTDATE", IcebergType::Timestamptz);
        let schema = |id, fields| Schema {
            schema_id: id,
            fields,
            identifier_field_ids: vec![1],
            last_column_id: 3,
        };
        vec![
            schema(0, vec![t.clone(), field(2, "RRP", dec(15))]),
            schema(
                1,
                vec![
                    t.clone(),
                    field(3, "RAISE", IcebergType::Double),
                    field(2, "RRP", dec(18)),
                ],
            ),
            schema(2, vec![t, field(2, "RRP", dec(18))]),
        ]
    }

    fn file(path: &str, schema_id: u32, min: i64, max: i64) -> DataFile {
        DataFile {
            path: path.into(),
            schema_id,
            min_us: Some(min),
            max_us: Some(max),
        }
    }

    #[test]
    fn every_file_reads_through_the_target() {
        let files = [
            file("a.parquet", 0, 0, 9),
            file("a2.parquet", 0, 10, 19),
            file("b.parquet", 1, 20, 29),
            file("c.parquet", 2, 30, 39),
            DataFile {
                path: "timeless.parquet".into(),
                schema_id: 2,
                min_us: None,
                max_us: None,
            },
        ];
        let reads = plan(&schemas(), &files, (5, 35), Target::AsOf(1)).unwrap();
        let paths: Vec<Vec<&str>> = reads
            .iter()
            .map(|r| r.paths.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(
            paths,
            [
                vec!["a.parquet", "a2.parquet"],
                vec!["b.parquet"],
                vec!["c.parquet", "timeless.parquet"]
            ]
        );
        // A file of schema A: RAISE is null, RRP widened.
        assert_eq!(
            reads[0].columns,
            [
                Column::Read {
                    source: "SETTLEMENTDATE".into(),
                    name: "SETTLEMENTDATE".into(),
                    cast: None
                },
                Column::Null {
                    name: "RAISE".into(),
                    ty: IcebergType::Double
                },
                Column::Read {
                    source: "RRP".into(),
                    name: "RRP".into(),
                    cast: Some(dec(18))
                },
            ]
        );
        // Outside the interval, nothing; the latest schema leaves RAISE out.
        assert!(
            plan(&schemas(), &files[..2], (100, 200), Target::Latest)
                .unwrap()
                .is_empty()
        );
        let latest = plan(&schemas(), &files[2..3], (0, 100), Target::Latest).unwrap();
        assert_eq!(latest[0].columns.len(), 2);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let s = schemas();
        assert_eq!(
            plan(&s, &[], (0, 1), Target::AsOf(9)),
            Err(PlanError::UnknownSchema(9))
        );
        assert_eq!(
            plan(&[], &[], (0, 1), Target::Latest),
            Err(PlanError::UnknownSchema(0))
        );
        let stray = [file("x.parquet", 7, 0, 1)];
        assert_eq!(
            plan(&s, &stray, (0, 2), Target::Latest),
            Err(PlanError::UnknownSchema(7))
        );
        // A narrower target than a file's type: no promotion reaches it.
        let wide = [file("b.parquet", 1, 0, 1)];
        let e = plan(&s, &wide, (0, 2), Target::AsOf(0)).unwrap_err();
        assert!(matches!(e, PlanError::NotAPromotion { ref name, schema: 1, .. } if name == "RRP"));
        assert!(e.to_string().contains("does not promote"), "{e}");
    }

    #[test]
    fn writes_one_duckdb_query() {
        let files = [file("a.parquet", 0, 0, 9), file("it's.parquet", 1, 20, 29)];
        let reads = plan(&schemas(), &files, (0, 100), Target::AsOf(1)).unwrap();
        let sql = duckdb_sql(&reads, Some("SETTLEMENTDATE"), (0, 100));
        assert_eq!(
            sql,
            "SELECT \"SETTLEMENTDATE\" AS \"SETTLEMENTDATE\", CAST(NULL AS DOUBLE) AS \"RAISE\", CAST(\"RRP\" AS DECIMAL(18, 5)) AS \"RRP\"\n\
             FROM read_parquet(['a.parquet'])\n\
             WHERE \"SETTLEMENTDATE\" >= make_timestamptz(0) AND \"SETTLEMENTDATE\" < make_timestamptz(100)\n\
             UNION ALL\n\
             SELECT \"SETTLEMENTDATE\" AS \"SETTLEMENTDATE\", \"RAISE\" AS \"RAISE\", \"RRP\" AS \"RRP\"\n\
             FROM read_parquet(['it''s.parquet'])\n\
             WHERE \"SETTLEMENTDATE\" >= make_timestamptz(0) AND \"SETTLEMENTDATE\" < make_timestamptz(100)"
        );
        // No time column: no bound.
        assert!(!duckdb_sql(&reads, None, (0, 100)).contains("WHERE"));
        assert!(!duckdb_sql(&reads, Some("NONE"), (0, 100)).contains("WHERE"));
        let all = [
            IcebergType::Boolean,
            IcebergType::Long,
            IcebergType::Date,
            IcebergType::String,
            IcebergType::Timestamptz,
        ]
        .map(duck_type);
        assert_eq!(all, ["BOOLEAN", "BIGINT", "DATE", "VARCHAR", "TIMESTAMPTZ"]);
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
    }
}
