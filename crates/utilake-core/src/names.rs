//! Column and report names as the lake writes them.

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
}
