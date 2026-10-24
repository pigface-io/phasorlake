//! Whether a zip nemweb publishes beside a folder of the same name holds
//! the same files, and removing it from the archive when it does.
//!
//! The archive publishes each month from 2015 on twice: as a folder of
//! per-table zips, and as one zip of the whole month. A pair is any file
//! `X.zip` the crawl lists beside a folder `X/`. Each file in the zip is
//! matched to the folder's file at the same path (the zip may put the
//! month's own folder, or something else, in front), first by size and
//! CRC-32, then, for a zip packed again, by the files it holds. A web page
//! in the zip (`Readme.htm`, `disclaimer.htm`, `Index.htm`) is not data:
//! one the folder lacks, or holds with other bytes, does not make the zip
//! differ; it is listed in the comparison and in the removal record.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, Write as _};
use std::path::{Path, PathBuf};

use crate::net::{Address, Archive, Provenance, Via};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::crawl::{Mirror, RemoteKind};
use crate::error::MirrorError;
use crate::stamp::Stamp;
use crate::zipdir::Entry;

const READ_BUFFER: usize = 1 << 20;
/// A zip inside the zip up to this size is read into memory to list it; a
/// bigger one is spooled to a temporary file.
const NESTED_IN_MEMORY: u64 = 64 << 20;

/// A zip and the folder of the same name beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pair {
    /// The zip's URL.
    pub zip: String,
    /// The folder's URL.
    pub folder: String,
    /// The zip's listed size.
    pub zip_size: u64,
    /// The folder's files: URL and listed size (0 for a rollup's, which
    /// come from the archive, not a listing).
    pub files: Vec<(String, u64)>,
    /// Whether the zip is a rollup: a daily zip of a report folder's
    /// `ARCHIVE` listing, whose entries are the folder's files of that day
    /// as `CURRENT` listed them.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rollup: bool,
}

/// What a comparison found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every file of the zip is in the folder, the same.
    Same,
    /// The zip holds files the folder lacks, or files that differ.
    Differs,
    /// Known only from the zip's listing, a file of it is not byte for byte
    /// the folder's: whether it is packed again needs the zip itself.
    NeedsDownload,
    /// Some file of the folder is not archived yet, or the zip is neither
    /// archived nor peeked.
    NotArchived,
}

/// The comparison of one pair.
#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    /// The zip's URL.
    pub zip: String,
    /// The folder's URL.
    pub folder: String,
    /// The zip's listed size.
    pub zip_size: u64,
    /// What it found.
    pub verdict: Verdict,
    /// Files of the zip whose bytes are the folder's.
    pub same: usize,
    /// Files of the zip packed again: other bytes, the same files inside.
    pub repacked: usize,
    /// Files of the zip with no file at the same path in the folder.
    pub only_in_zip: Vec<String>,
    /// Files of the zip whose content differs from the folder's.
    pub different: Vec<String>,
    /// Web pages of the zip (`.htm`, `.html`) that the folder lacks or
    /// holds with other bytes: not data, so not counted either way.
    pub ancillary: Vec<String>,
    /// Files of the zip, known from its listing, that are not byte for byte
    /// the folder's; the zip is needed to tell more.
    pub unsure: Vec<String>,
    /// Whether the zip was ruled on from its listing alone.
    pub from_listing: bool,
    /// Files of the folder the zip does not hold.
    pub only_in_folder: Vec<String>,
    /// Files of the pair not archived yet.
    pub not_archived: Vec<String>,
    /// Whether the zip is a rollup (`Pair::rollup`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rollup: bool,
    /// The zip's files found byte for byte in the folder: the entry's
    /// name and the folder file's path.
    #[serde(skip)]
    pub held: Vec<(String, PathBuf)>,
}

/// One removed duplicate: kept so the download does not fetch it again,
/// and so the map can still show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    /// The URL of the removed file.
    pub url: String,
    /// Its size.
    pub size: u64,
    /// Its sha256, from its sidecar; none for a zip never downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Whether it had been downloaded before it was found the same.
    #[serde(default = "downloaded")]
    pub downloaded: bool,
    /// The folder that holds the same files.
    pub same_as: String,
    /// How many files of the zip were found the same as the folder's; each
    /// is in the catalog on both sides, with its size and CRC-32.
    #[serde(default)]
    pub files: usize,
    /// Web pages of the zip that the folder lacks or holds with other
    /// bytes, left out of the comparison as not data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancillary: Vec<String>,
    /// When it was removed (UTC).
    pub removed_at: Stamp,
}

/// Whether a file of a zip is a web page rather than data: its name ends
/// in `.htm` or `.html`, in any case.
fn is_ancillary(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    let lower = file.to_ascii_lowercase();
    lower.ends_with(".htm") || lower.ends_with(".html")
}

fn downloaded() -> bool {
    true
}

impl Mirror {
    /// The record of removed duplicates.
    pub fn removed_path(&self) -> PathBuf {
        self.dir().join("removals.ndjson")
    }

    /// Every removed duplicate, by URL.
    pub fn removed(&self) -> Result<HashMap<String, Removed>, MirrorError> {
        let path = self.removed_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(MirrorError::io(&path)(e)),
        };
        text.lines()
            .enumerate()
            .filter(|(_, l)| !l.is_empty())
            .map(|(i, l)| {
                serde_json::from_str::<Removed>(l)
                    .map(|r| (r.url.clone(), r))
                    .map_err(|e| MirrorError::Record {
                        path: path.display().to_string(),
                        line: i as u64 + 1,
                        message: e.to_string(),
                    })
            })
            .collect()
    }
}

/// Every zip of the crawl with a folder of the same name beside it, then
/// every rollup (`rollups`). With `with_removed` false, a zip recorded
/// removed is left out.
pub fn pairs(
    mirror: &Mirror,
    archive: &Archive,
    with_removed: bool,
) -> Result<Vec<Pair>, MirrorError> {
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.done_path().display().to_string(),
        });
    }
    let record = mirror.record()?;
    let removed = match with_removed {
        true => HashMap::new(),
        false => mirror.removed()?,
    };
    let dirs: HashSet<&str> = record
        .iter()
        .filter(|r| r.kind == RemoteKind::Dir)
        .map(|r| r.url.as_str())
        .collect();
    let mut out = Vec::new();
    for r in record.iter().filter(|r| r.kind == RemoteKind::File) {
        if removed.contains_key(&r.url) {
            continue;
        }
        let Some(stem) = r
            .url
            .len()
            .checked_sub(4)
            .filter(|&n| r.url[n..].eq_ignore_ascii_case(".zip"))
            .map(|n| &r.url[..n])
        else {
            continue;
        };
        let folder = format!("{stem}/");
        if !dirs.contains(folder.as_str()) {
            continue;
        }
        let files = record
            .iter()
            .filter(|f| f.kind == RemoteKind::File && f.url.starts_with(&folder))
            .map(|f| (f.url.clone(), f.size.unwrap_or(0)))
            .collect();
        out.push(Pair {
            zip: r.url.clone(),
            folder,
            zip_size: r.size.unwrap_or(0),
            files,
            rollup: false,
        });
    }
    out.extend(rollups(archive, &record, &removed));
    Ok(out)
}

/// Every rollup the crawl lists: a zip `{prefix}{YYYYMMDD}.zip` of a
/// report folder's `ARCHIVE` listing, paired with the folder's files of
/// that day and the next held from its `CURRENT` listing. Those files come
/// from the archive on disk, since `CURRENT` lists a file for about four
/// days only. The folder (both listings are one, `layout`) is read once.
fn rollups(
    archive: &Archive,
    record: &[crate::crawl::Remote],
    removed: &HashMap<String, Removed>,
) -> Vec<Pair> {
    let mut names: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut out = Vec::new();
    for r in record.iter().filter(|r| r.kind == RemoteKind::File) {
        if removed.contains_key(&r.url) {
            continue;
        }
        let Some((current, prefix, day)) = rollup_of(&r.url) else {
            continue;
        };
        let zip_path = archive.path_for(&r.url);
        let dir = zip_path.parent().expect("an archived path is in a folder");
        let held = names.entry(dir.to_path_buf()).or_insert_with(|| {
            std::fs::read_dir(dir)
                .map(|rd| {
                    rd.filter_map(Result::ok)
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| {
                            !n.starts_with("._")
                                && !n.ends_with(".json")
                                && !n.ends_with(".part")
                                && !n.ends_with(".tmp")
                                && !n.contains('@')
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        let next = next_day(&day);
        let zip_name = zip_path.file_name().unwrap_or_default().to_string_lossy();
        let files: Vec<(String, u64)> = held
            .iter()
            .filter(|n| **n != *zip_name)
            .filter(|n| {
                n.strip_prefix(prefix.as_str()).is_some_and(|rest| {
                    rest.starts_with(day.as_str())
                        || next.as_deref().is_some_and(|d| rest.starts_with(d))
                })
            })
            .map(|n| (format!("{current}{n}"), 0))
            .collect();
        if files.is_empty() {
            continue;
        }
        out.push(Pair {
            zip: r.url.clone(),
            folder: current,
            zip_size: r.size.unwrap_or(0),
            files,
            rollup: true,
        });
    }
    out
}

/// Of a daily zip of an `ARCHIVE` listing: the folder's URL under the
/// `CURRENT` listing, the name before the date, and the date.
fn rollup_of(url: &str) -> Option<(String, String, String)> {
    const ARCHIVE: &str = "/Reports/ARCHIVE/";
    let at = url.find(ARCHIVE)?;
    if utilake_provenance::layout::listing(url).as_deref() != Some("ARCHIVE") {
        return None;
    }
    let (folder, name) = url.rsplit_once('/')?;
    let stem = name
        .strip_suffix(".zip")
        .or_else(|| name.strip_suffix(".ZIP"))?;
    let (prefix, day) = stem.split_at(stem.len().checked_sub(8)?);
    if !day.bytes().all(|b| b.is_ascii_digit()) || !prefix.ends_with('_') {
        return None;
    }
    let sub = folder.get(at + ARCHIVE.len()..).filter(|s| !s.is_empty())?;
    let current = format!("{}/Reports/CURRENT/{sub}/", &url[..at]);
    Some((current, prefix.to_owned(), day.to_owned()))
}

/// The day after `YYYYMMDD`, as `YYYYMMDD`.
fn next_day(day: &str) -> Option<String> {
    let n = |r: std::ops::Range<usize>| day.get(r)?.parse::<u32>().ok();
    let (y, m, d) = (n(0..4)?, n(4..6)?, n(6..8)?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    let (y, m, d) = match (d >= days, m == 12) {
        (false, _) => (y, m, d + 1),
        (true, false) => (y, m + 1, 1),
        (true, true) => (y + 1, 1, 1),
    };
    Some(format!("{y:04}{m:02}{d:02}"))
}

/// A file's size and CRC-32, or what a zip says of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sum {
    size: u64,
    crc32: u32,
}

/// One file of the zip: its name, size and CRC-32, and its index when the
/// zip itself is at hand.
struct ZipFile {
    name: String,
    sum: Sum,
    index: Option<usize>,
}

/// Compares the zip of `pair` with its folder. The folder's files come
/// from the archive; the zip's from the archive when it is there, else
/// from its peeked listing (`listing`).
pub fn compare(
    archive: &Archive,
    pair: &Pair,
    listing: Option<&[Entry]>,
    tmp: &Path,
) -> Result<Comparison, MirrorError> {
    let mut cmp = Comparison {
        zip: pair.zip.clone(),
        folder: pair.folder.clone(),
        zip_size: pair.zip_size,
        verdict: Verdict::Same,
        same: 0,
        repacked: 0,
        only_in_zip: Vec::new(),
        different: Vec::new(),
        ancillary: Vec::new(),
        unsure: Vec::new(),
        from_listing: false,
        only_in_folder: Vec::new(),
        not_archived: Vec::new(),
        rollup: pair.rollup,
        held: Vec::new(),
    };
    // The folder's files by their path under the folder, as listed; only
    // those the zip's files match need be archived.
    let folder: BTreeMap<String, PathBuf> = pair
        .files
        .iter()
        .map(|(url, _)| {
            (
                percent_decoded(&url[pair.folder.len()..]),
                archive.path_for(url),
            )
        })
        .collect();
    let zip_path = archive.path_for(&pair.zip);
    let mut zip = None;
    let files: Vec<ZipFile> = if zip_path.is_file() {
        let file = File::open(&zip_path).map_err(MirrorError::io(&zip_path))?;
        let bad = |e: zip::result::ZipError| MirrorError::Zip {
            path: zip_path.display().to_string(),
            message: e.to_string(),
        };
        let mut z = ZipArchive::new(BufReader::with_capacity(64 << 10, file)).map_err(bad)?;
        let mut files = Vec::with_capacity(z.len());
        for i in 0..z.len() {
            let f = z.by_index_raw(i).map_err(bad)?;
            let name = String::from_utf8_lossy(f.name_raw());
            // Zips made on Windows separate folders with `\`.
            if !f.is_dir() && !name.ends_with('\\') {
                files.push(ZipFile {
                    name: name.replace('\\', "/"),
                    sum: Sum {
                        size: f.size(),
                        crc32: f.crc32(),
                    },
                    index: Some(i),
                });
            }
        }
        zip = Some(z);
        files
    } else if let Some(entries) = listing {
        cmp.from_listing = true;
        entries
            .iter()
            .filter(|e| !e.name.ends_with(['/', '\\']))
            .map(|e| ZipFile {
                name: e.name.replace('\\', "/"),
                sum: Sum {
                    size: e.size,
                    crc32: e.crc32,
                },
                index: None,
            })
            .collect()
    } else {
        cmp.not_archived.push(pair.zip.clone());
        cmp.verdict = Verdict::NotArchived;
        return Ok(cmp);
    };
    // Every file of the zip at its folder path, which must be archived.
    let mut matched: Vec<(ZipFile, String)> = Vec::with_capacity(files.len());
    for f in files {
        match suffix_in(&f.name, &folder) {
            Some(rel) if folder[&rel].is_file() => matched.push((f, rel)),
            _ if is_ancillary(&f.name) => cmp.ancillary.push(f.name),
            Some(rel) => cmp.not_archived.push(rel),
            None => cmp.only_in_zip.push(f.name),
        }
    }
    if !cmp.not_archived.is_empty() {
        cmp.verdict = Verdict::NotArchived;
        return Ok(cmp);
    }
    let seen: HashSet<String> = matched.iter().map(|(_, rel)| rel.clone()).collect();
    for (f, rel) in matched {
        let path = &folder[&rel];
        if known_sum(path)? == f.sum {
            cmp.same += 1;
            cmp.held.push((f.name, path.clone()));
            continue;
        }
        if is_ancillary(&f.name) {
            cmp.ancillary.push(f.name);
            continue;
        }
        // Other bytes: the same files inside, packed again?
        let (Some(z), Some(i)) = (zip.as_mut(), f.index) else {
            cmp.unsure.push(rel);
            continue;
        };
        let inner = list_entry(z, i, tmp, NESTED_IN_MEMORY).ok();
        let theirs = File::open(path)
            .ok()
            .and_then(|f| list(BufReader::with_capacity(64 << 10, f)).ok());
        match (inner, theirs) {
            (Some(a), Some(b)) if a == b => cmp.repacked += 1,
            _ => cmp.different.push(rel),
        }
    }
    cmp.only_in_folder = folder
        .keys()
        .filter(|k| !seen.contains(*k))
        .cloned()
        .collect();
    if !cmp.only_in_zip.is_empty() || !cmp.different.is_empty() {
        cmp.verdict = Verdict::Differs;
    } else if !cmp.unsure.is_empty() {
        cmp.verdict = Verdict::NeedsDownload;
    }
    Ok(cmp)
}

/// Removes the zip of a comparison that found it the same as its folder,
/// recording it first: from the archive when it was downloaded, and from
/// the download to come when it was ruled on from its listing. Refuses any
/// other comparison.
pub fn remove(
    mirror: &Mirror,
    archive: &Archive,
    cmp: &Comparison,
    offset: i32,
) -> Result<Removed, MirrorError> {
    if cmp.verdict != Verdict::Same {
        return Err(MirrorError::NotSame {
            url: cmp.zip.clone(),
        });
    }
    let path = archive.path_for(&cmp.zip);
    let sidecar = Archive::read(&path)?;
    if !cmp.from_listing && sidecar.is_none() {
        return Err(MirrorError::NotSame {
            url: cmp.zip.clone(),
        });
    }
    let removed = Removed {
        url: cmp.zip.clone(),
        size: sidecar.as_ref().map_or(cmp.zip_size, |s| s.content.size),
        downloaded: sidecar.is_some(),
        sha256: sidecar.map(|s| s.content.sha256.hex()),
        same_as: cmp.folder.clone(),
        files: cmp.same + cmp.repacked,
        ancillary: cmp.ancillary.clone(),
        removed_at: Stamp::now(),
    };
    // A downloaded zip's listing is kept before it goes, so its side of the
    // comparison stays in the record and the catalog.
    if removed.downloaded && !cmp.from_listing {
        crate::peek::keep_listing(mirror, &cmp.zip, &path, offset)?;
    }
    let record = mirror.removed_path();
    let mut line = serde_json::to_vec(&removed).expect("a Removed always serialises");
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&record)
        .map_err(MirrorError::io(&record))?;
    f.write_all(&line).map_err(MirrorError::io(&record))?;
    f.sync_all().map_err(MirrorError::io(&record))?;
    note_entries(cmp)?;
    if removed.downloaded {
        crate::net::archive::remove(&path)?;
    }
    Ok(removed)
}

/// Adds the address `{zip}!{entry}` to the sidecar of each folder file a
/// rollup holds byte for byte (`how = "entry"`): where the file can still
/// be fetched once `CURRENT` stops listing it. The data files are not
/// touched. How many sidecars gained the address; none for a zip that is
/// not a rollup.
pub fn note_entries(cmp: &Comparison) -> Result<usize, MirrorError> {
    if !cmp.rollup {
        return Ok(0);
    }
    let now = Stamp::now();
    let mut n = 0;
    for (name, path) in &cmp.held {
        let url = format!("{}!{name}", cmp.zip);
        let entry = Address {
            url,
            seen: now,
            gone: None,
            via: Via::Entry,
        };
        if crate::net::archive::add_address(path, entry, false)? {
            n += 1;
        }
    }
    // One sync for the batch: on the volume, a sync per record costs
    // seconds once its folder is large.
    if n > 0 {
        rustix::fs::sync();
    }
    Ok(n)
}

/// A removed zip checked again against its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Verified {
    /// The removed zip.
    pub zip: String,
    /// The folder found to hold the same files.
    pub folder: String,
    /// Files of the zip checked, read again from the folder.
    pub checked: usize,
    /// Folder files that no longer match the zip's size and CRC-32.
    pub changed: Vec<String>,
    /// Folder files that are no longer in the archive.
    pub missing: Vec<String>,
    /// Whether the zip's listing is not in the record, so nothing could be
    /// checked.
    pub no_listing: bool,
}

impl Verified {
    /// Whether the folder still holds every file of the zip, the same.
    pub fn holds(&self) -> bool {
        self.changed.is_empty() && self.missing.is_empty() && !self.no_listing
    }
}

/// Reads every removed zip's folder files again, in full, and checks each
/// is still what the comparison saw: the CRC-32 recorded in its sidecar, or
/// the zip's for its file. The zip's listing says which files to check.
pub fn verify(mirror: &Mirror, archive: &Archive) -> Result<Vec<Verified>, MirrorError> {
    let removed = mirror.removed()?;
    let listings: HashMap<String, Vec<Entry>> = crate::peek::read_peeks(&mirror.peek_path())?
        .into_iter()
        .filter_map(|p| Some((p.url, p.entries?)))
        .collect();
    let mut out = Vec::new();
    for pair in pairs(mirror, archive, true)?
        .into_iter()
        .filter(|p| removed.contains_key(&p.zip))
    {
        let mut v = Verified {
            zip: pair.zip.clone(),
            folder: pair.folder.clone(),
            ..Verified::default()
        };
        let Some(entries) = listings.get(&pair.zip) else {
            v.no_listing = true;
            out.push(v);
            continue;
        };
        let folder: BTreeMap<String, PathBuf> = pair
            .files
            .iter()
            .map(|(url, _)| {
                (
                    percent_decoded(&url[pair.folder.len()..]),
                    archive.path_for(url),
                )
            })
            .collect();
        let left_out = &removed[&pair.zip].ancillary;
        for e in entries.iter().filter(|e| !e.name.ends_with(['/', '\\'])) {
            let name = e.name.replace('\\', "/");
            if left_out.contains(&name) {
                continue;
            }
            let Some(rel) = suffix_in(&name, &folder) else {
                v.missing.push(name);
                continue;
            };
            v.checked += 1;
            // What the comparison saw: the CRC compare recorded in the
            // folder file's sidecar (a file packed again matched by what it
            // holds, not its bytes), else the zip's.
            let path = &folder[&rel];
            let want = match Archive::read(path) {
                Ok(Some(Provenance { content, .. })) if content.crc32.is_some() => Sum {
                    size: content.size,
                    crc32: content.crc32.unwrap_or_default(),
                },
                _ => Sum {
                    size: e.size,
                    crc32: e.crc32,
                },
            };
            match sum_of(path) {
                Ok(sum) if sum == want => {}
                Ok(_) => v.changed.push(rel),
                Err(_) => v.missing.push(rel),
            }
        }
        out.push(v);
    }
    Ok(out)
}

/// The shortest tail of `name`, cut at a `/`, that is a path of `folder`.
fn suffix_in(name: &str, folder: &BTreeMap<String, PathBuf>) -> Option<String> {
    let mut tail = name;
    loop {
        if folder.contains_key(tail) {
            return Some(tail.to_owned());
        }
        tail = &tail[tail.find('/')? + 1..];
    }
}

/// The size and CRC-32 of an archived file: from its sidecar when it holds
/// them, else read in full and recorded there, so a file is read once.
fn known_sum(path: &Path) -> Result<Sum, MirrorError> {
    let sidecar = Archive::read(path)?;
    let len = std::fs::metadata(path)
        .map_err(MirrorError::io(path))?
        .len();
    if let Some(p) = &sidecar
        && p.content.size == len
        && let Some(crc32) = p.content.crc32
    {
        return Ok(Sum { size: len, crc32 });
    }
    let sum = sum_of(path).map_err(MirrorError::io(path))?;
    if let Some(mut s) = sidecar.filter(|s| s.content.size == sum.size) {
        s.content.crc32 = Some(sum.crc32);
        Archive::write(path, &s)?;
    }
    Ok(sum)
}

/// The size and CRC-32 of a file, read in full.
fn sum_of(path: &Path) -> std::io::Result<Sum> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; READ_BUFFER];
    let mut h = crc32fast::Hasher::new();
    let mut size = 0u64;
    loop {
        match f.read(&mut buf)? {
            0 => break,
            n => {
                h.update(&buf[..n]);
                size += n as u64;
            }
        }
    }
    Ok(Sum {
        size,
        crc32: h.finalize(),
    })
}

/// What a zip holds: each file's name (without folders), size and CRC-32.
type Listing = BTreeMap<String, (u64, u32)>;

fn list<R: Read + Seek>(r: R) -> zip::result::ZipResult<Listing> {
    let mut z = ZipArchive::new(r)?;
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let f = z.by_index_raw(i)?;
        if !f.is_dir() {
            let raw = String::from_utf8_lossy(f.name_raw());
            let name = raw.rsplit(['/', '\\']).next().unwrap_or("").to_owned();
            out.insert(name, (f.size(), f.crc32()));
        }
    }
    Ok(out)
}

/// The listing of entry `i` of `z`, itself a zip.
fn list_entry<R: Read + Seek>(
    z: &mut ZipArchive<R>,
    i: usize,
    tmp: &Path,
    in_memory: u64,
) -> std::io::Result<Listing> {
    let mut f = z.by_index(i).map_err(std::io::Error::other)?;
    let listed = if f.size() <= in_memory {
        let mut buf = Vec::with_capacity(usize::try_from(f.size()).unwrap_or(0));
        f.read_to_end(&mut buf)?;
        list(Cursor::new(buf))
    } else {
        std::fs::create_dir_all(tmp)?;
        let mut t = tempfile::tempfile_in(tmp)?;
        std::io::copy(&mut f, &mut t)?;
        t.rewind()?;
        list(BufReader::with_capacity(64 << 10, t))
    };
    listed.map_err(std::io::Error::other)
}

/// `%XX` escapes decoded, as the archive decodes a URL's path.
fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::diag::Diagnose as _;
    use crate::hosts::Offsets;
    use crate::net::transport::StaticTransport;
    use crate::net::{Fetcher, FetcherConfig};
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::crawl::{Remote, root};

    fn zip_of(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(method);
        for (name, bytes) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn archive_file(archive: &Archive, url: &str, bytes: &[u8]) {
        let p = archive.path_for(url);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        Archive::write(&p, &crate::net::archive::test_record(url, bytes)).unwrap();
    }

    struct Fixture {
        _home: tempfile::TempDir,
        mirror: Mirror,
        archive: Archive,
    }

    /// `M/` holds two table zips and a control file; `M.zip` holds the
    /// first table zip byte for byte under `M/`, the second packed again,
    /// and, when `extra`, a file the folder lacks.
    fn fixture(extra: bool) -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        let t1 = zip_of(&[("T1.CSV", b"a,b\n")], zip::CompressionMethod::Deflated);
        let t2 = zip_of(&[("T2.CSV", b"c,d\n")], zip::CompressionMethod::Deflated);
        let t2_again = zip_of(&[("T2.CSV", b"c,d\n")], zip::CompressionMethod::Stored);
        let folder = [
            ("https://h/A/M/S/DATA/T1%231.zip", &t1[..]),
            ("https://h/A/M/S/DATA/T2.zip", &t2[..]),
            ("https://h/A/M/S/CTL/T1.ctl", b"ctl"),
        ];
        let mut inside: Vec<(&str, &[u8])> = vec![
            ("M/S/DATA/", b""),
            ("M/S/DATA/T1#1.zip", &t1),
            ("M/S/DATA/T2.zip", &t2_again),
        ];
        if extra {
            inside.push(("M/README.txt", b"only here"));
        }
        let whole = zip_of(&inside, zip::CompressionMethod::Stored);
        let mut record = vec![
            Remote {
                url: "https://h/A/M/".into(),
                kind: RemoteKind::Dir,
                size: None,
                modified: "m".into(),
                modified_at: None,
                listed_on: "https://h/A/".into(),
                crawled_at: None,
            },
            Remote {
                url: "https://h/A/M.zip".into(),
                kind: RemoteKind::File,
                size: Some(whole.len() as u64),
                modified: "m".into(),
                modified_at: None,
                listed_on: "https://h/A/".into(),
                crawled_at: None,
            },
            Remote {
                url: "https://h/A/lone.zip".into(),
                kind: RemoteKind::File,
                size: Some(1),
                modified: "m".into(),
                modified_at: None,
                listed_on: "https://h/A/".into(),
                crawled_at: None,
            },
        ];
        archive_file(&archive, "https://h/A/M.zip", &whole);
        for (url, bytes) in folder {
            archive_file(&archive, url, bytes);
            record.push(Remote {
                url: url.into(),
                kind: RemoteKind::File,
                size: Some(bytes.len() as u64),
                modified: "m".into(),
                modified_at: None,
                listed_on: "https://h/A/M/S/".into(),
                crawled_at: None,
            });
        }
        std::fs::create_dir_all(mirror.dir()).unwrap();
        let lines: String = record
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        mirror.set_record_text(lines);
        std::fs::write(mirror.done_path(), "").unwrap();
        Fixture {
            _home: home,
            mirror,
            archive,
        }
    }

    #[test]
    fn a_daily_zip_whose_files_are_held_from_current_is_a_rollup() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        let cur = "https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/";
        let arc = "https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/";
        let a = zip_of(&[("A.CSV", b"a\n")], zip::CompressionMethod::Deflated);
        let b = zip_of(&[("B.CSV", b"b\n")], zip::CompressionMethod::Deflated);
        let c = zip_of(&[("C.CSV", b"c\n")], zip::CompressionMethod::Deflated);
        let (na, nb, nc) = (
            "PUBLIC_DISPATCHSCADA_202610170005_0000000001.zip",
            "PUBLIC_DISPATCHSCADA_202610180000_0000000002.zip",
            "PUBLIC_DISPATCHSCADA_202610190005_0000000003.zip",
        );
        // Held from CURRENT, which lists them no more.
        for (n, bytes) in [(na, &a), (nb, &b), (nc, &c)] {
            archive_file(&archive, &format!("{cur}{n}"), bytes);
        }
        let dir = archive.path_for(&format!("{cur}{na}"));
        let dir = dir.parent().unwrap();
        std::fs::write(dir.join(format!("._{na}")), b"apple").unwrap();
        archive_file(
            &archive,
            &format!("{cur}PUBLIC_DISPATCHSCADA_202610200005_4.zip"),
            b"x",
        );
        let day17 = format!("{arc}PUBLIC_DISPATCHSCADA_20261017.zip");
        let day19 = format!("{arc}PUBLIC_DISPATCHSCADA_20261019.zip");
        let whole17 = zip_of(
            &[(na, &a[..]), (nb, &b[..])],
            zip::CompressionMethod::Stored,
        );
        let whole19 = zip_of(
            &[
                (nc, &c[..]),
                ("PUBLIC_DISPATCHSCADA_202610190010_9.zip", b"new"),
            ],
            zip::CompressionMethod::Stored,
        );
        archive_file(&archive, &day17, &whole17);
        archive_file(&archive, &day19, &whole19);
        let file = |url: String| Remote {
            url,
            kind: RemoteKind::File,
            size: Some(1),
            modified: "m".into(),
            modified_at: None,
            listed_on: arc.into(),
            crawled_at: None,
        };
        let record = [
            file(day17.clone()),
            file(day19.clone()),
            // Not rollups: no date, no files held that day, no folder.
            file(format!("{arc}PUBLIC_DISPATCHSCADA.zip")),
            file(format!("{arc}PUBLIC_DISPATCHSCADA_20260101.zip")),
            file("https://www.nemweb.com.au/Reports/ARCHIVE/PUBLIC_X_20261017.zip".into()),
        ];
        std::fs::create_dir_all(mirror.dir()).unwrap();
        let lines: String = record
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        mirror.set_record_text(lines);
        std::fs::write(mirror.done_path(), "").unwrap();

        let found = pairs(&mirror, &archive, true).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        let p17 = found.iter().find(|p| p.zip == day17).unwrap();
        assert!(p17.rollup);
        assert_eq!(p17.folder, cur);
        let mut names: Vec<&str> = p17.files.iter().map(|(u, _)| &u[cur.len()..]).collect();
        names.sort_unstable();
        assert_eq!(names, [na, nb], "the day and the next, no AppleDouble twin");
        let tmp = mirror.tmp_dir();
        let cmp = compare(&archive, p17, None, &tmp).unwrap();
        assert_eq!(
            (cmp.verdict, cmp.same, cmp.rollup),
            (Verdict::Same, 2, true),
            "{cmp:?}"
        );
        let removed = remove(&mirror, &archive, &cmp, crate::stamp::AEST).unwrap();
        assert_eq!(removed.same_as, cur);
        assert!(!archive.path_for(&day17).exists());
        let sc = Archive::read(&archive.path_for(&format!("{cur}{na}")))
            .unwrap()
            .unwrap();
        assert_eq!(sc.addresses.len(), 2);
        assert_eq!(sc.addresses[1].url, format!("{day17}!{na}"));
        assert_eq!(sc.addresses[1].via, Via::Entry);
        assert_eq!(sc.content.sha256.hex(), crate::net::archive::sha256_hex(&a));
        // Removed: left out unless asked for.
        let left: Vec<String> = pairs(&mirror, &archive, false)
            .unwrap()
            .into_iter()
            .map(|p| p.zip)
            .collect();
        assert_eq!(left, std::slice::from_ref(&day19));
        let v = verify(&mirror, &archive).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].holds(), "{v:?}");

        // An entry not held: the zip is kept, its held file still gains
        // the address inside it.
        let p19 = pairs(&mirror, &archive, false).unwrap().remove(0);
        let cmp = compare(&archive, &p19, None, &tmp).unwrap();
        assert_eq!(cmp.verdict, Verdict::Differs);
        assert_eq!(note_entries(&cmp).unwrap(), 1);
        assert_eq!(note_entries(&cmp).unwrap(), 0);
        let sc = Archive::read(&archive.path_for(&format!("{cur}{nc}")))
            .unwrap()
            .unwrap();
        assert_eq!(sc.addresses[1].url, format!("{day19}!{nc}"));
        // A pair that is not a rollup notes nothing.
        let plain = Comparison {
            rollup: false,
            ..cmp
        };
        assert_eq!(note_entries(&plain).unwrap(), 0);
    }

    #[test]
    fn the_day_after_and_the_rollup_of_a_url() {
        assert_eq!(next_day("20261017").as_deref(), Some("20261018"));
        assert_eq!(next_day("20261130").as_deref(), Some("20261201"));
        assert_eq!(next_day("20261231").as_deref(), Some("20270101"));
        assert_eq!(next_day("20240228").as_deref(), Some("20240229"));
        assert_eq!(next_day("20230228").as_deref(), Some("20230301"));
        assert_eq!(next_day("21000228").as_deref(), Some("21000301"));
        assert_eq!(next_day("20000228").as_deref(), Some("20000229"));
        assert_eq!(next_day("20261317"), None);
        assert_eq!(next_day("x"), None);
        let (cur, prefix, day) = rollup_of(
            "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/Flows/PUBLIC_FLOWS_20261017.ZIP",
        )
        .unwrap();
        assert_eq!(
            (cur.as_str(), prefix.as_str(), day.as_str()),
            (
                "https://www.nemweb.com.au/Reports/CURRENT/GSH/Flows/",
                "PUBLIC_FLOWS_",
                "20261017"
            )
        );
        for url in [
            "https://www.nemweb.com.au/Reports/CURRENT/X/PUBLIC_X_20261017.zip",
            "https://h/Reports/ARCHIVE/X/PUBLIC_X_20261017.zip",
            "https://www.nemweb.com.au/Reports/ARCHIVE/X/PUBLIC_X_2026101A.zip",
            "https://www.nemweb.com.au/Reports/ARCHIVE/X/PUBLICX20261017.zip",
            "https://www.nemweb.com.au/Reports/ARCHIVE/X/X.zip",
            "https://www.nemweb.com.au/Reports/ARCHIVE/X/PUBLIC_X_20261017.csv",
        ] {
            assert!(rollup_of(url).is_none(), "{url}");
        }
    }

    #[tokio::test]
    async fn a_zip_the_same_as_its_folder_is_removed_and_stays_mapped() {
        let f = fixture(false);
        let pairs = pairs(&f.mirror, &f.archive, true).unwrap();
        assert_eq!(pairs.len(), 1, "lone.zip has no folder");
        assert_eq!(pairs[0].folder, "https://h/A/M/");
        assert_eq!(pairs[0].files.len(), 3);
        let tmp = f.mirror.tmp_dir();
        let cmp = compare(&f.archive, &pairs[0], None, &tmp).unwrap();
        assert_eq!(cmp.verdict, Verdict::Same, "{cmp:?}");
        assert_eq!((cmp.same, cmp.repacked), (1, 1));
        assert_eq!(cmp.only_in_folder, ["S/CTL/T1.ctl"]);

        let removed = remove(&f.mirror, &f.archive, &cmp, crate::stamp::AEST).unwrap();
        assert_eq!(removed.same_as, "https://h/A/M/");
        assert!(!f.archive.path_for("https://h/A/M.zip").exists());
        assert!(
            f.mirror
                .removed()
                .unwrap()
                .contains_key("https://h/A/M.zip")
        );

        // Not fetched again; counted as removed; mapped as removed.
        let t = Arc::new(StaticTransport::default());
        let fetcher = crate::net::Fetchers::one(Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(f.archive.root()),
            FetcherConfig {
                attempts: 1,
                backoff: Duration::ZERO,
                max_backoff: Duration::ZERO,
                min_interval: Duration::ZERO,
            },
        ));
        let s = crate::download(
            &fetcher,
            &f.mirror,
            &crate::download::DownloadOptions {
                downloads: 1,
                only: &[],
                folders: None,
                delete_duplicates: false,
            },
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!((s.files, s.removed, s.cached), (5, 1, 3));
        assert!(
            !t.requests
                .lock()
                .unwrap()
                .iter()
                .any(|u| u.ends_with("M.zip"))
        );
        let roots = [root("https://h/A/").unwrap()];
        let st = crate::status(&f.mirror, &f.archive, &roots).unwrap();
        assert_eq!(
            (st.roots[0].removed_files, st.roots[0].archived_files),
            (1, 3)
        );
        let m = crate::map(&f.mirror, &f.archive, &roots, 1, &Offsets::default()).unwrap();
        assert_eq!(m.removed, 1);
        let tree = std::fs::read_to_string(f.mirror.cache_dir().join("catalog.txt")).unwrap();
        // Removed, its listing kept: the catalog still says what it held.
        assert!(tree.contains("M.zip/ (zip, "), "{tree}");
        assert!(tree.contains("[removed: same as its folder]"), "{tree}");
        assert!(tree.contains("T1#1.zip/ (zip, "), "{tree}");
        assert_eq!(f.mirror.removed().unwrap()["https://h/A/M.zip"].files, 2);
        // Kept once.
        crate::peek::keep_listing(
            &f.mirror,
            "https://h/A/M.zip",
            Path::new("/none"),
            crate::stamp::AEST,
        )
        .unwrap();
        assert_eq!(
            crate::peek::read_peeks(&f.mirror.peek_path())
                .unwrap()
                .len(),
            1
        );
        let ndjson = std::fs::read_to_string(f.mirror.cache_dir().join("catalog.ndjson")).unwrap();
        let line = ndjson
            .lines()
            .find(|l| l.contains("\"h/A/M.zip\""))
            .unwrap();
        assert!(line.contains("\"same_as\":\"https://h/A/M/\""), "{line}");
        assert!(
            line.contains("\"matched\":2") && line.contains("\"removed_at\""),
            "{line}"
        );
    }

    #[test]
    fn a_zip_with_more_than_its_folder_is_kept() {
        let f = fixture(true);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        let cmp = compare(&f.archive, pair, None, &f.mirror.tmp_dir()).unwrap();
        assert_eq!(cmp.verdict, Verdict::Differs);
        assert_eq!(cmp.only_in_zip, ["M/README.txt"]);
        let e = remove(&f.mirror, &f.archive, &cmp, crate::stamp::AEST).unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.not_same");
        assert!(f.archive.path_for(&pair.zip).exists());
    }

    #[test]
    fn web_pages_in_the_zip_do_not_make_it_differ() {
        let f = fixture(false);
        let tmp = f.mirror.tmp_dir();
        let t1 = std::fs::read(f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip")).unwrap();
        let t2 = std::fs::read(f.archive.path_for("https://h/A/M/S/DATA/T2.zip")).unwrap();
        // The folder holds a disclaimer page; the zip holds it with other
        // bytes, a readme page the folder lacks, and an index page whose
        // folder file is listed but not archived.
        let folder_page = "https://h/A/M/S/disclaimer.htm";
        archive_file(&f.archive, folder_page, b"folder's words");
        let index_page = "https://h/A/M/S/Index.HTM";
        let mut record = f.mirror.record_text();
        for url in [folder_page, index_page] {
            record += &(serde_json::to_string(&Remote {
                url: url.into(),
                kind: RemoteKind::File,
                size: Some(1),
                modified: "m".into(),
                modified_at: None,
                listed_on: "https://h/A/M/S/".into(),
                crawled_at: None,
            })
            .unwrap()
                + "\n");
        }
        f.mirror.set_record_text(record);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        assert_eq!(pair.files.len(), 5);
        let whole = zip_of(
            &[
                ("M/S/DATA/T1#1.zip", &t1[..]),
                ("M/S/DATA/T2.zip", &t2[..]),
                ("M/S/disclaimer.htm", b"zip's words"),
                ("M/Readme.htm", b"only here"),
                ("M/S/Index.HTM", b"i"),
            ],
            zip::CompressionMethod::Stored,
        );
        let zip_path = f.archive.path_for(&pair.zip);
        std::fs::write(&zip_path, &whole).unwrap();
        let cmp = compare(&f.archive, pair, None, &tmp).unwrap();
        assert_eq!((cmp.verdict, cmp.same), (Verdict::Same, 2), "{cmp:?}");
        assert_eq!(
            cmp.ancillary,
            ["M/Readme.htm", "M/S/Index.HTM", "M/S/disclaimer.htm"]
        );
        assert!(cmp.only_in_zip.is_empty() && cmp.not_archived.is_empty());
        // From the listing alone the same pages are left out too.
        let entry = |name: &str, bytes: &[u8]| Entry {
            name: name.into(),
            size: bytes.len() as u64,
            compressed: bytes.len() as u64,
            crc32: crc32fast::hash(bytes),
            method: 0,
            modified: "m".into(),
            modified_at: None,
        };
        let listing = [
            entry("M/S/DATA/T1#1.zip", &t1),
            entry("M/S/DATA/T2.zip", &t2),
            entry("M/S/disclaimer.htm", b"zip's words"),
            entry("M/Readme.htm", b"only here"),
        ];
        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(Archive::record_path(&zip_path)).unwrap();
        let cmp = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(
            (cmp.verdict, cmp.unsure.len()),
            (Verdict::Same, 0),
            "{cmp:?}"
        );
        assert_eq!(cmp.ancillary, ["M/Readme.htm", "M/S/disclaimer.htm"]);
        // Recorded with the removal, and left out again when verified.
        let r = remove(&f.mirror, &f.archive, &cmp, crate::stamp::AEST).unwrap();
        assert_eq!(r.ancillary, cmp.ancillary);
        let line = std::fs::read_to_string(f.mirror.removed_path()).unwrap();
        assert!(
            line.contains("\"ancillary\":[\"M/Readme.htm\",\"M/S/disclaimer.htm\"]"),
            "{line}"
        );
        assert_eq!(
            f.mirror.removed().unwrap()[&pair.zip].ancillary,
            r.ancillary
        );
        let peek = crate::peek::Peeked {
            url: pair.zip.clone(),
            peeked_at: None,
            listed_size: None,
            listed_modified: None,
            listed_modified_at: None,
            size: None,
            etag: None,
            last_modified: None,
            last_modified_at: None,
            entries: Some(listing.to_vec()),
            error: None,
        };
        std::fs::write(
            f.mirror.peek_path(),
            serde_json::to_string(&peek).unwrap() + "\n",
        )
        .unwrap();
        let v = &verify(&f.mirror, &f.archive).unwrap()[0];
        assert!(v.holds(), "{v:?}");
        assert_eq!(v.checked, 2);
        assert!(!is_ancillary("M/S/DATA/T1#1.zip") && is_ancillary("x/INDEX.HTML"));
    }

    #[test]
    fn a_different_file_and_files_not_yet_archived() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        let t1 = f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip");
        std::fs::write(&t1, b"not the same").unwrap();
        let cmp = compare(&f.archive, pair, None, &f.mirror.tmp_dir()).unwrap();
        assert_eq!(cmp.verdict, Verdict::Differs);
        assert_eq!(cmp.different, ["S/DATA/T1#1.zip"]);

        // A file of the zip whose folder file is not downloaded: wait.
        std::fs::remove_file(&t1).unwrap();
        let cmp = compare(&f.archive, pair, None, &f.mirror.tmp_dir()).unwrap();
        assert_eq!(cmp.verdict, Verdict::NotArchived);
        assert_eq!(cmp.not_archived, ["S/DATA/T1#1.zip"]);
        // Neither downloaded nor peeked: wait for the zip.
        std::fs::remove_file(f.archive.path_for(&pair.zip)).unwrap();
        let cmp = compare(&f.archive, pair, None, &f.mirror.tmp_dir()).unwrap();
        assert_eq!(cmp.not_archived, ["https://h/A/M.zip"]);
    }

    #[test]
    fn a_broken_zip_or_record_says_where() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        std::fs::write(f.archive.path_for(&pair.zip), b"PK\x03\x04 broken").unwrap();
        let e = compare(&f.archive, pair, None, Path::new("/tmp")).unwrap_err();
        assert_eq!(e.diagnostic().code, "zip.open_failed");
        // The directory lists an entry whose header is not there.
        std::fs::write(
            f.archive.path_for(&pair.zip),
            crate::testzip::bad_second_header(),
        )
        .unwrap();
        let e = compare(&f.archive, pair, None, Path::new("/tmp")).unwrap_err();
        assert_eq!(e.diagnostic().code, "zip.open_failed");
        // A zip without its sidecar is not removed.
        std::fs::remove_file(f.archive.path_for(&pair.zip)).unwrap();
        let f2 = fixture(false);
        let pair2 = &pairs(&f2.mirror, &f2.archive, true).unwrap()[0];
        let cmp = compare(&f2.archive, pair2, None, &f2.mirror.tmp_dir()).unwrap();
        std::fs::remove_file(Archive::record_path(&f2.archive.path_for(&pair2.zip))).unwrap();
        let e = remove(&f2.mirror, &f2.archive, &cmp, crate::stamp::AEST).unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.not_same");
        // A record of removals that is a folder cannot be read.
        std::fs::create_dir_all(f.mirror.removed_path()).unwrap();
        assert_eq!(
            f.mirror.removed().unwrap_err().diagnostic().code,
            "mirror.io"
        );
        std::fs::remove_dir(f.mirror.removed_path()).unwrap();
        std::fs::write(f.mirror.removed_path(), "{}\n").unwrap();
        assert_eq!(
            f.mirror.removed().unwrap_err().diagnostic().code,
            "mirror.record_corrupt"
        );
        std::fs::remove_file(f.mirror.done_path()).unwrap();
        assert_eq!(
            pairs(&f.mirror, &f.archive, true)
                .unwrap_err()
                .diagnostic()
                .code,
            "mirror.crawl_incomplete"
        );
    }

    #[test]
    fn a_big_zip_inside_is_spooled_to_list_it() {
        let dir = tempfile::tempdir().unwrap();
        let inner = zip_of(&[("X.CSV", b"x")], zip::CompressionMethod::Stored);
        let outer = zip_of(&[("x.zip", &inner)], zip::CompressionMethod::Deflated);
        let mut z = ZipArchive::new(Cursor::new(outer)).unwrap();
        let a = list_entry(&mut z, 0, dir.path(), NESTED_IN_MEMORY).unwrap();
        let b = list_entry(&mut z, 0, &dir.path().join("spool"), 0).unwrap();
        assert_eq!(a["X.CSV"].0, 1);
        assert_eq!(a, b);
        assert_eq!(percent_decoded("a%23b%2"), "a#b%2");
    }

    #[test]
    fn a_zip_ruled_on_from_its_listing_is_never_downloaded() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        let zip_path = f.archive.path_for(&pair.zip);
        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(Archive::record_path(&zip_path)).unwrap();
        let entry = |name: &str, bytes: &[u8]| Entry {
            name: name.into(),
            size: bytes.len() as u64,
            compressed: bytes.len() as u64,
            crc32: crc32fast::hash(bytes),
            method: 0,
            modified: "m".into(),
            modified_at: None,
        };
        let t1 = std::fs::read(f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip")).unwrap();
        let t2 = std::fs::read(f.archive.path_for("https://h/A/M/S/DATA/T2.zip")).unwrap();
        let tmp = f.mirror.tmp_dir();
        let mut listing = vec![
            entry("M/S/DATA/", b""),
            entry("M/S/DATA/T1#1.zip", &t1),
            entry("M/S/DATA/T2.zip", &t2),
        ];
        let cmp = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(
            (cmp.verdict, cmp.from_listing, cmp.same),
            (Verdict::Same, true, 2)
        );
        // Not byte for byte: only the zip itself could tell more.
        listing[2] = entry("M/S/DATA/T2.zip", b"other bytes");
        let unsure = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(unsure.verdict, Verdict::NeedsDownload);
        assert_eq!(unsure.unsure, ["S/DATA/T2.zip"]);
        assert!(remove(&f.mirror, &f.archive, &unsure, crate::stamp::AEST).is_err());
        // A file the folder lacks settles it.
        listing.push(entry("M/README.txt", b"r"));
        let differs = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(differs.verdict, Verdict::Differs);

        let r = remove(&f.mirror, &f.archive, &cmp, crate::stamp::AEST).unwrap();
        assert_eq!(
            (r.downloaded, r.sha256.as_deref(), r.size),
            (false, None, pair.zip_size)
        );
        let back = &f.mirror.removed().unwrap()[&pair.zip];
        assert!(!back.downloaded);
        assert_eq!(back.removed_at, r.removed_at);
        crate::stamp::check::assert_aware_lines(
            &std::fs::read_to_string(f.mirror.removed_path()).unwrap(),
        );
        // A record written before this field existed was of a downloaded zip.
        std::fs::write(
            f.mirror.removed_path(),
            "{\"url\":\"u\",\"size\":1,\"sha256\":\"s\",\"same_as\":\"f\",\"removed_at\":\"2026-10-15T00:59:33Z\"}\n",
        )
        .unwrap();
        assert!(f.mirror.removed().unwrap()["u"].downloaded);
    }

    #[test]
    fn windows_names_and_a_folder_only_partly_downloaded() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        let t1 = std::fs::read(f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip")).unwrap();
        // The zip holds only T1, made on Windows; the folder's other
        // files need not be downloaded to rule on it.
        let z = crate::testzip::windows(&[("M|S|DATA|T1#1.zip", &t1), ("M|S|", b"")]);
        let zip_path = f.archive.path_for(&pair.zip);
        std::fs::write(&zip_path, &z).unwrap();
        std::fs::remove_file(f.archive.path_for("https://h/A/M/S/DATA/T2.zip")).unwrap();
        let tmp = f.mirror.tmp_dir();
        let cmp = compare(&f.archive, pair, None, &tmp).unwrap();
        assert_eq!((cmp.verdict, cmp.same), (Verdict::Same, 1), "{cmp:?}");
        // From its listing too, names as Windows wrote them.
        std::fs::remove_file(&zip_path).unwrap();
        let listing = [
            Entry {
                name: "M\\S\\DATA\\T1#1.zip".into(),
                size: t1.len() as u64,
                compressed: t1.len() as u64,
                crc32: crc32fast::hash(&t1),
                method: 0,
                modified: "m".into(),
                modified_at: None,
            },
            Entry {
                name: "M\\S\\".into(),
                size: 0,
                compressed: 0,
                crc32: 0,
                method: 0,
                modified: "m".into(),
                modified_at: None,
            },
        ];
        let cmp = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!((cmp.verdict, cmp.same), (Verdict::Same, 1), "{cmp:?}");
        // A file of the zip whose folder file is not downloaded: wait.
        let listing = [Entry {
            name: "M/S/DATA/T2.zip".into(),
            ..listing[0].clone()
        }];
        let cmp = compare(&f.archive, pair, Some(&listing), &tmp).unwrap();
        assert_eq!(cmp.verdict, Verdict::NotArchived);
        assert_eq!(cmp.not_archived, ["S/DATA/T2.zip"]);
    }

    #[test]
    fn a_folder_file_is_read_once_its_crc_kept_in_its_sidecar() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        let tmp = f.mirror.tmp_dir();
        let t1 = f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip");
        assert!(Archive::read(&t1).unwrap().unwrap().content.crc32.is_none());
        compare(&f.archive, pair, None, &tmp).unwrap();
        let crc = Archive::read(&t1).unwrap().unwrap().content.crc32.unwrap();
        assert_eq!(crc, crc32fast::hash(&std::fs::read(&t1).unwrap()));
        // The kept CRC is used: bytes changed on disk at the same size go
        // unnoticed by compare (verify reads them again).
        let mut bytes = std::fs::read(&t1).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&t1, &bytes).unwrap();
        let cmp = compare(&f.archive, pair, None, &tmp).unwrap();
        assert_eq!(cmp.verdict, Verdict::Same);
        // A file whose sidecar is of another size is read again.
        bytes.push(0);
        std::fs::write(&t1, &bytes).unwrap();
        let cmp = compare(&f.archive, pair, None, &tmp).unwrap();
        assert_eq!(cmp.verdict, Verdict::Differs);
    }

    #[test]
    fn verify_reads_the_folder_again_against_the_kept_listing() {
        let f = fixture(false);
        let pair = &pairs(&f.mirror, &f.archive, true).unwrap()[0];
        let tmp = f.mirror.tmp_dir();
        assert!(verify(&f.mirror, &f.archive).unwrap().is_empty());
        let cmp = compare(&f.archive, pair, None, &tmp).unwrap();
        remove(&f.mirror, &f.archive, &cmp, crate::stamp::AEST).unwrap();
        let v = &verify(&f.mirror, &f.archive).unwrap()[0];
        assert!(v.holds(), "{v:?}");
        assert_eq!(v.checked, 2);
        // A sidecar without a recorded CRC: the zip's is what was seen.
        let t1 = f.archive.path_for("https://h/A/M/S/DATA/T1%231.zip");
        let mut sc = Archive::read(&t1).unwrap().unwrap();
        sc.content.crc32 = None;
        Archive::write(&t1, &sc).unwrap();
        assert!(verify(&f.mirror, &f.archive).unwrap()[0].holds());
        let mut bytes = std::fs::read(&t1).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&t1, &bytes).unwrap();
        std::fs::remove_file(f.archive.path_for("https://h/A/M/S/DATA/T2.zip")).unwrap();
        let v = &verify(&f.mirror, &f.archive).unwrap()[0];
        assert!(!v.holds());
        assert_eq!(v.changed, ["S/DATA/T1#1.zip"]);
        assert_eq!(v.missing, ["S/DATA/T2.zip"]);
        // An entry whose folder file is not even listed, and a removal
        // without a kept listing.
        let mut peeks = crate::peek::read_peeks(&f.mirror.peek_path()).unwrap();
        peeks[0].entries.as_mut().unwrap().push(Entry {
            name: "M\\elsewhere.txt".into(),
            size: 1,
            compressed: 1,
            crc32: 1,
            method: 0,
            modified: "m".into(),
            modified_at: None,
        });
        let lines: String = peeks
            .iter()
            .map(|p| serde_json::to_string(&p.clone().derived()).unwrap() + "\n")
            .collect();
        std::fs::write(f.mirror.peek_path(), lines).unwrap();
        let v = &verify(&f.mirror, &f.archive).unwrap()[0];
        assert!(v.missing.contains(&"M/elsewhere.txt".to_owned()));
        std::fs::remove_file(f.mirror.peek_path()).unwrap();
        let v = &verify(&f.mirror, &f.archive).unwrap()[0];
        assert!(v.no_listing && !v.holds());
    }

    #[test]
    fn a_kept_listing_of_a_file_that_is_not_a_zip_or_cannot_be_read() {
        let f = fixture(false);
        let dir = f.mirror.dir();
        let text = dir.join("text.zip");
        std::fs::write(&text, b"not a zip").unwrap();
        crate::peek::keep_listing(&f.mirror, "https://h/t.zip", &text, crate::stamp::AEST).unwrap();
        let p = crate::peek::read_peeks(&f.mirror.peek_path()).unwrap();
        assert_eq!(p[0].error.as_ref().unwrap().code, "zip.not_a_zip");
        let e = crate::peek::keep_listing(
            &f.mirror,
            "https://h/u.zip",
            &dir.join("none"),
            crate::stamp::AEST,
        )
        .unwrap_err();
        assert_eq!(e.diagnostic().code, "mirror.io");
    }
}
