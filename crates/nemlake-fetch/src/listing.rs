//! Parsing an IIS directory listing, the only kind nemweb serves.
//!
//! ```html
//! <pre><A HREF="/x/">[To Parent Directory]</A><br><br>
//!  2/13/2024  8:27 AM      1584114 <A HREF="/x/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip">PUBLIC_DVD_DISPATCHPRICE_202401010000.zip</A><br>
//!  2/13/2024  8:27 AM        &lt;dir&gt; <A HREF="/x/sub/">sub</A><br>
//! ```

use memchr::memmem;

/// One link in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The href as written (usually an absolute path).
    pub href: String,
    /// The link text.
    pub name: String,
    /// Size in bytes when the line showed one.
    pub size: Option<u64>,
    /// The date and time text before the size, trimmed.
    pub modified: Option<String>,
    /// Whether the line marked it `<dir>`.
    pub is_dir: bool,
}

/// Extracts every link with the text that precedes it on its line.
pub fn parse(html: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let finder = memmem::Finder::new(b"HREF=\"");
    let finder_lower = memmem::Finder::new(b"href=\"");
    let mut pos = 0usize;
    let mut last_end = 0usize;
    while pos < html.len() {
        let hit = match (finder.find(&html[pos..]), finder_lower.find(&html[pos..])) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let Some(off) = hit else { break };
        let href_start = pos + off + 6;
        let Some(href_len) = memchr::memchr(b'"', &html[href_start..]) else {
            break;
        };
        let href = String::from_utf8_lossy(&html[href_start..href_start + href_len]).into_owned();
        // Anchor text: after the next '>' up to '<'.
        let after_tag = href_start + href_len;
        let Some(gt) = memchr::memchr(b'>', &html[after_tag..]) else {
            break;
        };
        let text_start = after_tag + gt + 1;
        let text_len = memchr::memchr(b'<', &html[text_start..]).unwrap_or(html.len() - text_start);
        let name = String::from_utf8_lossy(&html[text_start..text_start + text_len])
            .trim()
            .to_owned();
        // The prefix: text between the previous anchor end (or line break) and this tag's '<'.
        let tag_start = html[..pos + off]
            .iter()
            .rposition(|&b| b == b'<')
            .unwrap_or(pos + off);
        let prefix_start = html[last_end..tag_start]
            .iter()
            .rposition(|&b| b == b'>')
            .map_or(last_end, |i| last_end + i + 1);
        let prefix =
            String::from_utf8_lossy(&html[prefix_start..tag_start]).replace("&lt;dir&gt;", "<dir>");
        let prefix = prefix.trim();
        let is_dir = prefix.ends_with("<dir>");
        let (modified, size) = if is_dir {
            (
                Some(prefix.trim_end_matches("<dir>").trim().to_owned()).filter(|s| !s.is_empty()),
                None,
            )
        } else {
            match prefix.rsplit_once(char::is_whitespace) {
                Some((date, size))
                    if size.bytes().all(|b| b.is_ascii_digit()) && !size.is_empty() =>
                {
                    (
                        Some(date.trim().to_owned()).filter(|s| !s.is_empty()),
                        size.parse().ok(),
                    )
                }
                _ => (None, None),
            }
        };
        if !name.starts_with('[') {
            out.push(Entry {
                href,
                name,
                size,
                modified,
                is_dir,
            });
        }
        last_end = text_start + text_len;
        pos = last_end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nemweb_fixture() {
        let html = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/listings/mmsdm_2024_01_data.html"
        ))
        .unwrap();
        let entries = parse(&html);
        assert_eq!(
            entries.len(),
            149,
            "every file of the fixture listing, parent link excluded"
        );
        let price = entries
            .iter()
            .find(|e| e.name == "PUBLIC_DVD_DISPATCHPRICE_202401010000.zip")
            .unwrap();
        assert_eq!(price.size, Some(1_584_114));
        assert!(
            price
                .href
                .ends_with("/DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip")
        );
        assert!(price.modified.as_deref().unwrap().contains("2024"));
        assert!(!price.is_dir);
        assert!(
            entries.iter().all(|e| !e.name.starts_with('[')),
            "parent link skipped"
        );
    }

    #[test]
    fn parses_dirs_and_odd_markup() {
        let html = br#"<pre><A HREF="/a/">[To Parent Directory]</A><br><br> 2/13/2024  8:27 AM        &lt;dir&gt; <A HREF="/a/sub/">sub</A><br> 2/13/2024  8:28 AM          123 <a href="/a/f.zip">f.zip</a><br>junk <A HREF="/a/g.csv">g.csv</A></pre>"#;
        let e = parse(html);
        assert_eq!(e.len(), 3);
        assert!(e[0].is_dir);
        assert_eq!(e[0].name, "sub");
        assert_eq!(e[0].size, None);
        assert_eq!(e[0].modified.as_deref(), Some("2/13/2024  8:27 AM"));
        assert_eq!(e[1].size, Some(123));
        assert_eq!(e[1].name, "f.zip");
        assert_eq!(e[2].size, None);
        assert_eq!(e[2].modified, None);
        assert!(parse(b"<html>nothing</html>").is_empty());
        assert!(parse(b"<A HREF=\"unterminated").is_empty());
    }
}
