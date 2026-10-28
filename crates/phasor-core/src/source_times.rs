//! The times a source file carries, each from one place: the server's `Last-Modified`
//! and the timestamp in its file name. Neither is judged here; each is stamped on every row.

use crate::timetext;

/// The server's RFC 3339 `Last-Modified` in UTC microseconds; none when absent or unreadable.
pub fn server_last_modified(rfc3339: Option<&str>) -> Option<i64> {
    let s = timetext::parse_rfc3339(rfc3339?)?;
    Some(s.secs_utc * 1_000_000 + i64::from(s.nanos / 1_000))
}

/// The first run of exactly fourteen digits in a file's name, read as `YYYYMMDDHHMMSS`
/// at `offset_secs`, in UTC microseconds. A twelve-digit run is an interval, not a
/// creation time, and is not read. None when there is no such run or it is not a time.
pub fn filename_timestamp(url: &str, offset_secs: i32) -> Option<i64> {
    let d = digits_run(file_name(url), 14)?;
    let text = format!(
        "{}-{}-{}T{}:{}:{}",
        &d[0..4],
        &d[4..6],
        &d[6..8],
        &d[8..10],
        &d[10..12],
        &d[12..14]
    );
    timetext::parse_naive(&text, offset_secs).map(|s| s.secs_utc * 1_000_000)
}

/// The last path segment of a URL or path, without query or fragment.
fn file_name(url: &str) -> &str {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    no_query.rsplit(['/', '\\']).next().unwrap_or(no_query)
}

/// The first run of exactly `n` digits bounded by non-digits.
fn digits_run(name: &str, n: usize) -> Option<&str> {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == n {
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
    fn the_servers_word_is_read_as_it_was_recorded() {
        assert_eq!(
            server_last_modified(Some("2024-02-05T03:04:05Z")),
            Some(1_707_102_245_000_000)
        );
        assert_eq!(server_last_modified(Some("bad")), None);
        assert_eq!(server_last_modified(None), None);
    }

    #[test]
    fn a_names_fourteen_digits_are_when_it_was_made() {
        // 2016-12-23 04:05:19 +10:00.
        let yestbid =
            "https://h/Reports/ARCHIVE/YestBid/PUBLIC_YESTBID_201612220000_20161223040519.zip";
        assert_eq!(
            filename_timestamp(yestbid, NEM),
            Some(1_482_429_919_000_000)
        );
        // A gas restatement's stamp, after its `~`, in AEST too.
        let gas = "file:///x/int651_v1_ex_ante_market_price_rpt_1~20261023113222.csv";
        assert_eq!(filename_timestamp(gas, NEM), Some(1_792_719_142_000_000));
        // Twelve digits are an interval, sixteen a sequence number: no time.
        let scada = "https://h/PUBLIC_DISPATCHSCADA_202410170005_0000000441234567.zip?x=1";
        assert_eq!(filename_timestamp(scada, NEM), None);
        // Fourteen digits that are not a time; a monthly archive's name.
        assert_eq!(filename_timestamp("C:\\a\\X_20241399999999.csv", NEM), None);
        assert_eq!(
            filename_timestamp("PUBLIC_DVD_T_202401010000.zip", NEM),
            None
        );
        assert_eq!(digits_run("abc", 12), None);
    }
}
