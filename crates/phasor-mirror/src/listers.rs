//! How each kind of host lists a folder, in one place: what is asked of the
//! host for a folder's listing ([`ListingKind::fetch_folder`]) and what links
//! the answer holds ([`ListingKind::read_page`]). A new kind of host is a
//! variant of [`ListingKind`] and two arms here; the crawl, the watch and
//! the download know nothing of kinds.

use url::Url;

use crate::config::ListingKind;
use crate::crawl::{Remote, RemoteKind, resolve};
use crate::error::MirrorError;
use crate::listing::{self, Unparsed};
use crate::net::{FetchError, HostFetcher};
use crate::stamp::Stamp;

impl ListingKind {
    /// The bytes that stand for `folder`'s listing, as the host of `host`
    /// gives them: an IIS page whole, a blob store's pages one after another,
    /// a THREDDS catalog, or the headers of a host's documents as lines.
    pub async fn fetch_folder(
        self,
        host: &HostFetcher,
        folder: &str,
    ) -> Result<Vec<u8>, FetchError> {
        match self {
            Self::Iis => host.fetcher.fetch_listing(folder).await,
            Self::AzureBlob => azure::fetch(host, folder).await,
            Self::Thredds => {
                let catalog = thredds::catalog_url(folder).ok_or_else(|| {
                    FetchError::parse(folder, "thredds folder")("not under /thredds/fileServer/")
                })?;
                host.fetcher.fetch_listing(&catalog).await
            }
            Self::Documents => documents::fetch(host, folder).await,
            // The FTP transport answers a folder URL with its entries as lines.
            Self::Ftp => host.fetcher.fetch_listing(folder).await,
        }
    }

    /// Every link of `folder`'s listing `bytes` as a record (`offset` seconds
    /// east of UTC for an IIS page's times, the other kinds carry their own).
    pub fn read_page(
        self,
        folder: &Url,
        bytes: &[u8],
        roots: &[Url],
        crawled_at: Option<Stamp>,
        offset: i32,
    ) -> Result<Vec<Remote>, MirrorError> {
        let bad = |source: Unparsed| MirrorError::Listing {
            url: folder.to_string(),
            source,
        };
        let links = match self {
            Self::Iis => listing::parse_in(bytes, offset)
                .map_err(bad)?
                .iter()
                .map(|link| resolve(folder, link, roots))
                .collect::<Result<Vec<Remote>, Unparsed>>(),
            Self::AzureBlob => azure::read(folder, bytes, roots),
            Self::Thredds => thredds::read(folder, bytes),
            Self::Documents => documents::read(folder, bytes),
            Self::Ftp => ftp::read(folder, bytes, roots),
        };
        let mut links = links.map_err(bad)?;
        for r in &mut links {
            r.crawled_at = crawled_at;
        }
        Ok(links)
    }
}

/// A record of a link `folder`'s page lists.
fn remote(folder: &Url, url: &Url, kind: RemoteKind) -> Remote {
    Remote {
        url: url.to_string(),
        kind,
        size: None,
        modified: String::new(),
        modified_at: None,
        listed_on: folder.to_string(),
        crawled_at: None,
    }
}

/// An Azure Blob Storage container: a folder is a prefix, listed a page of
/// blobs and prefixes at a time ([`phasor_net::bloblist`]).
mod azure {
    use super::{Remote, RemoteKind, Unparsed, remote};
    use crate::net::{FetchError, HostFetcher};
    use crate::stamp::Stamp;
    use phasor_net::bloblist;
    use url::Url;

    /// The folder's pages, one after another, each a document.
    pub async fn fetch(host: &HostFetcher, folder: &str) -> Result<Vec<u8>, FetchError> {
        let mut out = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let request = bloblist::list_url(folder, marker.as_deref())
                .ok_or_else(|| FetchError::parse(folder, "blob folder")("not a folder"))?;
            let page = host.fetcher.fetch_listing(&request).await?;
            marker = bloblist::parse(&page)
                .map_err(FetchError::parse(&request, "blob listing"))?
                .next_marker;
            out.extend_from_slice(&page);
            if marker.is_none() {
                return Ok(out);
            }
        }
    }

    /// The prefixes (folders under the roots, else links out) and the blobs
    /// (files; a zero-length blob named for a folder is not one) the pages hold.
    pub fn read(folder: &Url, xml: &[u8], roots: &[Url]) -> Result<Vec<Remote>, Unparsed> {
        let page = bloblist::parse_pages(xml).map_err(|e| Unparsed {
            reason: e.reason,
            text: e.text,
        })?;
        // A blob's name is under the container, the folder's first segment.
        let mut container = folder.clone();
        let first = container
            .path_segments()
            .and_then(|mut s| s.next())
            .unwrap_or_default()
            .to_owned();
        container.set_path(&format!("/{first}/"));
        let under_roots = |url: &Url| roots.iter().any(|r| url.as_str().starts_with(r.as_str()));
        let join = |name: &str| {
            container.join(name).map_err(|_| Unparsed {
                reason: "a blob name that is not a URL",
                text: name.to_owned(),
            })
        };
        let mut out = Vec::new();
        for prefix in &page.prefixes {
            let url = join(prefix)?;
            let kind = if under_roots(&url) {
                RemoteKind::Dir
            } else {
                RemoteKind::External
            };
            out.push(remote(folder, &url, kind));
        }
        for b in page.blobs.iter().filter(|b| b.size > 0) {
            let url = join(&b.name)?;
            out.push(Remote {
                size: Some(b.size),
                modified: b.last_modified.clone(),
                modified_at: Stamp::from_http_date(&b.last_modified),
                ..remote(folder, &url, RemoteKind::File)
            });
        }
        Ok(out)
    }
}

/// A THREDDS data server: a folder `{origin}/thredds/fileServer/{rel}/` is
/// listed by `{origin}/thredds/catalog/{rel}/catalog.xml`, whose
/// `catalogRef`s are its folders and whose `dataset`s with a `urlPath` are
/// its files, served under `/thredds/fileServer/`. Times are UTC instants;
/// a size is kept only when the catalog gives it in bytes, whole.
mod thredds {
    use super::{Remote, RemoteKind, Unparsed, remote};
    use crate::stamp::Stamp;
    use url::Url;

    /// The catalog that lists `folder`, when the folder is under `fileServer`.
    pub fn catalog_url(folder: &str) -> Option<String> {
        let (before, rel) = folder.split_once("/thredds/fileServer/")?;
        Some(format!("{before}/thredds/catalog/{rel}catalog.xml"))
    }

    /// The folders and files a catalog lists.
    pub fn read(folder: &Url, xml: &[u8]) -> Result<Vec<Remote>, Unparsed> {
        let text = std::str::from_utf8(xml).map_err(|_| Unparsed {
            reason: "a catalog that is not UTF-8",
            text: String::new(),
        })?;
        if !text.contains("<catalog") {
            return Err(Unparsed {
                reason: "not a THREDDS catalog",
                text: text.chars().take(80).collect(),
            });
        }
        let origin = folder.origin().ascii_serialization();
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('<') {
            rest = &rest[start..];
            let Some(end) = rest.find('>') else {
                return Err(Unparsed {
                    reason: "a tag that never closes",
                    text: rest.chars().take(80).collect(),
                });
            };
            let tag = &rest[1..end];
            if let Some(attrs) = tag.strip_prefix("catalogRef ") {
                let href = attr(attrs, "xlink:href").ok_or_else(|| Unparsed {
                    reason: "a catalogRef without xlink:href",
                    text: tag.to_owned(),
                })?;
                let path = href.strip_suffix("catalog.xml").unwrap_or(href);
                let mut url = folder.join(path).map_err(|_| Unparsed {
                    reason: "a catalogRef that is not a URL",
                    text: href.to_owned(),
                })?;
                if !url.path().ends_with('/') {
                    let path = format!("{}/", url.path());
                    url.set_path(&path);
                }
                let kind = if url.as_str().starts_with(folder.as_str()) {
                    RemoteKind::Dir
                } else {
                    RemoteKind::External
                };
                out.push(remote(folder, &url, kind));
            } else if let Some(attrs) = tag.strip_prefix("dataset ")
                && let Some(url_path) = attr(attrs, "urlPath")
            {
                // The file's own block: what follows its tag up to its end.
                let block = &rest[end + 1..];
                let block = &block[..block.find("</dataset>").unwrap_or(block.len())];
                // The file's address is text the catalog gave: kept as it is.
                let url = format!("{origin}/thredds/fileServer/{url_path}");
                let kind = if url.starts_with(folder.as_str()) {
                    RemoteKind::File
                } else {
                    RemoteKind::External
                };
                let modified = text_of(block, "<date type=\"modified\">", "</date>").unwrap_or("");
                out.push(Remote {
                    url,
                    kind,
                    size: size_of(block),
                    modified: modified.to_owned(),
                    modified_at: Stamp::parse(modified).ok(),
                    listed_on: folder.to_string(),
                    crawled_at: None,
                });
            }
            rest = &rest[end + 1..];
        }
        Ok(out)
    }

    /// The value of attribute `name` in `attrs`.
    fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
        let key = format!("{name}=\"");
        let start = attrs.find(&key)? + key.len();
        let len = attrs[start..].find('"')?;
        Some(&attrs[start..start + len])
    }

    /// The text between `open` and `close` in `block`.
    fn text_of<'a>(block: &'a str, open: &str, close: &str) -> Option<&'a str> {
        let start = block.find(open)? + open.len();
        let len = block[start..].find(close)?;
        Some(&block[start..start + len])
    }

    /// A file's size in bytes, when the catalog gives it whole in bytes;
    /// Kbytes and Mbytes are rounded, so they are not a size the mirror
    /// could hold a file to.
    fn size_of(block: &str) -> Option<u64> {
        let start = block.find("<dataSize ")?;
        let tag = &block[start..];
        let units = attr(&tag[..tag.find('>')?], "units")?;
        if units != "bytes" {
            return None;
        }
        let value = text_of(tag, ">", "</dataSize>")?.trim();
        let whole = value.strip_suffix(".0").unwrap_or(value);
        whole.parse().ok()
    }
}

/// A fixed list of documents a host rewrites in place: a folder's listing is
/// the headers of the documents under it, one line each, as JSON.
mod documents {
    use super::{Remote, RemoteKind, Unparsed, remote};
    use crate::net::{FetchError, HostFetcher};
    use crate::stamp::Stamp;
    use serde::{Deserialize, Serialize};
    use url::Url;

    /// One document as its host's HEAD described it.
    #[derive(Serialize, Deserialize)]
    struct Line {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modified: Option<String>,
    }

    /// Whether `doc` is a file of `folder` itself (not of a folder below).
    fn in_folder(doc: &str, folder: &str) -> bool {
        doc.strip_prefix(folder)
            .is_some_and(|name| !name.is_empty() && !name.contains('/'))
    }

    /// HEADs each document of `folder`; one gone (404 or 410) is not listed.
    pub async fn fetch(host: &HostFetcher, folder: &str) -> Result<Vec<u8>, FetchError> {
        let mut out = Vec::new();
        for doc in host.documents.iter().filter(|d| in_folder(d, folder)) {
            let line = match host.fetcher.head(doc).await {
                Ok(h) => Line {
                    url: doc.clone(),
                    size: h.content_length,
                    modified: h.last_modified,
                },
                Err(FetchError::HttpStatus {
                    status: 404 | 410, ..
                }) => continue,
                Err(e) => return Err(e),
            };
            serde_json::to_writer(&mut out, &line).expect("a Line always serialises");
            out.push(b'\n');
        }
        Ok(out)
    }

    /// The documents the lines describe, each a file.
    pub fn read(folder: &Url, lines: &[u8]) -> Result<Vec<Remote>, Unparsed> {
        let text = std::str::from_utf8(lines).map_err(|_| Unparsed {
            reason: "document lines that are not UTF-8",
            text: String::new(),
        })?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let line: Line = serde_json::from_str(l).map_err(|_| Unparsed {
                    reason: "a line that is not a document's headers",
                    text: l.chars().take(80).collect(),
                })?;
                let url = Url::parse(&line.url).map_err(|_| Unparsed {
                    reason: "a document that is not a URL",
                    text: line.url.clone(),
                })?;
                Ok(Remote {
                    size: line.size,
                    modified: line.modified.clone().unwrap_or_default(),
                    modified_at: line.modified.as_deref().and_then(Stamp::from_http_date),
                    ..remote(folder, &url, RemoteKind::File)
                })
            })
            .collect()
    }
}

/// An FTP server: the transport lists a folder as one JSON line an entry
/// ([`phasor_ftp::listing::Entry`]); a folder under the roots is a folder,
/// a file is a file with the size and UTC time the server gave.
mod ftp {
    use super::{Remote, RemoteKind, Unparsed, remote};
    use crate::stamp::Stamp;
    use phasor_ftp::listing::Entry;
    use url::Url;

    /// The entries a folder's page holds.
    pub fn read(folder: &Url, page: &[u8], roots: &[Url]) -> Result<Vec<Remote>, Unparsed> {
        let entries = Entry::read(page).ok_or_else(|| Unparsed {
            reason: "a line that is not an FTP entry",
            text: String::from_utf8_lossy(page).chars().take(80).collect(),
        })?;
        let mut out = Vec::new();
        for e in entries {
            let mut url = folder.join(&e.name).map_err(|_| Unparsed {
                reason: "an entry that is not a URL",
                text: e.name.clone(),
            })?;
            if e.dir {
                if !url.path().ends_with('/') {
                    let path = format!("{}/", url.path());
                    url.set_path(&path);
                }
                let kind = if roots.iter().any(|r| url.as_str().starts_with(r.as_str())) {
                    RemoteKind::Dir
                } else {
                    RemoteKind::External
                };
                out.push(remote(folder, &url, kind));
            } else {
                out.push(Remote {
                    size: e.size,
                    modified: e.modified.clone().unwrap_or_default(),
                    modified_at: e.modified.as_deref().and_then(Stamp::from_http_date),
                    ..remote(folder, &url, RemoteKind::File)
                });
            }
        }
        Ok(out)
    }
}

/// Whether a file's name matches an `only` pattern (`*` any run of
/// characters, `?` any one, the rest itself): the FTP crate's, which the
/// FTP transport applies too.
pub use phasor_ftp::listing::glob;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::net::transport::{Canned, StaticTransport};
    use crate::net::{Archive, Fetcher, FetcherConfig};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/testdata/listings/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn host(t: &Arc<StaticTransport>, base: &str, kind: ListingKind, docs: &[&str]) -> HostFetcher {
        let dir = tempfile::tempdir().unwrap();
        HostFetcher {
            base_url: base.to_owned(),
            offset_secs: 0,
            listing: kind,
            documents: docs.iter().map(|d| (*d).to_owned()).collect(),
            only: Vec::new(),
            batch: None,
            fetcher: Fetcher::new(
                Arc::new(t.clone()),
                Archive::new(dir.keep()),
                FetcherConfig {
                    attempts: 1,
                    backoff: std::time::Duration::ZERO,
                    max_backoff: std::time::Duration::ZERO,
                    min_interval: std::time::Duration::ZERO,
                },
            ),
        }
    }

    const P1H: &str = "https://thredds.nci.org.au/thredds/fileServer/rv74/satellite-products/arc/der/himawari-ahi/solar/p1h/";

    #[test]
    fn a_thredds_catalog_lists_its_folders_and_files() {
        let folder = Url::parse(P1H).unwrap();
        let links = ListingKind::Thredds
            .read_page(
                &folder,
                &fixture("thredds_p1h.xml"),
                &[],
                Some(Stamp::now()),
                0,
            )
            .unwrap();
        let names: Vec<(&str, RemoteKind)> =
            links.iter().map(|r| (r.url.as_str(), r.kind)).collect();
        assert_eq!(
            names,
            [
                (&format!("{P1H}README.txt")[..], RemoteKind::File),
                (&format!("{P1H}latest/")[..], RemoteKind::Dir),
                (&format!("{P1H}v1.0/")[..], RemoteKind::Dir),
                (&format!("{P1H}v1.1/")[..], RemoteKind::Dir),
            ]
        );
        // A size in whole bytes is kept; the time is an instant.
        assert_eq!(links[0].size, Some(463));
        assert_eq!(links[0].modified, "2022-02-23T01:42:49Z");
        assert_eq!(
            links[0].modified_at.unwrap().rfc3339(),
            "2022-02-23T01:42:49Z"
        );
        assert!(links[1].crawled_at.is_some());
        // A day's files: Kbytes and Mbytes are no size to hold a file to.
        let day = Url::parse(&format!("{P1H}v1.1/2025/10/01/")).unwrap();
        let links = ListingKind::Thredds
            .read_page(&day, &fixture("thredds_day.xml"), &[], None, 0)
            .unwrap();
        assert_eq!(links.len(), 3);
        assert!(
            links
                .iter()
                .all(|r| r.kind == RemoteKind::File && r.size.is_none())
        );
        assert!(
            links[0]
                .url
                .ends_with("/2025/10/01/IDE02327.202509301830.nc")
        );
        assert_eq!(links[2].modified, "2026-03-04T00:12:44Z");
        // Not under the folder: a link out. Not a catalog: an error.
        let elsewhere = Url::parse("https://thredds.nci.org.au/thredds/fileServer/other/").unwrap();
        let links = ListingKind::Thredds
            .read_page(&elsewhere, &fixture("thredds_day.xml"), &[], None, 0)
            .unwrap();
        assert!(links.iter().all(|r| r.kind == RemoteKind::External));
        for bad in [
            &b"<html>no</html>"[..],
            b"<catalog><dataset urlPath=\"x\"",
            b"<catalog><catalogRef xlink:href=\"http://[bad/catalog.xml\"/></catalog>",
            b"\xff",
        ] {
            assert!(
                ListingKind::Thredds
                    .read_page(&day, bad, &[], None, 0)
                    .is_err(),
                "{bad:?}"
            );
        }
        let cat = r#"<catalog><catalogRef xlink:title="no href"/><catalogRef xlink:href="https://elsewhere/x/catalog.xml"/><catalogRef xlink:href="sub"/><dataset name="n"/><dataset urlPath="a/b/c.nc"><dataSize units="bytes">12</dataSize></dataset></catalog>"#;
        assert!(
            ListingKind::Thredds
                .read_page(&day, cat.as_bytes(), &[], None, 0)
                .is_err()
        );
        let cat = cat.replace(r#"<catalogRef xlink:title="no href"/>"#, "");
        let links = ListingKind::Thredds
            .read_page(&day, cat.as_bytes(), &[], None, 0)
            .unwrap();
        assert_eq!(links[0].kind, RemoteKind::External, "another host");
        assert_eq!(links[1].url, format!("{P1H}v1.1/2025/10/01/sub/"));
        assert_eq!(links[1].kind, RemoteKind::Dir);
        assert_eq!(links[2].size, Some(12));
        assert_eq!(
            links[2].kind,
            RemoteKind::External,
            "a urlPath outside the folder"
        );
        assert_eq!(
            thredds::catalog_url(P1H).as_deref(),
            Some(
                "https://thredds.nci.org.au/thredds/catalog/rv74/satellite-products/arc/der/himawari-ahi/solar/p1h/catalog.xml"
            )
        );
        assert_eq!(thredds::catalog_url("https://h/x/"), None);
    }

    #[tokio::test]
    async fn a_thredds_folder_is_fetched_as_its_catalog() {
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://thredds.nci.org.au/thredds/catalog/rv74/satellite-products/arc/der/himawari-ahi/solar/p1h/catalog.xml",
            Canned::ok(&fixture("thredds_p1h.xml")),
        );
        let h = host(&t, "https://thredds.nci.org.au", ListingKind::Thredds, &[]);
        let bytes = ListingKind::Thredds.fetch_folder(&h, P1H).await.unwrap();
        assert_eq!(bytes, fixture("thredds_p1h.xml"));
        assert!(matches!(
            ListingKind::Thredds
                .fetch_folder(&h, "https://thredds.nci.org.au/x/")
                .await
                .unwrap_err(),
            FetchError::Parse {
                what: "thredds folder",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn documents_are_listed_by_their_headers() {
        let t = Arc::new(StaticTransport::default());
        t.add("https://d/a/x.xlsx", Canned::ok(b"xlsx bytes"));
        t.add("https://d/a/gone.xlsx", Canned::status(404));
        t.add("https://d/a/broken.xlsx", Canned::status(500));
        let h = host(
            &t,
            "https://d",
            ListingKind::Documents,
            &[
                "https://d/a/x.xlsx",
                "https://d/a/gone.xlsx",
                "https://d/a/deeper/y.csv",
                "https://d/b/z.csv",
            ],
        );
        let lines = ListingKind::Documents
            .fetch_folder(&h, "https://d/a/")
            .await
            .unwrap();
        let text = String::from_utf8(lines.clone()).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("\"url\":\"https://d/a/x.xlsx\""), "{text}");
        let folder = Url::parse("https://d/a/").unwrap();
        let links = ListingKind::Documents
            .read_page(&folder, &lines, &[], None, 0)
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, RemoteKind::File);
        assert_eq!(links[0].size, Some(10));
        assert_eq!(links[0].modified, "Tue, 13 Feb 2024 08:27:00 GMT");
        assert_eq!(
            links[0].modified_at.unwrap().rfc3339(),
            "2024-02-13T08:27:00Z"
        );
        // A document whose host fails is the error, not a listing.
        let h = host(
            &t,
            "https://d",
            ListingKind::Documents,
            &["https://d/a/broken.xlsx"],
        );
        assert!(
            ListingKind::Documents
                .fetch_folder(&h, "https://d/a/")
                .await
                .is_err()
        );
        // Lines that are not documents.
        for bad in [
            &b"\xff"[..],
            b"{\"size\":1}\n",
            b"{\"url\":\"not a url\"}\n",
        ] {
            assert!(
                ListingKind::Documents
                    .read_page(&folder, bad, &[], None, 0)
                    .is_err(),
                "{bad:?}"
            );
        }
        assert!(
            ListingKind::Documents
                .read_page(
                    &folder,
                    b"\n{\"url\":\"https://d/a/x.xlsx\"}\n",
                    &[],
                    None,
                    0
                )
                .unwrap()[0]
                .size
                .is_none()
        );
    }

    fn entry_named(name: &str) -> phasor_ftp::listing::Entry {
        phasor_ftp::listing::Entry {
            name: name.to_owned(),
            dir: false,
            size: None,
            modified: None,
        }
    }

    #[test]
    fn an_ftp_folders_page_is_its_entries_and_a_glob_names_files() {
        use phasor_ftp::listing::Entry;
        let folder = Url::parse("ftp://f/anon/gen/fwo/").unwrap();
        let roots = [folder.clone()];
        let page = Entry::page(&[
            Entry {
                name: "IDN11060.xml".into(),
                dir: false,
                size: Some(12),
                modified: Some("Tue, 13 Feb 2024 08:27:00 GMT".into()),
            },
            Entry {
                name: "sub".into(),
                dir: true,
                size: None,
                modified: None,
            },
            Entry {
                name: "../up".into(),
                dir: true,
                size: None,
                modified: None,
            },
        ]);
        let links = ListingKind::Ftp
            .read_page(&folder, &page, &roots, Some(Stamp::now()), 0)
            .unwrap();
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].url, "ftp://f/anon/gen/fwo/IDN11060.xml");
        assert_eq!((links[0].kind, links[0].size), (RemoteKind::File, Some(12)));
        assert_eq!(
            links[0].modified_at.unwrap().rfc3339(),
            "2024-02-13T08:27:00Z"
        );
        assert_eq!(links[1].url, "ftp://f/anon/gen/fwo/sub/");
        assert_eq!(links[1].kind, RemoteKind::Dir);
        assert_eq!(
            links[2].kind,
            RemoteKind::External,
            "a folder above the root"
        );
        assert!(links.iter().all(|r| r.crawled_at.is_some()));
        assert!(
            ListingKind::Ftp
                .read_page(&folder, b"not json\n", &roots, None, 0)
                .is_err()
        );
        // An entry whose name is no URL under the folder.
        let odd = Entry::page(&[entry_named("http://[bad")]);
        assert!(
            ListingKind::Ftp
                .read_page(&folder, &odd, &roots, None, 0)
                .is_err()
        );
        // The FTP transport serves a folder URL as its page: the kind asks for the folder itself.
        let t = Arc::new(StaticTransport::default());
        t.add("ftp://f/anon/gen/fwo/", Canned::ok(&page));
        let h = host(&t, "ftp://f", ListingKind::Ftp, &[]);
        let fut = ListingKind::Ftp.fetch_folder(&h, "ftp://f/anon/gen/fwo/");
        let got = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
            .unwrap();
        assert_eq!(got, page);
        // Globs: `*` any run, `?` one character, the rest itself.
        assert!(glob("IDN11060*", "IDN11060.xml"));
        assert!(glob("IDN11060*", "IDN11060.amoc.xml"));
        assert!(glob("ID?60920.xml", "IDN60920.xml"));
        assert!(glob("*.csv", "a.csv") && glob("*", "") && glob("a*c", "abbc"));
        assert!(!glob("ID?60920.xml", "IDNN60920.xml"));
        assert!(!glob("IDN11060*", "IDV10753.xml"));
        assert!(!glob("a?", "a") && !glob("", "a") && !glob("abc", "abd"));
    }
}
