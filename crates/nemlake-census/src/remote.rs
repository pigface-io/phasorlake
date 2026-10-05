//! Gathering bootstrap inputs from nemweb without downloading whole archives.
//!
//! A monthly archive zip holds one CSV whose first bytes are the `C` header,
//! the `I` row and the first `D` rows: everything the sampler needs. Instead
//! of downloading the 20-500 MB file, [`fetch_heads`] asks for the first
//! [`HEAD_BYTES`] of each zip with a `Range` request, parses the first local
//! file header and inflates the truncated deflate stream as far as it goes.
//! For a month published only as one whole-month zip the per-table zip is a
//! member of that zip: the range starts at the member's offset and the head
//! is inflated twice.
//! The surviving lines are trimmed to every `C` and `I` row plus the first
//! `D` rows of each block and written as `{stem}.CSV` fixtures.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nemlake_acquire::listing;
use nemlake_acquire::mmsdm::MonthLayout;
use nemlake_acquire::source::Source;
use nemlake_acquire::{
    DiskCache, FetchError, Fetcher, FetcherConfig, MmsdmArchive, Query, RemoteFile,
    ReqwestTransport, Transport,
};
use nemlake_core::{Category, Diagnose, Diagnostic, YearMonth};

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
        &format!("nemlake-census/{}", env!("CARGO_PKG_VERSION")),
        Duration::from_secs(20),
        Duration::from_secs(60),
    )?))
}

/// How heads are taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadOptions {
    /// D rows kept per block.
    pub rows: usize,
    /// Bytes requested of each zip.
    pub bytes: u64,
}

impl Default for HeadOptions {
    fn default() -> Self {
        Self {
            rows: 20,
            bytes: HEAD_BYTES,
        }
    }
}

/// The fetcher every subcommand shares: one request at a time, `interval`
/// apart, retried where that makes sense, and stopped for good when the
/// server refuses. Listings and zip indexes are cached under `cache_root`.
pub fn paced_fetcher(
    transport: Arc<dyn Transport>,
    cache_root: &Path,
    interval: Duration,
) -> Fetcher {
    Fetcher::new(
        transport,
        DiskCache::new(cache_root),
        FetcherConfig {
            min_interval: interval,
            ..Default::default()
        },
    )
}

/// Fetches the head of every archive zip of `month` (every table, all three
/// data directories, either month layout) and writes them to `dir`, one
/// `{zip stem}.CSV` each, with at most `opts.rows` D rows per block.
pub async fn fetch_heads(
    fetcher: &Fetcher,
    archive: &MmsdmArchive,
    dir: &Path,
    month: YearMonth,
    opts: HeadOptions,
) -> Result<Vec<HeadOutcome>, HeadError> {
    let query = Query {
        exclude: Vec::new(),
        tables: Vec::new(),
        from: month,
        to: month,
    };
    let remotes = archive.list(&query, fetcher).await?;
    let write_err = |path: &Path, source| HeadError::Write {
        path: path.display().to_string(),
        source,
    };
    std::fs::create_dir_all(dir).map_err(|e| write_err(dir, e))?;
    // One request at a time: the fetcher paces them, retries what is worth
    // retrying and stops for good when the server refuses.
    let mut heads: Vec<(String, String, Result<Vec<u8>, HeadError>)> = Vec::new();
    for remote in &remotes {
        let name = remote.file_name();
        let stem = name.strip_suffix(".zip").unwrap_or(&name).to_owned();
        let head = fetch_head(fetcher, remote, opts).await;
        heads.push((remote.url.to_string(), stem, head));
    }
    if fetcher.is_blocked() {
        // Half a month of heads is worse than none: write nothing.
        let refused = heads
            .into_iter()
            .find_map(|(_, _, h)| match h {
                Err(HeadError::Fetch(e @ FetchError::Blocked { .. })) => Some(e),
                _ => None,
            })
            .expect("a blocked fetcher refused at least one head");
        return Err(refused.into());
    }
    heads.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut outcomes = Vec::with_capacity(heads.len());
    for (url, stem, head) in heads {
        let written = head.and_then(|bytes| {
            let path = dir.join(format!("{stem}.CSV"));
            std::fs::write(&path, &bytes).map_err(|e| write_err(&path, e))?;
            Ok(path)
        });
        outcomes.push(match written {
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
        });
    }
    Ok(outcomes)
}

/// The trimmed head of one remote zip.
async fn fetch_head(
    fetcher: &Fetcher,
    remote: &RemoteFile,
    opts: HeadOptions,
) -> Result<Vec<u8>, HeadError> {
    // A per-table zip that is itself a member of a whole-month zip starts
    // at the member's local header inside the container; the fetcher
    // reads it from the archive when it can, and archives what it fetches.
    let body = fetcher.fetch_head(remote, opts.bytes.max(1024)).await?;
    let identity = remote.url.as_str();
    let (_, mut inflated) = inflate_head(identity, &body)?;
    if remote.member.is_some() {
        // What was inflated is the head of the per-table zip; its first
        // entry is the CSV.
        inflated = inflate_head(identity, &inflated)?.1;
    }
    let trimmed = trim_head(&inflated, opts.rows);
    let has_i_row = trimmed.starts_with(b"I,") || trimmed.windows(3).any(|w| w == b"\nI,");
    if !has_i_row {
        return Err(HeadError::NoIRow {
            url: identity.to_owned(),
            bytes: inflated.len(),
        });
    }
    Ok(trimmed)
}

/// The table a control file loads, from its name in either archive naming
/// scheme, upper-case. A split digit of the old scheme stays on the name.
#[must_use]
pub fn ctl_table(file_name: &str) -> Option<String> {
    if !file_name.to_ascii_lowercase().ends_with(".ctl") {
        return None;
    }
    nemlake_core::archive_name::parse(file_name).map(|n| n.table)
}

/// How long a whole-month zip's index is trusted; it never changes.
const ZIP_INDEX_TTL: i64 = 90 * 86_400;

/// The control files of `month`, from its `CTL/` directory or, for a month
/// published only as one zip, the members of that zip.
pub async fn list_ctls(
    fetcher: &Fetcher,
    archive: &MmsdmArchive,
    month: YearMonth,
) -> Result<Vec<RemoteFile>, HeadError> {
    let remote = |url, member| RemoteFile {
        url,
        source: nemlake_acquire::mmsdm::SOURCE_ID,
        table: None,
        period: Some(month),
        size: None,
        modified: None,
        member,
    };
    let mut remotes = Vec::new();
    match archive.month_layout(month, fetcher).await? {
        MonthLayout::Directory => {
            let dir = archive
                .month_root(month)?
                .join("CTL/")
                .map_err(|e| FetchError::Parse {
                    url: "CTL/".into(),
                    what: "CTL directory URL",
                    message: e.to_string(),
                })?;
            let body = fetcher.fetch_listing(dir.as_str(), 86_400).await?;
            for e in listing::parse(&body) {
                if !e.name.to_ascii_lowercase().ends_with(".ctl") {
                    continue;
                }
                let url = dir.join(&e.href).map_err(|er| FetchError::Parse {
                    url: e.href.clone(),
                    what: "listing href",
                    message: er.to_string(),
                })?;
                remotes.push(remote(url, None));
            }
        }
        MonthLayout::WholeZip { url, .. } => {
            for m in fetcher
                .list_zip_members(url.as_str(), ZIP_INDEX_TTL)
                .await?
            {
                let lower = m.name.to_ascii_lowercase();
                if lower.contains("/ctl/") && lower.ends_with(".ctl") {
                    remotes.push(remote(
                        RemoteFile::member_url(&url, &m.name),
                        Some(Box::new(m)),
                    ));
                }
            }
        }
    }
    Ok(remotes)
}

/// Fetches one small file of the archive (a control file, the README)
/// into `out` under its own name.
async fn save_ctl(
    fetcher: &Fetcher,
    remote: &RemoteFile,
    out: &Path,
) -> Result<PathBuf, HeadError> {
    let write_err = |path: &Path, source| HeadError::Write {
        path: path.display().to_string(),
        source,
    };
    std::fs::create_dir_all(out).map_err(|e| write_err(out, e))?;
    let local = fetcher.fetch(remote).await?;
    let path = out.join(remote.file_name());
    std::fs::copy(&local.path, &path).map_err(|e| write_err(&path, e))?;
    Ok(path)
}

/// Downloads the `README.txt` of `month`'s archive, which lists the tables
/// AEMO publishes as standing data, into `out`.
pub async fn fetch_readme(
    fetcher: &Fetcher,
    archive: &MmsdmArchive,
    out: &Path,
    month: YearMonth,
) -> Result<PathBuf, HeadError> {
    let url = archive
        .month_root(month)?
        .join("DOCUMENTATION/README.txt")
        .map_err(|e| FetchError::Parse {
            url: "DOCUMENTATION/README.txt".into(),
            what: "README URL",
            message: e.to_string(),
        })?;
    let remote = RemoteFile {
        url,
        source: nemlake_acquire::mmsdm::SOURCE_ID,
        table: None,
        period: Some(month),
        size: None,
        modified: None,
        member: None,
    };
    save_ctl(fetcher, &remote, out).await
}

/// Downloads the `.ctl` files of `month` into `out`. A control file is
/// skipped when `out` or one of the `skip_known` directories already has
/// one for the same table.
pub async fn fetch_ctls(
    fetcher: &Fetcher,
    archive: &MmsdmArchive,
    out: &Path,
    month: YearMonth,
    skip_known: &[PathBuf],
) -> Result<Vec<PathBuf>, HeadError> {
    let mut known: BTreeSet<String> = BTreeSet::new();
    if !skip_known.is_empty() {
        for dir in skip_known.iter().map(PathBuf::as_path).chain([out]) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            known.extend(
                rd.filter_map(Result::ok)
                    .filter_map(|e| ctl_table(&e.file_name().to_string_lossy())),
            );
        }
    }
    let mut written = Vec::new();
    for r in &list_ctls(fetcher, archive, month).await? {
        if ctl_table(&r.file_name()).is_some_and(|t| known.contains(&t)) {
            continue;
        }
        written.push(save_ctl(fetcher, r, out).await?);
    }
    Ok(written)
}

/// The names under which a control file or a data stem may be looked up:
/// as written, and without a split digit.
fn table_keys(table: &str) -> Vec<String> {
    let upper = table.to_ascii_uppercase();
    let trimmed = upper.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.is_empty() || trimmed == upper {
        vec![upper]
    } else {
        vec![trimmed.to_owned(), upper]
    }
}

/// Whether the control file for `ctl_table` describes the data file for
/// `data_table`: the same name, a split of it, or with `siblings` the
/// daily `_D` file of the same table.
pub(crate) fn ctl_serves(ctl_table: &str, data_table: &str, siblings: bool) -> bool {
    let wanted = table_keys(data_table);
    let mut have = table_keys(ctl_table);
    if siblings && let Some(base) = ctl_table.to_ascii_uppercase().strip_suffix("_D") {
        have.extend(table_keys(base));
    }
    have.iter().any(|k| wanted.contains(k))
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
    async fn the_readme_of_a_month_is_fetched_as_published() {
        use nemlake_acquire::transport::{Canned, StaticTransport};
        let url = "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2026/MMSDM_2026_08/MMSDM_Historical_Data_SQLLoader/DOCUMENTATION/README.txt";
        let body = b"The following tables are the Static/Standing data tables\r\n\r\n\tREGION\r\n";
        let t = Arc::new(StaticTransport::default());
        t.add(url, Canned::ok(body));
        let archive = MmsdmArchive::new("https://h").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let fetcher = paced_fetcher(
            Arc::new(Arc::clone(&t)),
            &tmp.path().join("cache"),
            Duration::ZERO,
        );
        let out = tmp.path().join("MMSDM_2026_08");
        let path = fetch_readme(&fetcher, &archive, &out, "2026-08".parse().unwrap())
            .await
            .unwrap();
        assert_eq!(path, out.join("README.txt"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(
            crate::standing::readme_tables(&String::from_utf8_lossy(body)),
            ["REGION"]
        );
        // A month without one is the fetcher's error.
        let e = fetch_readme(&fetcher, &archive, &out, "2026-07".parse().unwrap())
            .await
            .unwrap_err();
        assert!(
            e.diagnostic().code.starts_with("net."),
            "{}",
            e.diagnostic().code
        );
    }

    #[tokio::test]
    async fn heads_and_ctls_over_a_canned_nemweb() {
        use nemlake_acquire::transport::{Canned, StaticTransport};
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
        let fetcher = paced_fetcher(
            Arc::new(Arc::clone(&t)),
            &tmp.path().join("cache"),
            Duration::ZERO,
        );
        let outcomes = fetch_heads(
            &fetcher,
            &archive,
            &tmp.path().join("heads"),
            month,
            HeadOptions {
                rows: 1,
                ..Default::default()
            },
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

        // The CTL fetch asks the year listing how the month is published.
        t.add(
            "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2024/",
            Canned::ok(br#"<pre> 2/13/2024 8:27 AM &lt;dir&gt; <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/">MMSDM_2024_01</A><br></pre>"#),
        );
        let ctls = fetch_ctls(&fetcher, &archive, &tmp.path().join("ctl"), month, &[])
            .await
            .unwrap();
        assert_eq!(ctls.len(), 1);
        assert_eq!(std::fs::read_to_string(&ctls[0]).unwrap(), "LOAD DATA");
        assert!(ctls[0].ends_with("PUBLIC_DVD_X_202401.ctl"));
    }

    #[tokio::test]
    async fn heads_and_ctls_out_of_a_whole_month_zip() {
        use nemlake_acquire::transport::{Canned, StaticTransport};
        let mut csv = b"C,x\r\nI,A,X,1,SETTLEMENTDATE,V\r\n".to_vec();
        for i in 0..50 {
            csv.extend_from_slice(format!("D,A,X,1,\"2012/01/01 00:00:00\",{i}\r\n").as_bytes());
        }
        let inner = zip_of(&csv);
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file(
                "01/MMSDM_Historical_Data_SQLLoader/CTL/PUBLIC_DVD_X_201201.ctl",
                opts,
            )
            .unwrap();
            w.write_all(b"LOAD DATA 2012").unwrap();
            w.start_file(
                "01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_X_201201010000.zip",
                opts,
            )
            .unwrap();
            w.write_all(&inner).unwrap();
            w.start_file(
                "01/MMSDM_Historical_Data_SQLLoader/DOCUMENTATION/readme.ctl.txt",
                opts,
            )
            .unwrap();
            w.write_all(b"not a ctl").unwrap();
            w.finish().unwrap();
        }
        let month_zip = cursor.into_inner();
        let year = "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2012/";
        let t = Arc::new(StaticTransport::default());
        t.add(
            year,
            Canned::ok(br#"<pre> 2/21/2012 3:10 PM 1912843433 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2012/MMSDM_2012_01.zip">MMSDM_2012_01.zip</A><br></pre>"#),
        );
        t.add(&format!("{year}MMSDM_2012_01.zip"), Canned::ok(&month_zip));
        let archive = MmsdmArchive::new("https://h").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let month: YearMonth = "2012-01".parse().unwrap();
        let fetcher = paced_fetcher(
            Arc::new(Arc::clone(&t)),
            &tmp.path().join("cache"),
            Duration::ZERO,
        );
        let outcomes = fetch_heads(
            &fetcher,
            &archive,
            &tmp.path().join("heads"),
            month,
            HeadOptions {
                rows: 2,
                bytes: 4096,
            },
        )
        .await
        .unwrap();
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        let path = outcomes[0].path.as_ref().expect("a head was written");
        assert!(path.ends_with("PUBLIC_DVD_X_201201010000.CSV"));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "C,x\r\nI,A,X,1,SETTLEMENTDATE,V\r\nD,A,X,1,\"2012/01/01 00:00:00\",0\r\nD,A,X,1,\"2012/01/01 00:00:00\",1\r\n"
        );
        // The head samples with its table and month.
        let sampled = crate::sample::sample_path(path, 10).unwrap();
        assert_eq!(sampled.table.as_deref(), Some("X"));
        assert_eq!(sampled.month, Some(month));
        assert_eq!(sampled.blocks[0].rows, 2);
        let ctls = fetch_ctls(&fetcher, &archive, &tmp.path().join("ctl"), month, &[])
            .await
            .unwrap();
        assert_eq!(ctls.len(), 1);
        assert!(ctls[0].ends_with("PUBLIC_DVD_X_201201.ctl"));
        assert_eq!(std::fs::read_to_string(&ctls[0]).unwrap(), "LOAD DATA 2012");
        // A table that already has a control file somewhere is not fetched again.
        let again = fetch_ctls(
            &fetcher,
            &archive,
            &tmp.path().join("ctl2"),
            month,
            &[tmp.path().join("ctl")],
        )
        .await
        .unwrap();
        assert!(again.is_empty());
        assert_eq!(
            ctl_table("PUBLIC_DVD_BIDPEROFFER_D_201201.ctl").as_deref(),
            Some("BIDPEROFFER_D")
        );
        assert_eq!(ctl_table("public_dvd_x_202401.CTL").as_deref(), Some("X"));
        assert_eq!(ctl_table("PUBLIC_DVD_X_2024.ctl"), None);
        assert_eq!(
            ctl_table("PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202608.ctl").as_deref(),
            Some("DISPATCHPRICE")
        );
        assert_eq!(ctl_table("PUBLIC_DVD_X_202401010000.zip"), None);
        assert_eq!(ctl_table("readme.txt"), None);
    }

    #[test]
    fn a_control_file_serves_its_table_its_splits_and_with_siblings_its_daily_file() {
        assert_eq!(table_keys("BIDPEROFFER1"), ["BIDPEROFFER", "BIDPEROFFER1"]);
        assert_eq!(table_keys("region"), ["REGION"]);
        assert_eq!(table_keys("2024"), ["2024"]);
        assert!(ctl_serves("BIDPEROFFER", "BIDPEROFFER1", false));
        assert!(ctl_serves("REGION", "region", false));
        assert!(!ctl_serves("REGIONAPC", "REGION", true));
        assert!(!ctl_serves(
            "PREDISPATCHCONSTRAINT_D",
            "PREDISPATCHCONSTRAINT1",
            false
        ));
        assert!(ctl_serves(
            "PREDISPATCHCONSTRAINT_D",
            "PREDISPATCHCONSTRAINT1",
            true
        ));
        assert!(ctl_serves("PREDISPATCHPRICE_D", "PREDISPATCHPRICE", true));
    }

    #[tokio::test]
    async fn a_refusal_stops_the_month_and_writes_nothing() {
        use nemlake_acquire::transport::{Canned, StaticTransport};
        let root = "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader";
        let link = |name: &str| {
            format!(
                r#" 2/13/2024 8:27 AM 12 <A HREF="/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/{name}">{name}</A><br>"#
            )
        };
        let listing = format!(
            "<pre>{}{}{}</pre>",
            link("PUBLIC_DVD_A_202401010000.zip"),
            link("PUBLIC_DVD_B_202401010000.zip"),
            link("PUBLIC_DVD_C_202401010000.zip")
        );
        let t = Arc::new(StaticTransport::default());
        t.add(&format!("{root}/DATA/"), Canned::ok(listing.as_bytes()))
            .add(&format!("{root}/P5MIN_ALL_DATA/"), Canned::status(404))
            .add(&format!("{root}/PREDISP_ALL_DATA/"), Canned::status(404));
        t.add(
            &format!("{root}/DATA/PUBLIC_DVD_A_202401010000.zip"),
            Canned::ok(&zip_of(b"I,A,X,1,V\r\nD,A,X,1,1\r\n")),
        );
        let mut refusal = Canned::status(403);
        refusal.body = b"Website is unavailable".to_vec().into();
        t.add(
            &format!("{root}/DATA/PUBLIC_DVD_B_202401010000.zip"),
            refusal,
        );
        t.add(
            &format!("{root}/DATA/PUBLIC_DVD_C_202401010000.zip"),
            Canned::ok(&zip_of(b"I,A,Z,1,V\r\nD,A,Z,1,1\r\n")),
        );
        let archive = MmsdmArchive::new("https://h").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let heads = tmp.path().join("heads");
        let fetcher = paced_fetcher(
            Arc::new(Arc::clone(&t)),
            &tmp.path().join("cache"),
            Duration::ZERO,
        );
        let e = fetch_heads(
            &fetcher,
            &archive,
            &heads,
            "2024-01".parse().unwrap(),
            HeadOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(e.diagnostic().code, "net.blocked");
        assert!(
            std::fs::read_dir(&heads).unwrap().next().is_none(),
            "a refused month writes no head"
        );
        // Three listings, head A, the refusal for B, and nothing for C.
        assert_eq!(t.request_count(), 5);
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
