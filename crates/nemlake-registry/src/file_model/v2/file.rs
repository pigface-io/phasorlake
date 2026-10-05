//! Reading and writing `schemas/{market}/{TABLE}/table.toml` and
//! `schemas/{market}/market.toml`.
//!
//! The file is ours and hand-editable: top-level scalars, one line per
//! column (`column = [{ id = 1, name = "…", type = "double", nullable =
//! true }, …]`; `[[column]]` tables read the same) and one `[[increment]]`
//! per control file AEMO changed. Unknown keys are errors. Writing is
//! deterministic, so a file that changes nothing is byte-identical.

use std::fmt::Write as _;
use std::path::Path;

use nemlake_core::{YearMonth, normalize_name};
use serde::{Deserialize, Serialize};

use crate::file_model::error::SchemaFileError;
use crate::file_model::v2::model::{
    Column, ColumnId, Dedupe, Increment, LakeType, LoadMode, Market, MarketDef, Table,
    TimestampFormat,
};

/// The format of `table.toml` this crate reads and writes.
pub const SCHEMA_FORMAT: u32 = 2;

/// The format of `market.toml`.
pub const MARKET_FORMAT: u32 = 1;

/// The name of a table's file in its directory.
pub const TABLE_FILE: &str = "table.toml";

/// The directory of a table's control files, beside `table.toml`.
pub const CTL_DIR: &str = "ctl";

const fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TableFile {
    schema_format: u32,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    report_pairs: Vec<(String, String)>,
    load_mode: LoadMode,
    dedupe: Dedupe,
    #[serde(default)]
    primary_key: Vec<String>,
    #[serde(default)]
    time_column: Option<String>,
    next_column_id: u32,
    #[serde(default)]
    column: Vec<ColumnFile>,
    #[serde(default)]
    increment: Vec<IncrementFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnFile {
    id: u32,
    name: String,
    #[serde(rename = "type")]
    ty: LakeType,
    #[serde(default = "default_true")]
    nullable: bool,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IncrementFile {
    n: u16,
    from: YearMonth,
    #[serde(default)]
    ctl: Option<String>,
    #[serde(default)]
    ctl_sha256: Option<String>,
    #[serde(default)]
    aemo_ctl: Option<String>,
    #[serde(default)]
    aemo_versions: Vec<u16>,
    columns: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketFile {
    schema_format: u32,
    market: Market,
    utc_offset_secs: i32,
    timestamp_format: TimestampFormat,
}

fn parse_err(path: &Path, text: &str, e: &toml::de::Error) -> SchemaFileError {
    SchemaFileError::Parse {
        path: path.display().to_string(),
        message: e.message().to_owned(),
        line: e
            .span()
            .map(|s| text[..s.start.min(text.len())].matches('\n').count() as u64 + 1),
    }
}

fn wrong_format(path: &Path, name: &str, found: u32, reads: u32) -> SchemaFileError {
    SchemaFileError::Invalid {
        path: path.display().to_string(),
        table: name.to_owned(),
        message: format!("schema_format is {found} but this nemlake reads {reads}"),
    }
}

/// A name as an I row writes it: upper case, unquoted.
fn norm(s: &mut String, buf: &mut String) {
    if let Some(n) = normalize_name(s, buf)
        && n != s.as_str()
    {
        *s = n.to_owned();
    }
}

/// Parses a table file's text. Names are normalised, so a hand-typed
/// lower-case name still matches an I row.
pub fn parse_table(path: &Path, text: &str, market: Market) -> Result<Table, SchemaFileError> {
    let f: TableFile = toml::from_str(text).map_err(|e| parse_err(path, text, &e))?;
    if f.schema_format != SCHEMA_FORMAT {
        return Err(wrong_format(path, &f.name, f.schema_format, SCHEMA_FORMAT));
    }
    let mut buf = String::new();
    let mut t = Table {
        market,
        name: f.name,
        description: f.description,
        report_pairs: f.report_pairs,
        load_mode: f.load_mode,
        dedupe: f.dedupe,
        primary_key: f.primary_key,
        time_column: f.time_column,
        next_column_id: f.next_column_id,
        columns: f
            .column
            .into_iter()
            .map(|c| Column {
                id: ColumnId(c.id),
                name: c.name,
                ty: c.ty,
                nullable: c.nullable,
                note: c.note,
            })
            .collect(),
        increments: f
            .increment
            .into_iter()
            .map(|i| Increment {
                n: i.n,
                from: i.from,
                ctl: i.ctl,
                ctl_sha256: i.ctl_sha256,
                aemo_ctl: i.aemo_ctl,
                aemo_versions: i.aemo_versions,
                columns: i.columns,
                note: i.note,
            })
            .collect(),
    };
    norm(&mut t.name, &mut buf);
    for (a, b) in &mut t.report_pairs {
        norm(a, &mut buf);
        norm(b, &mut buf);
    }
    t.primary_key.iter_mut().for_each(|k| norm(k, &mut buf));
    if let Some(tc) = &mut t.time_column {
        norm(tc, &mut buf);
    }
    t.columns
        .iter_mut()
        .for_each(|c| norm(&mut c.name, &mut buf));
    for i in &mut t.increments {
        i.columns.iter_mut().for_each(|c| norm(c, &mut buf));
    }
    Ok(t)
}

/// Reads `{dir}/table.toml`.
pub fn read_table(dir: &Path, market: Market) -> Result<Table, SchemaFileError> {
    let path = dir.join(TABLE_FILE);
    let text = std::fs::read_to_string(&path).map_err(|source| SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    parse_table(&path, &text, market)
}

/// A TOML string.
fn q(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

/// A short TOML array on one line.
fn inline<T: AsRef<str>>(items: &[T]) -> String {
    let parts: Vec<String> = items.iter().map(|s| q(s.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}

/// Renders a table as TOML, deterministically.
pub fn write_table(t: &Table) -> String {
    let mut out = format!(
        "# Our schema of {}. Hand edits are welcome; `nemlake-census sync` only appends increments and columns.\nschema_format = {SCHEMA_FORMAT}\nname = {}\n",
        t.name,
        q(&t.name)
    );
    if let Some(d) = &t.description {
        let _ = writeln!(out, "description = {}", q(d));
    }
    let pairs: Vec<String> = t
        .report_pairs
        .iter()
        .map(|(a, b)| format!("[{}, {}]", q(a), q(b)))
        .collect();
    let _ = writeln!(out, "report_pairs = [{}]", pairs.join(", "));
    let mode = match t.load_mode {
        LoadMode::Append => "append",
        LoadMode::Snapshot => "snapshot",
    };
    let dedupe = match t.dedupe {
        Dedupe::None => "none",
        Dedupe::Pk => "pk",
        Dedupe::PkLastchanged => "pk_lastchanged",
    };
    let _ = writeln!(out, "load_mode = \"{mode}\"\ndedupe = \"{dedupe}\"");
    let _ = writeln!(out, "primary_key = {}", inline(&t.primary_key));
    if let Some(tc) = &t.time_column {
        let _ = writeln!(out, "time_column = {}", q(tc));
    }
    let _ = writeln!(out, "next_column_id = {}", t.next_column_id);
    out.push_str("\ncolumn = [\n");
    for c in &t.columns {
        let _ = write!(
            out,
            "    {{ id = {}, name = {}, type = \"{}\", nullable = {}",
            c.id,
            q(&c.name),
            c.ty,
            c.nullable
        );
        if let Some(n) = &c.note {
            let _ = write!(out, ", note = {}", q(n));
        }
        out.push_str(" },\n");
    }
    out.push_str("]\n");
    for i in &t.increments {
        let _ = write!(out, "\n[[increment]]\nn = {}\nfrom = \"{}\"\n", i.n, i.from);
        for (key, value) in [
            ("ctl", &i.ctl),
            ("ctl_sha256", &i.ctl_sha256),
            ("aemo_ctl", &i.aemo_ctl),
        ] {
            if let Some(v) = value {
                let _ = writeln!(out, "{key} = {}", q(v));
            }
        }
        let versions: Vec<String> = i.aemo_versions.iter().map(u16::to_string).collect();
        let _ = writeln!(out, "aemo_versions = [{}]", versions.join(", "));
        out.push_str("columns = [\n");
        for c in &i.columns {
            let _ = writeln!(out, "    {},", q(c));
        }
        out.push_str("]\n");
        if let Some(n) = &i.note {
            let _ = writeln!(out, "note = {}", q(n));
        }
    }
    out
}

/// The control file of increment `n`'s name in the `ctl/` directory.
pub fn ctl_file_name(n: u16) -> String {
    format!("{n:03}.ctl")
}

/// Parses a market file's text.
pub fn parse_market(path: &Path, text: &str) -> Result<MarketDef, SchemaFileError> {
    let f: MarketFile = toml::from_str(text).map_err(|e| parse_err(path, text, &e))?;
    if f.schema_format != MARKET_FORMAT {
        return Err(wrong_format(
            path,
            f.market.as_str(),
            f.schema_format,
            MARKET_FORMAT,
        ));
    }
    Ok(MarketDef {
        market: f.market,
        utc_offset_secs: f.utc_offset_secs,
        timestamp_format: f.timestamp_format,
    })
}

/// Reads and parses a market file.
pub fn read_market(path: &Path) -> Result<MarketDef, SchemaFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    parse_market(path, &text)
}

/// Renders a market as TOML.
pub fn write_market(m: &MarketDef) -> String {
    let f = MarketFile {
        schema_format: MARKET_FORMAT,
        market: m.market,
        utc_offset_secs: m.utc_offset_secs,
        timestamp_format: m.timestamp_format,
    };
    toml::to_string_pretty(&f).expect("a MarketDef always serialises")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_model::v2::model::tests::unit_scada;

    fn parse(text: &str) -> Result<Table, SchemaFileError> {
        parse_table(Path::new("table.toml"), text, Market::Nem)
    }

    #[test]
    fn round_trips_and_is_deterministic() {
        let t = unit_scada();
        let text = write_table(&t);
        assert!(
            text.contains(
                "    { id = 3, name = \"SCADAVALUE\", type = \"double\", nullable = true },\n"
            ),
            "{text}"
        );
        assert!(text.contains("report_pairs = [[\"DISPATCH\", \"UNIT_SCADA\"]]"));
        assert!(text.contains("\n[[increment]]\nn = 2\nfrom = \"2026-08\"\n"));
        let back = parse(&text).unwrap();
        assert_eq!(back, t);
        assert_eq!(write_table(&back), text);
        // A column note and every optional key render and read back.
        let mut all = t.clone();
        all.columns[2].note = Some("a \"quoted\" note".into());
        all.increments[0].ctl = Some("001.ctl".into());
        all.increments[0].ctl_sha256 = Some("ab".into());
        all.time_column = None;
        all.description = None;
        all.dedupe = Dedupe::None;
        all.load_mode = LoadMode::Snapshot;
        let text = write_table(&all);
        assert_eq!(parse(&text).unwrap(), all);
        all.dedupe = Dedupe::Pk;
        assert_eq!(parse(&write_table(&all)).unwrap(), all);
    }

    #[test]
    fn reads_hand_written_forms() {
        // `[[column]]` tables, a missing nullable (true), lower-case names.
        let text = r#"
schema_format = 2
name = "region"
report_pairs = [["dispatch", "region"]]
load_mode = "snapshot"
dedupe = "pk"
primary_key = ["regionid"]
next_column_id = 3

[[column]]
id = 1
name = "regionid"
type = "string"
nullable = false

[[column]]
id = 2
name = "\"Description\""
type = "string"

[[increment]]
n = 1
from = "2024-01"
columns = ["REGIONID", "description"]
"#;
        let t = parse(text).unwrap();
        assert_eq!(t.name, "REGION");
        assert_eq!(t.report_pairs, [("DISPATCH".into(), "REGION".into())]);
        assert_eq!(t.primary_key, ["REGIONID"]);
        assert_eq!(t.columns[1].name, "DESCRIPTION");
        assert!(t.columns[1].nullable);
        assert_eq!(t.increments[0].columns, ["REGIONID", "DESCRIPTION"]);
        assert!(t.increments[0].aemo_versions.is_empty());
    }

    #[test]
    fn rejects_unknown_keys_wrong_format_and_bad_types() {
        let text = write_table(&unit_scada());
        let e = parse(&text.replace("dedupe =", "dedup =")).unwrap_err();
        assert!(matches!(e, SchemaFileError::Parse { .. }), "{e}");
        assert!(e.to_string().contains("dedup"), "{e}");
        let e = parse(&text.replace("schema_format = 2", "schema_format = 1")).unwrap_err();
        assert!(e.to_string().contains("schema_format is 1"), "{e}");
        let e = parse(&text.replace("type = \"double\"", "type = \"number\"")).unwrap_err();
        assert!(e.to_string().contains("number"), "{e}");
        let e = parse(&text.replace("aemo_versions", "versions")).unwrap_err();
        assert!(e.to_string().contains("versions"), "{e}");
        let e = parse("schema_format = 2\nname = 3\n").unwrap_err();
        assert!(
            matches!(e, SchemaFileError::Parse { line: Some(2), .. }),
            "{e:?}"
        );
    }

    #[test]
    fn reads_from_disk_and_markets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(TABLE_FILE), write_table(&unit_scada())).unwrap();
        assert_eq!(read_table(dir.path(), Market::Nem).unwrap(), unit_scada());
        assert!(matches!(
            read_table(&dir.path().join("none"), Market::Nem),
            Err(SchemaFileError::Read { .. })
        ));
        assert_eq!(ctl_file_name(2), "002.ctl");
        let m = MarketDef {
            market: Market::Wem,
            utc_offset_secs: 28_800,
            timestamp_format: TimestampFormat::Iso8601,
        };
        let mp = dir.path().join("market.toml");
        std::fs::write(&mp, write_market(&m)).unwrap();
        assert_eq!(read_market(&mp).unwrap(), m);
        assert!(read_market(&dir.path().join("none.toml")).is_err());
        let bad = write_market(&m).replace("schema_format = 1", "schema_format = 2");
        assert!(parse_market(&mp, &bad).is_err());
        assert!(parse_market(&mp, "schema_format = 1\nmarket = \"mars\"\n").is_err());
    }
}
