//! The same read as one SQL query: an independent statement of the
//! roles in standard SQL, which the DuckDB test runs against the kernel.
//!
//! ```sql
//! WITH raw AS (<the rows, each with its _ul_source and the three times its source was stamped with>),
//!      stamped AS (SELECT raw.*, greatest("LASTCHANGED", _ul_server_last_modified) AS _ul_known_from FROM raw),
//!      known AS (SELECT * FROM stamped WHERE _ul_known_from <= K),
//!      latest AS (SELECT <columns>, _ul_source, _ul_server_last_modified, _ul_known_from FROM (SELECT known.*, row_number() OVER (PARTITION BY entity…, valid ORDER BY order… DESC NULLS LAST, _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) AS _ul_rank FROM known) ranked WHERE _ul_rank = 1),
//!      ends AS (SELECT DISTINCT entity…, valid FROM latest),
//!      next_ends AS (SELECT entity…, valid, lead(valid) OVER (PARTITION BY entity… ORDER BY valid) AS next_valid FROM ends),
//!      spans AS (SELECT latest.*, CASE WHEN next_valid >= <no end> THEN NULL ELSE next_valid END AS _ul_valid_to FROM latest JOIN next_ends USING (entity…, valid))
//! SELECT <columns>, _ul_source, _ul_known_from FROM spans WHERE <window>
//! ```
//!
//! Only `raw` is of one dialect. For DuckDB it is every lake file read
//! with `read_parquet`, each projected onto the schema. For Postgres it is
//! one relation named for the table, already in the schema. Either way
//! every row carries its own `_ul_source` and the three times, as the lake
//! file holds them, so nothing is joined in. Everything after is window
//! functions, `greatest`, `CASE` and `TIMESTAMP WITH TIME ZONE`
//! literals.

use std::fmt::Write as _;

use arrow_schema::DataType;
use utilake_iceberg::Column;
use utilake_mms_registry::file_model::ValidAxis;

use crate::Lake;
use crate::query::{Bound, FieldIx, Resolution};
use crate::time::{OPEN_US, ValidWindow, rfc3339};

/// Where the rows come from: the one clause of the query that is not
/// standard SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    /// The lake's Parquet files, read in place (`read_parquet`).
    #[default]
    DuckDb,
    /// A relation named for the table, loaded from the lake's files,
    /// whose rows carry `_ul_source`.
    Postgres,
}

impl Dialect {
    /// The dialect's name on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DuckDb => "duckdb",
            Self::Postgres => "postgres",
        }
    }
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The SQL type of an Arrow type of the lake.
fn sql_type(t: &DataType) -> String {
    match t {
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Int64 => "BIGINT".into(),
        DataType::Float64 => "DOUBLE PRECISION".into(),
        DataType::Decimal128(p, s) => format!("DECIMAL({p}, {s})"),
        DataType::Date32 => "DATE".into(),
        DataType::Timestamp(_, _) => "TIMESTAMP WITH TIME ZONE".into(),
        _ => "VARCHAR".into(),
    }
}

/// An instant as a literal, in UTC to the microsecond.
fn instant(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let frac = us.rem_euclid(1_000_000);
    let text = rfc3339(secs * 1_000_000, 0);
    let text = text.trim_end_matches('Z').replace('T', " ");
    format!("TIMESTAMP WITH TIME ZONE '{text}.{frac:06}+00:00'")
}

/// A time column as an instant: a date is its local midnight.
fn as_instant(b: &Bound, ix: FieldIx) -> String {
    let f = b.schema.field(ix.0);
    let name = ident(f.name());
    if f.data_type() == &DataType::Date32 {
        format!(
            "(CAST({name} AS TIMESTAMP) AT TIME ZONE 'UTC' - INTERVAL '{}' SECOND)",
            b.offset.0
        )
    } else {
        name
    }
}

/// Every column of the schema, quoted, in order.
fn all_columns(b: &Bound) -> Vec<String> {
    b.schema.fields().iter().map(|f| ident(f.name())).collect()
}

/// The two columns each row carries: its source and publish time.
const STAMP: &str = "\"_ul_source\", \"_ul_server_last_modified\", \"_ul_zip_entry_modified\", \"_ul_filename_timestamp\"";

/// The `raw` CTE: the rows, each with its source and publish time.
fn raw(b: &Bound, lake: &Lake<'_>, dialect: Dialect) -> String {
    match dialect {
        Dialect::Postgres => format!(
            "SELECT {}, {STAMP} FROM {}",
            all_columns(b).join(", "),
            ident(&b.table)
        ),
        Dialect::DuckDb => {
            let mut selects = Vec::new();
            for fr in &b.files {
                let f = &lake.files[fr.file];
                let cols: Vec<String> = fr
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let ty = sql_type(b.schema.field(i).data_type());
                        match c {
                            Column::Read {
                                source,
                                name,
                                cast: None,
                            } => format!("{} AS {}", ident(source), ident(name)),
                            Column::Read { source, name, .. } => {
                                format!("CAST({} AS {ty}) AS {}", ident(source), ident(name))
                            }
                            Column::Null { name, .. } => {
                                format!("CAST(NULL AS {ty}) AS {}", ident(name))
                            }
                        }
                    })
                    .collect();
                selects.push(format!(
                    "SELECT {}, CAST(\"_ul_source\" AS VARCHAR) AS \"_ul_source\", \"_ul_server_last_modified\", \"_ul_zip_entry_modified\", \"_ul_filename_timestamp\" FROM read_parquet([{}])",
                    cols.join(", "),
                    literal(&f.path)
                ));
            }
            if selects.is_empty() {
                let cols: Vec<String> = b
                    .schema
                    .fields()
                    .iter()
                    .map(|f| {
                        format!(
                            "CAST(NULL AS {}) AS {}",
                            sql_type(f.data_type()),
                            ident(f.name())
                        )
                    })
                    .collect();
                selects.push(format!(
                    "SELECT {}, CAST(NULL AS VARCHAR) AS \"_ul_source\", CAST(NULL AS TIMESTAMP WITH TIME ZONE) AS \"_ul_server_last_modified\", CAST(NULL AS TIMESTAMP WITH TIME ZONE) AS \"_ul_zip_entry_modified\", CAST(NULL AS TIMESTAMP WITH TIME ZONE) AS \"_ul_filename_timestamp\" WHERE 1 = 0",
                    cols.join(", ")
                ));
            }
            selects.join("\n  UNION ALL\n  ")
        }
    }
}

/// The read as SQL.
pub fn render(b: &Bound, lake: &Lake<'_>, dialect: Dialect) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "WITH raw AS (\n  {}\n),", raw(b, lake, dialect));
    let Some(spec) = &b.spec else {
        return finish_raw(out, b);
    };
    if b.query.resolution == Resolution::Raw {
        return finish_raw(out, b);
    }
    // Known no earlier than the server says it was public, nor than the
    // cutoff says.
    let public = "\"_ul_server_last_modified\"".to_owned();
    let known = spec.cutoff.map_or_else(
        || public.clone(),
        |c| format!("greatest({}, {public})", as_instant(b, c)),
    );
    let _ = writeln!(
        out,
        "stamped AS (SELECT raw.*, {known} AS _ul_known_from FROM raw),"
    );
    let known_filter = b.query.resolution.known_at().map_or_else(String::new, |k| {
        format!("WHERE _ul_known_from <= {}", instant(k.get()))
    });
    let _ = writeln!(out, "known AS (SELECT * FROM stamped {known_filter}),");
    // The fact, and the rank within it.
    let names = |ixs: &[FieldIx]| -> Vec<String> {
        ixs.iter()
            .map(|i| ident(b.schema.field(i.0).name()))
            .collect()
    };
    let (key, mut rank): (Vec<String>, Vec<String>) = if spec.collapse_all {
        (all_columns(b), Vec::new())
    } else {
        let mut key = names(&spec.entity);
        key.extend(
            spec.valid
                .column()
                .map(|c| ident(b.schema.field(c.0).name())),
        );
        let order = names(&spec.order);
        match b.query.resolution {
            Resolution::Revisions { .. } => (key.into_iter().chain(order).collect(), Vec::new()),
            _ => (
                key,
                order
                    .into_iter()
                    .map(|o| format!("{o} DESC NULLS LAST"))
                    .collect(),
            ),
        }
    };
    rank.push("\"_ul_server_last_modified\" DESC NULLS LAST".to_owned());
    rank.push("\"_ul_source\" DESC".to_owned());
    let partition = if key.is_empty() {
        String::new()
    } else {
        format!("PARTITION BY {} ", key.join(", "))
    };
    let mut carried = all_columns(b);
    carried.extend(
        [
            "\"_ul_source\"",
            "\"_ul_server_last_modified\"",
            "\"_ul_zip_entry_modified\"",
            "\"_ul_filename_timestamp\"",
            "_ul_known_from",
        ]
        .map(str::to_owned),
    );
    let _ = writeln!(
        out,
        "latest AS (SELECT {cols} FROM (SELECT known.*, row_number() OVER ({partition}ORDER BY {rank}) AS _ul_rank FROM known) ranked WHERE _ul_rank = 1),",
        cols = carried.join(", "),
        rank = rank.join(", ")
    );
    let window = match (&spec.valid, &b.query.window) {
        (ValidAxis::From { column: c, until }, w) => {
            let valid = as_instant(b, *c);
            let entity = names(&spec.entity);
            let partition = if entity.is_empty() {
                String::new()
            } else {
                format!("PARTITION BY {} ", entity.join(", "))
            };
            // A version ends at the next greater valid time of its entity
            // (every revision of one version shares the end); the lake's
            // "no end" date is an open end, as in the kernel.
            let valid_name = ident(b.schema.field(c.0).name());
            let mut using: Vec<String> = entity.clone();
            using.push(valid_name.clone());
            // The next version's start, else open; an end column that is
            // set, not AEMO's "no end", not before the start, and before
            // the next version, ends it instead.
            let open = instant(OPEN_US);
            let next = format!(
                "CASE WHEN next_ends.next_valid >= {open} THEN NULL ELSE next_ends.next_valid END"
            );
            let end = match until {
                None => next,
                Some(u) => {
                    let u = as_instant(b, *u);
                    format!(
                        "CASE WHEN {u} IS NOT NULL AND {u} < {open} AND {u} >= {valid} AND (next_ends.next_valid IS NULL OR next_ends.next_valid >= {open} OR {u} < next_ends.next_valid) THEN {u} ELSE {next} END"
                    )
                }
            };
            let _ = writeln!(
                out,
                "ends AS (SELECT DISTINCT {cols} FROM latest),\nnext_ends AS (SELECT {cols}, lead({valid}) OVER ({partition}ORDER BY {valid}) AS next_valid FROM ends),\nspans AS (SELECT latest.*, {end} AS _ul_valid_to FROM latest JOIN next_ends USING ({using}))",
                cols = using.join(", "),
                using = using.join(", ")
            );
            let (upto, after) = match w {
                ValidWindow::Range { after, upto } => (upto.get(), after.get()),
                ValidWindow::At(t) => (t.get(), t.get()),
            };
            format!(
                "WHERE {valid} <= {} AND (_ul_valid_to IS NULL OR _ul_valid_to > {})",
                instant(upto),
                instant(after)
            )
        }
        (axis, w) => {
            let _ = writeln!(out, "spans AS (SELECT * FROM latest)");
            axis.column().map_or_else(String::new, |c| {
                window_sql(Some(axis), w, &as_instant(b, *c), b.day_start_us)
            })
        }
    };
    // The frame's columns: the kept ones, the source, the known time.
    let mut cols = kept(b);
    cols.extend(
        [
            "\"_ul_source\"",
            "\"_ul_server_last_modified\"",
            "\"_ul_zip_entry_modified\"",
            "\"_ul_filename_timestamp\"",
        ]
        .map(str::to_owned),
    );
    cols.push("_ul_known_from".to_owned());
    if matches!(spec.valid, ValidAxis::From { .. }) {
        cols.push("_ul_valid_to".to_owned());
    }
    let _ = write!(out, "SELECT {} FROM spans {window}", cols.join(", "));
    out
}

/// The window on an axis other than `from`, as a `WHERE` clause over the
/// valid column `v`: `(after, upto]` where the time ends an interval,
/// `[after, upto)` where it labels an instant, the trading day's overlap
/// where it labels a day (`day_start` microseconds after midnight).
fn window_sql(
    axis: Option<&ValidAxis<FieldIx>>,
    w: &ValidWindow,
    v: &str,
    day_start: i64,
) -> String {
    let secs = day_start / 1_000_000;
    let day_from = format!("({v} + INTERVAL '{secs}' SECOND)");
    let day_to = format!("({v} + INTERVAL '{}' SECOND)", secs + 86_400);
    match (axis, w) {
        (Some(ValidAxis::Timeless), _) => String::new(),
        (Some(ValidAxis::IntervalEnd(_)), ValidWindow::Range { after, upto }) => format!(
            "WHERE {v} > {} AND {v} <= {}",
            instant(after.get()),
            instant(upto.get())
        ),
        (Some(ValidAxis::Day(_)), ValidWindow::Range { after, upto }) => format!(
            "WHERE {day_from} < {} AND {day_to} > {}",
            instant(upto.get()),
            instant(after.get())
        ),
        (Some(ValidAxis::Day(_)), ValidWindow::At(t)) => format!(
            "WHERE {day_from} <= {} AND {} < {day_to}",
            instant(t.get()),
            instant(t.get())
        ),
        (_, ValidWindow::Range { after, upto }) => format!(
            "WHERE {v} >= {} AND {v} < {}",
            instant(after.get()),
            instant(upto.get())
        ),
        (_, ValidWindow::At(t)) => format!("WHERE {v} = {}", instant(t.get())),
    }
}

/// The columns the read returns, quoted.
fn kept(b: &Bound) -> Vec<String> {
    match &b.keep {
        Some(keep) => keep
            .iter()
            .map(|i| ident(b.schema.field(i.0).name()))
            .collect(),
        None => all_columns(b),
    }
}

/// A raw read: every row, bounded on the time column.
fn finish_raw(mut out: String, b: &Bound) -> String {
    let axis = b.spec.as_ref().map(|s| &s.valid);
    let valid = match (axis, b.time) {
        (Some(a), _) => a.column().copied(),
        (None, t) => t,
    };
    let where_ = valid.map_or_else(String::new, |c| {
        window_sql(axis, &b.query.window, &as_instant(b, c), b.day_start_us)
    });
    let _ = writeln!(out, "spans AS (SELECT raw.* FROM raw)");
    let mut cols = kept(b);
    cols.extend(
        [
            "\"_ul_source\"",
            "\"_ul_server_last_modified\"",
            "\"_ul_zip_entry_modified\"",
            "\"_ul_filename_timestamp\"",
        ]
        .map(str::to_owned),
    );
    let _ = write!(out, "SELECT {} FROM spans {where_}", cols.join(", "));
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_schema::{Field, Schema};

    use super::*;
    use crate::query::{BoundSpec, Query, Unit};
    use crate::time::{MarketOffset, ValidTime};

    #[test]
    fn literals_and_types() {
        assert_eq!(
            instant(1_704_031_200_000_000),
            "TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00'"
        );
        assert_eq!(
            instant(1_704_031_200_500_000),
            "TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.500000+00:00'"
        );
        assert_eq!(sql_type(&DataType::Boolean), "BOOLEAN");
        assert_eq!(sql_type(&DataType::Date32), "DATE");
        assert_eq!(sql_type(&DataType::Utf8), "VARCHAR");
        assert_eq!(sql_type(&DataType::Float64), "DOUBLE PRECISION");
        assert_eq!(Dialect::DuckDb.name(), "duckdb");
        assert_eq!(Dialect::Postgres.name(), "postgres");
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(literal("it's"), "'it''s'");
    }

    /// A table whose valid time is a date, with no files.
    fn dated(window: ValidWindow, axis: ValidAxis<FieldIx>) -> Bound {
        let schema = Arc::new(Schema::new(vec![
            Field::new("DAY", DataType::Date32, true),
            Field::new("X", DataType::Boolean, true),
        ]));
        Bound {
            query: Query::new("T", window, Resolution::Latest { known_at: None }),
            table: "T".into(),
            schema_id: Some(0),
            schema,
            spec: Some(BoundSpec {
                entity: vec![],
                valid: axis,
                order: vec![],
                cutoff: None,
                unit: Unit::Table,
                collapse_all: false,
            }),
            time: Some(FieldIx(0)),
            files: vec![],
            entity_filters: vec![],
            value_filters: vec![],
            keep: None,
            offset: MarketOffset(36_000),
            day_start_us: 14_400_000_000,
            warnings: vec![],
        }
    }

    #[test]
    fn a_day_label_a_version_end_and_a_bare_range_render_their_windows() {
        let lake = Lake {
            registry: &utilake_nem::registry(),
            files: vec![],
            sources: vec![],
            version: "test",
        };
        let after = ValidTime::new(1_704_031_200_000_000);
        let upto = ValidTime::new(1_704_117_600_000_000);
        let range = ValidWindow::Range { after, upto };
        // A day label covers its trading day: label + start < upto, label
        // + start + 1 day > after.
        let s = render(
            &dated(range, ValidAxis::Day(FieldIx(0))),
            &lake,
            Dialect::DuckDb,
        );
        assert!(
            s.contains("INTERVAL '14400' SECOND) < TIMESTAMP WITH TIME ZONE '2024-01-01 14:00:00.000000+00:00'")
                && s.contains("INTERVAL '100800' SECOND) > TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00'"),
            "{s}"
        );
        let at = ValidWindow::At(after);
        let s = render(
            &dated(at, ValidAxis::Day(FieldIx(0))),
            &lake,
            Dialect::DuckDb,
        );
        assert!(
            s.contains("INTERVAL '14400' SECOND) <= TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00' AND TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00' < ("),
            "{s}"
        );
        // A from table with an end column: the version ends at the end
        // column when that comes first.
        let from = ValidAxis::From {
            column: FieldIx(0),
            until: Some(FieldIx(1)),
        };
        let s = render(&dated(range, from), &lake, Dialect::Postgres);
        assert!(
            s.contains("CASE WHEN") && s.contains("\"X\"") && s.contains("next_ends.next_valid"),
            "{s}"
        );
        // No roles: the time column alone bounds the read.
        let mut bare = dated(range, ValidAxis::Timeless);
        bare.spec = None;
        let s = render(&bare, &lake, Dialect::DuckDb);
        assert!(
            s.contains(">= TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00' AND")
                && s.contains("< TIMESTAMP WITH TIME ZONE '2024-01-01 14:00:00.000000+00:00'"),
            "{s}"
        );
    }

    #[test]
    fn a_date_column_reads_as_its_local_midnight_and_no_file_reads_nothing() {
        let lake = Lake {
            registry: &utilake_nem::registry(),
            files: vec![],
            sources: vec![],
            version: "test",
        };
        let at = ValidWindow::At(ValidTime::new(1_704_031_200_000_000));
        let s = render(
            &dated(at, ValidAxis::Instant(FieldIx(0))),
            &lake,
            Dialect::DuckDb,
        );
        assert!(
            s.contains("WHERE (CAST(\"DAY\" AS TIMESTAMP) AT TIME ZONE 'UTC' - INTERVAL '36000' SECOND) = TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00'"),
            "{s}"
        );
        assert!(
            s.contains("SELECT CAST(NULL AS DATE) AS \"DAY\", CAST(NULL AS BOOLEAN) AS \"X\", CAST(NULL AS VARCHAR) AS \"_ul_source\", CAST(NULL AS TIMESTAMP WITH TIME ZONE) AS \"_ul_server_last_modified\", CAST(NULL AS TIMESTAMP WITH TIME ZONE) AS \"_ul_zip_entry_modified\", CAST(NULL AS TIMESTAMP WITH TIME ZONE) AS \"_ul_filename_timestamp\" WHERE 1 = 0"),
            "{s}"
        );
        assert!(!s.contains("VALUES"), "{s}");
        assert!(
            s.contains("SELECT \"DAY\", \"X\", \"_ul_source\", \"_ul_server_last_modified\", \"_ul_zip_entry_modified\", \"_ul_filename_timestamp\", _ul_known_from FROM spans WHERE"),
            "{s}"
        );
        // A timeless table takes no window; Postgres reads the relation.
        let s = render(&dated(at, ValidAxis::Timeless), &lake, Dialect::Postgres);
        assert!(
            s.contains(
                "SELECT \"DAY\", \"X\", \"_ul_source\", \"_ul_server_last_modified\", \"_ul_zip_entry_modified\", \"_ul_filename_timestamp\" FROM \"T\""
            ),
            "{s}"
        );
        assert!(!s.contains("read_parquet"), "{s}");
        assert!(
            s.ends_with("SELECT \"DAY\", \"X\", \"_ul_source\", \"_ul_server_last_modified\", \"_ul_zip_entry_modified\", \"_ul_filename_timestamp\", _ul_known_from FROM spans "),
            "{s}"
        );
    }
}
