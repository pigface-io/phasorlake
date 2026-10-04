//! What one file actually is: its report pair and its ordered columns,
//! each with the class of type AEMO's control file loads it as.
//!
//! A [`ShapeId`] is derived from that content and nothing else, so two
//! files with the same id are the same shape whenever and wherever they
//! were published, and a shape is never given an id by hand. It sits beside
//! the name-only [`Fingerprint`](crate::Fingerprint), which resolution keys
//! on: the fingerprint tells column lists apart, the shape id also tells
//! apart the same list loaded as other types.
//!
//! AEMO's version number is not part of the shape: AEMO has published one
//! column list under several versions and several under one, so it is an
//! attribute recorded beside the id.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The generation of the canonical form. Any change to [`CanonicalShape::bytes`]
/// changes every id, so it raises this number and is a breaking change.
pub const SHAPE_FORMAT: u32 = 1;

/// The class of type a column is loaded as, from the control file of its
/// month: what kind of value it holds, not its length or precision.
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
    /// The class's name in census files and canonical bytes.
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
/// (see [`normalize_name`](crate::normalize_name)), in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalShape {
    /// Report type, normalised.
    pub report_type: String,
    /// Report subtype, normalised.
    pub report_subtype: String,
    /// Columns in file order: normalised name and type class.
    pub columns: Vec<(String, TypeClass)>,
}

impl CanonicalShape {
    /// The canonical bytes: the format, the report pair, then each column
    /// as name and class. Fields end with `0x1F`, records with `0x1E`;
    /// neither can occur in a normalised name.
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(32 + self.columns.len() * 24);
        let field = |b: &mut Vec<u8>, s: &str| {
            b.extend_from_slice(s.as_bytes());
            b.push(0x1F);
        };
        field(&mut b, &format!("nemlake shape {SHAPE_FORMAT}"));
        b.push(0x1E);
        field(&mut b, &self.report_type);
        field(&mut b, &self.report_subtype);
        b.push(0x1E);
        for (name, class) in &self.columns {
            field(&mut b, name);
            field(&mut b, class.as_str());
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

    fn scada(columns: &[(&str, TypeClass)]) -> CanonicalShape {
        CanonicalShape {
            report_type: "DISPATCH".into(),
            report_subtype: "UNIT_SCADA".into(),
            columns: columns.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect(),
        }
    }

    const COLUMNS: [(&str, TypeClass); 3] = [
        ("SETTLEMENTDATE", TypeClass::Timestamp),
        ("DUID", TypeClass::Varchar),
        ("SCADAVALUE", TypeClass::Number),
    ];

    /// A canonicalisation that changes by accident fails here; one that
    /// changes on purpose raises SHAPE_FORMAT and updates this id.
    #[test]
    fn the_id_of_a_known_shape_is_pinned() {
        let s = scada(&COLUMNS);
        assert_eq!(SHAPE_FORMAT, 1);
        assert_eq!(s.id().to_string(), "2251ec37b51ac46e");
        assert_eq!(s.id(), scada(&COLUMNS).id());
    }

    #[test]
    fn order_types_and_pair_are_part_of_the_id() {
        let base = scada(&COLUMNS).id();
        let mut reordered = COLUMNS;
        reordered.swap(1, 2);
        assert_ne!(scada(&reordered).id(), base);
        let mut retyped = COLUMNS;
        retyped[2].1 = TypeClass::Varchar;
        assert_ne!(scada(&retyped).id(), base);
        let mut other = scada(&COLUMNS);
        other.report_subtype = "UNIT_SCADA2".into();
        assert_ne!(other.id(), base);
        // A name boundary is not a place to hide a column.
        let joined = scada(&[("AB", TypeClass::Varchar)]);
        let split = scada(&[("A", TypeClass::Varchar), ("B", TypeClass::Varchar)]);
        assert_ne!(joined.id(), split.id());
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
        for c in [
            TypeClass::Varchar,
            TypeClass::Number,
            TypeClass::Integer,
            TypeClass::Date,
            TypeClass::Timestamp,
            TypeClass::Unknown,
        ] {
            assert_eq!(c.to_string().parse::<TypeClass>().unwrap(), c);
        }
        assert!("float".parse::<TypeClass>().is_err());
    }
}
