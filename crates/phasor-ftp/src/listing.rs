//! What an FTP server says of a folder and a file, as text the mirror's
//! listers read: one JSON line per entry ([`Entry`]), the modification
//! time an HTTP date, so the same code reads an FTP folder and a host of
//! documents.

use phasor_provenance::Stamp;
use phasor_provenance::stamp::UTC;
use serde::{Deserialize, Serialize};

/// One entry of a folder, as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The entry's name in its folder.
    pub name: String,
    /// Whether it is a folder.
    #[serde(default)]
    pub dir: bool,
    /// Its size in bytes, when the server said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Its modification time as an HTTP date (UTC), when the server said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

impl Entry {
    /// An `MLSD` line: facts `key=value;` then a space and the name, e.g.
    /// `type=file;size=12;modify=20261105104730; IDN11060.xml`. Entries that
    /// are the folder itself or its parent (`cdir`, `pdir`) are none.
    pub fn from_mlsd(line: &str) -> Option<Self> {
        let (facts, name) = line.split_once(' ')?;
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let mut entry = Self {
            name: name.to_owned(),
            dir: false,
            size: None,
            modified: None,
        };
        for fact in facts.split(';').filter(|f| !f.is_empty()) {
            let Some((key, value)) = fact.split_once('=') else {
                continue;
            };
            match key.to_ascii_lowercase().as_str() {
                "type" => match value.to_ascii_lowercase().as_str() {
                    "dir" => entry.dir = true,
                    "cdir" | "pdir" => return None,
                    _ => {}
                },
                "size" => entry.size = value.parse().ok(),
                "modify" => entry.modified = http_date(value),
                _ => {}
            }
        }
        Some(entry)
    }

    /// The page of a folder: one line an entry.
    pub fn page(entries: &[Self]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in entries {
            serde_json::to_writer(&mut out, e).expect("an Entry always serialises");
            out.push(b'\n');
        }
        out
    }

    /// The entries of a page, in order; a line that is not one is `None`.
    pub fn read(page: &[u8]) -> Option<Vec<Self>> {
        std::str::from_utf8(page)
            .ok()?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

/// An FTP time (`YYYYMMDDHHMMSS`, UTC, a fraction allowed after it, as
/// `MDTM` answers and `MLSD`'s `modify` fact gives) as an HTTP date.
pub fn http_date(ftp_time: &str) -> Option<String> {
    let digits = ftp_time.trim().split('.').next()?;
    if digits.len() != 14 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let num = |range: std::ops::Range<usize>| digits[range].parse::<u32>().ok();
    let y = digits[0..4].parse::<i32>().ok()?;
    let (m, d) = (num(4..6)?, num(6..8)?);
    let (hh, mm, ss) = (num(8..10)?, num(10..12)?, num(12..14)?);
    let at = Stamp::at(y, m, d, hh, mm, ss, UTC)?;
    (at.unix() >= 0).then(|| at.http_date())
}

/// Whether `name` matches `pattern`: `*` any run of characters, `?` any one,
/// the rest itself.
pub fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

/// The time an `MDTM` reply gives, as an HTTP date: the reply is `213
/// YYYYMMDDHHMMSS`, or the digits alone.
pub fn mdtm_http_date(reply: &str) -> Option<String> {
    let text = reply.trim();
    let digits = text.strip_prefix("213").map_or(text, str::trim);
    http_date(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_mlsd_line_is_an_entry_and_the_page_round_trips() {
        let e = Entry::from_mlsd("type=file;size=12;modify=20261105104730; IDN11060.xml").unwrap();
        assert_eq!(
            e,
            Entry {
                name: "IDN11060.xml".into(),
                dir: false,
                size: Some(12),
                modified: Some("Thu, 05 Nov 2026 10:47:30 GMT".into()),
            }
        );
        let d = Entry::from_mlsd("Type=dir;Modify=20260101000000.123;perm=el; clim_data").unwrap();
        assert!(d.dir && d.size.is_none());
        assert_eq!(d.modified.as_deref(), Some("Thu, 01 Jan 2026 00:00:00 GMT"));
        assert!(Entry::from_mlsd("type=cdir; .").is_none());
        assert!(Entry::from_mlsd("type=pdir; ..").is_none());
        assert!(Entry::from_mlsd("nofacts").is_none());
        assert!(Entry::from_mlsd("type=file; ").is_none());
        let odd = Entry::from_mlsd("size=big;modify=yesterday;junk; a b").unwrap();
        assert_eq!(
            (odd.name.as_str(), odd.size, odd.modified),
            ("a b", None, None)
        );
        let page = Entry::page(&[e.clone(), d.clone()]);
        assert_eq!(Entry::read(&page).unwrap(), [e, d]);
        assert_eq!(Entry::read(b"\n\n").unwrap(), []);
        assert!(Entry::read(b"not json\n").is_none());
        assert!(Entry::read(b"\xff").is_none());
    }

    #[test]
    fn ftp_times_become_http_dates() {
        assert_eq!(
            http_date("20240213082700").as_deref(),
            Some("Tue, 13 Feb 2024 08:27:00 GMT")
        );
        assert_eq!(
            mdtm_http_date("213 20261105104730\r\n").as_deref(),
            Some("Thu, 05 Nov 2026 10:47:30 GMT")
        );
        assert_eq!(
            mdtm_http_date("20261105104730.5").as_deref(),
            Some("Thu, 05 Nov 2026 10:47:30 GMT")
        );
        for bad in [
            "213 nope",
            "2026110510473",
            "20261305104730",
            "20261105250000",
            "",
        ] {
            assert!(mdtm_http_date(bad).is_none(), "{bad}");
        }
        // A leap day, the epoch, and a date before it.
        assert_eq!(
            http_date("20240229120000").as_deref(),
            Some("Thu, 29 Feb 2024 12:00:00 GMT")
        );
        assert_eq!(
            http_date("19700101000000").as_deref(),
            Some("Thu, 01 Jan 1970 00:00:00 GMT")
        );
        assert!(http_date("19691231235959").is_none(), "before the epoch");
    }
}
