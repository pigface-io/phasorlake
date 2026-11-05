//! A strict reader of an Azure Blob Storage container listing, the way the
//! Electricity Authority's `emidatasets` account lists a folder:
//!
//! ```text
//! GET https://{account}.blob.core.windows.net/{container}
//!     ?restype=container&comp=list&prefix={folder}/&delimiter=/[&marker={next}]
//! ```
//!
//! ```xml
//! <EnumerationResults ContainerName="https://emidatasets.blob.core.windows.net/publicdata">
//!   <Prefix>Datasets/Wholesale/Generation/Generation_MD/</Prefix><Delimiter>/</Delimiter>
//!   <Blobs>
//!     <BlobPrefix><Name>Datasets/Wholesale/Generation/GenerationFleet/</Name></BlobPrefix>
//!     <Blob><Name>Datasets/Wholesale/Generation/Generation_MD/199708_Generation_MD.csv</Name>
//!       <Properties><Last-Modified>Mon, 15 Oct 2018 05:23:04 GMT</Last-Modified>
//!       <Etag>0x8D6325E48507017</Etag><Content-Length>406762</Content-Length>
//!       <Content-MD5>s6RPR+qrEXiB5gtJstQsHw==</Content-MD5></Properties></Blob>
//!   </Blobs>
//!   <NextMarker>2!152!MDAw...</NextMarker>
//! </EnumerationResults>
//! ```
//!
//! A page holds at most 5,000 blobs; `NextMarker` names the next page, and
//! is empty on the last. A folder is also a zero-length blob of the folder's
//! name (a hierarchical namespace's marker), which a reader skips by its
//! size. Anything in `<Blobs>` that is not a blob or a prefix is an
//! [`Unparsed`] error, so a change in the format stops a crawl.

/// One blob of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    /// Its name: the path under the container.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
    /// The HTTP date the store last wrote it, as written.
    pub last_modified: String,
    /// Its ETag.
    pub etag: Option<String>,
    /// Its MD5, base64, when the store has one (not for a large blob).
    pub md5: Option<String>,
}

/// One page of a listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    /// The blobs under the prefix, in name order.
    pub blobs: Vec<Blob>,
    /// The folders under the prefix, each ending in the delimiter.
    pub prefixes: Vec<String>,
    /// The marker of the next page; none on the last.
    pub next_marker: Option<String>,
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

/// The position of `needle` in `hay` at or after `from`.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| from + i)
}

/// The text of the first `<tag>…</tag>` in `xml`, or `None` when the tag
/// is absent or self-closing (`<tag />`), with XML's entities decoded.
fn element(xml: &[u8], tag: &str) -> Result<Option<String>, Unparsed> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = find(xml, open.as_bytes(), 0) else {
        return Ok(None);
    };
    let text_start = start + open.len();
    let end = find(xml, close.as_bytes(), text_start)
        .ok_or_else(|| Unparsed::new("an element is not closed", &xml[start..]))?;
    Ok(Some(decode(&xml[text_start..end])))
}

/// XML's five entities decoded; anything else is kept as written.
fn decode(text: &[u8]) -> String {
    let s = String::from_utf8_lossy(text);
    if !s.contains('&') {
        return s.into_owned();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every blob and prefix of a listing page, and the next page's marker.
pub fn parse(xml: &[u8]) -> Result<Page, Unparsed> {
    let xml = xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(xml);
    let open = find(xml, b"<Blobs>", 0)
        .ok_or_else(|| Unparsed::new("the page has no <Blobs> listing", xml))?;
    let body_start = open + b"<Blobs>".len();
    let close = find(xml, b"</Blobs>", body_start)
        .ok_or_else(|| Unparsed::new("the <Blobs> listing is not closed", &xml[open..]))?;
    let body = &xml[body_start..close];
    let mut page = Page::default();
    let mut pos = 0;
    while pos < body.len() {
        let rest = &body[pos..];
        if rest.iter().all(u8::is_ascii_whitespace) {
            break;
        }
        let item_end = |tag: &[u8]| {
            find(rest, tag, 0).ok_or_else(|| Unparsed::new("an item is not closed", rest))
        };
        if rest.starts_with(b"<Blob>") {
            let end = item_end(b"</Blob>")?;
            let item = &rest[..end];
            let need = |tag: &str| {
                element(item, tag)?.ok_or_else(|| Unparsed::new("a blob lacks a part", item))
            };
            let name = need("Name")?;
            let size: u64 = need("Content-Length")?
                .parse()
                .map_err(|_| Unparsed::new("a blob's size is not a number", item))?;
            page.blobs.push(Blob {
                name,
                size,
                last_modified: need("Last-Modified")?,
                etag: element(item, "Etag")?,
                md5: element(item, "Content-MD5")?,
            });
            pos += end + b"</Blob>".len();
        } else if rest.starts_with(b"<BlobPrefix>") {
            let end = item_end(b"</BlobPrefix>")?;
            let item = &rest[..end];
            let name = element(item, "Name")?
                .ok_or_else(|| Unparsed::new("a prefix lacks its name", item))?;
            page.prefixes.push(name);
            pos += end + b"</BlobPrefix>".len();
        } else if rest[0].is_ascii_whitespace() {
            pos += 1;
        } else {
            return Err(Unparsed::new(
                "the listing holds something that is neither a blob nor a prefix",
                rest,
            ));
        }
    }
    page.next_marker = element(&xml[close..], "NextMarker")?.filter(|m| !m.is_empty());
    Ok(page)
}

/// Every blob and prefix of one or more listing pages kept one after
/// another, each an `<EnumerationResults>` document, as a mirror keeps a
/// folder's pages; the last page's marker. A single page without the
/// wrapper reads as [`parse`] reads it.
pub fn parse_pages(bytes: &[u8]) -> Result<Page, Unparsed> {
    const OPEN: &[u8] = b"<EnumerationResults";
    const CLOSE: &[u8] = b"</EnumerationResults>";
    let Some(mut pos) = find(bytes, OPEN, 0) else {
        return parse(bytes);
    };
    let mut all = Page::default();
    while let Some(start) = find(bytes, OPEN, pos) {
        let end = find(bytes, CLOSE, start)
            .ok_or_else(|| Unparsed::new("a listing page is not closed", &bytes[start..]))?;
        let page = parse(&bytes[start..end + CLOSE.len()])?;
        all.blobs.extend(page.blobs);
        all.prefixes.extend(page.prefixes);
        all.next_marker = page.next_marker;
        pos = end + CLOSE.len();
    }
    Ok(all)
}

/// The listing request for `folder` (a blob folder's URL, `https://{account}/
/// {container}/{prefix}/`): one page, from `marker` when given. The folder
/// URL stays plain everywhere else; only this request carries a query.
pub fn list_url(folder: &str, marker: Option<&str>) -> Option<String> {
    let (scheme, rest) = folder.split_once("://")?;
    let (host, path) = rest.split_once('/')?;
    let (container, prefix) = path.split_once('/').unwrap_or((path, ""));
    if host.is_empty() || container.is_empty() {
        return None;
    }
    let mut prefix = prefix.trim_start_matches('/').to_owned();
    if !prefix.is_empty() && !prefix.ends_with('/') {
        prefix.push('/');
    }
    let mut url = format!(
        "{scheme}://{host}/{container}?restype=container&comp=list&prefix={}&delimiter=/",
        encode(&prefix)
    );
    if let Some(m) = marker {
        url.push_str("&marker=");
        url.push_str(&encode(m));
    }
    Some(url)
}

/// `s` as a query value: letters, digits, `-._~!/` as they are, the rest
/// percent-encoded (a marker is base64-like text; a prefix is a path).
fn encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!/".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/listings/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn a_page_of_blobs_with_its_next_marker() {
        let page = parse(&fixture("emi_generation_md_page.xml")).unwrap();
        assert_eq!(page.blobs.len(), 3);
        assert!(page.prefixes.is_empty());
        let first = &page.blobs[0];
        assert_eq!(
            first.name,
            "Datasets/Wholesale/Generation/Generation_MD/199708_Generation_MD.csv"
        );
        assert_eq!(first.size, 406_762);
        assert_eq!(first.last_modified, "Mon, 15 Oct 2018 05:23:04 GMT");
        assert_eq!(first.etag.as_deref(), Some("0x8D6325E48507017"));
        assert_eq!(first.md5.as_deref(), Some("s6RPR+qrEXiB5gtJstQsHw=="));
        assert!(page.next_marker.as_deref().unwrap().starts_with("2!152!"));
    }

    #[test]
    fn a_folder_listing_has_prefixes_and_zero_length_folder_blobs_and_no_next_page() {
        let page = parse(&fixture("emi_dispatch_and_pricing.xml")).unwrap();
        assert!(
            page.prefixes.contains(
                &"Datasets/Wholesale/DispatchAndPricing/DispatchEnergyPrices/".to_owned()
            )
        );
        assert!(page.prefixes.iter().all(|p| p.ends_with('/')));
        // The folder markers: zero-length blobs with no MD5.
        let marker = page
            .blobs
            .iter()
            .find(|b| b.name.ends_with("/DispatchEnergyPrices"))
            .unwrap();
        assert_eq!((marker.size, marker.md5.as_deref()), (0, None));
        assert_eq!(page.next_marker, None, "an empty <NextMarker /> is the end");
    }

    #[test]
    fn what_is_not_a_listing() {
        let bad = |xml: &str| parse(xml.as_bytes()).unwrap_err().reason;
        assert_eq!(bad("<x/>"), "the page has no <Blobs> listing");
        assert_eq!(bad("<Blobs><Blob>"), "the <Blobs> listing is not closed");
        assert_eq!(
            bad("<Blobs><Blob><Name>a</Name></Blobs>"),
            "an item is not closed"
        );
        assert_eq!(
            bad("<Blobs><Blob><Name>a</Name></Blob></Blobs>"),
            "a blob lacks a part"
        );
        assert_eq!(
            bad(
                "<Blobs><Blob><Name>a</Name><Last-Modified>x</Last-Modified><Content-Length>big</Content-Length></Blob></Blobs>"
            ),
            "a blob's size is not a number"
        );
        assert_eq!(
            bad("<Blobs><BlobPrefix></BlobPrefix></Blobs>"),
            "a prefix lacks its name"
        );
        assert_eq!(
            bad("<Blobs><Other/></Blobs>"),
            "the listing holds something that is neither a blob nor a prefix"
        );
        assert_eq!(
            bad("<Blobs><Blob><Name>a</Blob></Blobs>"),
            "an element is not closed"
        );
        let e = parse(b"<Blobs><Other/></Blobs>").unwrap_err();
        assert!(e.to_string().starts_with("the listing holds"));
        // Whitespace between items, entities in names, a page with nothing.
        let page = parse(
            b"\xEF\xBB\xBF<Blobs>\n  <BlobPrefix><Name>a &amp; b/</Name></BlobPrefix>\n  <Blob><Name>a &amp; b/x&lt;y&gt;.csv</Name><Properties><Last-Modified>Mon, 15 Oct 2018 05:23:04 GMT</Last-Modified><Content-Length>7</Content-Length></Properties></Blob>\n</Blobs><NextMarker />",
        )
        .unwrap();
        assert_eq!(page.prefixes, ["a & b/"]);
        assert_eq!(page.blobs[0].name, "a & b/x<y>.csv");
        assert_eq!(page.blobs[0].etag, None);
        assert!(parse(b"<Blobs></Blobs>").unwrap().blobs.is_empty());
        assert_eq!(decode(b"it's &quot;x&apos;"), "it's \"x'");
        assert_eq!(find(b"ab", b"", 0), None);
        assert_eq!(find(b"ab", b"a", 5), None);
        assert_eq!(find(b"ab", b"b", 1), Some(1));
    }

    #[test]
    fn pages_kept_one_after_another_read_as_one() {
        let mut both = fixture("emi_generation_md_page.xml");
        both.extend(fixture("emi_dispatch_and_pricing.xml"));
        let all = parse_pages(&both).unwrap();
        assert_eq!(
            all.blobs.len(),
            3 + parse(&fixture("emi_dispatch_and_pricing.xml"))
                .unwrap()
                .blobs
                .len()
        );
        assert!(!all.prefixes.is_empty());
        assert_eq!(all.next_marker, None, "the last page's marker");
        // One page alone, with or without its wrapper; a page cut short.
        assert_eq!(
            parse_pages(&fixture("emi_generation_md_page.xml"))
                .unwrap()
                .blobs
                .len(),
            3
        );
        assert!(parse_pages(b"<Blobs></Blobs>").unwrap().blobs.is_empty());
        assert_eq!(
            parse_pages(b"<EnumerationResults><Blobs></Blobs>")
                .unwrap_err()
                .reason,
            "a listing page is not closed"
        );
    }

    #[test]
    fn the_listing_request_names_the_container_the_prefix_and_the_marker() {
        let base = "https://emidatasets.blob.core.windows.net/publicdata";
        assert_eq!(
            list_url(
                &format!("{base}/Datasets/Wholesale/Generation/Generation_MD/"),
                None
            )
            .unwrap(),
            format!(
                "{base}?restype=container&comp=list&prefix=Datasets/Wholesale/Generation/Generation_MD/&delimiter=/"
            )
        );
        // A folder without its slash gets one; a space is encoded; the
        // container alone lists the root; a marker is appended.
        assert_eq!(
            list_url(
                &format!("{base}/Datasets/Forward markets"),
                Some("2!152!MDAw--")
            )
            .unwrap(),
            format!(
                "{base}?restype=container&comp=list&prefix=Datasets/Forward%20markets/&delimiter=/&marker=2!152!MDAw--"
            )
        );
        assert_eq!(
            list_url(base, None).unwrap(),
            format!("{base}?restype=container&comp=list&prefix=&delimiter=/")
        );
        assert_eq!(
            list_url(&format!("{base}/"), None).unwrap(),
            format!("{base}?restype=container&comp=list&prefix=&delimiter=/")
        );
        assert_eq!(list_url("not a url", None), None);
        assert_eq!(list_url("https://host", None), None);
        assert_eq!(list_url("https:///c/", None), None);
    }
}
