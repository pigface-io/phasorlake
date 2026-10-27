//! Moving an archive kept with each URL as its path to the layout of
//! `phasor_provenance::layout`: one folder per report folder, the
//! listing segment (`CURRENT`, `ARCHIVE`, `current`, `previous`) dropped.
//!
//! Every file with a sidecar in a folder under a listing moves, with its
//! sidecar, to the path of its sidecar's URL: a folder whose place is free
//! by one rename, else file by file, copied and held to its sha256. On
//! exFAT a rename into a large folder, and an unlink from one, take
//! seconds, so a folder left holding only copied sources is set aside
//! whole under `state/tmp/trash/`, to be deleted when nothing waits on the
//! disk; any other folder has its copied sources removed. A file already at the target with the same bytes keeps
//! its place, and the moving file's URL joins its sidecar's `also`
//! (`how = "same"`); the moving file is dropped. A target with other bytes
//! is never written over: the moving file is kept beside it as
//! `{name}@{at}`, `at` the time the listing gave it, else when it was
//! downloaded. The journals of a folder's two listings are merged into
//! its one journal, and `held.ndjson` lines whose file is now a version
//! are rewritten.
//!
//! Every step is logged to `state/migrations/relayout.ndjson` before it is taken, so
//! a run cut off is finished by the next. A second run finds nothing to
//! move.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crawl::Mirror;
use crate::download::HeldEntry;
use crate::error::MirrorError;
use crate::net::{Address, Archive, DirNames, Provenance, Sha256, Via, is_apple_double};
use crate::stamp::Stamp;

/// What became of one archived file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Moved to the path of its URL.
    Move,
    /// Moved beside a file of other bytes at that path, as a version.
    Version,
    /// The same bytes as the file at that path: its URL added there, the
    /// file dropped.
    Same,
    /// A whole folder moved to its folder in the layout, which was not
    /// there: one rename, whatever it holds.
    Folder,
}

/// One step, as logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// When it was taken (UTC).
    pub at: Stamp,
    /// The file's URL.
    pub url: String,
    /// Where it was.
    pub from: PathBuf,
    /// Where it goes, or the file it is the same as.
    pub to: PathBuf,
    /// What is done.
    pub action: Action,
}

/// What a relayout did, or with `dry_run` would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RelayoutSummary {
    /// Files moved to the path of their URL.
    pub moved: usize,
    /// Files kept beside another URL's file of other bytes.
    pub versions: usize,
    /// Files dropped as the same bytes as the file at their path.
    pub same: usize,
    /// Folders moved whole, their folder in the layout not there yet.
    pub folders_moved: usize,
    /// Files whose sidecar does not read: left where they are.
    pub unreadable: Vec<PathBuf>,
    /// Journals merged into their folder's journal.
    pub journals: usize,
    /// `held.ndjson` lines rewritten.
    pub held: usize,
    /// Empty folders of the old layout removed.
    pub folders_removed: usize,
    /// Folders left holding only sources already copied, set aside whole
    /// under `state/tmp/trash/` to delete: on exFAT an unlink in a folder
    /// of tens of thousands of entries takes seconds.
    pub folders_set_aside: usize,
    /// Folders whose few files were copied into a folder of many that then
    /// took their place, the folder of few set aside.
    pub folders_swapped: usize,
    /// Whether nothing was changed.
    pub dry_run: bool,
}

impl Mirror {
    /// The log of relayout steps.
    pub fn relayout_path(&self) -> PathBuf {
        self.migrations_dir().join("relayout.ndjson")
    }
}

/// Moves the archive (and the mirror's journals) to the layout of
/// `phasor_provenance::layout`. With `dry_run`, only says what it would
/// do. `on_step` hears every step, before it is taken.
pub fn relayout(
    mirror: &Mirror,
    archive: &Archive,
    dry_run: bool,
    mut on_step: impl FnMut(&Step),
) -> Result<RelayoutSummary, MirrorError> {
    let mut summary = RelayoutSummary {
        dry_run,
        ..RelayoutSummary::default()
    };
    if !dry_run {
        finish_last(mirror)?;
    }
    let (dirs, stale) = stale_dirs(archive);
    // Targets taken in this run, by sha256: what a dry run would have put
    // there, so it plans as a run would.
    let mut placed: HashMap<PathBuf, Sha256> = HashMap::new();
    let mut versions: HashMap<String, Option<String>> = HashMap::new();
    // The names in each folder a file may go to, read once: on exFAT a
    // look for a name that is not there reads the whole folder.
    let mut names = DirNames::default();
    let mut whole: Vec<PathBuf> = Vec::new();
    let mut whole_to: Vec<PathBuf> = Vec::new();
    // Files copied to their place whose sources go once the copies are on
    // the disk: one sync for a batch, not one for each file.
    let mut copied: Vec<(PathBuf, PathBuf)> = Vec::new();
    // What the steps taken wrote, to hand to the disk before any source goes.
    let mut written: Vec<PathBuf> = Vec::new();
    // Steps waiting to be taken together, and their bytes.
    let mut pending: Vec<Pending> = Vec::new();
    let mut bytes = 0u64;
    for dir in &stale {
        if whole.iter().any(|w| dir.starts_with(w)) {
            continue;
        }
        // A folder goes whole only where nothing has gone yet, this run
        // included: a dry run makes no folder.
        let free = |to: &Path| !whole_to.iter().any(|t| to.starts_with(t));
        if let Some(step) = folder_step(archive, dir).filter(|s| free(&s.to)) {
            on_step(&step);
            summary.folders_moved += 1;
            if !dry_run {
                log(mirror, &step, true)?;
                let parent = step.to.parent().expect("a folder of the layout is in one");
                std::fs::create_dir_all(parent).map_err(MirrorError::io(parent))?;
                std::fs::rename(&step.from, &step.to).map_err(MirrorError::io(&step.to))?;
            }
            if dry_run {
                // Where the folder's files would be, as a run would find
                // them there.
                for file in Archive::walk(dir).unwrap_or_default() {
                    if let (Ok(rel), Ok(Some(p))) = (file.strip_prefix(dir), Archive::read(&file)) {
                        placed.insert(step.to.join(rel), p.content.sha256);
                    }
                }
            }
            whole.push(dir.clone());
            whole_to.push(step.to);
            continue;
        }
        if !dry_run {
            let to = folder_step_to(archive, dir).filter(|t| free(t));
            let found = to.and_then(|t| swappable(dir, &t).map(|n| (t, n)));
            if let Some((to, (n, both))) = found
                && swap(mirror, archive, dir, &to, &both)?
            {
                summary.moved += n - both.len();
                summary.folders_swapped += 1;
                whole.push(dir.clone());
                whole_to.push(to);
                continue;
            }
        }
        // Each entry with its type as the folder listing gives it: on
        // exFAT a look at one name reads the whole folder.
        let mut files: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| (e.path(), e.file_type().is_ok_and(|t| t.is_file())))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        let here: std::collections::HashSet<String> = files
            .iter()
            .filter_map(|(p, _)| Some(p.file_name()?.to_string_lossy().into_owned()))
            .collect();
        for (path, is_file) in files {
            if !is_file || !is_data_file(&path) {
                continue;
            }
            let Some(record) = Archive::record_where(&path, |n| here.contains(n)) else {
                continue;
            };
            // Read once, kept for the copy: each look for a name in a
            // folder too large for the cache reads it from the disk.
            let Some((sidecar, kept)) = read_held(&record, None)
                .ok()
                .and_then(|b| Some((Provenance::from_json(&b).ok()?, b)))
            else {
                summary.unreadable.push(path);
                continue;
            };
            let Some(step) = plan(archive, &path, &sidecar, &placed, &mut names) else {
                continue;
            };
            on_step(&step);
            match step.action {
                Action::Move | Action::Folder => summary.moved += 1,
                Action::Version => summary.versions += 1,
                Action::Same => summary.same += 1,
            }
            if step.action != Action::Same {
                placed.insert(step.to.clone(), sidecar.content.sha256);
                let plain = archive.path_for(&step.url);
                let name = (step.to != plain)
                    .then(|| {
                        step.to
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                    })
                    .flatten();
                versions.insert(step.url.clone(), name);
            }
            if !dry_run {
                bytes += sidecar.content.size;
                pending.push((step, sidecar, record, kept));
                if pending.len() >= BATCH || bytes >= CHUNK_BYTES {
                    copied.extend(take(mirror, std::mem::take(&mut pending), &mut written)?);
                    bytes = 0;
                    to_disk(&mut written)?;
                }
            }
        }
        copied.extend(take(mirror, std::mem::take(&mut pending), &mut written)?);
        if !copied.is_empty() {
            to_disk(&mut written)?;
            if drained(&here, &copied) {
                set_aside(mirror, archive, dir)?;
                copied.clear();
                summary.folders_set_aside += 1;
            } else {
                settle(&mut copied)?;
            }
        }
    }
    summary.journals = merge_journals(mirror, dry_run)?;
    if !dry_run {
        summary.held = rewrite_held(mirror, &versions)?;
        // Deepest first, so a folder emptied of folders goes too.
        for dir in dirs.iter().rev().filter(|d| stale.contains(d)) {
            if std::fs::remove_dir(dir).is_ok() {
                summary.folders_removed += 1;
            }
        }
    }
    Ok(summary)
}

/// Whether the file at `path` is a data file a sidecar may describe: not
/// a sidecar, a download in progress, a sidecar being written, or an
/// AppleDouble file.
fn is_data_file(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    !is_apple_double(path)
        && !name.ends_with(".json")
        && !name.ends_with(".part")
        && !name.ends_with(".tmp")
}

/// Every folder under the archive, parents before children, and those of
/// them whose files belong elsewhere under the layout: a folder under a
/// listing segment. Only those folders' sidecars are read.
fn stale_dirs(archive: &Archive) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut stack = vec![archive.root().to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut subs: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| !is_apple_double(p))
            .collect();
        subs.sort();
        stack.extend(subs.into_iter().rev());
        dirs.push(d);
    }
    let stale = dirs
        .iter()
        .filter(|d| {
            let rel = d.strip_prefix(archive.root()).unwrap_or(Path::new(""));
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            // A file in it, as a URL: kept elsewhere when the layout drops
            // a segment of its path.
            let url = format!("https://{}/_", rel.join("/"));
            !rel.is_empty() && archive.path_for(&url) != archive.url_path(&url)
        })
        .cloned()
        .collect();
    (dirs, stale)
}

/// The move of the whole folder `dir` to its folder in the layout, when
/// that folder is not there yet: everything in `dir` belongs there, so one
/// rename moves it all.
fn folder_step(archive: &Archive, dir: &Path) -> Option<Step> {
    let to = folder_step_to(archive, dir)?;
    let url = folder_url(archive, dir)?;
    std::fs::symlink_metadata(&to).is_err().then(|| Step {
        at: Stamp::now(),
        url,
        from: dir.to_path_buf(),
        to,
        action: Action::Folder,
    })
}

/// The URL of the folder `dir` of the archive, ending in `/`.
fn folder_url(archive: &Archive, dir: &Path) -> Option<String> {
    let rel = dir.strip_prefix(archive.root()).ok()?;
    let rel: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(format!("https://{}/", rel.join("/")))
}

/// The folder of the layout that the files of `dir` belong in, when that
/// is not `dir`.
fn folder_step_to(archive: &Archive, dir: &Path) -> Option<PathBuf> {
    let url = folder_url(archive, dir)?;
    let to = archive.path_for(&format!("{url}_")).parent()?.to_path_buf();
    (to != dir).then_some(to)
}

/// The names in `dir` by kind: data files, records and folders, less
/// AppleDouble files; none when `dir` does not read.
fn listing(dir: &Path) -> Option<(Vec<String>, Vec<String>, usize)> {
    let (mut data, mut records, mut folders) = (Vec::new(), Vec::new(), 0);
    for e in std::fs::read_dir(dir).ok()?.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("._") {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            folders += 1;
        } else if name.ends_with(".json") {
            records.push(name);
        } else {
            data.push(name);
        }
    }
    Some((data, records, folders))
}

/// When `dir` can be swapped for `to`, the folder its files belong in: how
/// many files `dir` holds, and those of them also in `to` (copies a run
/// cut off left there). `to` holds fewer files than `dir`, and no folder;
/// every file of `to` has a record, and so has every file of `dir` not in
/// `to`; nothing else is in either (no download or record in progress),
/// and no record of `to` is in `dir` but those of the files in both.
fn swappable(dir: &Path, to: &Path) -> Option<(usize, Vec<String>)> {
    let (data, records, _) = listing(dir)?;
    let (to_data, to_records, to_folders) = listing(to)?;
    let set = |v: &[String]| -> std::collections::HashSet<String> { v.iter().cloned().collect() };
    let (to_set, records_set, to_records_set) = (set(&to_data), set(&records), set(&to_records));
    let has_record = |d: &str, records: &std::collections::HashSet<String>| {
        records.contains(&format!("{d}.json")) || records.contains(&format!("{d}.provenance.json"))
    };
    let of = |r: &str| {
        r.strip_suffix(".provenance.json")
            .or_else(|| r.strip_suffix(".json"))
            .map(str::to_owned)
    };
    let both: Vec<String> = data
        .iter()
        .filter(|d| to_set.contains(*d))
        .cloned()
        .collect();
    let ok = to_folders == 0
        && to_data.len() < data.len()
        && to_records.len() == to_data.len()
        && to_data
            .iter()
            .all(|d| is_data_file(Path::new(d)) && has_record(d, &to_records_set))
        && data.iter().all(|d| {
            is_data_file(Path::new(d)) && (to_set.contains(d) || has_record(d, &records_set))
        })
        && records
            .iter()
            .all(|r| of(r).is_some_and(|d| data.contains(&d)))
        && to_records
            .iter()
            .all(|r| !records_set.contains(r) || of(r).is_some_and(|d| to_set.contains(&d)));
    ok.then_some((data.len(), both))
}

/// Swaps the folder `dir` for `to`, the folder its files belong in, which
/// holds fewer: `to`'s files and records are renamed into `dir` (a rename
/// between folders takes seconds there whatever the file's size, and
/// copies nothing), `to` is removed, and `dir` is renamed to `to`. A few
/// renames, where moving `dir`'s files one by one would create each in a
/// folder that grows, reading it whole each time. A file in both, a copy
/// a run cut off left in `dir`, must hash to the record in `to`, else
/// nothing is swapped; it is set aside under `state/tmp/trash/` with its
/// record. A run cut off part way finds `to` holding fewer still, and
/// goes on; one cut off after `to` went moves `dir` whole. Whether it was
/// swapped.
fn swap(
    mirror: &Mirror,
    archive: &Archive,
    dir: &Path,
    to: &Path,
    both: &[String],
) -> Result<bool, MirrorError> {
    let (data, _, _) = listing(to).unwrap_or_default();
    let names = |d: &Path| -> Result<std::collections::HashSet<String>, MirrorError> {
        Ok(std::fs::read_dir(d)
            .map_err(MirrorError::io(d))?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect())
    };
    let (here, mine) = (names(to)?, names(dir)?);
    let record_of = |from: &Path, names: &std::collections::HashSet<String>| {
        Archive::record_where(from, |n| names.contains(n))
    };
    let mut copies = Vec::with_capacity(both.len());
    for name in both {
        let record =
            record_of(&to.join(name), &here).expect("a swappable folder's files have records");
        let sidecar = Archive::read_record(&record)?;
        if read_held(&dir.join(name), Some(&sidecar.content.sha256)).is_err() {
            return Ok(false);
        }
        copies.push((name, sidecar));
    }
    for (name, sidecar) in copies {
        let copy = dir.join(name);
        let step = Step {
            at: Stamp::now(),
            url: sidecar.url().to_owned(),
            from: copy.clone(),
            to: to.join(name),
            action: Action::Same,
        };
        log(mirror, &step, true)?;
        if let Some(record) = record_of(&copy, &mine) {
            set_aside(mirror, archive, &record)?;
        }
        set_aside(mirror, archive, &copy)?;
    }
    for name in &data {
        let from = to.join(name);
        let record = record_of(&from, &here).expect("a swappable folder's files have records");
        let sidecar = Archive::read_record(&record)?;
        // Logged where it is, which is where it stays once `dir` is
        // renamed.
        let step = Step {
            at: Stamp::now(),
            url: sidecar.url().to_owned(),
            from: from.clone(),
            to: from.clone(),
            action: Action::Move,
        };
        log(mirror, &step, true)?;
        let into = dir.join(name);
        std::fs::rename(&from, &into).map_err(MirrorError::io(&into))?;
        let record_into = dir.join(record.file_name().expect("a record has a name"));
        std::fs::rename(&record, &record_into).map_err(MirrorError::io(&record_into))?;
    }
    let step = Step {
        at: Stamp::now(),
        url: folder_url(archive, dir).unwrap_or_default(),
        from: dir.to_path_buf(),
        to: to.to_path_buf(),
        action: Action::Folder,
    };
    log(mirror, &step, true)?;
    // Empty but for AppleDouble files, which macOS removes with it.
    std::fs::remove_dir(to).map_err(MirrorError::io(to))?;
    std::fs::rename(dir, to).map_err(MirrorError::io(to))?;
    Ok(true)
}

/// Where the file at `path` goes, and how; none when it is in place.
fn plan(
    archive: &Archive,
    path: &Path,
    sidecar: &Provenance,
    placed: &HashMap<PathBuf, Sha256>,
    names: &mut DirNames,
) -> Option<Step> {
    let base = archive.path_for(sidecar.url());
    let name = path.file_name()?.to_string_lossy().into_owned();
    let base_name = base.file_name()?.to_string_lossy().into_owned();
    // A version keeps its `@{at}`.
    let target = match name.strip_prefix(&format!("{base_name}@")) {
        Some(at) => base.with_file_name(format!("{base_name}@{at}")),
        None => base.clone(),
    };
    if target == path {
        return None;
    }
    let at = sidecar
        .listed(sidecar.url())
        .and_then(|l| l.at)
        .unwrap_or_else(|| sidecar.fetched_at());
    let candidates = [
        target.clone(),
        Archive::version_path(&base, at),
        Archive::version_path(&base, Stamp::now()),
    ];
    let mut action = Action::Move;
    for to in candidates {
        // A file there whose record does not read holds other bytes.
        let held = placed.get(&to).copied().map(Some).or_else(|| {
            names
                .holds(&to)
                .then(|| Archive::read(&to).ok().flatten().map(|s| s.content.sha256))
        });
        match held {
            None => {
                return Some(Step {
                    at: Stamp::now(),
                    url: sidecar.url().to_owned(),
                    from: path.to_path_buf(),
                    to,
                    action,
                });
            }
            Some(Some(sha)) if sha == sidecar.content.sha256 => {
                return Some(Step {
                    at: Stamp::now(),
                    url: sidecar.url().to_owned(),
                    from: path.to_path_buf(),
                    to,
                    action: Action::Same,
                });
            }
            Some(_) => action = Action::Version,
        }
    }
    None
}

/// Copies handed to the disk together: one sync for them all.
const BATCH: usize = if cfg!(test) { 2 } else { 512 };

/// The most bytes read into memory for one batch.
const CHUNK_BYTES: u64 = 256 << 20;

/// Removes the sources of copies already on the disk (the file, then its
/// record). A record gone already, by a run cut off, is passed over.
fn settle(copied: &mut Vec<(PathBuf, PathBuf)>) -> Result<(), MirrorError> {
    for (from, record) in copied.drain(..) {
        std::fs::remove_file(&from).map_err(MirrorError::io(&from))?;
        match std::fs::remove_file(&record) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(MirrorError::io(&record)(e));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether the folder whose names are `here` holds nothing but the
/// sources `copied` and their records, records whose file is gone (a run
/// cut off between the two), and AppleDouble files: no folder, nothing
/// uncopied.
fn drained(here: &std::collections::HashSet<String>, copied: &[(PathBuf, PathBuf)]) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let gone: std::collections::HashSet<String> = copied
        .iter()
        .flat_map(|(from, record)| [name(from), name(record)])
        .flatten()
        .collect();
    here.iter().all(|n| {
        let orphan = n
            .strip_suffix(".provenance.json")
            .or_else(|| n.strip_suffix(".json"))
            .is_some_and(|data| !here.contains(data));
        gone.contains(n) || orphan || n.starts_with("._")
    })
}

/// Moves the folder `dir` whole to `state/tmp/trash/`, under its path in
/// the archive: one rename where removing its files one by one would read
/// the folder again for each.
fn set_aside(mirror: &Mirror, archive: &Archive, dir: &Path) -> Result<(), MirrorError> {
    let rel = dir.strip_prefix(archive.root()).unwrap_or(dir);
    let base = mirror.tmp_dir().join("trash").join(rel);
    let mut to = base.clone();
    let mut n = 1;
    while to.exists() {
        n += 1;
        to = PathBuf::from(format!("{}.{n}", base.display()));
    }
    let parent = to.parent().expect("a folder set aside is in one");
    std::fs::create_dir_all(parent).map_err(MirrorError::io(parent))?;
    std::fs::rename(dir, &to).map_err(MirrorError::io(&to))
}

/// Appends `step` to the log, handed to the disk when `durable` (a batch's
/// sync hands the others).
fn log(mirror: &Mirror, step: &Step, durable: bool) -> Result<(), MirrorError> {
    let path = mirror.relayout_path();
    let dir = mirror.migrations_dir();
    std::fs::create_dir_all(&dir).map_err(MirrorError::io(&dir))?;
    let mut line = serde_json::to_vec(step).expect("a Step always serialises");
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(MirrorError::io(&path))?;
    f.write_all(&line).map_err(MirrorError::io(&path))?;
    if durable {
        crate::net::archive::to_disk(&f).map_err(MirrorError::io(&path))?;
    }
    Ok(())
}

/// The record beside the file at `path`, whichever name it has.
fn record_at(path: &Path) -> Option<PathBuf> {
    let mut old = path.as_os_str().to_owned();
    old.push(phasor_provenance::archive::OLD_RECORD);
    [Archive::record_path(path), PathBuf::from(old)]
        .into_iter()
        .find(|r| r.is_file())
}

/// Moves the record of the file at `from` beside `to`, under the name it
/// has: a rename, the record not read or written again.
fn move_record(from: &Path, to: &Path) -> Result<(), MirrorError> {
    record_at(from).map_or(Ok(()), |record| rename_record(from, &record, to))
}

/// What `record`'s name adds to the name of the file at `from`:
/// `.provenance.json`, or `.json` for format 1.
fn record_suffix(from: &Path, record: &Path) -> String {
    record
        .file_name()
        .and_then(|r| {
            let data = from.file_name()?.to_string_lossy().into_owned();
            Some(r.to_string_lossy()[data.len()..].to_owned())
        })
        .unwrap_or_default()
}

/// Renames `record`, the record of the file at `from`, beside `to` under
/// the same suffix.
fn rename_record(from: &Path, record: &Path, to: &Path) -> Result<(), MirrorError> {
    let mut target = to.as_os_str().to_owned();
    target.push(record_suffix(from, record));
    std::fs::rename(record, &target).map_err(MirrorError::io(Path::new(&target)))
}

/// A file read whole.
type Bytes = Vec<u8>;

/// Reads the file at `from` whole, held to `sha256` when given.
fn read_held(from: &Path, sha256: Option<&Sha256>) -> Result<Bytes, MirrorError> {
    let bytes = std::fs::read(from).map_err(MirrorError::io(from))?;
    let got = Sha256::of(&bytes);
    if let Some(want) = sha256.filter(|w| **w != got) {
        return Err(crate::net::FetchError::Sha256Mismatch {
            path: from.display().to_string(),
            expected: want.hex(),
            observed: got.hex(),
        }
        .into());
    }
    Ok(bytes)
}

/// Writes `bytes` at `to`, in another folder than they came from, without
/// a rename between folders: exFAT under macOS's FSKit takes seconds for
/// one when a folder holds tens of thousands of entries, and each look for
/// a name in a folder too large for the cache reads it from the disk. So
/// the bytes go straight to `to`, with no `.part` and rename: a source
/// goes only after the batch's sync, and a copy cut off by a crash is
/// found by hashing the copies before the set-aside sources are deleted.
/// Handed to the disk when `durable` (else by the caller's sync). The
/// copy is not given the modified time it had: setting one takes most of
/// a second there, and the record keeps when the file was fetched.
fn put(to: &Path, bytes: &Bytes, durable: bool) -> Result<(), MirrorError> {
    use std::io::Write as _;
    let mut dst = std::fs::File::create(to).map_err(MirrorError::io(to))?;
    dst.write_all(bytes).map_err(MirrorError::io(to))?;
    if durable {
        crate::net::archive::to_disk(&dst).map_err(MirrorError::io(to))?;
    }
    Ok(())
}

/// Hands what a batch wrote to the disk, and forgets it.
fn to_disk(written: &mut Vec<PathBuf>) -> Result<(), MirrorError> {
    crate::net::archive::batch_to_disk(written.iter().map(PathBuf::as_path))
        .map_err(|(p, e)| MirrorError::io(&p)(e))?;
    written.clear();
    Ok(())
}

/// A step waiting to be taken: the step, the moving file's record, the
/// record's path and its bytes as read.
type Pending = (Step, Provenance, PathBuf, Bytes);

/// Removes every record beside `path`.
fn drop_records(path: &Path) -> Result<(), MirrorError> {
    while let Some(r) = record_at(path) {
        std::fs::remove_file(&r).map_err(MirrorError::io(&r))?;
    }
    Ok(())
}

/// Steps taken together, each logged first, every source read before
/// any target is written: on exFAT a look for a name reads its whole
/// folder, which stays in the cache only while that folder is the one in
/// use, so going back and forth between a large source folder and a large
/// target folder reads each from the disk every time. A file moved is
/// copied to its place, held to its record's sha256; a file the same as
/// the one there adds its addresses to that one's record. Each source and
/// record is returned, to go once the batch is on the disk; what was
/// written is added to `written`, to be handed to it.
fn take(
    mirror: &Mirror,
    steps: Vec<Pending>,
    written: &mut Vec<PathBuf>,
) -> Result<Vec<(PathBuf, PathBuf)>, MirrorError> {
    for (step, ..) in &steps {
        log(mirror, step, false)?;
    }
    if !steps.is_empty() {
        written.push(mirror.relayout_path());
    }
    let mut read = Vec::with_capacity(steps.len());
    for (step, sidecar, ..) in &steps {
        read.push(match step.action {
            Action::Same => None,
            _ => Some(read_held(&step.from, Some(&sidecar.content.sha256))?),
        });
    }
    let mut done = Vec::with_capacity(steps.len());
    for ((step, sidecar, record, kept), bytes) in steps.into_iter().zip(read) {
        match bytes {
            Some(data) => {
                let dir = step.to.parent().expect("an archived path is in a folder");
                std::fs::create_dir_all(dir).map_err(MirrorError::io(dir))?;
                let mut record_to = step.to.as_os_str().to_owned();
                record_to.push(record_suffix(&step.from, &record));
                put(&step.to, &data, false)?;
                put(Path::new(&record_to), &kept, false)?;
                written.extend([step.to.clone(), record_to.into()]);
            }
            None => {
                same(&step, sidecar)?;
                written.push(Archive::record_path(&step.to));
            }
        }
        done.push((step.from, record));
    }
    Ok(done)
}

/// Adds the addresses of a file dropped as the same bytes as the file at
/// its target to the target's record, handed to the disk with the batch.
fn same(step: &Step, sidecar: Provenance) -> Result<(), MirrorError> {
    if let Some(mut kept) = Archive::read(&step.to)? {
        let url = sidecar.url().to_owned();
        let mut changed = kept.add(Address {
            via: Via::Same {
                listed: sidecar.listed(&url),
            },
            url,
            seen: Stamp::now(),
            gone: None,
        });
        for a in sidecar.addresses.into_iter().skip(1) {
            changed |= kept.add(a);
        }
        if changed {
            Archive::write_unsynced(&step.to, &kept)?;
        }
    }
    Ok(())
}

/// Finishes the step a run before logged last, when it was cut off
/// between moving a file and its record.
fn finish_last(mirror: &Mirror) -> Result<(), MirrorError> {
    let path = mirror.relayout_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let Some(step) = text
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Step>(l).ok())
    else {
        return Ok(());
    };
    if record_at(&step.from).is_none() {
        return Ok(());
    }
    if step.from.exists() {
        return Ok(());
    }
    if matches!(step.action, Action::Move | Action::Version) {
        return move_record(&step.from, &step.to);
    }
    drop_records(&step.from)
}

/// Merges each journal kept under a listing (`journal/`, before `state/`)
/// into its folder's events, lines in time order. How many journals were (or would be) merged.
fn merge_journals(mirror: &Mirror, dry_run: bool) -> Result<usize, MirrorError> {
    let journals = Archive::new(mirror.dir().join("journal"));
    let (_, stale) = stale_dirs(&journals);
    let mut n = 0;
    for dir in stale {
        let old = dir.join("changes.ndjson");
        if !old.is_file() {
            continue;
        }
        let rel = dir
            .strip_prefix(journals.root())
            .expect("a journal is under the journals");
        let url = format!("https://{}/", rel.to_string_lossy());
        let new = mirror.journal_path(&url);
        n += 1;
        if dry_run {
            continue;
        }
        let read = |p: &Path| std::fs::read_to_string(p).map_err(MirrorError::io(p));
        let a = match new.is_file() {
            true => read(&new)?,
            false => String::new(),
        };
        let b = read(&old)?;
        let mut lines: Vec<&str> = a
            .lines()
            .chain(b.lines())
            .filter(|l| !l.is_empty())
            .collect();
        let seen = |l: &str| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| {
                    v.get("at")
                        .or_else(|| v.get("seen_at"))?
                        .as_str()
                        .and_then(|s| Stamp::parse(s).ok())
                })
                .map_or(i64::MAX, Stamp::unix)
        };
        lines.sort_by_key(|l| seen(l));
        let mut text = lines.join("\n");
        text.push('\n');
        let parent = new.parent().expect("a journal is in a folder");
        std::fs::create_dir_all(parent).map_err(MirrorError::io(parent))?;
        crate::crawl::write_atomic(&new, text.as_bytes())?;
        std::fs::remove_file(&old).map_err(MirrorError::io(&old))?;
        let mut d = Some(dir.as_path());
        while let Some(x) = d.filter(|x| *x != journals.root()) {
            if std::fs::remove_dir(x).is_err() {
                break;
            }
            d = x.parent();
        }
    }
    Ok(n)
}

/// Rewrites the `held.ndjson` lines of each URL whose file name changed
/// (`versions`: the new name, none for the plain one). How many lines.
fn rewrite_held(
    mirror: &Mirror,
    versions: &HashMap<String, Option<String>>,
) -> Result<usize, MirrorError> {
    let path = mirror.held_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(0);
    };
    let mut n = 0;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match serde_json::from_str::<HeldEntry>(line) {
            Ok(mut h) if versions.get(&h.url).is_some_and(|v| *v != h.file) => {
                h.file.clone_from(&versions[&h.url]);
                out.push_str(&serde_json::to_string(&h).expect("a Held line serialises"));
                n += 1;
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    if n > 0 {
        crate::crawl::write_atomic(&path, out.as_bytes())?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Address;
    use crate::net::archive::sha256_hex;

    const CUR: &str = "https://www.nemweb.com.au/Reports/CURRENT/GSH/B/";
    const ARC: &str = "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/B/";

    /// A file kept the old way, at its URL's path, with its sidecar.
    fn old(archive: &Archive, url: &str, name: Option<&str>, bytes: &[u8]) -> PathBuf {
        let mut p = archive.url_path(url);
        if let Some(n) = name {
            p.set_file_name(n);
        }
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        // As the mirror wrote records before the relayout: format 1.
        let v1 = serde_json::json!({
            "url": url, "size": bytes.len(), "sha256": sha256_hex(bytes),
            "fetched_at": "2026-10-05T00:00:00Z",
        });
        std::fs::write(format!("{}.json", p.display()), v1.to_string()).unwrap();
        p
    }

    #[test]
    fn both_listings_move_to_one_folder_and_nothing_is_written_over() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        old(&archive, &format!("{CUR}a.csv"), None, b"a");
        old(&archive, &format!("{ARC}s.csv"), None, b"same");
        let s_cur = old(&archive, &format!("{CUR}s.csv"), None, b"same");
        let same = |url: &str| Address {
            url: url.to_owned(),
            seen: Stamp::now(),
            gone: None,
            via: Via::Same { listed: None },
        };
        crate::net::archive::add_address(&s_cur, same("https://h/elsewhere/s.csv"), true).unwrap();
        crate::net::archive::add_address(&s_cur, same(&format!("{ARC}s.csv")), false).unwrap();
        // A file whose URL is kept where it already is, under a listing.
        let odd = format!("{CUR}z.csv").replace("/CURRENT/", "/CURRENT/CURRENT/");
        let in_place = old(&archive, &odd, None, b"z");
        let z = archive.url_path(&format!("{CUR}z.csv"));
        std::fs::rename(&in_place, &z).unwrap();
        std::fs::rename(
            format!("{}.json", in_place.display()),
            format!("{}.json", z.display()),
        )
        .unwrap();
        old(&archive, &format!("{ARC}m.zip"), None, b"final");
        old(&archive, &format!("{CUR}m.zip"), None, b"partial");
        old(
            &archive,
            &format!("{CUR}v.csv"),
            Some("v.csv@20261012T104500+1000"),
            b"v2",
        );
        old(
            &archive,
            "https://www.nemweb.com.au/Data_Archive/X/d.zip",
            None,
            b"d",
        );
        let bad = archive.url_path(&format!("{CUR}bad.csv"));
        std::fs::write(&bad, b"b").unwrap();
        std::fs::write(Archive::record_path(&bad), b"{").unwrap();
        let cur_dir = archive.url_path(CUR);
        std::fs::write(cur_dir.join("._a.csv"), b"apple").unwrap();
        std::fs::write(cur_dir.join("nosidecar.csv"), b"n").unwrap();
        // Journals of both listings, and a held line of the moved version.
        let journals = Archive::new(mirror.dir().join("journal"));
        for (url, at) in [(CUR, "2026-10-02T00:00:00Z"), (ARC, "2026-10-01T00:00:00Z")] {
            let p = journals.url_path(url).join("changes.ndjson");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(
                &p,
                format!("{{\"seen_at\":\"{at}\",\"url\":\"{url}x\",\"event\":\"new\"}}\n\n"),
            )
            .unwrap();
        }
        let held = format!(
            "{{\"url\":\"{CUR}m.zip\",\"size\":7}}\n{{\"url\":\"{CUR}a.csv\",\"size\":1}}\nnot json\n"
        );
        std::fs::create_dir_all(mirror.cache_dir()).unwrap();
        std::fs::write(mirror.held_path(), &held).unwrap();

        let mut steps = Vec::new();
        let dry = relayout(&mirror, &archive, true, |s| steps.push(s.clone())).unwrap();
        assert!(dry.dry_run);
        // The ARCHIVE folder goes whole; CURRENT's files go one by one
        // into it.
        assert_eq!(
            (
                dry.folders_moved,
                dry.moved,
                dry.versions,
                dry.same,
                dry.journals
            ),
            (1, 2, 1, 1, 2),
            "{steps:?}"
        );
        assert!(
            archive.url_path(&format!("{CUR}a.csv")).exists(),
            "a dry run moves nothing"
        );
        assert!(!mirror.relayout_path().exists());

        let s = relayout(&mirror, &archive, false, |_| {}).unwrap();
        assert_eq!(
            (
                s.folders_moved,
                s.moved,
                s.versions,
                s.same,
                s.journals,
                s.held
            ),
            (1, 2, 1, 1, 2, 1),
            "{s:?}"
        );
        assert_eq!(s.unreadable, std::slice::from_ref(&bad));
        let folder = archive.path_for(CUR);
        assert_eq!(std::fs::read(folder.join("a.csv")).unwrap(), b"a");
        assert_eq!(std::fs::read(folder.join("m.zip")).unwrap(), b"final");
        let version = archive.lookup(&format!("{CUR}m.zip")).unwrap().unwrap();
        assert_eq!(std::fs::read(&version.path).unwrap(), b"partial");
        assert!(
            version
                .path
                .to_string_lossy()
                .contains("m.zip@20261005T000000")
        );
        let same = Archive::read(&folder.join("s.csv")).unwrap().unwrap();
        assert_eq!(same.url(), format!("{ARC}s.csv"));
        assert!(same.names(&format!("{CUR}s.csv")));
        assert!(
            same.names("https://h/elsewhere/s.csv"),
            "the dropped file's addresses kept"
        );
        assert!(z.exists(), "in place already");
        assert!(folder.join("v.csv@20261012T104500+1000").exists());
        assert!(
            archive
                .url_path("https://www.nemweb.com.au/Data_Archive/X/d.zip")
                .exists()
        );
        assert!(!archive.url_path(ARC).exists(), "an emptied folder goes");
        assert!(cur_dir.exists(), "a folder still holding files stays");
        // One journal, in time order.
        let j = mirror.journal(CUR).unwrap();
        assert_eq!(j.len(), 2);
        assert_eq!(j[0].url, format!("{ARC}x"));
        assert!(!journals.url_path(CUR).join("changes.ndjson").exists());
        // The held line of the version names it; the rest as they were.
        let lines = std::fs::read_to_string(mirror.held_path()).unwrap();
        let first: HeldEntry = serde_json::from_str(lines.lines().next().unwrap()).unwrap();
        assert_eq!(
            first.file.as_deref(),
            version.path.file_name().unwrap().to_str()
        );
        assert!(lines.contains("not json"));
        let log = std::fs::read_to_string(mirror.relayout_path()).unwrap();
        // One step for the ARCHIVE folder, four for CURRENT's files.
        assert_eq!(log.lines().count(), 5);

        // Again: nothing to move.
        let again = relayout(&mirror, &archive, false, |_| {}).unwrap();
        assert_eq!(
            (
                again.moved,
                again.versions,
                again.same,
                again.journals,
                again.held
            ),
            (0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn a_copy_across_folders_is_held_to_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a"), dir.path().join("b/a"));
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::write(&from, b"bytes").unwrap();
        let wrong = crate::net::Sha256::of(b"other");
        let e = read_held(&from, Some(&wrong)).unwrap_err();
        assert!(e.to_string().contains("hashes to"), "{e}");
        let held = read_held(&from, Some(&crate::net::Sha256::of(b"bytes"))).unwrap();
        put(&to, &held, true).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"bytes");
        assert!(read_held(&dir.path().join("none"), None).is_err());
        assert!(put(&dir.path().join("no/such/c"), &held, false).is_err());
    }

    #[test]
    fn a_step_cut_off_is_finished_by_the_next_run() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        // No log yet; a log without a step.
        relayout(&mirror, &archive, false, |_| {}).unwrap();
        std::fs::create_dir_all(mirror.migrations_dir()).unwrap();
        std::fs::write(mirror.relayout_path(), "\n").unwrap();
        relayout(&mirror, &archive, false, |_| {}).unwrap();
        // A move cut off after the file, before its sidecar.
        let from = old(&archive, &format!("{CUR}a.csv"), None, b"a");
        let to = archive.path_for(&format!("{CUR}a.csv"));
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(&from, &to).unwrap();
        let step = |action, to: &Path| Step {
            at: Stamp::now(),
            url: format!("{CUR}a.csv"),
            from: from.clone(),
            to: to.to_path_buf(),
            action,
        };
        log(&mirror, &step(Action::Move, &to), true).unwrap();
        relayout(&mirror, &archive, false, |_| {}).unwrap();
        // The record moves under the name it had.
        let v1 = |p: &Path| PathBuf::from(format!("{}.json", p.display()));
        assert!(v1(&to).exists() && !v1(&from).exists());
        // Finished already: nothing to do.
        finish_last(&mirror).unwrap();
        // A drop cut off after the file, before its sidecar.
        let from2 = old(&archive, &format!("{CUR}a.csv"), None, b"a");
        std::fs::remove_file(&from2).unwrap();
        log(&mirror, &step(Action::Same, &to), true).unwrap();
        finish_last(&mirror).unwrap();
        assert!(!Archive::record_path(&from2).exists());
        // A step whose file is still where it was: nothing was taken yet.
        let from3 = old(&archive, &format!("{CUR}a.csv"), None, b"a");
        finish_last(&mirror).unwrap();
        assert!(from3.exists());
        // The same bytes, with no record left at the target: the file goes
        // all the same, once the batch is on the disk; its record went
        // already.
        let sc = Archive::read(&from3).unwrap().unwrap();
        let record = v1(&from3);
        // Twice: the second time the target names it already, and its
        // record is left as it is.
        for _ in 0..2 {
            same(&step(Action::Same, &to), sc.clone()).unwrap();
        }
        std::fs::remove_file(format!("{}.json", to.display())).unwrap();
        let none = Vec::new();
        let mut written = Vec::new();
        let done = take(
            &mirror,
            vec![(step(Action::Same, &to), sc, record.clone(), none)],
            &mut written,
        )
        .unwrap();
        assert_eq!(done, [(from3.clone(), record.clone())]);
        // The log and the target's record, to be handed to the disk.
        assert_eq!(written, [mirror.relayout_path(), Archive::record_path(&to)]);
        to_disk(&mut written).unwrap();
        assert!(written.is_empty());
        let done = done.into_iter().next().unwrap();
        std::fs::remove_file(&record).unwrap();
        settle(&mut vec![done]).unwrap();
        assert!(!from3.exists());
        // A source already gone is an error, and so is a record that
        // does not go.
        assert!(settle(&mut vec![(from3.clone(), record)]).is_err());
        let from4 = old(&archive, &format!("{CUR}a.csv"), None, b"a");
        let stuck = home.path().join("stuck");
        std::fs::create_dir_all(stuck.join("in")).unwrap();
        assert!(settle(&mut vec![(from4, stuck)]).is_err());
    }

    #[test]
    fn a_folder_of_many_takes_the_place_of_one_of_few() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("state"));
        let archive = Archive::new(home.path().join("archive"));
        const X: &str = "https://www.nemweb.com.au/Reports/CURRENT/X/";
        const XA: &str = "https://www.nemweb.com.au/Reports/ARCHIVE/X/";
        for n in ["a", "b", "c"] {
            old(&archive, &format!("{X}{n}.zip"), None, n.as_bytes());
        }
        std::fs::write(archive.url_path(&format!("{X}._a.zip")), b"").unwrap();
        // The folder of few: its files already moved there, whole.
        let few = archive
            .path_for(&format!("{X}_"))
            .parent()
            .unwrap()
            .to_path_buf();
        let d = old(&archive, &format!("{XA}d.zip"), None, b"d");
        std::fs::create_dir_all(&few).unwrap();
        std::fs::rename(&d, few.join("d.zip")).unwrap();
        std::fs::rename(format!("{}.json", d.display()), few.join("d.zip.json")).unwrap();
        // Left behind by the renames, and removed with the folder.
        std::fs::write(few.join("._stray"), b"").unwrap();
        // Copies a run cut off left: `d` without its record, `e` with.
        std::fs::write(archive.url_path(&format!("{X}d.zip")), b"d").unwrap();
        let e = old(&archive, &format!("{XA}e.zip"), None, b"e");
        std::fs::rename(&e, few.join("e.zip")).unwrap();
        std::fs::rename(format!("{}.json", e.display()), few.join("e.zip.json")).unwrap();
        let cur_e = archive.url_path(&format!("{X}e.zip"));
        std::fs::write(&cur_e, b"e").unwrap();
        std::fs::copy(few.join("e.zip.json"), format!("{}.json", cur_e.display())).unwrap();
        let s = relayout(&mirror, &archive, false, |_| {}).unwrap();
        assert_eq!((s.folders_swapped, s.moved), (1, 3), "{s:?}");
        for n in ["a", "b", "c", "d", "e"] {
            let held = archive.lookup(&format!("{X}{n}.zip")).unwrap();
            let held = held.or_else(|| archive.lookup(&format!("{XA}{n}.zip")).unwrap());
            assert_eq!(std::fs::read(held.unwrap().path).unwrap(), n.as_bytes());
        }
        assert!(!archive.url_path(X).exists());
        let trash = mirror
            .tmp_dir()
            .join("trash/www.nemweb.com.au/Reports/CURRENT/X");
        assert_eq!(std::fs::read(trash.join("d.zip")).unwrap(), b"d");
        assert!(trash.join("e.zip").exists() && trash.join("e.zip.json").exists());
        // Logged: the copies set aside, the files moved where they stay,
        // then the folder.
        let log = std::fs::read_to_string(mirror.relayout_path()).unwrap();
        assert_eq!(log.lines().count(), 5);

        // Not swapped: a name in both, a folder of few with a folder in
        // it, or with a file without a record; or not fewer.
        let (dir, to) = (home.path().join("dir"), home.path().join("to"));
        for p in [&dir, &to] {
            std::fs::create_dir_all(p).unwrap();
        }
        let put = |p: &Path, n: &str| {
            std::fs::write(p.join(n), n).unwrap();
            std::fs::write(p.join(format!("{n}.json")), "{}").unwrap();
        };
        put(&dir, "a");
        put(&dir, "b");
        std::fs::write(dir.join("e"), "e").unwrap();
        std::fs::write(dir.join("e.provenance.json"), "{}").unwrap();
        assert_eq!(swappable(&dir, &to), Some((3, vec![])));
        // A name in both: swapped only if the copy hashes to the record.
        put(&to, "a");
        let both = swappable(&dir, &to).unwrap().1;
        assert_eq!(both, ["a"]);
        let record = serde_json::json!({
            "url": "https://h/a", "size": 1, "sha256": sha256_hex(b"other"),
            "fetched_at": "2026-10-05T00:00:00Z",
        });
        std::fs::write(to.join("a.json"), record.to_string()).unwrap();
        let (m, a) = (
            Mirror::new(home.path().join("s2")),
            Archive::new(home.path()),
        );
        assert!(!swap(&m, &a, &dir, &to, &both).unwrap());
        std::fs::remove_file(to.join("a")).unwrap();
        std::fs::remove_file(to.join("a.json")).unwrap();
        // A record in `dir` of no file there.
        std::fs::write(dir.join("z.json"), "{}").unwrap();
        assert_eq!(swappable(&dir, &to), None);
        std::fs::remove_file(dir.join("z.json")).unwrap();
        std::fs::create_dir(to.join("sub")).unwrap();
        assert_eq!(swappable(&dir, &to), None);
        std::fs::remove_dir(to.join("sub")).unwrap();
        std::fs::write(to.join("bare"), "x").unwrap();
        assert_eq!(swappable(&dir, &to), None);
        std::fs::remove_file(to.join("bare")).unwrap();
        put(&to, "c");
        put(&to, "d");
        put(&to, "f");
        assert_eq!(swappable(&dir, &to), None);
        assert_eq!(swappable(&home.path().join("none"), &to), None);
    }

    #[test]
    fn a_folder_of_copied_sources_is_set_aside_whole() {
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect();
        let copied = vec![(PathBuf::from("/d/a.zip"), PathBuf::from("/d/a.zip.json"))];
        // Copied sources, a record whose file went, AppleDouble files.
        let here = names(&["a.zip", "a.zip.json", "b.zip.provenance.json", "._a.zip"]);
        assert!(drained(&here, &copied));
        // A file not copied, or a folder, keeps the folder.
        assert!(!drained(&names(&["a.zip", "c.zip"]), &copied));
        assert!(!drained(&names(&["a.zip", "DUPLICATE"]), &copied));

        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("state"));
        let archive = Archive::new(home.path().join("archive"));
        let dir = archive.root().join("h/Reports/CURRENT/X");
        for _ in 0..2 {
            std::fs::create_dir_all(&dir).unwrap();
            set_aside(&mirror, &archive, &dir).unwrap();
            assert!(!dir.exists());
        }
        let trash = mirror.tmp_dir().join("trash/h/Reports/CURRENT");
        assert!(trash.join("X").is_dir() && trash.join("X.2").is_dir());
    }

    #[test]
    fn a_target_taken_every_way_is_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let archive = Archive::new(home.path().join("archive"));
        let from = old(&archive, &format!("{CUR}t.csv"), None, b"mine");
        let sc = Archive::read(&from).unwrap().unwrap();
        let base = archive.path_for(sc.url());
        let mut placed = HashMap::new();
        let other = crate::net::Sha256::of(b"other");
        placed.insert(base.clone(), other);
        placed.insert(Archive::version_path(&base, sc.fetched_at()), other);
        // The third, at the time now, taken too: a file without a sidecar.
        let now = Archive::version_path(&base, Stamp::now());
        std::fs::create_dir_all(now.parent().unwrap()).unwrap();
        std::fs::write(&now, b"x").unwrap();
        let p = plan(&archive, &from, &sc, &placed, &mut DirNames::default());
        // The second may tick over; either way nothing is written over.
        assert!(p.is_none() || p.unwrap().to != now);
    }
}
