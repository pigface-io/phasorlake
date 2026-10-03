//! In-memory shape of a table definition.

use std::fmt;

use nemlake_core::{Fingerprint, YearMonth};
use serde::{Deserialize, Serialize};

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

/// What kind of file feeds a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// MMS C/I/D rows; blocks identified by `(report_type, report_subtype)`.
    Mms,
    /// A CSV with a header row; identified by dataset name (WEM).
    PlainCsv,
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

/// How overlapping sources are reconciled by a later `compact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dedupe {
    /// Keep every row.
    None,
    /// Keep one row per primary key.
    Pk,
    /// Keep the row with the latest `LASTCHANGED` per primary key.
    PkLastchanged,
}

/// AEMO's visibility class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Published on nemweb.
    Public,
    /// Participant-only; appears in the data model but never in the archive.
    Private,
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

/// The logical type of a column, before any Arrow mapping.
///
/// The TOML form is flat (`type = "number", precision = 15, scale = 5`);
/// see [`crate::file`]. This enum is the in-memory and blob form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColumnType {
    /// Text; `VARCHAR2(n)` / `CHAR(n)`.
    Utf8 {
        /// Declared maximum length.
        max_len: Option<u32>,
        /// Hint that the column is low-cardinality (ids), for the writer.
        dict: bool,
    },
    /// `NUMBER(p,s)`; `f64` by default, `Decimal128(p,s)` on request.
    Number {
        /// Declared precision.
        precision: Option<u8>,
        /// Declared scale.
        scale: Option<u8>,
    },
    /// `NUMBER(p,0)` with p <= 18; `Int64`.
    Integer,
    /// `DATE` with a time; `Timestamp(us, market offset)`.
    Timestamp,
    /// A date with no time; `Date32`.
    Date,
    /// WEM booleans.
    Boolean,
}

impl ColumnType {
    /// Short name for messages.
    pub const fn kind_name(self) -> &'static str {
        match self {
            Self::Utf8 { .. } => "utf8",
            Self::Number { .. } => "number",
            Self::Integer => "integer",
            Self::Timestamp => "timestamp",
            Self::Date => "date",
            Self::Boolean => "boolean",
        }
    }
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

/// The version AEMO stamps on an I row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(pub u16);

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where a fact in a schema file came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// CTL file the loader types came from.
    pub ctl: Option<String>,
    /// Data model page the Oracle types and keys came from.
    pub data_model: Option<String>,
    /// Archive months whose I rows were sampled.
    pub sampled_months: Vec<YearMonth>,
    /// When the file was last regenerated.
    pub generated: Option<String>,
    /// Free-text notes for humans.
    pub note: Option<String>,
}

/// One column of the unified lake schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    /// Stable id.
    pub id: ColumnId,
    /// Canonical upper-case name.
    pub name: String,
    /// Former names this column was published under.
    pub aliases: Vec<String>,
    /// Logical type.
    pub ty: ColumnType,
    /// Whether nulls are allowed. `false` only for primary-key columns.
    pub nullable: bool,
    /// Oracle type verbatim from the data model, e.g. `NUMBER(15,5)`.
    pub oracle: Option<String>,
    /// SQL*Loader type verbatim from the CTL, e.g. `FLOAT EXTERNAL`.
    pub loader: Option<String>,
    /// The version that introduced the column.
    pub added_in: SchemaVersion,
    /// The version that dropped it, if any; the column stays in the lake.
    pub removed_in: Option<SchemaVersion>,
    /// Description from the data model.
    pub description: Option<String>,
    /// Sources that agreed on this column: `ctl`, `dm`, `irow`, `inferred`, `manual`.
    pub src: Vec<String>,
    /// Hand-edited; the regenerator leaves it alone.
    pub curated: bool,
}

/// One published shape of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionDef {
    /// The I-row version.
    pub n: SchemaVersion,
    /// Column names in I-row order.
    pub columns: Vec<String>,
    /// Fingerprint of `columns`; stored so a hand edit is caught.
    pub fingerprint: Fingerprint,
    /// Earliest archive month seen with this version.
    pub first_seen: Option<YearMonth>,
    /// Latest archive month seen with this version.
    pub last_seen: Option<YearMonth>,
    /// Where the version was observed, e.g. `irow:MMSDM_2024_01`.
    pub src: Option<String>,
    /// Other column lists published under this same version number.
    pub variants: Vec<VariantDef>,
}

/// Another column list AEMO published under an existing version number.
///
/// AEMO appends and renames columns without bumping the I-row version, so
/// one version can have several real shapes. `VersionDef::columns` is the
/// widest or newest; every other observed shape is a variant and resolves
/// exactly instead of as drift. Variant columns are written as observed, so
/// they may use an alias of a column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantDef {
    /// Column names in I-row order, as published (aliases allowed).
    pub columns: Vec<String>,
    /// Fingerprint of `columns`.
    pub fingerprint: Fingerprint,
    /// Earliest archive month seen with this shape.
    pub first_seen: Option<YearMonth>,
    /// Latest archive month seen with this shape.
    pub last_seen: Option<YearMonth>,
}

impl VersionDef {
    /// The variant with this fingerprint.
    pub fn variant(&self, fp: Fingerprint) -> Option<&VariantDef> {
        self.variants.iter().find(|x| x.fingerprint == fp)
    }
}

/// A rule a person wrote for the files of an interval.
///
/// The registry maps a file's columns onto the lake by name. Where that is
/// not right for some files (a column published under another name for a
/// few years, a column whose values were of another kind before a market
/// change) a rule says so, for exactly the files it names: those of a span
/// of months, of one version, of one column list, or any combination. Each
/// rule routes named file columns to declared lake columns; the column's
/// declared type decides how the values are read.
///
/// Rules are never generated and never inferred. Regeneration carries them
/// over as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDef {
    /// Why the rule exists and how it was checked. Required.
    pub note: String,
    /// First source month the rule applies to.
    pub from: Option<YearMonth>,
    /// Last source month the rule applies to, inclusive.
    pub to: Option<YearMonth>,
    /// Only blocks published under this version number.
    pub version: Option<SchemaVersion>,
    /// Only blocks with this column list.
    pub fingerprint: Option<Fingerprint>,
    /// The routes: which file column feeds which lake column.
    pub columns: Vec<RuleColumn>,
}

/// One route of a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleColumn {
    /// The column's name in the file.
    pub source: String,
    /// The lake column it feeds: a declared column, by its own name.
    pub column: String,
}

impl RuleDef {
    /// Whether the rule is for a block of this version and column list in
    /// a source of this month. A rule bounded by months does not apply to
    /// a source whose month is not known.
    pub fn applies(
        &self,
        period: Option<YearMonth>,
        version: SchemaVersion,
        fingerprint: Fingerprint,
    ) -> bool {
        if self.from.is_some() || self.to.is_some() {
            let Some(p) = period else {
                return false;
            };
            if self.from.is_some_and(|f| p < f) || self.to.is_some_and(|t| p > t) {
                return false;
            }
        }
        self.version.is_none_or(|v| v == version)
            && self.fingerprint.is_none_or(|f| f == fingerprint)
    }
}

/// A table: its identity, its unified columns, and every version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    /// Market.
    pub market: Market,
    /// File kind.
    pub source: SourceKind,
    /// Data-model name, e.g. `DISPATCH_UNIT_SCADA`.
    pub name: String,
    /// Data-model package, e.g. `DISPATCH`.
    pub package: String,
    /// Visibility class.
    pub visibility: Visibility,
    /// Description from the data model.
    pub description: Option<String>,
    /// `(report_type, report_subtype)` pairs whose I rows feed this table.
    pub report_pairs: Vec<(String, String)>,
    /// Dataset names, for plain-CSV sources.
    pub datasets: Vec<String>,
    /// Archive file stems, e.g. `DISPATCH_UNIT_SCADA`; a stem plus digits
    /// (`BIDPEROFFER1`) is a split of the same table.
    pub archive_files: Vec<String>,
    /// `Reports/Current` directories that carry this table.
    pub feeds: Vec<String>,
    /// Append or snapshot.
    pub load_mode: LoadMode,
    /// Dedupe strategy.
    pub dedupe: Dedupe,
    /// Primary key column names.
    pub primary_key: Vec<String>,
    /// The timestamp column that drives partitioning.
    pub time_column: Option<String>,
    /// The next id to hand out; never decreases.
    pub next_column_id: u32,
    /// Marked when AEMO retired the table; files are kept.
    pub deprecated: bool,
    /// Columns in lake order.
    pub columns: Vec<ColumnDef>,
    /// Versions ascending.
    pub versions: Vec<VersionDef>,
    /// Hand-written rules for the files of an interval, in the order they
    /// are applied.
    pub rules: Vec<RuleDef>,
    /// Where the facts came from.
    pub provenance: Provenance,
}

impl TableDef {
    /// The column with this canonical name or alias.
    pub fn column(&self, name: &str) -> Option<&ColumnDef> {
        self.columns
            .iter()
            .find(|c| c.name == name || c.aliases.iter().any(|a| a == name))
    }

    /// Index into `columns` of this name or alias.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.name == name || c.aliases.iter().any(|a| a == name))
    }

    /// The version definition for `n`.
    pub fn version(&self, n: SchemaVersion) -> Option<&VersionDef> {
        self.versions.iter().find(|v| v.n == n)
    }

    /// The highest declared version.
    pub fn latest_version(&self) -> Option<&VersionDef> {
        self.versions.last()
    }

    /// The highest declared version not above `n`, else the lowest.
    pub fn nearest_version(&self, n: SchemaVersion) -> Option<&VersionDef> {
        self.versions
            .iter()
            .rev()
            .find(|v| v.n <= n)
            .or_else(|| self.versions.first())
    }

    /// Lower-case name for directory paths.
    pub fn dir_name(&self) -> String {
        self.name.to_ascii_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(ColumnType::Integer.kind_name(), "integer");
        assert_eq!(ColumnId(3).to_string(), "3");
        assert_eq!(SchemaVersion(4).to_string(), "4");
    }

    #[test]
    fn a_rule_applies_within_its_bounds() {
        let month = |s: &str| s.parse::<YearMonth>().unwrap();
        let fp = Fingerprint::of_names(["A", "B"]);
        let other = Fingerprint::of_names(["A"]);
        let mut rule = RuleDef {
            note: "why".into(),
            from: None,
            to: None,
            version: None,
            fingerprint: None,
            columns: vec![],
        };
        // Unbounded: every block, whatever is known of its source.
        assert!(rule.applies(None, SchemaVersion(1), fp));
        assert!(rule.applies(Some(month("2012-01")), SchemaVersion(9), other));
        // Bounded by months, both ends inclusive.
        rule.from = Some(month("2010-01"));
        rule.to = Some(month("2012-06"));
        assert!(rule.applies(Some(month("2010-01")), SchemaVersion(1), fp));
        assert!(rule.applies(Some(month("2012-06")), SchemaVersion(1), fp));
        assert!(!rule.applies(Some(month("2009-12")), SchemaVersion(1), fp));
        assert!(!rule.applies(Some(month("2012-07")), SchemaVersion(1), fp));
        assert!(
            !rule.applies(None, SchemaVersion(1), fp),
            "a month nobody knows is in no interval"
        );
        // One end alone.
        rule.to = None;
        assert!(rule.applies(Some(month("2026-08")), SchemaVersion(1), fp));
        // By version and by column list.
        rule.version = Some(SchemaVersion(2));
        assert!(!rule.applies(Some(month("2011-01")), SchemaVersion(1), fp));
        assert!(rule.applies(Some(month("2011-01")), SchemaVersion(2), fp));
        rule.fingerprint = Some(fp);
        assert!(rule.applies(Some(month("2011-01")), SchemaVersion(2), fp));
        assert!(!rule.applies(Some(month("2011-01")), SchemaVersion(2), other));
    }
}
