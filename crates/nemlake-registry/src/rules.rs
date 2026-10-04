//! The identity of a transform.
//!
//! What a file of one shape is written as depends on the rules it is read
//! with: the columns of the lake and their types, which field feeds which
//! column and how it is parsed, how rows are partitioned, where the files
//! go. [`RulesHash`] is one number over all of that. It is recorded with
//! every Parquet file and every ingest, so a file says which rules made it,
//! and an ingest whose rules have changed since can be found and done
//! again from the archived source.
//!
//! The hash covers what decides the output and nothing else: a description
//! or a provenance note in a schema file does not change it, a column's
//! type does.

use std::fmt;

use crate::file_model::TableDef;
use arrow_schema::Schema;
use nemlake_core::Fingerprint;

use crate::plan::{Cast, ColumnRoute};

/// The generation of the conversions the code itself performs: how a
/// timestamp, a number or a date is read, how a partition is chosen. Raise
/// it when the same schema would be written differently by a newer nemlake,
/// so that lakes written by the older one are known to be behind.
pub const RULES_FORMAT: u32 = 1;

/// The identity of the rules one shape is read with. Two files with the
/// same source bytes, shape and rules hash have the same rows.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RulesHash(pub u128);

impl fmt::Debug for RulesHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for RulesHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rules:{:032x}", self.0)
    }
}

fn cast_token(c: Cast) -> String {
    match c {
        Cast::Utf8 => "utf8".into(),
        Cast::Float64 => "float64".into(),
        Cast::Int64 => "int64".into(),
        Cast::Decimal128 { precision, scale } => format!("decimal128({precision},{scale})"),
        Cast::TimestampNanos { offset_secs } => format!("timestamp_ns({offset_secs})"),
        Cast::Date32 => "date32".into(),
        Cast::Boolean => "boolean".into(),
    }
}

fn indexes(label: &str, of: impl IntoIterator<Item = u16>) -> String {
    let mut s = String::from(label);
    for i in of {
        s.push(' ');
        s.push_str(&i.to_string());
    }
    s
}

/// Hashes everything that decides what a shape is written as.
pub(crate) fn hash(
    t: &TableDef,
    dir: &str,
    arrow: &Schema,
    routes: &[ColumnRoute],
    missing: &[u16],
    primary_key: &[u16],
    time_column: Option<u16>,
) -> RulesHash {
    let mut tokens: Vec<String> = Vec::with_capacity(8 + 2 * arrow.fields().len() + routes.len());
    tokens.push(format!("nemlake rules {RULES_FORMAT}"));
    tokens.push(format!("table {}", t.name));
    tokens.push(format!("market {}", t.market));
    tokens.push(format!("dir {dir}"));
    tokens.push(format!("load {:?}", t.load_mode));
    tokens.push(indexes("key", primary_key.iter().copied()));
    tokens.push(indexes("time", time_column));
    tokens.push(indexes("missing", missing.iter().copied()));
    let mut md: Vec<(&String, &String)> = arrow.metadata().iter().collect();
    md.sort();
    for (k, v) in md {
        tokens.push(format!("meta {k}={v}"));
    }
    for f in arrow.fields() {
        tokens.push(format!(
            "field {} {} {}",
            f.name(),
            f.data_type(),
            if f.is_nullable() { "null" } else { "not null" }
        ));
        let mut md: Vec<(&String, &String)> = f.metadata().iter().collect();
        md.sort();
        for (k, v) in md {
            tokens.push(format!("field meta {k}={v}"));
        }
    }
    for c in routes {
        tokens.push(format!(
            "cast {} {} {}",
            c.target
                .map_or_else(|| "drop".to_owned(), |i| i.to_string()),
            cast_token(c.cast),
            if c.nullable { "null" } else { "not null" }
        ));
    }
    RulesHash(Fingerprint::of_names(tokens.iter().map(String::as_str)).0)
}

#[cfg(test)]
mod tests {
    use nemlake_core::IRow;

    use crate::registry::{Registry, SchemaSource, TypeOptions};
    use crate::resolve::ResolveOptions;

    use super::*;

    fn scada<'a>(columns: &'a [&'a str]) -> IRow<'a> {
        IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns,
        }
    }

    #[test]
    fn text_form_and_tokens() {
        let h = RulesHash(0xabc);
        assert_eq!(h.to_string(), format!("rules:{:032x}", 0xabc));
        assert_eq!(format!("{h:?}"), h.to_string());
        assert_eq!(
            cast_token(Cast::Decimal128 {
                precision: 15,
                scale: 5
            }),
            "decimal128(15,5)"
        );
        assert_eq!(
            cast_token(Cast::TimestampNanos {
                offset_secs: 36_000
            }),
            "timestamp_ns(36000)"
        );
        let all = [
            Cast::Utf8,
            Cast::Float64,
            Cast::Int64,
            Cast::Date32,
            Cast::Boolean,
        ]
        .map(cast_token);
        assert_eq!(all, ["utf8", "float64", "int64", "date32", "boolean"]);
        assert_eq!(indexes("key", [1, 2]), "key 1 2");
        assert_eq!(indexes("time", None), "time");
    }

    /// The hash is the same for the same shape and rules, and differs when
    /// the shape, the type mode or the table differs.
    #[test]
    fn follows_what_decides_the_output() {
        let reg = Registry::load_embedded(TypeOptions::default());
        let opts = ResolveOptions::default();
        let declared = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
        let a = reg.resolve(&scada(&declared), opts).unwrap();
        let again = reg.resolve(&scada(&declared), opts).unwrap();
        assert_eq!(a.rules(), again.rules());
        assert_eq!(a.source, SchemaSource::Registry);

        // Another column list of the same table is read by other rules.
        let fewer = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let b = reg.resolve(&scada(&fewer), opts).unwrap();
        assert_ne!(a.rules(), b.rules());
        let extra = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "NOBODY_KNOWS"];
        let c = reg.resolve(&scada(&extra), opts).unwrap();
        assert_ne!(a.rules(), c.rules());
        assert_ne!(b.rules(), c.rules());

        // Decimal mode writes other types.
        let dec = Registry::load_embedded(TypeOptions { decimal: true });
        let d = dec.resolve(&scada(&declared), opts).unwrap();
        assert_ne!(a.rules(), d.rules());

        // A table derived from the file has rules of its own, and the same
        // ones in another registry that derives it again.
        let cols = ["A", "B"];
        let unknown = IRow {
            report_type: "RULES_TEST",
            report_subtype: "NOBODY",
            version: 1,
            columns: &cols,
        };
        let e = reg.resolve(&unknown, opts).unwrap();
        assert_eq!(e.source, SchemaSource::File);
        assert_ne!(e.rules(), a.rules());
        let f = dec.resolve(&unknown, opts).unwrap();
        assert_eq!(e.rules(), f.rules());
        assert_eq!(SchemaSource::File.label(), "file");
        assert_eq!(SchemaSource::User.to_string(), "user");
        assert_eq!(SchemaSource::Registry.label(), "registry");
    }
}
