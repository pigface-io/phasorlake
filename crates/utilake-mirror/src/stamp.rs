//! A point in time with its UTC offset, as RFC 3339 text: every time field
//! the mirror writes. The type is the family's one
//! ([`utilake_provenance::stamp`]); the mirror's listing times and the DOS
//! times in AEMO's zips are AEST (UTC+10, no daylight saving), AEMO's WA
//! host's are AWST (UTC+8); an HTTP date and the mirror's own clock are UTC.

pub use utilake_provenance::stamp::{AEST, AWST, BadStamp, Stamp};

/// Checks of written records: every time field is aware, or has its aware
/// twin beside it.
#[cfg(test)]
pub mod check {
    use super::Stamp;

    /// Whether a key names a time field.
    fn time_key(key: &str) -> bool {
        key.ends_with("_at")
            || key.contains("modified")
            || matches!(key, "as_of" | "at" | "seen" | "gone" | "read")
    }

    /// Whether a string reads as a time in any form the mirror meets.
    fn looks_like_a_time(s: &str) -> bool {
        let dated = s.as_bytes().windows(10).any(|w| {
            w[4] == b'-' && w[7] == b'-' && w.iter().filter(|b| b.is_ascii_digit()).count() == 8
        });
        dated || s.ends_with(" AM") || s.ends_with(" PM") || s.ends_with(" GMT")
    }

    fn aware(v: &serde_json::Value) -> bool {
        v.as_str().is_some_and(|s| Stamp::parse(s).is_ok())
    }

    /// Fails unless every time field of every object in `v` is RFC 3339
    /// with an offset, or has a `{name}_at` field beside it that is.
    pub fn assert_aware(v: &serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    if let Some(s) = value.as_str() {
                        if time_key(key) {
                            let twin = map.get(&format!("{key}_at"));
                            assert!(
                                aware(value) || twin.is_some_and(aware),
                                "`{key}`: `{s}` is naive and has no aware `{key}_at` beside it"
                            );
                        } else {
                            assert!(!looks_like_a_time(s), "`{key}` holds a time: `{s}`");
                        }
                    } else {
                        assert_aware(value);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(assert_aware),
            _ => {}
        }
    }

    /// `assert_aware` over every line of an NDJSON text.
    pub fn assert_aware_lines(ndjson: &str) {
        for line in ndjson.lines().filter(|l| !l.is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
            assert_aware(&v);
        }
    }
}
