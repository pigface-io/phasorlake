//! The MMS `I` row: the schema header that starts every block of data rows.
//!
//! ```text
//! I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE
//! ```
//!
//! Fields 1..=3 are the report type, report subtype and version; the rest are
//! column names. Data rows repeat the first three fields before their values.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A parsed `I` row, borrowing from the line it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IRow<'a> {
    /// Field 1, e.g. `DISPATCH`.
    pub report_type: &'a str,
    /// Field 2, e.g. `UNIT_SCADA`.
    pub report_subtype: &'a str,
    /// Field 3, the schema version AEMO stamps on the block.
    pub version: u16,
    /// Fields 4.., already normalised with [`normalize_name`], trailing
    /// empty names dropped (AEMO leaves a trailing comma on some I rows).
    pub columns: &'a [&'a str],
}

impl IRow<'_> {
    /// The fingerprint of this row's column list.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of_names(self.columns.iter().copied())
    }
}

/// Why an `I` row could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IRowError {
    /// Fewer than four fields.
    #[error(
        "the I row has {0} fields; it needs a report type, a subtype, a version and at least one column"
    )]
    TooShort(usize),
    /// Field 3 is not a small integer.
    #[error("the I row's version field {0:?} is not an integer between 0 and 65535")]
    Version(String),
    /// A field failed [`normalize_name`].
    #[error("the I row's field {index} {raw:?} is empty or contains a control character")]
    Name {
        /// 0-based field index in the row.
        index: usize,
        /// The offending text.
        raw: String,
    },
}

/// Normalises a report or column name: strips a UTF-8 BOM, surrounding ASCII
/// whitespace, one pair of double quotes, and upper-cases ASCII letters.
///
/// Returns `None` for an empty result or one containing a control character
/// or a comma; those can never be real names and would corrupt a fingerprint.
/// Writes into `buf`, which the caller reuses, so the hot path allocates once
/// per row at most.
pub fn normalize_name<'b>(raw: &str, buf: &'b mut String) -> Option<&'b str> {
    let s = raw
        .strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .trim_matches([' ', '\t', '\r']);
    let s = match (s.strip_prefix('"'), s.len() >= 2) {
        (Some(inner), true) => inner.strip_suffix('"').unwrap_or(s),
        _ => s,
    }
    .trim_matches([' ', '\t']);
    if s.is_empty()
        || s.bytes()
            .any(|b| b.is_ascii_control() || b == b',' || b == b'"')
    {
        return None;
    }
    buf.clear();
    buf.reserve(s.len());
    for b in s.bytes() {
        buf.push(b.to_ascii_uppercase() as char);
    }
    // Non-ASCII bytes were pushed byte-wise above only if ASCII; keep the
    // original characters for anything else so the name is still faithful.
    if !s.is_ascii() {
        buf.clear();
        buf.push_str(&s.to_ascii_uppercase());
    }
    Some(buf.as_str())
}

/// Parses the version field of an `I` or `D` row.
pub fn parse_version(raw: &str) -> Result<u16, IRowError> {
    raw.trim()
        .parse()
        .map_err(|_| IRowError::Version(raw.to_owned()))
}

/// A 128-bit identity of an ordered column list.
///
/// xxh3-128 over each normalised name followed by `0x1F`; order-sensitive,
/// so a reordering is a different shape, and insensitive to quoting, case
/// and trailing commas because names are normalised first.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Fingerprint(pub u128);

impl Fingerprint {
    /// Fingerprint of already-normalised names, in order.
    pub fn of_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut h = xxhash_rust::xxh3::Xxh3::new();
        for n in names {
            h.update(n.as_bytes());
            h.update(&[0x1F]);
        }
        Self(h.digest128())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "xxh3:{:032x}", self.0)
    }
}

/// Why a string is not a fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a fingerprint of the form xxh3:<32 hex digits>")]
pub struct FingerprintError(String);

impl std::str::FromStr for Fingerprint {
    type Err = FingerprintError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("xxh3:")
            .ok_or_else(|| FingerprintError(s.to_owned()))?;
        if hex.len() != 32 {
            return Err(FingerprintError(s.to_owned()));
        }
        u128::from_str_radix(hex, 16)
            .map(Self)
            .map_err(|_| FingerprintError(s.to_owned()))
    }
}

impl TryFrom<String> for Fingerprint {
    type Error = FingerprintError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Fingerprint> for String {
    fn from(fp: Fingerprint) -> Self {
        fp.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(s: &str) -> Option<String> {
        let mut b = String::new();
        normalize_name(s, &mut b).map(str::to_owned)
    }

    #[test]
    fn normalises_names() {
        assert_eq!(norm("settlementdate").as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(norm(" \"DUID\" \r").as_deref(), Some("DUID"));
        assert_eq!(norm("\u{feff}REGIONID").as_deref(), Some("REGIONID"));
        assert_eq!(norm("\"\"").as_deref(), None);
        assert_eq!(norm(""), None);
        assert_eq!(norm("   "), None);
        assert_eq!(norm("A\tB"), None);
        assert_eq!(norm("A,B"), None);
        assert_eq!(norm("\""), None);
        assert_eq!(norm("héllo").as_deref(), Some("HéLLO"));
    }

    #[test]
    fn fingerprint_is_order_sensitive_and_stable() {
        let a = Fingerprint::of_names(["A", "B"]);
        let b = Fingerprint::of_names(["B", "A"]);
        let c = Fingerprint::of_names(["A", "B"]);
        assert_ne!(a, b);
        assert_eq!(a, c);
        // Separator means ["AB"] and ["A","B"] differ.
        assert_ne!(Fingerprint::of_names(["AB"]), a);
        let s = a.to_string();
        assert!(s.starts_with("xxh3:") && s.len() == 37);
        assert_eq!(s.parse::<Fingerprint>().unwrap(), a);
        assert_eq!(format!("{a:?}"), format!("Fingerprint({a})"));
        assert!("abc".parse::<Fingerprint>().is_err());
        assert!("xxh3:zz".parse::<Fingerprint>().is_err());
        assert!(
            "xxh3:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
                .parse::<Fingerprint>()
                .is_err()
        );
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Fingerprint>(&json).unwrap(), a);
    }

    #[test]
    fn irow_fingerprint_and_version() {
        let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        assert_eq!(row.fingerprint(), Fingerprint::of_names(cols));
        assert_eq!(parse_version(" 4 ").unwrap(), 4);
        assert!(matches!(parse_version("x"), Err(IRowError::Version(_))));
        assert!(matches!(parse_version("70000"), Err(IRowError::Version(_))));
        let e = IRowError::Name {
            index: 5,
            raw: String::new(),
        };
        assert!(e.to_string().contains("field 5"));
        assert!(IRowError::TooShort(2).to_string().contains('2'));
    }
}
