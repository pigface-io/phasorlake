//! Time as text outside the lake: RFC 3339 with an offset, and the forms it is
//! read from. The lake's names for [`phasor_provenance::stamp`].

pub use phasor_provenance::stamp::Stamp;

/// A UTC instant in whole seconds as RFC 3339 in `offset_secs`.
#[must_use]
pub fn format_rfc3339(secs_utc: i64, offset_secs: i32) -> String {
    Stamp::new(secs_utc, offset_secs).rfc3339()
}

/// Reads time text in any form phasor meets ([`Stamp::read`]).
#[must_use]
pub fn parse(s: &str, naive_offset_secs: i32) -> Option<Stamp> {
    Stamp::read(s, naive_offset_secs)
}

/// `s` as RFC 3339 with an offset, from any form; `None` when not a time.
#[must_use]
pub fn normalise(s: &str, naive_offset_secs: i32) -> Option<String> {
    parse(s, naive_offset_secs).map(|t| t.rfc3339())
}

/// Whether `s` is RFC 3339 text that says its offset.
#[must_use]
pub fn is_rfc3339(s: &str) -> bool {
    Stamp::parse(s).is_ok()
}

/// RFC 3339 with an offset, and nothing else.
#[must_use]
pub fn parse_rfc3339(s: &str) -> Option<Stamp> {
    Stamp::parse(s).ok()
}

/// The HTTP date a server sends in `Last-Modified`.
#[must_use]
pub fn parse_http_date(s: &str) -> Option<Stamp> {
    Stamp::from_http_date(s)
}

/// AEMO's WEM manifest `generatedAt`: `Thu Nov 02 2023 11:35:00Z`.
#[must_use]
pub fn parse_wem_generated(s: &str) -> Option<Stamp> {
    Stamp::from_wem_generated(s)
}

/// The time an IIS directory listing shows, in the server's local time.
#[must_use]
pub fn parse_iis_listing(s: &str, offset_secs: i32) -> Option<Stamp> {
    Stamp::from_listing(s, offset_secs)
}

/// `YYYY-MM-DD HH:MM:SS` (or `/`, or a `T`) with no offset, read in `offset_secs`.
#[must_use]
pub fn parse_naive(s: &str, offset_secs: i32) -> Option<Stamp> {
    Stamp::from_naive(s, offset_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::NEM_OFFSET_SECS;

    #[test]
    fn the_lake_names_read_and_write_the_family_stamp() {
        let t = 1_707_812_820; // 2024-02-13T08:27:00Z
        assert_eq!(
            format_rfc3339(t, NEM_OFFSET_SECS),
            "2024-02-13T18:27:00+10:00"
        );
        assert_eq!(
            parse_rfc3339("2024-02-13T08:27:00Z"),
            Some(Stamp::new(t, 0))
        );
        assert!(is_rfc3339("2024-02-13T08:27:00Z") && !is_rfc3339("2024-02-13 08:27:00"));
        assert_eq!(
            parse_http_date("Tue, 13 Feb 2024 08:27:00 GMT"),
            Some(Stamp::new(t, 0))
        );
        assert_eq!(
            parse_wem_generated("Thu Nov 02 2023 11:35:00Z")
                .map(|s| s.rfc3339())
                .as_deref(),
            Some("2023-11-02T11:35:00Z")
        );
        assert_eq!(
            parse_iis_listing("2/13/2024  6:27 PM", NEM_OFFSET_SECS),
            Some(Stamp::new(t, NEM_OFFSET_SECS))
        );
        assert_eq!(
            parse_naive("2024-02-13 18:27:00", NEM_OFFSET_SECS),
            Some(Stamp::new(t, NEM_OFFSET_SECS))
        );
        assert_eq!(
            normalise("Monday, May 18, 2026 05:27 PM", NEM_OFFSET_SECS).as_deref(),
            Some("2026-05-18T17:27:00+10:00")
        );
        assert_eq!(parse("yesterday", 0), None);
    }
}
