//! The identity of an ordered column list.

use std::fmt;

use serde::{Deserialize, Serialize};

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
}
