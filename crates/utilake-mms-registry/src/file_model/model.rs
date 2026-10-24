//! In-memory shape of a table: its columns, and its history as an Iceberg
//! table — the schemas its files were written under and the runs of files
//! of each.

use std::fmt;
use utilake_iceberg::IcebergType;

use serde::{Deserialize, Serialize};
use utilake_core::YearMonth;

use crate::file_model::temporal::Temporal;

/// The market a table belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Market {
    /// National Electricity Market (eastern states).
    Nem,
    /// Wholesale Electricity Market (Western Australia).
    Wem,
}

impl Market {
    /// The directory name under `schemas/` and `lake/`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nem => "nem",
            Self::Wem => "wem",
        }
    }
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-market constants, from `schemas/{market}/market.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketDef {
    /// The market.
    pub market: Market,
    /// Fixed UTC offset of market time in seconds, e.g. 36000 for AEST.
    pub utc_offset_secs: i32,
    /// How timestamps are written in this market's files.
    pub timestamp_format: TimestampFormat,
    /// Seconds after midnight at which the market's trading day begins:
    /// 14400 (04:00) for the NEM, 28800 (08:00) for the WEM. A table that
    /// labels its rows with a trading day (`valid.kind = "day"`) covers
    /// `[label + start, label + start + 1 day)`.
    pub trading_day_start_secs: i32,
}

impl MarketDef {
    /// The offset as Arrow writes it, e.g. `+10:00`.
    pub fn arrow_tz(&self) -> String {
        let sign = if self.utc_offset_secs < 0 { '-' } else { '+' };
        let abs = self.utc_offset_secs.unsigned_abs();
        format!("{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60)
    }
}

/// Whether monthly files append rows or replace the whole table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadMode {
    /// Each file adds rows for its period.
    Append,
    /// Each file is a full snapshot of standing data.
    Snapshot,
}

/// How a timestamp column is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimestampFormat {
    /// `yyyy/mm/dd hh24:mi:ss`, the MMS form.
    #[default]
    AemoSlash,
    /// `yyyy-mm-ddThh:mm:ss`, used by WEM.
    Iso8601,
}

/// A stable column identifier, never reused within a table: the Iceberg
/// field id of the table's history, and the Parquet field id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnId(pub u32);

impl fmt::Display for ColumnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The exact type of a lake column, one to one with an Iceberg type.
///
/// | type           | Arrow                      | Iceberg V3       |
/// |----------------|----------------------------|------------------|
/// | `string`       | `Utf8`                     | `string`         |
/// | `double`       | `Float64`                  | `double`         |
/// | `long`         | `Int64`                    | `long`           |
/// | `boolean`      | `Boolean`                  | `boolean`        |
/// | `date`         | `Date32`                   | `date`           |
/// | `timestamp`    | `Timestamp(µs, market tz)` | `timestamptz`    |
/// | `decimal(P,S)` | `Decimal128(P, S)`         | `decimal(P, S)`  |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum LakeType {
    /// Text.
    String,
    /// A 64-bit float.
    Double,
    /// A 64-bit integer.
    Long,
    /// `true` or `false`.
    Boolean,
    /// A day.
    Date,
    /// An instant, written in market time.
    Timestamp,
    /// An exact decimal: `precision` digits in all, `scale` after the
    /// point, fixed for the life of the column.
    Decimal {
        /// Digits in all, 1 to 38.
        precision: u8,
        /// Digits after the point.
        scale: u8,
    },
}

impl LakeType {
    /// Every type, in the order of the table above, with one decimal.
    pub const ALL: [Self; 7] = [
        Self::String,
        Self::Double,
        Self::Long,
        Self::Boolean,
        Self::Date,
        Self::Timestamp,
        Self::Decimal {
            precision: 15,
            scale: 5,
        },
    ];

    /// The name in `table.toml`: `decimal(15,5)` for a decimal.
    pub fn name(self) -> String {
        match self {
            Self::String => "string".into(),
            Self::Double => "double".into(),
            Self::Long => "long".into(),
            Self::Boolean => "boolean".into(),
            Self::Date => "date".into(),
            Self::Timestamp => "timestamp".into(),
            Self::Decimal { precision, scale } => format!("decimal({precision},{scale})"),
        }
    }

    /// Reads a name from [`LakeType::name`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "string" => Self::String,
            "double" => Self::Double,
            "long" => Self::Long,
            "boolean" => Self::Boolean,
            "date" => Self::Date,
            "timestamp" => Self::Timestamp,
            other => {
                let IcebergType::Decimal { precision, scale } = IcebergType::parse(other)? else {
                    return None;
                };
                Self::Decimal { precision, scale }
            }
        })
    }

    /// The Iceberg V3 type.
    pub const fn iceberg(self) -> IcebergType {
        match self {
            Self::String => IcebergType::String,
            Self::Double => IcebergType::Double,
            Self::Long => IcebergType::Long,
            Self::Boolean => IcebergType::Boolean,
            Self::Date => IcebergType::Date,
            Self::Timestamp => IcebergType::Timestamptz,
            Self::Decimal { precision, scale } => IcebergType::Decimal { precision, scale },
        }
    }

    /// The lake type of an Iceberg type.
    pub const fn of_iceberg(t: IcebergType) -> Self {
        match t {
            IcebergType::String => Self::String,
            IcebergType::Double => Self::Double,
            IcebergType::Long => Self::Long,
            IcebergType::Boolean => Self::Boolean,
            IcebergType::Date => Self::Date,
            IcebergType::Timestamptz => Self::Timestamp,
            IcebergType::Decimal { precision, scale } => Self::Decimal { precision, scale },
        }
    }
}

impl fmt::Display for LakeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

impl TryFrom<String> for LakeType {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s).ok_or_else(|| {
            format!("{s:?} is not a lake type (string, double, long, boolean, date, timestamp, decimal(P,S))")
        })
    }
}

impl From<LakeType> for String {
    fn from(t: LakeType) -> Self {
        t.name()
    }
}

/// One lake column, ever: one field of the table's history, so its id is
/// never reused and a renamed column is a new column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// Stable id; the Iceberg and Parquet field id.
    pub id: ColumnId,
    /// Upper-case name, as AEMO's files write it.
    pub name: String,
    /// The type.
    pub ty: LakeType,
    /// Whether nulls are allowed: unless every schema of the history
    /// requires the field.
    pub nullable: bool,
    /// Why the column is what it is, when that is not plain.
    pub note: Option<String>,
}

/// One schema of the table's history: the columns of the files written
/// under it, in file order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSchema {
    /// The Iceberg schema id.
    pub id: u16,
    /// Its fields in file order: the column, and whether every row of its
    /// files has a value.
    pub fields: Vec<(ColumnId, bool)>,
    /// The fields of the key.
    pub key: Vec<ColumnId>,
}

/// Files in a row that AEMO released under one schema, version and
/// report pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// The schema.
    pub schema: u16,
    /// The report type and subtype.
    pub report: (String, String),
    /// AEMO's version of the report.
    pub version: u16,
    /// The kinds of file it came in (`archive`, `dvd`, …).
    pub kinds: Vec<String>,
    /// The month its first file was released in.
    pub from: YearMonth,
    /// The month its last was.
    pub to: YearMonth,
    /// Files in it.
    pub files: u64,
    /// Rows in them.
    pub rows: u64,
}

/// A table: its columns, and its history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// Market, from the directory the table is in.
    pub market: Market,
    /// Data-model name, e.g. `DISPATCH_UNIT_SCADA`.
    pub name: String,
    /// Description from the data model.
    pub description: Option<String>,
    /// `(report_type, report_subtype)` pairs whose I rows feed this table.
    pub report_pairs: Vec<(String, String)>,
    /// Append or snapshot.
    pub load_mode: LoadMode,
    /// What a fact is about, when it applies, and which revision is newest.
    pub temporal: Temporal<String>,
    /// Primary key column names.
    pub primary_key: Vec<String>,
    /// The column that partitions the table by month.
    pub time_column: Option<String>,
    /// Every lake column, by id.
    pub columns: Vec<Column>,
    /// The schemas of its history, by id.
    pub schemas: Vec<TableSchema>,
    /// The runs of its history, in release order.
    pub runs: Vec<Run>,
    /// The schema a reader of the table gets today: the published
    /// `current-schema-id`, the last run's.
    pub current_schema: u16,
}

impl Table {
    /// The column with this name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The column with this id.
    pub fn column_by_id(&self, id: ColumnId) -> Option<&Column> {
        self.columns.iter().find(|c| c.id == id)
    }

    /// Index into `columns` of this name.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// The schema with this id.
    pub fn schema(&self, id: u16) -> Option<&TableSchema> {
        self.schemas.iter().find(|s| s.id == id)
    }

    /// The names of a schema's columns, in file order.
    pub fn schema_columns(&self, s: &TableSchema) -> Vec<&str> {
        s.fields
            .iter()
            .filter_map(|(id, _)| self.column_by_id(*id))
            .map(|c| c.name.as_str())
            .collect()
    }

    /// The schema a block of `report` released in `release` is compared
    /// with when no run fits it: that of the last run of the pair released
    /// by then, else the pair's first, else the table's last run.
    pub fn reference_schema(
        &self,
        report: (&str, &str),
        release: Option<YearMonth>,
    ) -> Option<u16> {
        let of_pair = || {
            self.runs
                .iter()
                .filter(|r| r.report.0 == report.0 && r.report.1 == report.1)
        };
        of_pair()
            .rev()
            .find(|r| release.is_none_or(|m| r.from <= m))
            .or_else(|| of_pair().next())
            .or_else(|| self.runs.last())
            .map(|r| r.schema)
    }

    /// Whether a run of `report` came under AEMO version `version`.
    pub fn has_version(&self, report: (&str, &str), version: u16) -> bool {
        self.runs
            .iter()
            .any(|r| r.report.0 == report.0 && r.report.1 == report.1 && r.version == version)
    }

    /// The history as an Iceberg table, the form `History::resolve` and
    /// the planner read. Only what the registry keeps is filled in: a mark
    /// has its month, not its file, and no column breaks (a table with a
    /// breaking column is refused on reading, see [`mod@super::doc`]).
    pub fn history(&self) -> utilake_iceberg::History {
        let last = self.columns.iter().map(|c| c.id.0).max().unwrap_or(0);
        let mark = |m: YearMonth| utilake_iceberg::history::Mark {
            file: String::new(),
            sha256: String::new(),
            release: m.to_string(),
            data_time: None,
        };
        utilake_iceberg::History {
            table: self.name.clone(),
            format_version: 3,
            current_schema_id: u32::from(self.current_schema),
            // Every header of an MMS file is its column's name.
            headers: std::collections::BTreeMap::new(),
            schemas: self
                .schemas
                .iter()
                .map(|s| utilake_iceberg::Schema {
                    schema_id: u32::from(s.id),
                    fields: s
                        .fields
                        .iter()
                        .filter_map(|(id, required)| {
                            let c = self.column_by_id(*id)?;
                            Some(utilake_iceberg::Field {
                                id: c.id.0,
                                name: c.name.clone(),
                                ty: c.ty.iceberg(),
                                required: *required,
                                doc: None,
                            })
                        })
                        .collect(),
                    identifier_field_ids: s.key.iter().map(|k| k.0).collect(),
                    last_column_id: last,
                })
                .collect(),
            runs: self
                .runs
                .iter()
                .map(|r| utilake_iceberg::history::Run {
                    schema_id: u32::from(r.schema),
                    report: [r.report.0.clone(), r.report.1.clone()],
                    version: r.version,
                    kinds: r.kinds.clone(),
                    from: mark(r.from),
                    to: mark(r.to),
                    files: r.files,
                    rows: r.rows,
                    changes: Vec::new(),
                    updates: Vec::new(),
                    overlaps: false,
                })
                .collect(),
            breaking: Vec::new(),
        }
    }

    /// Lower-case name for lake directories.
    pub fn dir_name(&self) -> String {
        self.name.to_ascii_lowercase()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn month(s: &str) -> YearMonth {
        s.parse().unwrap()
    }

    fn col(id: u32, name: &str, ty: LakeType, nullable: bool) -> Column {
        Column {
            id: ColumnId(id),
            name: name.into(),
            ty,
            nullable,
            note: None,
        }
    }

    /// A run of DISPATCH_UNIT_SCADA.
    pub(crate) fn run(schema: u16, version: u16, from: &str, to: &str) -> Run {
        Run {
            schema,
            report: ("DISPATCH".into(), "UNIT_SCADA".into()),
            version,
            kinds: vec!["archive".into()],
            from: month(from),
            to: month(to),
            files: 2,
            rows: 9,
        }
    }

    /// DISPATCH_UNIT_SCADA in two schemas: three columns from 2024-01,
    /// four from 2026-08.
    pub(crate) fn unit_scada() -> Table {
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let key = vec![ColumnId(1), ColumnId(2)];
        let fields = |n: u32| (1..=n).map(|i| (ColumnId(i), i <= 2)).collect();
        Table {
            market: Market::Nem,
            name: "DISPATCH_UNIT_SCADA".into(),
            description: Some("SCADA MW per unit.".into()),
            report_pairs: vec![("DISPATCH".into(), "UNIT_SCADA".into())],
            load_mode: LoadMode::Append,
            temporal: Temporal {
                origin: crate::file_model::temporal::RoleOrigin::Derived,
                entity: names(&["DUID"]),
                valid: crate::file_model::temporal::ValidAxis::IntervalEnd("SETTLEMENTDATE".into()),
                order: names(&["LASTCHANGED"]),
                cutoff: Some("LASTCHANGED".into()),
            },
            primary_key: names(&["SETTLEMENTDATE", "DUID"]),
            time_column: Some("SETTLEMENTDATE".into()),
            columns: vec![
                col(1, "SETTLEMENTDATE", LakeType::Timestamp, false),
                col(2, "DUID", LakeType::String, false),
                col(3, "SCADAVALUE", LakeType::Double, true),
                col(4, "LASTCHANGED", LakeType::Timestamp, true),
            ],
            schemas: vec![
                TableSchema {
                    id: 0,
                    fields: fields(3),
                    key: key.clone(),
                },
                TableSchema {
                    id: 1,
                    fields: fields(4),
                    key,
                },
            ],
            runs: vec![
                run(0, 1, "2024-01", "2026-07"),
                run(1, 1, "2026-08", "2026-09"),
            ],
            current_schema: 1,
        }
    }

    #[test]
    fn market_tz() {
        let m = MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
            trading_day_start_secs: 14_400,
        };
        assert_eq!(m.arrow_tz(), "+10:00");
        let w = MarketDef {
            market: Market::Wem,
            utc_offset_secs: 28_800,
            timestamp_format: TimestampFormat::Iso8601,
            trading_day_start_secs: 28_800,
        };
        assert_eq!(w.arrow_tz(), "+08:00");
        let neg = MarketDef {
            market: Market::Wem,
            utc_offset_secs: -(5 * 3600 + 1800),
            timestamp_format: TimestampFormat::default(),
            trading_day_start_secs: 0,
        };
        assert_eq!(neg.arrow_tz(), "-05:30");
        assert_eq!(Market::Nem.to_string(), "nem");
        assert_eq!(Market::Wem.as_str(), "wem");
        assert_eq!(ColumnId(3).to_string(), "3");
    }

    #[test]
    fn schemas_runs_and_the_reference_schema() {
        let t = unit_scada();
        let pair = ("DISPATCH", "UNIT_SCADA");
        assert_eq!(
            t.reference_schema(pair, Some(month("2009-07"))),
            Some(0),
            "before any"
        );
        assert_eq!(t.reference_schema(pair, Some(month("2026-07"))), Some(0));
        assert_eq!(t.reference_schema(pair, Some(month("2026-08"))), Some(1));
        assert_eq!(t.reference_schema(pair, None), Some(1), "latest");
        assert_eq!(
            t.reference_schema(("X", "Y"), None),
            Some(1),
            "the last run"
        );
        assert!(t.has_version(pair, 1) && !t.has_version(pair, 2));
        assert_eq!(
            t.schema_columns(t.schema(0).unwrap()),
            ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]
        );
        assert!(t.schema(9).is_none());
        assert_eq!(t.column_index("SCADAVALUE"), Some(2));
        assert_eq!(t.column("DUID").unwrap().ty, LakeType::String);
        assert_eq!(t.column_by_id(ColumnId(4)).unwrap().name, "LASTCHANGED");
        assert!(t.column("NOPE").is_none());
        assert_eq!(t.dir_name(), "dispatch_unit_scada");
        let mut empty = t;
        empty.runs.clear();
        assert_eq!(empty.reference_schema(pair, None), None);
    }

    /// The history the registry keeps reads back as the Iceberg history
    /// `History::resolve` reads.
    #[test]
    fn the_history_is_an_iceberg_history() {
        let t = unit_scada();
        let h = t.history();
        assert_eq!(h.schemas.len(), 2);
        assert_eq!(h.current_schema_id, 1);
        assert_eq!(h.schemas[1].fields[3].ty, IcebergType::Timestamptz);
        assert_eq!(h.schemas[0].identifier_field_ids, [1, 2]);
        assert_eq!(
            (h.schemas[0].last_column_id, h.runs[1].from.release.as_str()),
            (4, "2026-08")
        );
        assert!(h.schemas[0].fields[0].required && !h.schemas[0].fields[2].required);
        let pair = ["DISPATCH", "UNIT_SCADA"];
        let three = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        assert_eq!(h.resolve(pair, 1, &three, "2024-06"), Some(0));
        assert_eq!(h.resolve(pair, 1, &three[..2], "2024-06"), None);
    }

    #[test]
    fn types_name_and_map_to_iceberg() {
        for t in LakeType::ALL {
            assert_eq!(LakeType::parse(&t.to_string()), Some(t));
            assert_eq!(LakeType::try_from(t.name()), Ok(t));
            assert_eq!(LakeType::of_iceberg(t.iceberg()), t);
        }
        assert_eq!(LakeType::Timestamp.iceberg().name(), "timestamptz");
        assert_eq!(LakeType::Long.iceberg(), IcebergType::Long);
        assert!(
            LakeType::try_from("float".to_owned())
                .unwrap_err()
                .contains("not a lake type")
        );
        assert_eq!(LakeType::parse("timestamptz"), None);
        assert_eq!(LakeType::parse("timestamp_ns"), None);
    }
}
