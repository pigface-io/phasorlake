//! Gathering bootstrap inputs from nemweb without downloading whole archives.
//!
//! A monthly archive zip holds one CSV whose first bytes are the `C` header,
//! the `I` row and the first `D` rows: everything the sampler needs. Instead
//! of downloading the 20-500 MB file, [`fetch_heads`] asks for the first
//! [`HEAD_BYTES`] of each zip with a `Range` request, parses the first local
//! file header and inflates the truncated deflate stream as far as it goes.
//! The surviving lines are trimmed to every `C` and `I` row plus the first
//! `D` rows of each block and written as `{stem}.CSV` fixtures.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nemlake_core::{Category, Diagnose, Diagnostic, YearMonth};
use nemlake_fetch::listing;
use nemlake_fetch::source::Source;
use nemlake_fetch::{
    ByteRange, DiskCache, FetchError, Fetcher, FetcherConfig, HttpRequest, MmsdmArchive, Query,
    ReqwestTransport, Transport,
};

/// How much of each zip to request. The I row sits in the first few KiB of
/// the inflated stream, but wide tables with long `C` comments and 20 rows
/// of 100 columns need more; 256 KiB of deflate inflates to well over 1 MiB.
pub const HEAD_BYTES: u64 = 256 * 1024;

/// Zip local file header signature.
const LOCAL_HEADER: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// Why a head could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum HeadError {
    /// The response was not the first bytes of a zip.
    #[error("The first bytes of {url} are not a zip local file header.")]
    NotZip {
        /// URL.
        url: String,
    },
    /// Compression we do not inflate.
    #[error(
        "The first entry of {url} uses zip compression method {method}, not deflate or stored."
    )]
    Method {
        /// URL.
        url: String,
        /// Method code.
        method: u16,
    },
    /// Nothing usable came back.
    #[error("The head of {url} inflated to {bytes} bytes but held no I row.")]
    NoIRow {
        /// URL.
        url: String,
        /// Inflated size.
        bytes: usize,
    },
    /// Transport.
    #[error(transparent)]
    Fetch(#[from] FetchError),
    /// Writing.
    #[error("The head file {path} could not be written.")]
    Write {
        /// Path.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

impl Diagnose for HeadError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::NotZip { url } => Diagnostic::error("schema.head_not_zip", Category::Schema, msg)
                .with_url(url.clone()),
            Self::Method { url, method } => {
                let mut d = Diagnostic::error("schema.head_method", Category::Schema, msg)
                    .with_url(url.clone());
                d.observed = Some(method.to_string());
                d
            }
            Self::NoIRow { url, .. } => {
                Diagnostic::error("schema.head_no_irow", Category::Schema, msg)
                    .with_url(url.clone())
            }
            Self::Fetch(e) => e.diagnostic(),
            Self::Write { path, source } => {
                Diagnostic::error("schema.head_write", Category::Schema, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
        }
    }
}

/// The first entry of a (possibly truncated) zip, inflated as far as the
/// bytes allow. Returns the entry name and the inflated bytes.
pub fn inflate_head(url: &str, zip_bytes: &[u8]) -> Result<(String, Vec<u8>), HeadError> {
    if zip_bytes.len() < 30 || zip_bytes[..4] != LOCAL_HEADER {
        return Err(HeadError::NotZip {
            url: url.to_owned(),
        });
    }
    let u16_at = |i: usize| u16::from_le_bytes([zip_bytes[i], zip_bytes[i + 1]]);
    let method = u16_at(8);
    let name_len = usize::from(u16_at(26));
    let extra_len = usize::from(u16_at(28));
    let data_start = 30 + name_len + extra_len;
    if zip_bytes.len() < data_start {
        return Err(HeadError::NotZip {
            url: url.to_owned(),
        });
    }
    let name = String::from_utf8_lossy(&zip_bytes[30..30 + name_len]).into_owned();
    let data = &zip_bytes[data_start..];
    let out = match method {
        0 => data.to_vec(),
        8 => {
            let mut dec = flate2::read::DeflateDecoder::new(data);
            let mut out = Vec::with_capacity(data.len() * 4);
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match dec.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => out.extend_from_slice(&buf[..n]),
                    // A truncated stream ends in an error; what came before is good.
                    Err(_) => break,
                }
            }
            out
        }
        m => {
            return Err(HeadError::Method {
                url: url.to_owned(),
                method: m,
            });
        }
    };
    Ok((name, out))
}

/// Keeps every `C` and `I` line and the first `max_rows` `D` lines after each
/// `I` line; drops the partial last line of a truncated stream. The
/// `END OF REPORT` footer is kept only when no row was dropped, because its
/// line count would no longer be true. Line endings are preserved.
#[must_use]
pub fn trim_head(csv: &[u8], max_rows: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rows_in_block = 0usize;
    let mut saw_i = false;
    let mut dropped = false;
    let complete_end = csv.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    for line in csv[..complete_end].split_inclusive(|&b| b == b'\n') {
        match line.first() {
            Some(b'C') if line.windows(13).any(|w| w == b"END OF REPORT") => {
                if !dropped {
                    out.extend_from_slice(line);
                }
            }
            Some(b'C') => out.extend_from_slice(line),
            Some(b'I') => {
                saw_i = true;
                rows_in_block = 0;
                out.extend_from_slice(line);
            }
            Some(b'D') if saw_i && rows_in_block < max_rows => {
                rows_in_block += 1;
                out.extend_from_slice(line);
            }
            _ => dropped = true,
        }
    }
    out
}

/// What [`fetch_heads`] did for one zip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadOutcome {
    /// Zip URL.
    pub url: String,
    /// Written fixture, when one was.
    pub path: Option<PathBuf>,
    /// Error text, when it failed.
    pub error: Option<String>,
}

/// The production transport for the bootstrap tool.
pub fn nemweb_transport() -> Result<Arc<dyn Transport>, HeadError> {
    Ok(Arc::new(ReqwestTransport::new(
        &format!("nemlake-schema-bootstrap/{}", env!("CARGO_PKG_VERSION")),
        Duration::from_secs(20),
        Duration::from_secs(60),
    )?))
}

/// Fetches the head of every archive zip of `month` (every table, all three
/// data directories) and writes `{out}/{zip stem}.CSV` fixtures of at most
/// `max_rows` D rows per block. Runs `parallel` requests at once. Listings
/// are cached under `cache_root`.
pub async fn fetch_heads(
    transport: Arc<dyn Transport>,
    archive: &MmsdmArchive,
    cache_root: &Path,
    out: &Path,
    month: YearMonth,
    max_rows: usize,
    parallel: usize,
) -> Result<Vec<HeadOutcome>, HeadError> {
    let fetcher = Fetcher::new(
        Arc::clone(&transport),
        DiskCache::new(cache_root),
        FetcherConfig::default(),
    );
    let query = Query {
        tables: Vec::new(),
        from: month,
        to: month,
    };
    let remotes = archive.list(&query, &fetcher).await?;
    std::fs::create_dir_all(out).map_err(|source| HeadError::Write {
        path: out.display().to_string(),
        source,
    })?;
    let outcomes = futures::stream::iter(remotes)
        .map(|remote| {
            let transport = Arc::clone(&transport);
            let out = out.to_path_buf();
            async move {
                let url = remote.url.to_string();
                match fetch_one(transport.as_ref(), &url, &out, max_rows).await {
                    Ok(path) => HeadOutcome {
                        url,
                        path: Some(path),
                        error: None,
                    },
                    Err(e) => HeadOutcome {
                        url,
                        path: None,
                        error: Some(e.diagnostic().render()),
                    },
                }
            }
        })
        .buffer_unordered(parallel.max(1))
        .collect::<Vec<_>>()
        .await;
    Ok(outcomes)
}

async fn fetch_one(
    transport: &dyn Transport,
    url: &str,
    out: &Path,
    max_rows: usize,
) -> Result<PathBuf, HeadError> {
    let req = HttpRequest::get(url).with_range(ByteRange {
        start: 0,
        end: Some(HEAD_BYTES - 1),
    });
    let resp = transport.get(req).await?;
    if resp.status != 206 && resp.status != 200 {
        return Err(FetchError::HttpStatus {
            url: url.to_owned(),
            status: resp.status,
            body_head: Vec::new(),
        }
        .into());
    }
    let mut body = Vec::new();
    let mut stream = resp.body;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|message| FetchError::Body {
            url: url.to_owned(),
            received: body.len() as u64,
            message,
        })?;
        body.extend_from_slice(&chunk);
        if body.len() as u64 >= HEAD_BYTES {
            break;
        }
    }
    let (_, inflated) = inflate_head(url, &body)?;
    let trimmed = trim_head(&inflated, max_rows);
    let has_i_row = trimmed.starts_with(b"I,") || trimmed.windows(3).any(|w| w == b"\nI,");
    if !has_i_row {
        return Err(HeadError::NoIRow {
            url: url.to_owned(),
            bytes: inflated.len(),
        });
    }
    let stem = url
        .rsplit('/')
        .next()
        .and_then(|n| n.strip_suffix(".zip"))
        .unwrap_or("head");
    let path = out.join(format!("{stem}.CSV"));
    std::fs::write(&path, &trimmed).map_err(|source| HeadError::Write {
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

/// Lists the CTL directory of `month` and downloads every `.ctl` into `out`.
pub async fn fetch_ctls(
    transport: Arc<dyn Transport>,
    archive: &MmsdmArchive,
    cache_root: &Path,
    out: &Path,
    month: YearMonth,
) -> Result<Vec<PathBuf>, HeadError> {
    let fetcher = Fetcher::new(
        Arc::clone(&transport),
        DiskCache::new(cache_root),
        FetcherConfig::default(),
    );
    let dir = archive
        .month_root(month)?
        .join("CTL/")
        .map_err(|e| FetchError::Parse {
            url: "CTL/".into(),
            what: "CTL directory URL",
            message: e.to_string(),
        })?;
    let body = fetcher.fetch_listing(dir.as_str(), 0).await?;
    std::fs::create_dir_all(out).map_err(|source| HeadError::Write {
        path: out.display().to_string(),
        source,
    })?;
    let mut written = Vec::new();
    for e in listing::parse(&body) {
        if !e.name.to_ascii_lowercase().ends_with(".ctl") {
            continue;
        }
        let url = dir.join(&e.href).map_err(|er| FetchError::Parse {
            url: e.href.clone(),
            what: "listing href",
            message: er.to_string(),
        })?;
        let resp = transport.get(HttpRequest::get(url.as_str())).await?;
        if resp.status != 200 {
            return Err(FetchError::HttpStatus {
                url: url.to_string(),
                status: resp.status,
                body_head: Vec::new(),
            }
            .into());
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut stream = resp.body;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|message| FetchError::Body {
                url: url.to_string(),
                received: bytes.len() as u64,
                message,
            })?;
            bytes.extend_from_slice(&chunk);
        }
        let path = out.join(&e.name);
        std::fs::write(&path, &bytes).map_err(|source| HeadError::Write {
            path: path.display().to_string(),
            source,
        })?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(csv: &[u8]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            w.start_file(
                "PUBLIC_DVD_X_202401010000.CSV",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            w.write_all(csv).unwrap();
            w.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn inflates_truncated_zip() {
        let mut csv = Vec::new();
        csv.extend_from_slice(b"C,NEMP.WORLD,DVD,AEMO,PUBLIC,2024/02/01,00:00:00,,,\r\n");
        csv.extend_from_slice(b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\n");
        for i in 0..5000 {
            csv.extend_from_slice(
                format!("D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",UNIT{i},{i}.5\r\n")
                    .as_bytes(),
            );
        }
        csv.extend_from_slice(b"C,\"END OF REPORT\",5003\r\n");
        let z = zip_of(&csv);
        let cut = &z[..z.len() / 3];
        let (name, inflated) = inflate_head("u", cut).unwrap();
        assert_eq!(name, "PUBLIC_DVD_X_202401010000.CSV");
        assert!(inflated.starts_with(b"C,NEMP.WORLD"));
        assert!(inflated.len() < csv.len());
        let trimmed = trim_head(&inflated, 3);
        let text = String::from_utf8(trimmed).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(lines[1].starts_with("I,DISPATCH"));
        assert!(lines[4].starts_with("D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",UNIT2,"));

        // The whole zip inflates to the whole CSV, and the trailer survives.
        let (_, whole) = inflate_head("u", &z).unwrap();
        assert_eq!(whole, csv);
        let t = String::from_utf8(trim_head(&whole, 1)).unwrap();
        assert!(!t.contains("END OF REPORT"), "a cut file loses its footer");
        let t = String::from_utf8(trim_head(&whole, 5000)).unwrap();
        assert!(t.ends_with("C,\"END OF REPORT\",5003\r\n"));
    }

    #[test]
    fn rejects_non_zip_and_short() {
        assert!(matches!(
            inflate_head("u", b"<html>"),
            Err(HeadError::NotZip { .. })
        ));
        assert!(matches!(
            inflate_head("u", &LOCAL_HEADER),
            Err(HeadError::NotZip { .. })
        ));
        let mut bad = zip_of(b"I,A,B,1,X\r\n");
        bad[8] = 12; // bzip2
        assert!(matches!(
            inflate_head("u", &bad),
            Err(HeadError::Method { method: 12, .. })
        ));
    }

    #[tokio::test]
    async fn heads_and_ctls_over_a_canned_nemweb() {
        use nemlake_fetch::transport::{Canned, StaticTransport};
        let root = "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader";
        let t = Arc::new(StaticTransport::default());
        let link = |dir: &str, name: &str| {
            format!(
                r#" 2/13/2024 8:27 AM 12 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/{dir}/{name}">{name}</A><br>"#
            )
        };
        let data_listing = format!(
            "<pre>{}{}</pre>",
            link("DATA", "PUBLIC_DVD_X_202401010000.zip"),
            link("DATA", "PUBLIC_DVD_Y_202401010000.zip")
        );
        t.add(
            &format!("{root}/DATA/"),
            Canned::ok(data_listing.as_bytes()),
        )
        .add(&format!("{root}/P5MIN_ALL_DATA/"), Canned::status(404))
        .add(&format!("{root}/PREDISP_ALL_DATA/"), Canned::status(404));
        let csv = b"C,x\r\nI,A,X,1,SETTLEMENTDATE,V\r\nD,A,X,1,\"2024/01/01 00:00:00\",1\r\nD,A,X,1,\"2024/01/01 00:05:00\",2\r\nC,\"END OF REPORT\",5\r\n";
        t.add(
            &format!("{root}/DATA/PUBLIC_DVD_X_202401010000.zip"),
            Canned::ok(&zip_of(csv)),
        )
        .add(
            &format!("{root}/DATA/PUBLIC_DVD_Y_202401010000.zip"),
            Canned::ok(b"<html>not a zip"),
        );
        let ctl_listing = format!("<pre>{}</pre>", link("CTL", "PUBLIC_DVD_X_202401.ctl"));
        t.add(&format!("{root}/CTL/"), Canned::ok(ctl_listing.as_bytes()))
            .add(
                &format!("{root}/CTL/PUBLIC_DVD_X_202401.ctl"),
                Canned::ok(b"LOAD DATA"),
            );

        let archive = MmsdmArchive::new("https://h").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let month: YearMonth = "2024-01".parse().unwrap();
        let outcomes = fetch_heads(
            Arc::new(Arc::clone(&t)),
            &archive,
            &tmp.path().join("cache"),
            &tmp.path().join("heads"),
            month,
            1,
            2,
        )
        .await
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        let ok = outcomes
            .iter()
            .find(|o| o.url.ends_with("X_202401010000.zip"))
            .unwrap();
        let written = std::fs::read_to_string(ok.path.as_ref().unwrap()).unwrap();
        assert_eq!(
            written,
            "C,x\r\nI,A,X,1,SETTLEMENTDATE,V\r\nD,A,X,1,\"2024/01/01 00:00:00\",1\r\n"
        );
        let bad = outcomes
            .iter()
            .find(|o| o.url.ends_with("Y_202401010000.zip"))
            .unwrap();
        assert!(bad.path.is_none());
        assert!(
            bad.error
                .as_deref()
                .unwrap()
                .contains("not a zip local file header"),
            "{bad:?}"
        );

        let ctls = fetch_ctls(
            Arc::new(t),
            &archive,
            &tmp.path().join("cache"),
            &tmp.path().join("ctl"),
            month,
        )
        .await
        .unwrap();
        assert_eq!(ctls.len(), 1);
        assert_eq!(std::fs::read_to_string(&ctls[0]).unwrap(), "LOAD DATA");
        assert!(ctls[0].ends_with("PUBLIC_DVD_X_202401.ctl"));
    }

    #[test]
    fn diagnostics_have_codes_and_context() {
        let d = HeadError::NotZip {
            url: "https://h/x.zip".into(),
        }
        .diagnostic();
        assert_eq!(d.code, "schema.head_not_zip");
        assert_eq!(d.url.as_deref(), Some("https://h/x.zip"));
        let d = HeadError::Method {
            url: "u".into(),
            method: 12,
        }
        .diagnostic();
        assert_eq!(d.code, "schema.head_method");
        assert_eq!(d.observed.as_deref(), Some("12"));
        let d = HeadError::NoIRow {
            url: "u".into(),
            bytes: 9,
        }
        .diagnostic();
        assert_eq!(d.code, "schema.head_no_irow");
        assert!(d.message.contains("9 bytes"));
        let d = HeadError::Write {
            path: "/x/y.CSV".into(),
            source: std::io::Error::other("disk full"),
        }
        .diagnostic();
        assert_eq!(d.code, "schema.head_write");
        assert_eq!(d.file.as_deref(), Some("/x/y.CSV"));
        assert!(d.cause.iter().any(|c| c.contains("disk full")));
        let d = HeadError::Fetch(FetchError::Offline { url: "u".into() }).diagnostic();
        assert_eq!(d.code, "net.offline");
    }

    #[test]
    fn trim_keeps_blocks_and_drops_partial_tail() {
        let csv = b"C,x\nI,A,B,1,X\nD,A,B,1,1\nD,A,B,1,2\nD,A,B,1,3\nI,A,C,2,Y\nD,A,C,2,1\nD,A,C,2,2\nD,A,C,2,par";
        let t = String::from_utf8(trim_head(csv, 2)).unwrap();
        assert_eq!(
            t,
            "C,x\nI,A,B,1,X\nD,A,B,1,1\nD,A,B,1,2\nI,A,C,2,Y\nD,A,C,2,1\nD,A,C,2,2\n"
        );
        assert!(trim_head(b"D,before,i\n", 5).is_empty());
    }
}
