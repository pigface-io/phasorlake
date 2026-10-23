//! The same read as one DuckDB query: an independent statement of the
//! roles, which the oracle test runs against the kernel.
//!
//! ```sql
//! WITH raw AS (SELECT ..., filename FROM read_parquet([...], filename = true) UNION ALL ...),
//!      sources(filename, published) AS (VALUES (...)),
//!      stamped AS (SELECT raw.*, published, coalesce("LASTCHANGED", published) AS _nl_known FROM raw JOIN sources USING (filename)),
//!      known AS (SELECT * FROM stamped WHERE _nl_known <= K),
//!      latest AS (SELECT * FROM known QUALIFY row_number() OVER (PARTITION BY entity…, valid ORDER BY order… DESC NULLS LAST, published DESC, filename DESC) = 1),
//!      ends AS (SELECT DISTINCT entity…, valid FROM latest),
//!      next_ends AS (SELECT entity…, valid, lead(valid) OVER (PARTITION BY entity… ORDER BY valid) AS next_valid FROM ends),
//!      spans AS (SELECT latest.*, nullif(next_ends.next_valid, <no end>) AS _nl_valid_to FROM latest JOIN next_ends USING (entity…, valid))
//! SELECT ... FROM spans WHERE <window>
//! ```

use std::fmt::Write as _;

use arrow_schema::DataType;
use utilake_iceberg::Column;
use utilake_mms_registry::file_model::ValidAxis;

use crate::Lake;
use crate::query::{Bound, FieldIx, Resolution};
use crate::time::{OPEN_US, ValidWindow, rfc3339};

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// DuckDB's name for an Arrow type of the lake.
fn duck_type(t: &DataType) -> String {
    match t {
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Int64 => "BIGINT".into(),
        DataType::Float64 => "DOUBLE".into(),
        DataType::Decimal128(p, s) => format!("DECIMAL({p}, {s})"),
        DataType::Date32 => "DATE".into(),
        DataType::Timestamp(_, _) => "TIMESTAMPTZ".into(),
        _ => "VARCHAR".into(),
    }
}

/// An instant as DuckDB reads it.
fn instant(us: i64) -> String {
    format!("make_timestamptz({us})")
}

/// A time column as an instant: a date is its local midnight.
fn as_instant(b: &Bound, ix: FieldIx) -> String {
    let f = b.schema.field(ix.0);
    let name = ident(f.name());
    if f.data_type() == &DataType::Date32 {
        format!(
            "(({name})::TIMESTAMP - INTERVAL {} SECOND)::TIMESTAMPTZ",
            b.offset.0
        )
    } else {
        name
    }
}

/// The read as DuckDB SQL.
pub fn render(b: &Bound, lake: &Lake<'_>) -> String {
    let mut out = String::new();
    // raw: every file projected onto the target.
    let mut selects = Vec::new();
    for fr in &b.files {
        let f = &lake.files[fr.file];
        let cols: Vec<String> = fr
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let ty = duck_type(b.schema.field(i).data_type());
                match c {
                    Column::Read {
                        source,
                        name,
                        cast: None,
                    } => format!("{} AS {}", ident(source), ident(name)),
                    Column::Read { source, name, .. } => {
                        format!("CAST({} AS {ty}) AS {}", ident(source), ident(name))
                    }
                    Column::Null { name, .. } => format!("NULL::{ty} AS {}", ident(name)),
                }
            })
            .collect();
        selects.push(format!(
            "SELECT {}, filename FROM read_parquet([{}], filename = true)",
            cols.join(", "),
            literal(&f.path)
        ));
    }
    if selects.is_empty() {
        selects.push("SELECT NULL AS filename WHERE false".to_owned());
    }
    let _ = writeln!(
        out,
        "WITH raw AS (\n  {}\n),",
        selects.join("\n  UNION ALL\n  ")
    );
    // sources: each file's publish time.
    let rows: Vec<String> = b
        .files
        .iter()
        .map(|fr| {
            let f = &lake.files[fr.file];
            let p = f.published.map_or_else(
                || "NULL::TIMESTAMPTZ".to_owned(),
                |p| format!("{}::TIMESTAMPTZ", literal(&rfc3339(p.get(), 0))),
            );
            format!("({}, {p})", literal(&f.path))
        })
        .collect();
    let rows = if rows.is_empty() {
        "(NULL, NULL::TIMESTAMPTZ)".to_owned()
    } else {
        rows.join(", ")
    };
    let _ = writeln!(out, "sources(filename, published) AS (VALUES {rows}),");
    let Some(spec) = &b.spec else {
        return finish_raw(out, b);
    };
    if b.query.resolution == Resolution::Raw {
        return finish_raw(out, b);
    }
    let known = spec.cutoff.map_or_else(
        || "published".to_owned(),
        |c| format!("coalesce({}, published)", as_instant(b, c)),
    );
    let _ = writeln!(
        out,
        "stamped AS (SELECT raw.*, published, {known} AS _nl_known FROM raw JOIN sources USING (filename)),"
    );
    let known_filter = match b.query.resolution {
        Resolution::Latest { known_at: Some(k) } => {
            format!("WHERE _nl_known <= {}", instant(k.get()))
        }
        _ => String::new(),
    };
    let _ = writeln!(out, "known AS (SELECT * FROM stamped {known_filter}),");
    // The fact, and the rank within it.
    let names = |ixs: &[FieldIx]| -> Vec<String> {
        ixs.iter()
            .map(|i| ident(b.schema.field(i.0).name()))
            .collect()
    };
    let (mut key, mut rank): (Vec<String>, Vec<String>) = if spec.collapse_all {
        (
            b.schema.fields().iter().map(|f| ident(f.name())).collect(),
            Vec::new(),
        )
    } else {
        let mut key = names(&spec.entity);
        key.extend(
            spec.valid
                .column()
                .map(|c| ident(b.schema.field(c.0).name())),
        );
        let order = names(&spec.order);
        match b.query.resolution {
            Resolution::Revisions => (key.into_iter().chain(order).collect(), Vec::new()),
            _ => (
                key,
                order
                    .into_iter()
                    .map(|o| format!("{o} DESC NULLS LAST"))
                    .collect(),
            ),
        }
    };
    rank.push("published DESC NULLS LAST".to_owned());
    rank.push("filename DESC".to_owned());
    let partition = if key.is_empty() {
        String::new()
    } else {
        format!("PARTITION BY {} ", key.join(", "))
    };
    let _ = writeln!(
        out,
        "latest AS (SELECT * FROM known QUALIFY row_number() OVER ({partition}ORDER BY {}) = 1),",
        rank.join(", ")
    );
    key.clear();
    let window = match (&spec.valid, &b.query.window) {
        (ValidAxis::From(c), w) => {
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
            let _ = writeln!(
                out,
                "ends AS (SELECT DISTINCT {cols} FROM latest),\nnext_ends AS (SELECT {cols}, lead({valid}) OVER ({partition}ORDER BY {valid}) AS next_valid FROM ends),\nspans AS (SELECT latest.*, nullif(next_ends.next_valid, {open}) AS _nl_valid_to FROM latest JOIN next_ends USING ({using}))",
                cols = using.join(", "),
                open = instant(OPEN_US),
                using = using.join(", ")
            );
            let (upto, after) = match w {
                ValidWindow::Range { after, upto } => (upto.get(), after.get()),
                ValidWindow::At(t) => (t.get(), t.get()),
            };
            format!(
                "WHERE {valid} <= {} AND (_nl_valid_to IS NULL OR _nl_valid_to > {})",
                instant(upto),
                instant(after)
            )
        }
        (axis, w) => {
            let _ = writeln!(out, "spans AS (SELECT * FROM latest)");
            axis.column().map_or_else(String::new, |c| {
                window_sql(Some(axis), w, &as_instant(b, *c))
            })
        }
    };
    let cols = kept(b, "filename, published");
    let _ = write!(out, "SELECT {cols} FROM spans {window}");
    out
}

/// The window on an axis other than `from`, as a `WHERE` clause over the
/// valid column `v`: `(after, upto]` where the time ends an interval,
/// `[after, upto)` where it labels an instant.
fn window_sql(axis: Option<&ValidAxis<FieldIx>>, w: &ValidWindow, v: &str) -> String {
    match (axis, w) {
        (Some(ValidAxis::Timeless), _) => String::new(),
        (Some(ValidAxis::IntervalEnd(_)), ValidWindow::Range { after, upto }) => format!(
            "WHERE {v} > {} AND {v} <= {}",
            instant(after.get()),
            instant(upto.get())
        ),
        (_, ValidWindow::Range { after, upto }) => format!(
            "WHERE {v} >= {} AND {v} < {}",
            instant(after.get()),
            instant(upto.get())
        ),
        (_, ValidWindow::At(t)) => format!("WHERE {v} = {}", instant(t.get())),
    }
}

/// The columns the read returns, as a select list; `hidden` are the
/// working columns `*` must leave out.
fn kept(b: &Bound, hidden: &str) -> String {
    match &b.keep {
        Some(keep) => keep
            .iter()
            .map(|i| ident(b.schema.field(i.0).name()))
            .collect::<Vec<_>>()
            .join(", "),
        None => format!("* EXCLUDE ({hidden})"),
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
        window_sql(axis, &b.query.window, &as_instant(b, c))
    });
    let _ = writeln!(
        out,
        "spans AS (SELECT raw.* FROM raw JOIN sources USING (filename))"
    );
    let _ = write!(out, "SELECT {} FROM spans {where_}", kept(b, "filename"));
    out
}
