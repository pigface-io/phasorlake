//! Reading and writing `schemas/{market}/{TABLE}/table.toml` and
//! `schemas/{market}/market.toml`.
//!
//! The file is ours and hand-editable: top-level scalars, one line per
//! column (`column = [{ id = 1, name = "…", type = "double", nullable =
//! true }, …]`; `[[column]]` tables read the same) and one `[[increment]]`
//! per control file AEMO changed, and the table's time roles under
//! `[temporal]`. Unknown keys are errors. Writing is deterministic, so a
//! file that changes nothing is byte-identical.
//!
//! Format 2 had a `dedupe` key instead of `[temporal]`; such a file (a
//! user's own table written before) still reads, with its roles derived,
//! and is written back as format 3.

use std::fmt::Write as _;
use std::path::Path;

use nemlake_core::{YearMonth, normalize_name};
use serde::{Deserialize, Serialize};

use crate::file_model::error::SchemaFileError;
use crate::file_model::model::{
    Column, ColumnId, Increment, LakeType, LoadMode, Market, MarketDef, Table, TimestampFormat,
};
use crate::file_model::temporal::{RoleOrigin, Temporal, ValidAxis, derive};

/// The format of `table.toml` this crate reads and writes.
pub const SCHEMA_FORMAT: u32 = 3;

/// The format before `[temporal]`, still read.
const SCHEMA_FORMAT_DEDUPE: u32 = 2;

/// Format 2's `dedupe`, read and dropped: `[temporal]` says it now.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyDedupe {
    None,
    Pk,
    PkLastchanged,
}

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
    #[serde(default)]
    dedupe: Option<LegacyDedupe>,
    #[serde(default)]
    primary_key: Vec<String>,
    #[serde(default)]
    time_column: Option<String>,
    next_column_id: u32,
    #[serde(default)]
    column: Vec<ColumnFile>,
    #[serde(default)]
    increment: Vec<IncrementFile>,
    #[serde(default)]
    temporal: Option<TemporalFile>,
}

/// `valid = { kind = "from", column = "EFFECTIVEDATE" }`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidFile {
    kind: ValidKind,
    #[serde(default)]
    column: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ValidKind {
    Timeless,
    IntervalEnd,
    Instant,
    From,
}

impl ValidFile {
    fn axis(self) -> Result<ValidAxis<String>, &'static str> {
        match (self.kind, self.column) {
            (ValidKind::Timeless, None) => Ok(ValidAxis::Timeless),
            (ValidKind::Timeless, Some(_)) => {
                Err("temporal valid kind \"timeless\" takes no column")
            }
            (_, None) => Err("temporal valid needs a column unless its kind is \"timeless\""),
            (ValidKind::IntervalEnd, Some(c)) => Ok(ValidAxis::IntervalEnd(c)),
            (ValidKind::Instant, Some(c)) => Ok(ValidAxis::Instant(c)),
            (ValidKind::From, Some(c)) => Ok(ValidAxis::From(c)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemporalFile {
    origin: RoleOrigin,
    #[serde(default)]
    entity: Vec<String>,
    valid: ValidFile,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    cutoff: Option<String>,
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
    let invalid = |message: &str| SchemaFileError::Invalid {
        path: path.display().to_string(),
        table: f.name.clone(),
        message: message.to_owned(),
    };
    match (f.schema_format, &f.dedupe, &f.temporal) {
        (SCHEMA_FORMAT, None, Some(_)) | (SCHEMA_FORMAT_DEDUPE, Some(_), None) => {}
        (SCHEMA_FORMAT, Some(_), _) => {
            return Err(invalid(
                "dedupe is format 2; format 3 says it under [temporal]",
            ));
        }
        (SCHEMA_FORMAT, None, None) => return Err(invalid("[temporal] is missing")),
        (SCHEMA_FORMAT_DEDUPE, _, _) => {
            return Err(invalid(
                "format 2 has dedupe and no [temporal]; write it as schema_format = 3",
            ));
        }
        (found, _, _) => return Err(wrong_format(path, &f.name, found, SCHEMA_FORMAT)),
    }
    let roles = match f.temporal {
        Some(r) => Some(Temporal {
            origin: r.origin,
            entity: r.entity,
            valid: r.valid.axis().map_err(invalid)?,
            order: r.order,
            cutoff: r.cutoff,
        }),
        None => None,
    };
    let mut buf = String::new();
    let mut t = Table {
        market,
        name: f.name,
        description: f.description,
        report_pairs: f.report_pairs,
        load_mode: f.load_mode,
        // Format 2: derived below, once the names are normalised.
        temporal: roles.unwrap_or(Temporal {
            origin: RoleOrigin::Derived,
            entity: Vec::new(),
            valid: ValidAxis::Timeless,
            order: Vec::new(),
            cutoff: None,
        }),
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
    if f.schema_format == SCHEMA_FORMAT_DEDUPE {
        t.temporal = derive(&t.primary_key, &t.columns);
    } else {
        let r = &mut t.temporal;
        for n in r.entity.iter_mut().chain(&mut r.order).chain(&mut r.cutoff) {
            norm(n, &mut buf);
        }
        r.valid = std::mem::replace(&mut r.valid, ValidAxis::Timeless).map(|mut n| {
            norm(&mut n, &mut buf);
            n
        });
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
    let _ = writeln!(out, "load_mode = \"{mode}\"");
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
    write_temporal(&mut out, &t.temporal);
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

/// `[temporal]`: who chose the roles, then each role on one line.
fn write_temporal(out: &mut String, r: &Temporal<String>) {
    let origin = match r.origin {
        RoleOrigin::Derived => "derived",
        RoleOrigin::Hand => "hand",
    };
    let valid = match r.valid.column() {
        Some(c) => format!("{{ kind = \"{}\", column = {} }}", r.valid.kind(), q(c)),
        None => format!("{{ kind = \"{}\" }}", r.valid.kind()),
    };
    let _ = write!(
        out,
        "\n[temporal]\norigin = \"{origin}\"\nentity = {}\nvalid = {valid}\norder = {}\n",
        inline(&r.entity),
        inline(&r.order)
    );
    if let Some(c) = &r.cutoff {
        let _ = writeln!(out, "cutoff = {}", q(c));
    }
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
    use crate::file_model::model::tests::unit_scada;

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
        assert!(
            text.contains(
                "\n[temporal]\norigin = \"derived\"\nentity = [\"DUID\"]\nvalid = { kind = \"interval_end\", column = \"SETTLEMENTDATE\" }\norder = [\"LASTCHANGED\"]\ncutoff = \"LASTCHANGED\"\n\n[[increment]]"
            ),
            "{text}"
        );
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
        all.load_mode = LoadMode::Snapshot;
        all.temporal.origin = RoleOrigin::Hand;
        all.temporal.cutoff = None;
        all.temporal.valid = ValidAxis::Timeless;
        let text = write_table(&all);
        assert!(text.contains("valid = { kind = \"timeless\" }\n"), "{text}");
        assert!(!text.contains("cutoff"), "{text}");
        assert_eq!(parse(&text).unwrap(), all);
        all.temporal.valid = ValidAxis::From("settlementdate".into());
        let back = parse(&write_table(&all)).unwrap();
        assert_eq!(
            back.temporal.valid,
            ValidAxis::From("SETTLEMENTDATE".into()),
            "normalised"
        );
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
        // Format 2's dedupe is dropped and the roles derived; written back as 3.
        assert_eq!(t.temporal, derive(&t.primary_key, &t.columns));
        assert!(write_table(&t).contains("schema_format = 3\n"));
    }

    #[test]
    fn rejects_unknown_keys_wrong_format_and_bad_types() {
        let text = write_table(&unit_scada());
        let e = parse(&text.replace("origin =", "orign =")).unwrap_err();
        assert!(matches!(e, SchemaFileError::Parse { .. }), "{e}");
        assert!(e.to_string().contains("orign"), "{e}");
        let e = parse(&text.replace("kind = \"interval_end\"", "kind = \"end\"")).unwrap_err();
        assert!(e.to_string().contains("end"), "{e}");
        let e = parse(&text.replace("kind = \"interval_end\"", "kind = \"timeless\"")).unwrap_err();
        assert!(e.to_string().contains("takes no column"), "{e}");
        let e = parse(&text.replace(", column = \"SETTLEMENTDATE\" }", " }")).unwrap_err();
        assert!(e.to_string().contains("needs a column"), "{e}");
        let e = parse(&text.replace(
            "load_mode = \"append\"\n",
            "load_mode = \"append\"\ndedupe = \"pk\"\n",
        ))
        .unwrap_err();
        assert!(e.to_string().contains("dedupe is format 2"), "{e}");
        let no_roles = text.split("\n[temporal]").next().unwrap().to_owned();
        let e = parse(&no_roles).unwrap_err();
        assert!(e.to_string().contains("[temporal] is missing"), "{e}");
        let e = parse(&text.replace("schema_format = 3", "schema_format = 2")).unwrap_err();
        assert!(
            e.to_string()
                .contains("format 2 has dedupe and no [temporal]"),
            "{e}"
        );
        let e = parse(&text.replace("schema_format = 3", "schema_format = 1")).unwrap_err();
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
