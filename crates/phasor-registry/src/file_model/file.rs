//! Reading and writing a hand-written table, `{TABLE}/table.toml`, and
//! `schemas/{market}/market.toml`; unknown keys are errors and writing is
//! deterministic.

use std::fmt::Write as _;
use std::path::Path;

use phasor_core::{YearMonth, normalize_name};
use serde::{Deserialize, Serialize};

use crate::file_model::error::SchemaFileError;
use crate::file_model::model::{
    Column, ColumnId, LakeType, LoadMode, Market, MarketDef, Run, Table, TableSchema,
    TimestampFormat,
};
use crate::file_model::temporal::{RoleOrigin, Temporal, ValidAxis};

/// The format of `table.toml` this crate reads and writes.
pub const SCHEMA_FORMAT: u32 = 4;

/// The format of `market.toml`.
pub const MARKET_FORMAT: u32 = 1;

/// The name of a hand-written table's file in its directory.
pub const TABLE_FILE: &str = "table.toml";

/// The month a schema with no `from` applies from: the NEM's first.
pub const NEM_START: (i32, u8) = (1998, 12);

const fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TableFile {
    /// Read first, by [`FormatOnly`].
    #[serde(rename = "schema_format")]
    _schema_format: u32,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    report_pairs: Vec<(String, String)>,
    #[serde(default)]
    with_columns: Vec<String>,
    load_mode: LoadMode,
    #[serde(default)]
    primary_key: Vec<String>,
    #[serde(default)]
    time_column: Option<String>,
    #[serde(default)]
    column: Vec<ColumnFile>,
    temporal: TemporalFile,
    #[serde(default)]
    schema: Vec<SchemaFile>,
}

/// The format of a file, read before the rest so an older one is named.
#[derive(Deserialize)]
struct FormatOnly {
    schema_format: u32,
    #[serde(default)]
    name: String,
}

/// `valid = { kind = "from", column = "EFFECTIVEDATE", until = "END_DATE" }`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidFile {
    kind: ValidKind,
    #[serde(default)]
    column: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ValidKind {
    Timeless,
    IntervalEnd,
    Instant,
    Day,
    From,
}

impl ValidFile {
    fn axis(self) -> Result<ValidAxis<String>, &'static str> {
        if self.until.is_some() && !matches!(self.kind, ValidKind::From) {
            return Err("temporal valid takes an until column only when its kind is \"from\"");
        }
        match (self.kind, self.column) {
            (ValidKind::Timeless, None) => Ok(ValidAxis::Timeless),
            (ValidKind::Timeless, Some(_)) => {
                Err("temporal valid kind \"timeless\" takes no column")
            }
            (_, None) => Err("temporal valid needs a column unless its kind is \"timeless\""),
            (ValidKind::IntervalEnd, Some(c)) => Ok(ValidAxis::IntervalEnd(c)),
            (ValidKind::Instant, Some(c)) => Ok(ValidAxis::Instant(c)),
            (ValidKind::Day, Some(c)) => Ok(ValidAxis::Day(c)),
            (ValidKind::From, Some(c)) => Ok(ValidAxis::From {
                column: c,
                until: self.until,
            }),
        }
    }
}

/// A table's roles as a file says them, here and in a pulled table.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TemporalFile {
    origin: RoleOrigin,
    #[serde(default)]
    entity: Vec<String>,
    valid: ValidFile,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    cutoff: Option<String>,
}

impl TemporalFile {
    /// The roles, or why the valid axis is not one.
    pub(crate) fn roles(self) -> Result<Temporal<String>, &'static str> {
        Ok(Temporal {
            origin: self.origin,
            entity: self.entity,
            valid: self.valid.axis()?,
            order: self.order,
            cutoff: self.cutoff,
        })
    }
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
struct SchemaFile {
    id: u16,
    #[serde(default)]
    from: Option<YearMonth>,
    versions: Vec<u16>,
    columns: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketFile {
    schema_format: u32,
    market: Market,
    utc_offset_secs: i32,
    timestamp_format: TimestampFormat,
    /// Absent in a file from before trading days were known: midnight.
    #[serde(default)]
    trading_day_start_secs: i32,
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
        message: format!("schema_format is {found} but this phasor reads {reads}"),
    }
}

/// A name as an I row writes it: upper case, unquoted.
pub(crate) fn norm(s: &mut String, buf: &mut String) {
    if let Some(n) = normalize_name(s, buf)
        && n != s.as_str()
    {
        *s = n.to_owned();
    }
}

/// Parses a table file's text, normalising names so lower-case ones still match.
pub fn parse_table(path: &Path, text: &str, market: Market) -> Result<Table, SchemaFileError> {
    let format: FormatOnly = toml::from_str(text).map_err(|e| parse_err(path, text, &e))?;
    let invalid = |message: String| SchemaFileError::Invalid {
        path: path.display().to_string(),
        table: format.name.clone(),
        message,
    };
    match format.schema_format {
        SCHEMA_FORMAT => {}
        2 | 3 => {
            return Err(invalid(format!(
                "schema_format is {}: its [[increment]] lists are gone; write schema_format = {SCHEMA_FORMAT} with one [[schema]] (id, versions, columns, optionally from) per column list the files carry",
                format.schema_format
            )));
        }
        found => return Err(wrong_format(path, &format.name, found, SCHEMA_FORMAT)),
    }
    let f: TableFile = toml::from_str(text).map_err(|e| parse_err(path, text, &e))?;
    let mut temporal = f.temporal.roles().map_err(|m| invalid(m.to_owned()))?;
    let mut buf = String::new();
    let mut name = f.name;
    norm(&mut name, &mut buf);
    let mut report_pairs = f.report_pairs;
    for (a, b) in &mut report_pairs {
        norm(a, &mut buf);
        norm(b, &mut buf);
    }
    let mut primary_key = f.primary_key;
    primary_key.iter_mut().for_each(|k| norm(k, &mut buf));
    let mut time_column = f.time_column;
    if let Some(tc) = &mut time_column {
        norm(tc, &mut buf);
    }
    let mut columns: Vec<Column> = f
        .column
        .into_iter()
        .map(|c| {
            let mut name = c.name;
            norm(&mut name, &mut buf);
            Column {
                id: ColumnId(c.id),
                name,
                ty: c.ty,
                nullable: c.nullable,
                note: c.note,
            }
        })
        .collect();
    columns.sort_by_key(|c| c.id);
    for n in temporal
        .entity
        .iter_mut()
        .chain(&mut temporal.order)
        .chain(&mut temporal.cutoff)
    {
        norm(n, &mut buf);
    }
    temporal.valid = std::mem::replace(&mut temporal.valid, ValidAxis::Timeless).map(|mut n| {
        norm(&mut n, &mut buf);
        n
    });
    let id_of = |n: &str| columns.iter().find(|c| c.name == n);
    let mut schemas = Vec::with_capacity(f.schema.len());
    let mut runs = Vec::new();
    for s in f.schema {
        if s.versions.is_empty() {
            return Err(invalid(format!(
                "schema {} lists no versions; name the AEMO versions its files come under",
                s.id
            )));
        }
        let mut fields = Vec::with_capacity(s.columns.len());
        for mut c in s.columns {
            norm(&mut c, &mut buf);
            let Some(col) = id_of(&c) else {
                return Err(invalid(format!(
                    "schema {} lists column {c}, which is not a column; add it to column",
                    s.id
                )));
            };
            fields.push((col.id, !col.nullable));
        }
        let key = primary_key
            .iter()
            .filter_map(|k| id_of(k))
            .map(|c| c.id)
            .filter(|id| fields.iter().any(|(f, _)| f == id))
            .collect();
        let from = s
            .from
            .unwrap_or_else(|| YearMonth::new(NEM_START.0, NEM_START.1).expect("a month"));
        for (a, b) in &report_pairs {
            for v in &s.versions {
                runs.push(Run {
                    schema: s.id,
                    report: (a.clone(), b.clone()),
                    version: *v,
                    kinds: Vec::new(),
                    from,
                    to: from,
                    files: 0,
                    rows: 0,
                });
            }
        }
        schemas.push(TableSchema {
            id: s.id,
            fields,
            key,
        });
    }
    runs.sort_by_key(|r| r.from);
    let current_schema = runs
        .last()
        .map_or_else(|| schemas.last().map_or(0, |s| s.id), |r| r.schema);
    Ok(Table {
        market,
        name,
        description: f.description,
        report_pairs,
        with_columns: f.with_columns,
        older_names: Vec::new(),
        published: String::new(),
        load_mode: f.load_mode,
        temporal,
        primary_key,
        time_column,
        columns,
        schemas,
        runs,
        current_schema,
    })
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
        "# The table {}, written by hand or by `phasor schema show`.\nschema_format = {SCHEMA_FORMAT}\nname = {}\n",
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
    if !t.with_columns.is_empty() {
        let _ = writeln!(out, "with_columns = {}", inline(&t.with_columns));
    }
    let mode = match t.load_mode {
        LoadMode::Append => "append",
        LoadMode::Snapshot => "snapshot",
    };
    let _ = writeln!(out, "load_mode = \"{mode}\"");
    let _ = writeln!(out, "primary_key = {}", inline(&t.primary_key));
    if let Some(tc) = &t.time_column {
        let _ = writeln!(out, "time_column = {}", q(tc));
    }
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
    for s in &t.schemas {
        let runs = || t.runs.iter().filter(|r| r.schema == s.id);
        let _ = write!(out, "\n[[schema]]\nid = {}\n", s.id);
        if let Some(from) = runs().map(|r| r.from).min() {
            let _ = writeln!(out, "from = \"{from}\"");
        }
        let mut versions: Vec<u16> = runs().map(|r| r.version).collect();
        versions.sort_unstable();
        versions.dedup();
        let versions: Vec<String> = versions.iter().map(u16::to_string).collect();
        let _ = writeln!(out, "versions = [{}]", versions.join(", "));
        out.push_str("columns = [\n");
        for c in t.schema_columns(s) {
            let _ = writeln!(out, "    {},", q(c));
        }
        out.push_str("]\n");
    }
    out
}

/// `[temporal]`: who chose the roles, then each role on one line.
fn write_temporal(out: &mut String, r: &Temporal<String>) {
    let origin = match r.origin {
        RoleOrigin::Derived => "derived",
        RoleOrigin::Hand => "hand",
    };
    let valid = match (r.valid.column(), r.valid.until()) {
        (Some(c), Some(u)) => format!(
            "{{ kind = \"{}\", column = {}, until = {} }}",
            r.valid.kind(),
            q(c),
            q(u)
        ),
        (Some(c), None) => format!("{{ kind = \"{}\", column = {} }}", r.valid.kind(), q(c)),
        (None, _) => format!("{{ kind = \"{}\" }}", r.valid.kind()),
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

/// Parses a market file's text.
pub fn parse_market(path: &Path, text: &str) -> Result<MarketDef, SchemaFileError> {
    let f: MarketFile = toml::from_str(text).map_err(|e| parse_err(path, text, &e))?;
    if f.schema_format != MARKET_FORMAT {
        return Err(wrong_format(
            path,
            f.market.id(),
            f.schema_format,
            MARKET_FORMAT,
        ));
    }
    Ok(MarketDef {
        market: f.market,
        utc_offset_secs: f.utc_offset_secs,
        timestamp_format: f.timestamp_format,
        trading_day_start_secs: f.trading_day_start_secs,
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
        trading_day_start_secs: m.trading_day_start_secs,
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

    /// The fixture as a hand-written file says it: one run per schema.
    fn as_written() -> Table {
        let mut t = unit_scada();
        for r in &mut t.runs {
            r.to = r.from;
            r.kinds.clear();
            (r.files, r.rows) = (0, 0);
        }
        t
    }

    #[test]
    fn round_trips_and_is_deterministic() {
        let t = as_written();
        let text = write_table(&unit_scada());
        assert!(
            text.contains(
                "    { id = 3, name = \"SCADAVALUE\", type = \"double\", nullable = true },\n"
            ),
            "{text}"
        );
        assert!(text.contains("report_pairs = [[\"DISPATCH\", \"UNIT_SCADA\"]]"));
        assert!(!text.contains("with_columns"), "absent for every block");
        // A table of one kind of a pair's rows says which, and reads back.
        let mut area = as_written();
        area.with_columns = vec!["AREAID".into()];
        let written = write_table(&area);
        assert!(
            written.contains("with_columns = [\"AREAID\"]\n"),
            "{written}"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(TABLE_FILE), &written).unwrap();
        assert_eq!(read_table(dir.path(), Market::Nem).unwrap(), area);
        assert!(
            text.contains("\n[[schema]]\nid = 1\nfrom = \"2026-08\"\nversions = [1]\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "\n[temporal]\norigin = \"derived\"\nentity = [\"DUID\"]\nvalid = { kind = \"interval_end\", column = \"SETTLEMENTDATE\" }\norder = [\"LASTCHANGED\"]\ncutoff = \"LASTCHANGED\"\n\n[[schema]]"
            ),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back, t);
        assert_eq!(write_table(&back), text);
        // A column note and every optional key render and read back.
        let mut all = t.clone();
        all.columns[2].note = Some("a \"quoted\" note".into());
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
        all.temporal.valid = ValidAxis::From {
            column: "settlementdate".into(),
            until: None,
        };
        let back = parse(&write_table(&all)).unwrap();
        assert_eq!(
            back.temporal.valid,
            ValidAxis::From {
                column: "SETTLEMENTDATE".into(),
                until: None
            },
            "normalised"
        );
        // An end column round-trips with its version's start; it belongs
        // to a `from` axis only.
        all.temporal.valid = ValidAxis::From {
            column: "settlementdate".into(),
            until: Some("duid".into()),
        };
        let text = write_table(&all);
        assert!(
            text.contains(
                "valid = { kind = \"from\", column = \"settlementdate\", until = \"duid\" }"
            ),
            "{text}"
        );
        assert_eq!(
            parse(&text).unwrap().temporal.valid,
            ValidAxis::From {
                column: "SETTLEMENTDATE".into(),
                until: Some("DUID".into())
            }
        );
        let wrong = text.replace("kind = \"from\"", "kind = \"instant\"");
        let e = parse(&wrong).unwrap_err();
        assert!(format!("{e}").contains("until"), "{e}");
        // A schema with no run has no month.
        let mut lone = t;
        lone.runs.clear();
        let text = write_table(&lone);
        assert!(text.contains("id = 0\nversions = []\n"), "{text}");
    }

    #[test]
    fn reads_hand_written_forms() {
        // `[[column]]` tables, a missing nullable (true), lower-case names,
        // a schema with no month and two versions.
        let text = r#"
schema_format = 4
name = "region"
report_pairs = [["dispatch", "region"]]
load_mode = "snapshot"
primary_key = ["regionid"]

[[column]]
id = 2
name = "\"Description\""
type = "string"

[[column]]
id = 1
name = "regionid"
type = "string"
nullable = false

[temporal]
origin = "hand"
entity = ["regionid"]
valid = { kind = "timeless" }

[[schema]]
id = 0
versions = [1, 2]
columns = ["REGIONID", "description"]
"#;
        let t = parse(text).unwrap();
        assert_eq!(t.name, "REGION");
        assert_eq!(t.report_pairs, [("DISPATCH".into(), "REGION".into())]);
        assert_eq!(t.primary_key, ["REGIONID"]);
        assert_eq!(t.columns[0].name, "REGIONID", "by id");
        assert_eq!(t.columns[1].name, "DESCRIPTION");
        assert!(t.columns[1].nullable);
        assert_eq!(t.temporal.entity, ["REGIONID"]);
        assert_eq!(
            t.schemas[0].fields,
            [(ColumnId(1), true), (ColumnId(2), false)]
        );
        assert_eq!(t.schemas[0].key, [ColumnId(1)]);
        assert_eq!(t.runs.len(), 2);
        assert_eq!(t.runs[1].version, 2);
        assert_eq!(t.runs[0].from.to_string(), "1998-12");
    }

    #[test]
    fn rejects_unknown_keys_old_formats_and_bad_types() {
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
        let e = parse(&text.replace("schema_format = 4", "schema_format = 3")).unwrap_err();
        assert!(
            e.to_string().contains("[[increment]] lists are gone"),
            "{e}"
        );
        let e = parse(&text.replace("schema_format = 4", "schema_format = 1")).unwrap_err();
        assert!(e.to_string().contains("schema_format is 1"), "{e}");
        let e = parse(&text.replace("type = \"double\"", "type = \"number\"")).unwrap_err();
        assert!(e.to_string().contains("number"), "{e}");
        let e = parse(&text.replace("versions = [1]", "versions = []")).unwrap_err();
        assert!(e.to_string().contains("lists no versions"), "{e}");
        let e = parse(&text.replacen("    \"DUID\",\n", "    \"GHOST\",\n", 1)).unwrap_err();
        assert!(
            e.to_string().contains("GHOST, which is not a column"),
            "{e}"
        );
        let e = parse("schema_format = 4\nname = 3\n").unwrap_err();
        assert!(
            matches!(e, SchemaFileError::Parse { line: Some(2), .. }),
            "{e:?}"
        );
    }

    #[test]
    fn reads_from_disk_and_markets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(TABLE_FILE), write_table(&unit_scada())).unwrap();
        assert_eq!(read_table(dir.path(), Market::Nem).unwrap(), as_written());
        assert!(matches!(
            read_table(&dir.path().join("none"), Market::Nem),
            Err(SchemaFileError::Read { .. })
        ));
        let m = MarketDef {
            market: Market::Wem,
            utc_offset_secs: 28_800,
            timestamp_format: TimestampFormat::Iso8601,
            trading_day_start_secs: 28_800,
        };
        let mp = dir.path().join("market.toml");
        std::fs::write(&mp, write_market(&m)).unwrap();
        assert_eq!(read_market(&mp).unwrap(), m);
        // A file from before trading days were kept starts its day at midnight.
        let old = parse_market(
            &mp,
            "schema_format = 1\nmarket = \"wem\"\nutc_offset_secs = 28800\ntimestamp_format = \"iso8601\"\n",
        )
        .unwrap();
        assert_eq!(old.trading_day_start_secs, 0);
        assert!(read_market(&dir.path().join("none.toml")).is_err());
        let bad = write_market(&m).replace("schema_format = 1", "schema_format = 2");
        assert!(parse_market(&mp, &bad).is_err());
        assert!(parse_market(&mp, "schema_format = 1\nmarket = \"mars\"\n").is_err());
    }
}
