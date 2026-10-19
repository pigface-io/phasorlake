//! In-memory shape of a table: our schema, and the increments AEMO's
//! control files set.

use std::fmt;

use serde::{Deserialize, Serialize};
use utilake_core::{TypeClass, YearMonth};

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

/// A stable column identifier, never reused within a table; becomes the
/// Parquet field id so a later Iceberg table can adopt it unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnId(pub u32);

impl fmt::Display for ColumnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The exact type of a lake column, as Parquet and Iceberg name it.
///
/// | type           | Arrow                      | Iceberg V3       |
/// |----------------|----------------------------|------------------|
/// | `string`       | `Utf8`                     | `string`         |
/// | `double`       | `Float64`                  | `double`         |
/// | `long`         | `Int64`                    | `long`           |
/// | `boolean`      | `Boolean`                  | `boolean`        |
/// | `date`         | `Date32`                   | `date`           |
/// | `timestamp_ns` | `Timestamp(ns, market tz)` | `timestamptz_ns` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LakeType {
    /// Text.
    String,
    /// A 64-bit float; every number AEMO publishes.
    Double,
    /// A 64-bit integer.
    Long,
    /// `true` or `false`.
    Boolean,
    /// A day.
    Date,
    /// An instant in nanoseconds, written in market time.
    TimestampNs,
}

impl LakeType {
    /// Every type, in the order of the table above.
    pub const ALL: [Self; 6] = [
        Self::String,
        Self::Double,
        Self::Long,
        Self::Boolean,
        Self::Date,
        Self::TimestampNs,
    ];

    /// The name in `table.toml`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Double => "double",
            Self::Long => "long",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::TimestampNs => "timestamp_ns",
        }
    }

    /// The Iceberg V3 type.
    pub const fn iceberg(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Double => "double",
            Self::Long => "long",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::TimestampNs => "timestamptz_ns",
        }
    }

    /// Whether a column of this type may become `to` without rewriting a
    /// file: only an Iceberg promotion, and none of ours is one, so only
    /// the same type.
    pub fn promotes_to(self, to: Self) -> bool {
        self == to
    }

    /// Whether values a control file loads as `class` are read into this
    /// type without loss. Text keeps anything; an unknown class says
    /// nothing either way.
    pub const fn holds(self, class: TypeClass) -> bool {
        matches!(
            (class, self),
            (TypeClass::Unknown, _)
                | (_, Self::String)
                | (TypeClass::Number | TypeClass::Integer, Self::Double)
                | (TypeClass::Integer, Self::Long)
                | (TypeClass::Date, Self::Date | Self::TimestampNs)
                | (TypeClass::Timestamp, Self::TimestampNs)
        )
    }
}

impl fmt::Display for LakeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One lake column, ever: its id is never reused, and a renamed column is
/// a new column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// Stable id; the Parquet field id.
    pub id: ColumnId,
    /// Upper-case name, as AEMO's files write it.
    pub name: String,
    /// The type, our choice: it stays whatever a later control file says.
    pub ty: LakeType,
    /// Whether nulls are allowed. `false` only for primary-key columns.
    pub nullable: bool,
    /// Why the column is what it is, when that is not plain.
    pub note: Option<String>,
}

/// `TABLE@n`: the file shape one AEMO control file sets, from a month on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Increment {
    /// Our version, never reused.
    pub n: u16,
    /// The month it applies from, until the next increment's.
    pub from: YearMonth,
    /// The control file kept beside `table.toml`, e.g. `002.ctl`.
    pub ctl: Option<String>,
    /// SHA-256 of that file's bytes, lower-case hex.
    pub ctl_sha256: Option<String>,
    /// AEMO's name of the control file.
    pub aemo_ctl: Option<String>,
    /// The I-row versions seen under it.
    pub aemo_versions: Vec<u16>,
    /// The columns its files carry, in file order.
    pub columns: Vec<String>,
    /// Why it differs from its control file, when it does.
    pub note: Option<String>,
}

/// A table: our schema, and its increments in the order of their months.
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
    /// The next id to hand out; never decreases.
    pub next_column_id: u32,
    /// Every lake column, in lake order.
    pub columns: Vec<Column>,
    /// Increments, by ascending `from`.
    pub increments: Vec<Increment>,
}

impl Table {
    /// The column with this name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Index into `columns` of this name.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// Increment `n`.
    pub fn increment(&self, n: u16) -> Option<&Increment> {
        self.increments.iter().find(|i| i.n == n)
    }

    /// The increment in force in `month`: the last that applies from it or
    /// earlier, else the first.
    pub fn in_force(&self, month: YearMonth) -> Option<&Increment> {
        self.increments
            .iter()
            .rev()
            .find(|i| i.from <= month)
            .or_else(|| self.increments.first())
    }

    /// The increment with the latest `from`.
    pub fn latest(&self) -> Option<&Increment> {
        self.increments.last()
    }

    /// The next free increment number.
    pub fn next_increment(&self) -> u16 {
        self.increments.iter().map(|i| i.n).max().unwrap_or(0) + 1
    }

    /// Lower-case name for lake directories.
    pub fn dir_name(&self) -> String {
        self.name.to_ascii_lowercase()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn month(s: &str) -> YearMonth {
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

    /// DISPATCH_UNIT_SCADA in two increments: three columns from 2024-01,
    /// four from 2026-08.
    pub(crate) fn unit_scada() -> Table {
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
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
            next_column_id: 5,
            columns: vec![
                col(1, "SETTLEMENTDATE", LakeType::TimestampNs, false),
                col(2, "DUID", LakeType::String, false),
                col(3, "SCADAVALUE", LakeType::Double, true),
                col(4, "LASTCHANGED", LakeType::TimestampNs, true),
            ],
            increments: vec![
                Increment {
                    n: 1,
                    from: month("2024-01"),
                    ctl: None,
                    ctl_sha256: None,
                    aemo_ctl: Some("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401.ctl".into()),
                    aemo_versions: vec![1],
                    columns: names(&["SETTLEMENTDATE", "DUID", "SCADAVALUE"]),
                    note: None,
                },
                Increment {
                    n: 2,
                    from: month("2026-08"),
                    ctl: None,
                    ctl_sha256: None,
                    aemo_ctl: None,
                    aemo_versions: vec![1],
                    columns: names(&["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"]),
                    note: Some("LASTCHANGED came with the 2026 control file.".into()),
                },
            ],
        }
    }

    #[test]
    fn market_tz() {
        let m = MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
        };
        assert_eq!(m.arrow_tz(), "+10:00");
        let w = MarketDef {
            market: Market::Wem,
            utc_offset_secs: 28_800,
            timestamp_format: TimestampFormat::Iso8601,
        };
        assert_eq!(w.arrow_tz(), "+08:00");
        let neg = MarketDef {
            market: Market::Wem,
            utc_offset_secs: -(5 * 3600 + 1800),
            timestamp_format: TimestampFormat::default(),
        };
        assert_eq!(neg.arrow_tz(), "-05:30");
        assert_eq!(Market::Nem.to_string(), "nem");
        assert_eq!(Market::Wem.as_str(), "wem");
        assert_eq!(ColumnId(3).to_string(), "3");
    }

    #[test]
    fn increments_in_force() {
        let t = unit_scada();
        assert_eq!(t.in_force(month("2009-07")).unwrap().n, 1, "before any");
        assert_eq!(t.in_force(month("2024-01")).unwrap().n, 1);
        assert_eq!(t.in_force(month("2026-07")).unwrap().n, 1);
        assert_eq!(t.in_force(month("2026-08")).unwrap().n, 2);
        assert_eq!(t.in_force(month("2030-01")).unwrap().n, 2);
        assert_eq!(t.latest().unwrap().n, 2);
        assert_eq!(t.increment(1).unwrap().columns.len(), 3);
        assert!(t.increment(9).is_none());
        assert_eq!(t.next_increment(), 3);
        assert_eq!(t.column_index("SCADAVALUE"), Some(2));
        assert_eq!(t.column("DUID").unwrap().ty, LakeType::String);
        assert!(t.column("NOPE").is_none());
        assert_eq!(t.dir_name(), "dispatch_unit_scada");
        let mut empty = t;
        empty.increments.clear();
        assert!(empty.in_force(month("2024-01")).is_none());
        assert_eq!(empty.next_increment(), 1);
    }

    #[test]
    fn types_name_and_hold() {
        for t in LakeType::ALL {
            assert_eq!(t.to_string(), t.as_str());
            assert!(t.promotes_to(t));
            assert!(t.holds(TypeClass::Unknown));
        }
        assert_eq!(LakeType::TimestampNs.iceberg(), "timestamptz_ns");
        assert_eq!(LakeType::Long.iceberg(), "long");
        assert!(!LakeType::Long.promotes_to(LakeType::Double));
        assert!(!LakeType::Date.promotes_to(LakeType::TimestampNs));
        assert!(LakeType::String.holds(TypeClass::Timestamp));
        assert!(LakeType::Double.holds(TypeClass::Integer));
        assert!(LakeType::Long.holds(TypeClass::Integer));
        assert!(!LakeType::Long.holds(TypeClass::Number));
        assert!(LakeType::TimestampNs.holds(TypeClass::Date));
        assert!(!LakeType::Date.holds(TypeClass::Timestamp));
        assert!(!LakeType::Double.holds(TypeClass::Varchar));
        assert!(!LakeType::Boolean.holds(TypeClass::Number));
    }
}
