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

/// Bytes as text, borrowing when they are UTF-8 (they always are on nemweb).
fn text(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

/// `b` without ASCII whitespace at either end.
fn trim(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &b[start..end]
}

/// The next `href="` at or after `pos`, either case, in one pass over the
/// document: each finder's last hit is kept and searched again only once
/// the scan has passed it.
struct Hrefs<'h> {
    html: &'h [u8],
    finders: [(memmem::Finder<'static>, Option<usize>); 2],
}

impl<'h> Hrefs<'h> {
    fn new(html: &'h [u8]) -> Self {
        let finder = |needle: &'static [u8]| {
            let f = memmem::Finder::new(needle);
            let first = f.find(html);
            (f, first)
        };
        Self {
            html,
            finders: [finder(b"HREF=\""), finder(b"href=\"")],
        }
    }

    fn next(&mut self, pos: usize) -> Option<usize> {
        let html = self.html;
        let mut best: Option<usize> = None;
        for (finder, hit) in &mut self.finders {
            if let Some(h) = *hit
                && h < pos
            {
                *hit = finder.find(&html[pos..]).map(|i| pos + i);
            }
            if let Some(h) = *hit {
                best = Some(best.map_or(h, |b| b.min(h)));
            }
        }
        best
    }
}

/// Extracts every link with the text that precedes it on its line. One pass
/// over the document; only the strings an entry keeps are allocated.
pub fn parse(html: &[u8]) -> Vec<Entry> {
    // A row of an IIS listing is some 150 to 250 bytes.
    let mut out = Vec::with_capacity(html.len() / 160);
    let mut hrefs = Hrefs::new(html);
    let mut pos = 0usize;
    let mut last_end = 0usize;
    while pos < html.len() {
        let Some(at) = hrefs.next(pos) else { break };
        let href_start = at + 6;
        let Some(href_len) = memchr::memchr(b'"', &html[href_start..]) else {
            break;
        };
        // Anchor text: after the next '>' up to '<'.
        let after_tag = href_start + href_len;
        let Some(gt) = memchr::memchr(b'>', &html[after_tag..]) else {
            break;
        };
        let text_start = after_tag + gt + 1;
        let text_len = memchr::memchr(b'<', &html[text_start..]).unwrap_or(html.len() - text_start);
        let name = trim(&html[text_start..text_start + text_len]);
        last_end = {
            // The prefix: text between the previous anchor end (or line
            // break) and this tag's '<'.
            let tag_start = memchr::memrchr(b'<', &html[last_end..at]).map_or(at, |i| last_end + i);
            let prefix_start = memchr::memrchr(b'>', &html[last_end..tag_start])
                .map_or(last_end, |i| last_end + i + 1);
            let prefix = trim(&html[prefix_start..tag_start]);
            if name.first() != Some(&b'[') {
                let dir_mark = [&b"&lt;dir&gt;"[..], b"<dir>"]
                    .into_iter()
                    .find(|m| prefix.ends_with(m));
                let (modified, size) = match dir_mark {
                    Some(m) => (trim(&prefix[..prefix.len() - m.len()]), None),
                    None => match prefix.iter().rposition(u8::is_ascii_whitespace) {
                        Some(i)
                            if i + 1 < prefix.len()
                                && prefix[i + 1..].iter().all(u8::is_ascii_digit) =>
                        {
                            (trim(&prefix[..i]), text(&prefix[i + 1..]).parse().ok())
                        }
                        _ => (&prefix[..0], None),
                    },
                };
                out.push(Entry {
                    href: text(&html[href_start..href_start + href_len]).into_owned(),
                    name: text(name).into_owned(),
                    size,
                    modified: (!modified.is_empty()).then(|| text(modified).into_owned()),
                    is_dir: dir_mark.is_some(),
                });
            }
            text_start + text_len
        };
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
