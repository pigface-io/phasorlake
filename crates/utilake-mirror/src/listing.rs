//! A strict reader of nemweb's IIS directory listings.
//!
//! ```html
//! <pre><A HREF="/x">[To Parent Directory]</A><br><br>
//!     Tuesday, December 9, 2025 09:05 AM   1912544534 <A HREF="/x/y/a.zip">a.zip</A><br>
//!     Monday, October 12, 2026 10:45 AM        &lt;dir&gt; <A HREF="/x/y/sub/">sub</A><br></pre>
//! ```
//!
//! Every line of the `<pre>` block must be the parent link, a folder or a
//! file, and no link may stand outside the block. Anything else is an
//! [`Unparsed`] error rather than a link quietly passed over, so a change
//! in how AEMO lists its folders stops the crawl instead of losing files.

use crate::stamp::Stamp;

/// What a listed link is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// A folder, marked `<dir>`.
    Dir,
    /// A file, with its size.
    File,
}

/// One folder or file of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The href as written.
    pub href: String,
    /// The link text, with HTML entities decoded.
    pub name: String,
    /// Folder or file.
    pub kind: LinkKind,
    /// The size in bytes; files only.
    pub size: Option<u64>,
    /// The date and time text in front of the size, as written.
    pub modified: String,
    /// The same time as an instant: the listing's times are AEST.
    pub modified_at: Stamp,
}

/// A listing that is not in the expected form: the first part that is not.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: `{text}`")]
pub struct Unparsed {
    /// What was wrong.
    pub reason: &'static str,
    /// The text, bounded.
    pub text: String,
}

impl Unparsed {
    fn new(reason: &'static str, text: &[u8]) -> Self {
        let end = text.len().min(300);
        Self {
            reason,
            text: String::from_utf8_lossy(&text[..end]).into_owned(),
        }
    }
}

/// Every folder and file a listing page links to, in page order.
pub fn parse(html: &[u8]) -> Result<Vec<Link>, Unparsed> {
    let open = find_ci(html, b"<pre>", 0)
        .ok_or_else(|| Unparsed::new("the page has no <pre> listing", html))?;
    let body_start = open + b"<pre>".len();
    let close = find_ci(html, b"</pre>", body_start)
        .ok_or_else(|| Unparsed::new("the <pre> listing is not closed", &html[open..]))?;
    for outside in [&html[..open], &html[close..]] {
        if let Some(at) = find_anchor(outside) {
            return Err(Unparsed::new(
                "a link stands outside the listing",
                &outside[at..],
            ));
        }
    }
    let body = &html[body_start..close];
    let mut out = Vec::with_capacity(body.len() / 160);
    let mut parent_seen = false;
    // Lines of a page mostly share a date: the last one read is kept.
    let mut last_date: Option<(String, Stamp)> = None;
    for line in split_ci(body, b"<br>") {
        let line = trim(line);
        if line.is_empty() {
            continue;
        }
        match parse_line(line, &mut last_date)? {
            Line::Parent if !parent_seen && out.is_empty() => parent_seen = true,
            Line::Parent => return Err(Unparsed::new("a second parent link", line)),
            Line::Link(link) => out.push(link),
        }
    }
    Ok(out)
}

enum Line {
    Parent,
    Link(Link),
}

/// One `<br>`-separated line: `{date} {size|<dir>} <A HREF="{href}">{name}</A>`,
/// or the parent link alone. `last_date` is the date text read before and
/// its instant, reused when this line's is the same.
fn parse_line(line: &[u8], last_date: &mut Option<(String, Stamp)>) -> Result<Line, Unparsed> {
    let at = find_ci(line, b"<a href=\"", 0)
        .ok_or_else(|| Unparsed::new("a listing line without a link", line))?;
    let href_start = at + b"<a href=\"".len();
    let href_len = memchr::memchr(b'"', &line[href_start..])
        .ok_or_else(|| Unparsed::new("a link whose href is not closed", line))?;
    let after_href = href_start + href_len + 1;
    if line.get(after_href) != Some(&b'>') {
        return Err(Unparsed::new("a link with more than an href", line));
    }
    let text_start = after_href + 1;
    let text_end = find_ci(line, b"</a>", text_start)
        .ok_or_else(|| Unparsed::new("a link that is not closed", line))?;
    if !trim(&line[text_end + b"</a>".len()..]).is_empty() {
        return Err(Unparsed::new("text after the link", line));
    }
    let href = &line[href_start..href_start + href_len];
    let text = &line[text_start..text_end];
    let prefix = trim(&line[..at]);
    if text == b"[To Parent Directory]" {
        return if prefix.is_empty() {
            Ok(Line::Parent)
        } else {
            Err(Unparsed::new("text before the parent link", line))
        };
    }
    if href.is_empty() || text.is_empty() || memchr::memchr2(b'<', b'>', text).is_some() {
        return Err(Unparsed::new("a link without an href or a name", line));
    }
    let (kind, size, modified) = if let Some(m) = strip_suffix(prefix, b"&lt;dir&gt;") {
        (LinkKind::Dir, None, trim(m))
    } else {
        let split = prefix
            .iter()
            .rposition(u8::is_ascii_whitespace)
            .ok_or_else(|| Unparsed::new("a line with neither a size nor <dir>", line))?;
        let digits = &prefix[split + 1..];
        let size = std::str::from_utf8(digits)
            .ok()
            .filter(|d| d.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|d| d.parse::<u64>().ok())
            .ok_or_else(|| Unparsed::new("a line with neither a size nor <dir>", line))?;
        (LinkKind::File, Some(size), trim(&prefix[..split]))
    };
    if modified.is_empty() {
        return Err(Unparsed::new("a line without a date", line));
    }
    let modified = String::from_utf8_lossy(modified).into_owned();
    let modified_at = match last_date {
        Some((text, at)) if *text == modified => *at,
        _ => {
            let at = Stamp::from_listing(&modified)
                .ok_or_else(|| Unparsed::new("a line whose date cannot be read", line))?;
            *last_date = Some((modified.clone(), at));
            at
        }
    };
    Ok(Line::Link(Link {
        href: String::from_utf8_lossy(href).into_owned(),
        name: unescape(&String::from_utf8_lossy(text)),
        kind,
        size,
        modified,
        modified_at,
    }))
}

/// The first `<a` followed by whitespace or `>`, any case.
fn find_anchor(b: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(at) = find_ci(b, b"<a", from) {
        match b.get(at + 2) {
            Some(c) if c.is_ascii_whitespace() || *c == b'>' => return Some(at),
            _ => from = at + 2,
        }
    }
    None
}

/// `needle` (lower case) in `hay` at or after `from`, any case.
fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let first = needle[0];
    let mut pos = from;
    while pos + needle.len() <= hay.len() {
        let rel = memchr::memchr2(first, first.to_ascii_uppercase(), &hay[pos..])?;
        let at = pos + rel;
        if at + needle.len() > hay.len() {
            return None;
        }
        if hay[at..at + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(at);
        }
        pos = at + 1;
    }
    None
}

/// The pieces of `hay` between occurrences of `sep` (lower case), any case.
fn split_ci<'h>(hay: &'h [u8], sep: &'h [u8]) -> impl Iterator<Item = &'h [u8]> {
    let mut rest = Some(hay);
    std::iter::from_fn(move || {
        let h = rest?;
        if let Some(at) = find_ci(h, sep, 0) {
            rest = Some(&h[at + sep.len()..]);
            Some(&h[..at])
        } else {
            rest = None;
            Some(h)
        }
    })
}

fn strip_suffix<'b>(b: &'b [u8], suffix: &[u8]) -> Option<&'b [u8]> {
    (b.len() >= suffix.len() && b[b.len() - suffix.len()..].eq_ignore_ascii_case(suffix))
        .then(|| &b[..b.len() - suffix.len()])
}

fn trim(b: &[u8]) -> &[u8] {
    b.trim_ascii()
}

/// An href with its HTML character references decoded, as a browser reads
/// it: the five named ones and numeric ones (`&#x2B;` is `+`), so a `#`
/// written as a reference is not taken for a fragment. A reference that
/// is not one of these, or not a character, is kept as written.
pub fn href_text(href: &str) -> String {
    if !href.contains('&') {
        return href.to_owned();
    }
    let mut out = String::with_capacity(href.len());
    let mut rest = href;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let n = name.strip_prefix('#')?;
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((c, end + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The five entities IIS writes into link text.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_href_is_read_with_its_references_decoded() {
        assert_eq!(
            href_text("/R/Electricity&#x2B;Data&#x2B;Model&#43;Report.htm"),
            "/R/Electricity+Data+Model+Report.htm"
        );
        assert_eq!(
            href_text("/x/a&amp;b&lt;&gt;&quot;&apos;.zip"),
            "/x/a&b<>\"'.zip"
        );
        assert_eq!(href_text("/plain.zip"), "/plain.zip");
        // Not a reference, or not a character: kept as written.
        assert_eq!(href_text("/a&b.zip"), "/a&b.zip");
        assert_eq!(
            href_text("/a&nbsp;b&;c&#xZZ;d&#xD800;e&#x2B"),
            "/a&nbsp;b&;c&#xZZ;d&#xD800;e&#x2B"
        );
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/listings/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn page(lines: &str) -> Vec<u8> {
        format!(
            "<html><head><script>var a='<a'</script></head><body><H1>x</H1><hr>\n<pre>{lines}</pre><hr></body></html>"
        )
        .into_bytes()
    }

    #[test]
    fn reads_folders_from_a_real_page() {
        let links = parse(&fixture("nemweb_wholesale_electricity.html")).unwrap();
        let names: Vec<_> = links.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["FCAS_Causer_Pays", "MMSDM", "NEMDE"]);
        assert!(
            links
                .iter()
                .all(|l| l.kind == LinkKind::Dir && l.size.is_none())
        );
        assert_eq!(links[1].href, "/Data_Archive/Wholesale_Electricity/MMSDM/");
        assert_eq!(links[1].modified, "Monday, October 12, 2026 10:45 AM");
        assert_eq!(
            links[1].modified_at.to_string(),
            "2026-10-12T10:45:00+10:00"
        );
    }

    #[test]
    fn reads_files_from_a_real_page() {
        let links = parse(&fixture("nemweb_fcas_causer_pays_2017.html")).unwrap();
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].kind, LinkKind::File);
        assert_eq!(links[0].size, Some(1_912_544_534));
        assert_eq!(links[0].name, "FCAS_Causer_Pays_2017_01.zip");
        assert_eq!(links[0].modified, "Tuesday, December 9, 2025 09:05 AM");
    }

    #[test]
    fn reads_every_link_of_the_mmsdm_page() {
        let html = fixture("mmsdm_2024_01_data.html");
        let links = parse(&html).unwrap();
        let anchors = html
            .windows(8)
            .filter(|w| w.eq_ignore_ascii_case(b"<a href="))
            .count();
        assert_eq!(links.len() + 1, anchors, "every anchor but the parent link");
    }

    #[test]
    fn short_dates_entities_and_any_case() {
        let html = page(
            "<a href=\"/x\">[To Parent Directory]</a><BR><BR> 2/13/2024  8:27 AM   12 <a href=\"/x/a&amp;b.zip\">a&amp;b &lt;1&gt;.zip</a><br>",
        );
        let links = parse(&html).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "a&b <1>.zip");
        assert_eq!(links[0].href, "/x/a&amp;b.zip");
        assert_eq!(links[0].modified, "2/13/2024  8:27 AM");
        assert_eq!(
            links[0].modified_at.to_string(),
            "2024-02-13T08:27:00+10:00"
        );
        assert!(parse(&page("")).unwrap().is_empty());
    }

    #[test]
    fn refuses_anything_else() {
        let line = |l: &str| parse(&page(l)).unwrap_err().reason;
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5 a.zip"),
            "a listing line without a link"
        );
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5 <A HREF=\"/a"),
            "a link whose href is not closed"
        );
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5 <A HREF=\"/a\" title=\"t\">a</A>"),
            "a link with more than an href"
        );
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5 <A HREF=\"/a\">a"),
            "a link that is not closed"
        );
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5 <A HREF=\"/a\">a</A> new"),
            "text after the link"
        );
        assert_eq!(
            line(" x <A HREF=\"/\">[To Parent Directory]</A>"),
            "text before the parent link"
        );
        assert_eq!(
            line(
                " 1/1/2024 1:00 AM 5 <A HREF=\"/a\">a</A><br><A HREF=\"/\">[To Parent Directory]</A>"
            ),
            "a second parent link"
        );
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5 <A HREF=\"\">a</A>"),
            "a link without an href or a name"
        );
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5 <A HREF=\"/a\"></A>"),
            "a link without an href or a name"
        );
        assert_eq!(
            line(" 1/1/2024 1:00 AM 5x <A HREF=\"/a\">a</A>"),
            "a line with neither a size nor <dir>"
        );
        assert_eq!(
            line("<A HREF=\"/a\">a</A>"),
            "a line with neither a size nor <dir>"
        );
        assert_eq!(
            line(" 5 <A HREF=\"/a\">a</A>"),
            "a line with neither a size nor <dir>"
        );
        assert_eq!(
            line(" &lt;dir&gt; <A HREF=\"/a/\">a</A>"),
            "a line without a date"
        );
        assert_eq!(
            line(" 13/40/2024 8:27 AM 5 <A HREF=\"/a\">a</A>"),
            "a line whose date cannot be read"
        );
        assert_eq!(
            line(" soon &lt;dir&gt; <A HREF=\"/a/\">a</A>"),
            "a line whose date cannot be read"
        );
        assert_eq!(
            parse(b"<html><a href=\"/x\">x</a><pre></pre>")
                .unwrap_err()
                .reason,
            "a link stands outside the listing"
        );
        assert_eq!(
            parse(b"<pre></pre><A\thref=\"/x\">x</A>")
                .unwrap_err()
                .reason,
            "a link stands outside the listing"
        );
        assert_eq!(
            parse(b"<html></html>").unwrap_err().reason,
            "the page has no <pre> listing"
        );
        assert_eq!(
            parse(b"<html><p").unwrap_err().reason,
            "the page has no <pre> listing"
        );
        assert_eq!(
            parse(b"<pre>").unwrap_err().reason,
            "the <pre> listing is not closed"
        );
        let e = parse(b"<pre>").unwrap_err();
        assert_eq!(e.to_string(), "the <pre> listing is not closed: `<pre>`");
    }
}
