//! Wall-clock time as RFC 3339 UTC text, for manifests and sidecars. Never
//! used on the hot path.

/// Now, as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_utc_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    rfc3339_from_unix(secs)
}

/// A Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339_from_unix(secs: i64) -> String {
    crate::timetext::format_rfc3339(secs, 0)
}

/// Seconds since the Unix epoch, now.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_704_067_200), "2024-01-01T00:00:00Z");
        let n = now_utc_rfc3339();
        assert_eq!(n.len(), 20);
        // What the clock writes says its offset, UTC.
        let read = crate::timetext::parse_rfc3339(&n).expect("the clock writes RFC 3339");
        assert_eq!(read.offset_secs, 0);
        assert!(read.secs_utc > 1_700_000_000);
        assert!(unix_now() > 1_700_000_000);
    }
}
