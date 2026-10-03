//! Reading and writing the TOML form of a table and of a market.
//!
//! The TOML shape is flat and human-first: top-level scalars, `report_pairs`
//! as an array of two-element arrays, `[[column]]` and `[[version]]` arrays
//! of tables with `type = "number", precision = 15, scale = 5`, and a
//! `[provenance]` table. The file structs here mirror that shape exactly and
//! convert to and from the in-memory [`TableDef`]; writing is deterministic
//! (declaration order), so a regenerated file that changes nothing is
//! byte-identical. Unknown keys are errors.

use std::path::Path;

use nemlake_core::{Fingerprint, YearMonth, normalize_name};
use serde::{Deserialize, Serialize};

use crate::error::SchemaFileError;
use crate::model::{
    ColumnDef, ColumnId, ColumnType, Dedupe, LoadMode, Market, MarketDef, Provenance,
    SchemaVersion, SourceKind, TableDef, TimestampFormat, VersionDef, Visibility,
};

/// The format version this crate reads and writes.
pub const SCHEMA_FORMAT: u32 = 1;

const fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TableFile {
    schema_format: u32,
    market: Market,
    source: SourceKind,
    name: String,
    package: String,
    visibility: Visibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    report_pairs: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    datasets: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    archive_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    feeds: Vec<String>,
    load_mode: LoadMode,
    dedupe: Dedupe,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    primary_key: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    time_column: Option<String>,
    next_column_id: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deprecated: bool,
    #[serde(rename = "column")]
    columns: Vec<ColumnFile>,
    #[serde(rename = "version")]
    versions: Vec<VersionFile>,
    #[serde(default, skip_serializing_if = "is_default_provenance")]
    provenance: ProvenanceFile,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnFile {
    id: u32,
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    #[serde(rename = "type")]
    ty: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_len: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    dict: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    precision: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scale: Option<u8>,
    #[serde(default = "default_true", skip_serializing_if = "Clone::clone")]
    nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oracle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loader: Option<String>,
    added_in: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    removed_in: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    src: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    curated: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionFile {
    n: u16,
    columns: Vec<String>,
    fingerprint: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_seen: Option<YearMonth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen: Option<YearMonth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    src: Option<String>,
}

#[derive(Serialize, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields, default)]
struct ProvenanceFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    ctl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_model: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sampled_months: Vec<YearMonth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

fn is_default_provenance(p: &ProvenanceFile) -> bool {
    *p == ProvenanceFile::default()
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

fn invalid(path: &Path, table: &str, message: impl Into<String>) -> SchemaFileError {
    SchemaFileError::Invalid {
        path: path.display().to_string(),
        table: table.to_owned(),
        message: message.into(),
    }
}

fn column_type(path: &Path, table: &str, c: &ColumnFile) -> Result<ColumnType, SchemaFileError> {
    let bad_key = |key: &str| {
        invalid(
            path,
            table,
            format!(
                "column {} has type {} but sets {key}, which belongs to another type",
                c.name, c.ty
            ),
        )
    };
    let ty = match c.ty.as_str() {
        "utf8" => {
            if c.precision.is_some() || c.scale.is_some() {
                return Err(bad_key("precision/scale"));
            }
            ColumnType::Utf8 {
                max_len: c.max_len,
                dict: c.dict,
            }
        }
        "number" => {
            if c.max_len.is_some() || c.dict {
                return Err(bad_key("max_len/dict"));
            }
            ColumnType::Number {
                precision: c.precision,
                scale: c.scale,
            }
        }
        "integer" => ColumnType::Integer,
        "timestamp" => ColumnType::Timestamp,
        "date" => ColumnType::Date,
        "boolean" => ColumnType::Boolean,
        other => {
            return Err(invalid(
                path,
                table,
                format!(
                    "column {} has unknown type {other:?}; one of utf8, number, integer, timestamp, date, boolean",
                    c.name
                ),
            ));
        }
    };
    if !matches!(ty, ColumnType::Utf8 { .. } | ColumnType::Number { .. })
        && (c.max_len.is_some() || c.dict || c.precision.is_some() || c.scale.is_some())
    {
        return Err(bad_key("max_len/dict/precision/scale"));
    }
    Ok(ty)
}

fn norm(s: &mut String, buf: &mut String) {
    if let Some(n) = normalize_name(s, buf)
        && n != s.as_str()
    {
        *s = n.to_owned();
    }
}

fn to_model(path: &Path, f: TableFile) -> Result<TableDef, SchemaFileError> {
    if f.schema_format != SCHEMA_FORMAT {
        return Err(invalid(
            path,
            &f.name,
            format!(
                "schema_format is {} but this nemlake reads {SCHEMA_FORMAT}",
                f.schema_format
            ),
        ));
    }
    let mut buf = String::new();
    let mut columns = Vec::with_capacity(f.columns.len());
    for c in &f.columns {
        let ty = column_type(path, &f.name, c)?;
        let mut name = c.name.clone();
        norm(&mut name, &mut buf);
        let mut aliases = c.aliases.clone();
        aliases.iter_mut().for_each(|a| norm(a, &mut buf));
        columns.push(ColumnDef {
            id: ColumnId(c.id),
            name,
            aliases,
            ty,
            nullable: c.nullable,
            oracle: c.oracle.clone(),
            loader: c.loader.clone(),
            added_in: SchemaVersion(c.added_in),
            removed_in: c.removed_in.map(SchemaVersion),
            description: c.description.clone(),
            src: c.src.clone(),
            curated: c.curated,
        });
    }
    let versions = f
        .versions
        .into_iter()
        .map(|v| {
            let mut columns = v.columns;
            columns.iter_mut().for_each(|c| norm(c, &mut buf));
            VersionDef {
                n: SchemaVersion(v.n),
                columns,
                fingerprint: v.fingerprint,
                first_seen: v.first_seen,
                last_seen: v.last_seen,
                src: v.src,
            }
        })
        .collect();
    let mut t = TableDef {
        market: f.market,
        source: f.source,
        name: f.name,
        package: f.package,
        visibility: f.visibility,
        description: f.description,
        report_pairs: f.report_pairs,
        datasets: f.datasets,
        archive_files: f.archive_files,
        feeds: f.feeds,
        load_mode: f.load_mode,
        dedupe: f.dedupe,
        primary_key: f.primary_key,
        time_column: f.time_column,
        next_column_id: f.next_column_id,
        deprecated: f.deprecated,
        columns,
        versions,
        provenance: Provenance {
            ctl: f.provenance.ctl,
            data_model: f.provenance.data_model,
            sampled_months: f.provenance.sampled_months,
            generated: f.provenance.generated,
            note: f.provenance.note,
        },
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
    Ok(t)
}

fn from_model(t: &TableDef) -> TableFile {
    TableFile {
        schema_format: SCHEMA_FORMAT,
        market: t.market,
        source: t.source,
        name: t.name.clone(),
        package: t.package.clone(),
        visibility: t.visibility,
        description: t.description.clone(),
        report_pairs: t.report_pairs.clone(),
        datasets: t.datasets.clone(),
        archive_files: t.archive_files.clone(),
        feeds: t.feeds.clone(),
        load_mode: t.load_mode,
        dedupe: t.dedupe,
        primary_key: t.primary_key.clone(),
        time_column: t.time_column.clone(),
        next_column_id: t.next_column_id,
        deprecated: t.deprecated,
        columns: t
            .columns
            .iter()
            .map(|c| {
                let (ty, max_len, dict, precision, scale) = match c.ty {
                    ColumnType::Utf8 { max_len, dict } => ("utf8", max_len, dict, None, None),
                    ColumnType::Number { precision, scale } => {
                        ("number", None, false, precision, scale)
                    }
                    ColumnType::Integer => ("integer", None, false, None, None),
                    ColumnType::Timestamp => ("timestamp", None, false, None, None),
                    ColumnType::Date => ("date", None, false, None, None),
                    ColumnType::Boolean => ("boolean", None, false, None, None),
                };
                ColumnFile {
                    id: c.id.0,
                    name: c.name.clone(),
                    aliases: c.aliases.clone(),
                    ty: ty.to_owned(),
                    max_len,
                    dict,
                    precision,
                    scale,
                    nullable: c.nullable,
                    oracle: c.oracle.clone(),
                    loader: c.loader.clone(),
                    added_in: c.added_in.0,
                    removed_in: c.removed_in.map(|v| v.0),
                    description: c.description.clone(),
                    src: c.src.clone(),
                    curated: c.curated,
                }
            })
            .collect(),
        versions: t
            .versions
            .iter()
            .map(|v| VersionFile {
                n: v.n.0,
                columns: v.columns.clone(),
                fingerprint: v.fingerprint,
                first_seen: v.first_seen,
                last_seen: v.last_seen,
                src: v.src.clone(),
            })
            .collect(),
        provenance: ProvenanceFile {
            ctl: t.provenance.ctl.clone(),
            data_model: t.provenance.data_model.clone(),
            sampled_months: t.provenance.sampled_months.clone(),
            generated: t.provenance.generated.clone(),
            note: t.provenance.note.clone(),
        },
    }
}

/// Parses a table file's text. Names are normalised so a hand-typed
/// lower-case name still matches an I row.
pub fn parse_table(path: &Path, text: &str) -> Result<TableDef, SchemaFileError> {
    let f: TableFile = toml::from_str(text).map_err(|e| parse_err(path, text, &e))?;
    to_model(path, f)
}

/// Reads and parses a table file.
pub fn read_table(path: &Path) -> Result<TableDef, SchemaFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    parse_table(path, &text)
}

/// Renders a table as TOML, deterministically.
pub fn write_table(t: &TableDef) -> String {
    let body = toml::to_string_pretty(&from_model(t)).expect("a TableDef always serialises");
    format!(
        "# Generated by `nemlake-schema-bootstrap regen`; hand edits go in fields marked `curated = true`.\n{body}"
    )
}

/// Parses a market file's text.
pub fn parse_market(path: &Path, text: &str) -> Result<MarketDef, SchemaFileError> {
    let f: MarketFile = toml::from_str(text).map_err(|e| parse_err(path, text, &e))?;
    if f.schema_format != SCHEMA_FORMAT {
        return Err(invalid(
            path,
            f.market.as_str(),
            format!(
                "schema_format is {} but this nemlake reads {SCHEMA_FORMAT}",
                f.schema_format
            ),
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
        schema_format: SCHEMA_FORMAT,
        market: m.market,
        utc_offset_secs: m.utc_offset_secs,
        timestamp_format: m.timestamp_format,
    };
    toml::to_string_pretty(&f).expect("a MarketDef always serialises")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A valid DISPATCH_UNIT_SCADA definition for tests across the crate.
    pub(crate) fn unit_scada() -> TableDef {
        let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        TableDef {
            market: Market::Nem,
            source: SourceKind::Mms,
            name: "DISPATCH_UNIT_SCADA".into(),
            package: "DISPATCH".into(),
            visibility: Visibility::Public,
            description: Some("SCADA MW per unit.".into()),
            report_pairs: vec![("DISPATCH".into(), "UNIT_SCADA".into())],
            datasets: vec![],
            archive_files: vec!["DISPATCH_UNIT_SCADA".into()],
            feeds: vec!["Dispatch_SCADA".into()],
            load_mode: LoadMode::Append,
            dedupe: Dedupe::Pk,
            primary_key: vec!["SETTLEMENTDATE".into(), "DUID".into()],
            time_column: Some("SETTLEMENTDATE".into()),
            next_column_id: 4,
            deprecated: false,
            columns: vec![
                ColumnDef {
                    id: ColumnId(1),
                    name: "SETTLEMENTDATE".into(),
                    aliases: vec![],
                    ty: ColumnType::Timestamp,
                    nullable: false,
                    oracle: Some("DATE".into()),
                    loader: Some("DATE \"yyyy/mm/dd hh24:mi:ss\"".into()),
                    added_in: SchemaVersion(1),
                    removed_in: None,
                    description: None,
                    src: vec!["ctl".into(), "irow".into()],
                    curated: false,
                },
                ColumnDef {
                    id: ColumnId(2),
                    name: "DUID".into(),
                    aliases: vec![],
                    ty: ColumnType::Utf8 {
                        max_len: Some(20),
                        dict: true,
                    },
                    nullable: false,
                    oracle: Some("VARCHAR2(20)".into()),
                    loader: Some("CHAR(20)".into()),
                    added_in: SchemaVersion(1),
                    removed_in: None,
                    description: None,
                    src: vec![],
                    curated: false,
                },
                ColumnDef {
                    id: ColumnId(3),
                    name: "SCADAVALUE".into(),
                    aliases: vec![],
                    ty: ColumnType::Number {
                        precision: Some(15),
                        scale: Some(5),
                    },
                    nullable: true,
                    oracle: Some("NUMBER(15,5)".into()),
                    loader: Some("FLOAT EXTERNAL".into()),
                    added_in: SchemaVersion(1),
                    removed_in: None,
                    description: None,
                    src: vec![],
                    curated: false,
                },
            ],
            versions: vec![VersionDef {
                n: SchemaVersion(1),
                columns: cols.iter().map(|s| (*s).to_owned()).collect(),
                fingerprint: Fingerprint::of_names(cols),
                first_seen: Some("2009-07".parse().unwrap()),
                last_seen: None,
                src: Some("irow:MMSDM_2024_01".into()),
            }],
            provenance: Provenance {
                ctl: Some("x.ctl".into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn round_trips_and_is_deterministic() {
        let t = unit_scada();
        let text = write_table(&t);
        assert!(text.contains("[[column]]"), "{text}");
        assert!(text.contains("type = \"number\""), "{text}");
        assert!(text.contains("precision = 15"), "{text}");
        assert!(
            text.contains("report_pairs = [[") && text.contains("\"UNIT_SCADA\","),
            "{text}"
        );
        assert!(
            !text.contains("nullable = true"),
            "default nullable must be omitted: {text}"
        );
        let back = parse_table(Path::new("x.toml"), &text).unwrap();
        assert_eq!(back, t);
        assert_eq!(write_table(&back), text);
        // Every type renders and reads back.
        let mut all = unit_scada();
        for (i, ty) in [
            ColumnType::Integer,
            ColumnType::Date,
            ColumnType::Boolean,
            ColumnType::Utf8 {
                max_len: None,
                dict: false,
            },
        ]
        .into_iter()
        .enumerate()
        {
            all.columns[i % 3].ty = ty;
            let back = parse_table(Path::new("x.toml"), &write_table(&all)).unwrap();
            assert_eq!(back.columns[i % 3].ty, ty);
        }
        // Empty provenance is omitted entirely.
        let mut bare = unit_scada();
        bare.provenance = Provenance::default();
        assert!(!write_table(&bare).contains("[provenance]"));
    }

    #[test]
    fn normalises_names_on_read() {
        let mut t = unit_scada();
        t.name = "dispatch_unit_scada".into();
        t.report_pairs[0].1 = "unit_scada".into();
        t.columns[1].name = "duid".into();
        t.columns[1].aliases.push("old_duid".into());
        t.versions[0].columns[1] = "\"duid\"".into();
        t.primary_key[1] = "duid".into();
        t.time_column = Some("settlementdate".into());
        let back = parse_table(Path::new("x.toml"), &write_table(&t)).unwrap();
        assert_eq!(back.name, "DISPATCH_UNIT_SCADA");
        assert_eq!(back.report_pairs[0].1, "UNIT_SCADA");
        assert_eq!(back.columns[1].name, "DUID");
        assert_eq!(back.columns[1].aliases, vec!["OLD_DUID".to_owned()]);
        assert_eq!(back.versions[0].columns[1], "DUID");
        assert_eq!(back.primary_key[1], "DUID");
        assert_eq!(back.time_column.as_deref(), Some("SETTLEMENTDATE"));
    }

    #[test]
    fn rejects_unknown_keys_wrong_format_and_bad_types() {
        let text = write_table(&unit_scada()).replace("package =", "pakage =");
        let e = parse_table(Path::new("x.toml"), &text).unwrap_err();
        assert!(matches!(e, SchemaFileError::Parse { .. }), "{e}");
        assert!(e.to_string().contains("pakage"), "{e}");
        let text = write_table(&unit_scada()).replace("schema_format = 1", "schema_format = 9");
        let e = parse_table(Path::new("x.toml"), &text).unwrap_err();
        assert!(e.to_string().contains("schema_format is 9"), "{e}");
        let e = parse_table(Path::new("x.toml"), "schema_format = 1\nname = 3\n").unwrap_err();
        assert!(
            matches!(e, SchemaFileError::Parse { line: Some(2), .. }),
            "{e:?}"
        );
        let text = write_table(&unit_scada()).replace("type = \"number\"", "type = \"float\"");
        let e = parse_table(Path::new("x.toml"), &text).unwrap_err();
        assert!(e.to_string().contains("unknown type \"float\""), "{e}");
        let text = write_table(&unit_scada()).replace(
            "type = \"timestamp\"",
            "type = \"timestamp\"\nprecision = 3",
        );
        let e = parse_table(Path::new("x.toml"), &text).unwrap_err();
        assert!(e.to_string().contains("belongs to another type"), "{e}");
        let text = write_table(&unit_scada())
            .replace("type = \"number\"", "type = \"number\"\nmax_len = 3");
        assert!(parse_table(Path::new("x.toml"), &text).is_err());
        let text = write_table(&unit_scada()).replace("dict = true", "dict = true\nscale = 1");
        assert!(parse_table(Path::new("x.toml"), &text).is_err());
    }

    #[test]
    fn market_file() {
        let m = MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
        };
        let text = write_market(&m);
        assert!(text.contains("market = \"nem\""), "{text}");
        assert_eq!(parse_market(Path::new("m.toml"), &text).unwrap(), m);
        let bad = text.replace("schema_format = 1", "schema_format = 2");
        assert!(parse_market(Path::new("m.toml"), &bad).is_err());
        assert!(
            parse_market(
                Path::new("m.toml"),
                "schema_format = 1\nmarket = \"mars\"\n"
            )
            .is_err()
        );
    }

    #[test]
    fn reads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("T.toml");
        std::fs::write(&p, write_table(&unit_scada())).unwrap();
        assert_eq!(read_table(&p).unwrap(), unit_scada());
        assert!(matches!(
            read_table(&dir.path().join("none.toml")),
            Err(SchemaFileError::Read { .. })
        ));
        let mp = dir.path().join("market.toml");
        let m = MarketDef {
            market: Market::Wem,
            utc_offset_secs: 28_800,
            timestamp_format: TimestampFormat::Iso8601,
        };
        std::fs::write(&mp, write_market(&m)).unwrap();
        assert_eq!(read_market(&mp).unwrap(), m);
        assert!(read_market(&dir.path().join("none.toml")).is_err());
    }
}
