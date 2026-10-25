//! When a source was published: what the server said when it served it,
//! else the time in a report file's name. Never the time of the fetch.
//! The writer stamps it on every lake row; the query reads it back.

use crate::timetext;

/// The publish time of a source file.
///
/// 1. The server's `Last-Modified` (`last_modified_at`, RFC 3339), when it
///    gave one: the moment the bytes went public. A report's name stamps
///    the interval it reports, which comes a little before.
/// 2. Otherwise, a name that is not an archive's (`PUBLIC_DVD_T_…`,
///    `PUBLIC_ARCHIVE#…`, which carry a month, not a time) with a run of
///    twelve digits bounded by non-digits
///    (`PUBLIC_DISPATCHSCADA_202410170005_0000000441234567.zip`) is
///    published at that `YYYYMMDDHHMM` in the market's clock.
/// 3. Otherwise nothing.
///
/// In microseconds since 1970 (UTC); `offset_secs` is the market's clock.
pub fn published(url: &str, last_modified_at: Option<&str>, offset_secs: i32) -> Option<i64> {
    if let Some(s) = last_modified_at.and_then(timetext::parse_rfc3339) {
        return Some(s.secs_utc * 1_000_000 + i64::from(s.nanos / 1_000));
    }
    let name = file_name(url);
    if utilake_mms_decode::filename::parse(name).is_some() {
        return None;
    }
    let d = twelve_digits(name)?;
    let text = format!(
        "{}-{}-{}T{}:{}:00",
        &d[0..4],
        &d[4..6],
        &d[6..8],
        &d[8..10],
        &d[10..12]
    );
    timetext::parse_naive(&text, offset_secs).map(|s| s.secs_utc * 1_000_000)
}

/// The last path segment of a URL or path, without a query string.
fn file_name(url: &str) -> &str {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    no_query.rsplit(['/', '\\']).next().unwrap_or(no_query)
}

/// The first run of exactly twelve digits bounded by non-digits.
fn twelve_digits(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 12 {
                return Some(&name[start..i]);
            }
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEM: i32 = 36_000;

    #[test]
    fn an_archive_month_is_not_a_time() {
        let url = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip";
        assert_eq!(published(url, None, NEM), None);
        let t = published(url, Some("2024-02-05T03:04:05Z"), NEM).unwrap();
        assert_eq!(t, 1_707_102_245_000_000);
        let hashed = "https://h/PUBLIC_ARCHIVE%23DISPATCHPRICE%23FILE01%23202401010000.zip";
        assert_eq!(published(hashed, Some("bad"), NEM), None);
    }

    #[test]
    fn a_report_names_its_time() {
        let url = "https://nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_202410170005_0000000441234567.zip?x=1";
        // 2024-10-17 00:05 +10:00, without the server's word.
        assert_eq!(published(url, None, NEM), Some(1_729_087_500_000_000));
        // With it, the server's: when the bytes went public.
        let t = published(url, Some("2024-10-16T14:05:12Z"), NEM).unwrap();
        assert_eq!(t, 1_729_087_512_000_000);
        // Thirteen digits are not a time; the server's word is used.
        let t = published(
            "file:///x/PUBLIC_X_2024101700051.zip",
            Some("2024-10-20T00:00:00Z"),
            NEM,
        )
        .unwrap();
        assert_eq!(t, 1_729_382_400_000_000);
        assert_eq!(
            published("file:///x/PUBLIC_X_2024101700051.zip", None, NEM),
            None
        );
        // A month that does not exist is not a time.
        assert_eq!(
            published("file:///x/PUBLIC_X_202413170005.zip", None, NEM),
            None
        );
        assert_eq!(file_name("C:\\a\\b.zip#frag"), "b.zip");
        assert_eq!(twelve_digits("abc"), None);
    }
}
