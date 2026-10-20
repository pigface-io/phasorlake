//! What one file actually is: its report pair and its ordered column names.
//!
//! A [`ShapeId`] is derived from that content and nothing else, so two
//! files with the same id are the same shape whenever and wherever they
//! were published, and a shape is never given an id by hand. Types are not
//! part of it: what a column holds is observed from its values and recorded
//! beside the shape (`file_model::shapes`), so a column that is empty in one
//! month and full in the next is one shape, not two.
//!
//! AEMO's version number is not part of the shape either: AEMO has
//! published one column list under several versions and several under one,
//! so the versions are recorded beside the id.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The generation of the canonical form. Any change to [`CanonicalShape::bytes`]
/// changes every id, so it raises this number and is a breaking change.
pub const SHAPE_FORMAT: u32 = 2;

/// The class of type AEMO's control file of a month declares a column as:
/// what kind of value it holds, not its length or precision. A declaration,
/// recorded beside what the values were observed to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeClass {
    /// Text (`CHAR(n)`).
    Varchar,
    /// A number (`FLOAT EXTERNAL`, `DECIMAL EXTERNAL`).
    Number,
    /// A whole number (`INTEGER EXTERNAL`).
    Integer,
    /// A date without a time of day (`DATE "yyyy/mm/dd"`).
    Date,
    /// A date and time (`DATE "yyyy/mm/dd hh24:mi:ss"`, `TIMESTAMP`).
    Timestamp,
    /// No control file of the month names the column.
    Unknown,
}

impl TypeClass {
    /// Every class.
    pub const ALL: [Self; 6] = [
        Self::Varchar,
        Self::Number,
        Self::Integer,
        Self::Date,
        Self::Timestamp,
        Self::Unknown,
    ];

    /// One letter per class, for compact census records.
    pub const fn letter(self) -> char {
        match self {
            Self::Varchar => 'v',
            Self::Number => 'n',
            Self::Integer => 'i',
            Self::Date => 'D',
            Self::Timestamp => 't',
            Self::Unknown => '?',
        }
    }

    /// The class of a letter from [`TypeClass::letter`].
    pub fn of_letter(c: char) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.letter() == c)
    }

    /// The class's name in census and shapes files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Varchar => "varchar",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Date => "date",
            Self::Timestamp => "timestamp",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for TypeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a string is not a type class.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a type class (varchar, number, integer, date, timestamp, unknown)")]
pub struct TypeClassError(String);

impl std::str::FromStr for TypeClass {
    type Err = TypeClassError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "varchar" => Self::Varchar,
            "number" => Self::Number,
            "integer" => Self::Integer,
            "date" => Self::Date,
            "timestamp" => Self::Timestamp,
            "unknown" => Self::Unknown,
            _ => return Err(TypeClassError(s.to_owned())),
        })
    }
}

/// A shape in its canonical form: names normalised as an I row's are
/// (see [`normalize_name`](utilake_core::normalize_name)), in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalShape {
    /// Report type, normalised.
    pub report_type: String,
    /// Report subtype, normalised.
    pub report_subtype: String,
    /// Column names in file order, normalised.
    pub columns: Vec<String>,
}

impl CanonicalShape {
    /// The canonical bytes: the format, the report pair, then each column
    /// name. Fields end with `0x1F`, records with `0x1E`; neither can occur
    /// in a normalised name.
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(32 + self.columns.len() * 24);
        let field = |b: &mut Vec<u8>, s: &str| {
            b.extend_from_slice(s.as_bytes());
            b.push(0x1F);
        };
        // The domain of the hash, as written when shape ids were first
        // recorded: ids are content-addressed and persisted in schemas/ and
        // in every lake file's footer, so this text never changes with the
        // tool's name.
        field(&mut b, &format!("nemlake shape {SHAPE_FORMAT}"));
        b.push(0x1E);
        field(&mut b, &self.report_type);
        field(&mut b, &self.report_subtype);
        b.push(0x1E);
        for name in &self.columns {
            field(&mut b, name);
            b.push(0x1E);
        }
        b
    }

    /// The shape's id.
    pub fn id(&self) -> ShapeId {
        ShapeId((xxhash_rust::xxh3::xxh3_128(&self.bytes()) >> 64) as u64)
    }
}

/// The content-addressed id of a shape: the upper 64 bits of the xxh3-128
/// of its [canonical bytes](CanonicalShape::bytes), written as 16 hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ShapeId(pub u64);

impl fmt::Debug for ShapeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShapeId({self})")
    }
}

impl fmt::Display for ShapeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// Why a string is not a shape id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a shape id of 16 lower-case hex digits")]
pub struct ShapeIdError(String);

impl std::str::FromStr for ShapeId {
    type Err = ShapeIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 16 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(ShapeIdError(s.to_owned()));
        }
        u64::from_str_radix(s, 16)
            .map(Self)
            .map_err(|_| ShapeIdError(s.to_owned()))
    }
}

impl TryFrom<String> for ShapeId {
    type Error = ShapeIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<ShapeId> for String {
    fn from(id: ShapeId) -> Self {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scada(columns: &[&str]) -> CanonicalShape {
        CanonicalShape {
            report_type: "DISPATCH".into(),
            report_subtype: "UNIT_SCADA".into(),
            columns: columns.iter().map(|n| (*n).to_owned()).collect(),
        }
    }

    const COLUMNS: [&str; 3] = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];

    /// A canonicalisation that changes by accident fails here; one that
    /// changes on purpose raises SHAPE_FORMAT and updates this id.
    #[test]
    fn the_id_of_a_known_shape_is_pinned() {
        let s = scada(&COLUMNS);
        assert_eq!(SHAPE_FORMAT, 2);
        assert_eq!(s.id().to_string(), PINNED);
        assert_eq!(s.id(), scada(&COLUMNS).id());
    }

    const PINNED: &str = "c9a60b6ab1eb80ad";

    #[test]
    fn order_names_and_pair_are_the_id() {
        let base = scada(&COLUMNS).id();
        let mut reordered = COLUMNS;
        reordered.swap(1, 2);
        assert_ne!(scada(&reordered).id(), base);
        let mut other = scada(&COLUMNS);
        other.report_subtype = "UNIT_SCADA2".into();
        assert_ne!(other.id(), base);
        // A name boundary is not a place to hide a column.
        assert_ne!(scada(&["AB"]).id(), scada(&["A", "B"]).id());
    }

    #[test]
    fn ids_and_classes_read_back_from_text() {
        let id = scada(&COLUMNS).id();
        assert_eq!(id.to_string().parse::<ShapeId>().unwrap(), id);
        assert_eq!(format!("{id:?}"), format!("ShapeId({id})"));
        assert!("0AA7ED98E8CBFC59".parse::<ShapeId>().is_err());
        assert!("abc".parse::<ShapeId>().is_err());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ShapeId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<ShapeId>("\"zz\"").is_err());
        for c in TypeClass::ALL {
            assert_eq!(c.to_string().parse::<TypeClass>().unwrap(), c);
            assert_eq!(TypeClass::of_letter(c.letter()), Some(c));
        }
        assert_eq!(TypeClass::of_letter('x'), None);
        assert!("float".parse::<TypeClass>().is_err());
    }
}
