//! The record of a table's shapes: `schemas/{market}/shapes/{TABLE}.toml`.
//!
//! Every distinct shape of the table seen in the archive, once, folded from
//! every census file (`utilake-nem-census fold`): the AEMO versions and
//! kinds of file it came in, the exact file and time it was first and last
//! seen, and what each column was observed to hold, beside what AEMO's
//! control files declared. The registry embeds these files, so resolution
//! can tell which shape a block is, and the census gate can hold every
//! shape to our schema.

use std::path::Path;

use crate::shape::{CanonicalShape, ShapeId, TypeClass};
use serde::{Deserialize, Serialize};
use utilake_core::{Fingerprint, YearMonth};
use utilake_iceberg::Kind;

use crate::file_model::error::SchemaFileError;

/// A point in the archive: the file a shape was seen in, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mark {
    /// The archive month the file is of.
    pub month: YearMonth,
    /// The file, as the census names it.
    pub file: String,
    /// The sha256 of the archived file it was read out of.
    #[serde(default)]
    pub sha256: Option<String>,
    /// When AEMO published that file (its HTTP `Last-Modified`), RFC 3339.
    #[serde(default)]
    pub published: Option<String>,
    /// The least (in a `from`) or greatest (in a `to`) value of the block's
    /// time column, RFC 3339 in market time.
    #[serde(default)]
    pub data_time: Option<String>,
}

/// What one column of a shape was observed to hold over every file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapeColumn {
    /// The name, normalised.
    pub name: String,
    /// The class AEMO's control files declared; `unknown` when none did.
    pub ctl: TypeClass,
    /// What every value was: the join of every block's kind.
    pub kind: Kind,
    /// Rows without a value.
    pub nulls: u64,
    /// The first file a null was seen in.
    #[serde(default)]
    pub nulls_since: Option<String>,
    /// The most digits before the point.
    pub int_digits: u8,
    /// The most digits after it.
    pub scale: u8,
    /// The most significant digits of any one value.
    #[serde(default)]
    pub sig_digits: u8,
    /// The longest value in bytes.
    pub max_len: u32,
    /// The least value of a number or time, as written.
    #[serde(default)]
    pub min: Option<String>,
    /// The greatest value of a number or time, as written.
    #[serde(default)]
    pub max: Option<String>,
}

impl ShapeColumn {
    /// What the observation says of the column's type.
    pub fn evidence(&self) -> utilake_iceberg::Evidence {
        utilake_iceberg::Evidence {
            kind: self.kind,
            int_digits: self.int_digits,
            scale: self.scale,
            sig_digits: self.sig_digits,
        }
    }
}

/// One distinct shape of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapeEntry {
    /// The shape's id.
    pub id: ShapeId,
    /// Report type.
    pub report_type: String,
    /// Report subtype.
    pub report_subtype: String,
    /// Every version AEMO stamped on it, ascending.
    pub versions: Vec<u16>,
    /// The kinds of file it came in (`dvd`, `archive`, `#all` for a file of
    /// every run), ascending.
    pub file_kinds: Vec<String>,
    /// The first month it was seen in.
    pub first_seen: YearMonth,
    /// The last month it was seen in.
    pub last_seen: YearMonth,
    /// Where it was first seen.
    pub from: Mark,
    /// Where it was last seen.
    pub to: Mark,
    /// Files it was seen in.
    pub files: u64,
    /// D rows over every file.
    pub rows: u64,
    /// Other shapes of the table seen in some of the same months.
    pub concurrent_with: Vec<ShapeId>,
    /// Columns in file order.
    pub columns: Vec<ShapeColumn>,
}

impl ShapeEntry {
    /// The canonical shape the entry records.
    pub fn shape(&self) -> CanonicalShape {
        CanonicalShape {
            report_type: self.report_type.clone(),
            report_subtype: self.report_subtype.clone(),
            columns: self.names().map(str::to_owned).collect(),
        }
    }

    /// Column names in file order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.columns.iter().map(|c| c.name.as_str())
    }

    /// What was observed of the column `name`.
    pub fn evidence_of(&self, name: &str) -> Option<utilake_iceberg::Evidence> {
        self.columns
            .iter()
            .find(|c| c.name == name)
            .map(ShapeColumn::evidence)
    }

    /// The fingerprint of its column names, which resolution keys on.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of_names(self.names())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapesFile {
    #[serde(default)]
    shape: Vec<ShapeEntry>,
}

/// The first line of every shapes file.
pub const SHAPES_HEADER: &str = "# Generated by `utilake-nem-census fold` from every census file: each distinct shape of the table, where it was first and last seen, and what each column held.\n";

/// Parses a shapes file's text; `path` names it in errors.
pub fn parse_shapes(path: &Path, text: &str) -> Result<Vec<ShapeEntry>, SchemaFileError> {
    toml::from_str::<ShapesFile>(text)
        .map(|f| f.shape)
        .map_err(|e| SchemaFileError::Parse {
            path: path.display().to_string(),
            message: e.message().to_owned(),
            line: e
                .span()
                .map(|s| text[..s.start].bytes().filter(|&b| b == b'\n').count() as u64 + 1),
        })
}

/// Reads a shapes file.
pub fn read_shapes(path: &Path) -> Result<Vec<ShapeEntry>, SchemaFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    parse_shapes(path, &text)
}

/// A value as TOML writes it inline.
fn inline<T: Serialize>(v: &T) -> String {
    toml::Value::try_from(v)
        .expect("a shape serialises")
        .to_string()
}

/// An inline table of `(key, value)` pairs in the order given, leaving out
/// absent values.
fn table(pairs: &[(&str, Option<String>)]) -> String {
    let body: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k} = {v}")))
        .collect();
    format!("{{ {} }}", body.join(", "))
}

fn mark(m: &Mark) -> String {
    table(&[
        ("month", Some(inline(&m.month.to_string()))),
        ("file", Some(inline(&m.file))),
        ("sha256", m.sha256.as_ref().map(inline)),
        ("published", m.published.as_ref().map(inline)),
        ("data_time", m.data_time.as_ref().map(inline)),
    ])
}

fn column(c: &ShapeColumn) -> String {
    table(&[
        ("name", Some(inline(&c.name))),
        ("ctl", Some(inline(&c.ctl))),
        ("kind", Some(inline(&c.kind))),
        ("nulls", Some(c.nulls.to_string())),
        ("nulls_since", c.nulls_since.as_ref().map(inline)),
        ("int_digits", Some(c.int_digits.to_string())),
        ("scale", Some(c.scale.to_string())),
        ("sig_digits", Some(c.sig_digits.to_string())),
        ("max_len", Some(c.max_len.to_string())),
        ("min", c.min.as_ref().map(inline)),
        ("max", c.max.as_ref().map(inline)),
    ])
}

/// A shapes file's text: the header, then each shape with one column a
/// line.
pub fn render_shapes(shapes: &[ShapeEntry]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(SHAPES_HEADER);
    for e in shapes {
        let _ = write!(
            out,
            "\n[[shape]]\nid = \"{}\"\nreport_type = {}\nreport_subtype = {}\nversions = {}\nfile_kinds = {}\nfirst_seen = \"{}\"\nlast_seen = \"{}\"\nfrom = {}\nto = {}\nfiles = {}\nrows = {}\nconcurrent_with = {}\ncolumns = [\n",
            e.id,
            inline(&e.report_type),
            inline(&e.report_subtype),
            inline(&e.versions),
            inline(&e.file_kinds),
            e.first_seen,
            e.last_seen,
            mark(&e.from),
            mark(&e.to),
            e.files,
            e.rows,
            inline(&e.concurrent_with),
        );
        for c in &e.columns {
            let _ = writeln!(out, "    {},", column(c));
        }
        out.push_str("]\n");
    }
    out
}

/// What is wrong with a shapes file's entries: an id that is not its
/// shape's (the file was edited by hand), or months the wrong way round.
pub fn shape_issues(table: &str, shapes: &[ShapeEntry]) -> Vec<String> {
    let mut out = Vec::new();
    for e in shapes {
        let id = e.shape().id();
        if id != e.id {
            out.push(format!(
                "shapes of {table}: shape {} has the content of shape {id}; shapes files are written by `utilake-nem-census fold`, not by hand",
                e.id
            ));
        }
        if e.first_seen > e.last_seen {
            out.push(format!(
                "shapes of {table}: shape {} was last seen in {} before it was first seen in {}",
                e.id, e.last_seen, e.first_seen
            ));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn column(name: &str, ctl: TypeClass, kind: Kind) -> ShapeColumn {
        ShapeColumn {
            name: name.into(),
            ctl,
            kind,
            nulls: 0,
            nulls_since: None,
            int_digits: 0,
            scale: 0,
            sig_digits: 0,
            max_len: 0,
            min: None,
            max: None,
        }
    }

    fn mark(month: &str, file: &str) -> Mark {
        Mark {
            month: month.parse().unwrap(),
            file: file.into(),
            sha256: None,
            published: None,
            data_time: None,
        }
    }

    pub(crate) fn scada_entry() -> ShapeEntry {
        let mut scada = column("SCADAVALUE", TypeClass::Number, Kind::Decimal);
        scada.nulls = 3;
        scada.nulls_since = Some("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202402010000.zip".into());
        scada.min = Some("-1.5".into());
        scada.max = Some("400".into());
        let mut e = ShapeEntry {
            id: ShapeId(0),
            report_type: "DISPATCH".into(),
            report_subtype: "UNIT_SCADA".into(),
            versions: vec![1],
            file_kinds: vec!["dvd".into()],
            first_seen: "2024-01".parse().unwrap(),
            last_seen: "2026-08".parse().unwrap(),
            from: Mark {
                sha256: Some("ab".into()),
                published: Some("2024-02-03T01:02:03Z".into()),
                data_time: Some("2024-01-01T00:05:00+10:00".into()),
                ..mark("2024-01", "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip")
            },
            to: mark(
                "2026-08",
                "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.zip",
            ),
            files: 32,
            rows: 1000,
            concurrent_with: Vec::new(),
            columns: vec![
                column("SETTLEMENTDATE", TypeClass::Timestamp, Kind::Timestamp),
                column("DUID", TypeClass::Varchar, Kind::Text),
                scada,
            ],
        };
        e.id = e.shape().id();
        e
    }

    #[test]
    fn renders_parses_and_checks() {
        let e = scada_entry();
        assert_eq!(e.evidence_of("SCADAVALUE").unwrap().kind, Kind::Decimal);
        let text = render_shapes(std::slice::from_ref(&e));
        assert!(text.starts_with(SHAPES_HEADER));
        assert!(
            text.contains("    { name = \"DUID\", ctl = \"varchar\", kind = \"text\""),
            "{text}"
        );
        let back = parse_shapes(Path::new("x.toml"), &text).unwrap();
        assert_eq!(back, std::slice::from_ref(&e));
        assert_eq!(render_shapes(&back), text);
        assert_eq!(
            e.fingerprint(),
            Fingerprint::of_names(["SETTLEMENTDATE", "DUID", "SCADAVALUE"])
        );
        assert!(shape_issues("T", &back).is_empty());
        let mut edited = e.clone();
        edited.columns.pop();
        edited.first_seen = "2027-01".parse().unwrap();
        let issues = shape_issues("T", &[edited]);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].contains("not by hand"));
        let bad = text.replace("files = 32", "files = \"many\"");
        let err = parse_shapes(Path::new("x.toml"), &bad).unwrap_err();
        assert!(
            matches!(err, SchemaFileError::Parse { line: Some(_), .. }),
            "{err}"
        );
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("T.toml");
        std::fs::write(&p, &text).unwrap();
        assert_eq!(read_shapes(&p).unwrap().len(), 1);
        assert!(read_shapes(&dir.path().join("none.toml")).is_err());
    }
}
