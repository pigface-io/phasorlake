//! Moving a home from the layout before `state/` to the one now: each of
//! the mirror's records to the name that says what it holds, the crawl
//! record and the watch's overlays into one listing file per local
//! folder, pages and journals beside them, and every provenance record of
//! format 1 written again in format 2.
//!
//! Nothing in `archive/` but a record is read or written: the bytes are
//! never opened. Every step is a rename on the one volume, or a record
//! written whole; each is logged to `state/migrations/upgrade.ndjson`
//! before it is taken, and a step done already is not taken again, so a
//! run cut off is finished by the next and a second run does nothing.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Home, Mirror};
use crate::crawl::{Remote, read_record};
use crate::error::MirrorError;
use crate::net::Archive;
use crate::stamp::Stamp;

/// What one step does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// A record moved to its new name.
    Move,
    /// The crawl record and the watch's overlays split into listing files.
    Listings,
    /// A format-1 provenance record written again as format 2.
    Record,
}

/// One step, as logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// When it was taken (UTC).
    pub at: Stamp,
    /// What it does.
    pub action: Action,
    /// What it reads.
    pub from: PathBuf,
    /// What it writes.
    pub to: PathBuf,
}

/// What an upgrade did, or with `dry_run` would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UpgradeSummary {
    /// Nothing was changed.
    pub dry_run: bool,
    /// Records moved to their new names.
    pub moved: usize,
    /// Listing files written from the crawl record.
    pub listings: usize,
    /// Provenance records written again as format 2.
    pub records: usize,
    /// Provenance records that do not read, left as they are.
    pub unreadable: Vec<PathBuf>,
    /// Folders too large to write again, with their entries: their
    /// format-1 records are left, and read as they are.
    pub skipped: Vec<(PathBuf, usize)>,
    /// What is left under the old records' folder: files no step knows.
    pub left: Vec<PathBuf>,
}

/// What a watch kept of a folder it read since the crawl, before `state/`.
#[derive(Deserialize)]
struct FolderRead {
    folder: String,
    links: Vec<Remote>,
}

/// Upgrades the home: the mirror's records first, then, with `records`,
/// every format-1 provenance record in `archive` in a folder of at most
/// that many entries. On exFAT a name is found by reading its folder's
/// directory from the start, and only the start of a large one stays
/// cached: in a folder of 150 thousand entries each name past the first
/// tens of thousands costs seconds, so such a folder's records are left
/// (format 1 reads as well) and named in [`UpgradeSummary::skipped`].
/// `on_step` sees each step before it is taken.
pub fn upgrade(
    home: &Home,
    archive: &Archive,
    dry_run: bool,
    records: Option<usize>,
    mut on_step: impl FnMut(&Step),
) -> Result<UpgradeSummary, MirrorError> {
    let old = home.old_records();
    let state = home.records();
    let mut summary = UpgradeSummary {
        dry_run,
        ..UpgradeSummary::default()
    };
    let mut take = |step: Step, summary: &mut UpgradeSummary| -> Result<bool, MirrorError> {
        on_step(&step);
        match step.action {
            Action::Move => summary.moved += 1,
            Action::Listings => {}
            Action::Record => summary.records += 1,
        }
        if !dry_run {
            log(&state, &step)?;
        }
        Ok(!dry_run)
    };
    for (from, to) in moves(home, &old, &state) {
        if from.exists() && !to.exists() && take(step(Action::Move, &from, &to), &mut summary)? {
            rename(&from, &to)?;
        }
    }
    for (from, to) in tree_moves(&old.join("crawl/listings"), "index.html", |url| {
        state.page_path(&url::Url::parse(url).ok()?).into()
    }) {
        if !to.exists() && take(step(Action::Move, &from, &to), &mut summary)? {
            rename(&from, &to)?;
        }
    }
    // Both listings of a report folder have one journal now: a second is
    // merged into the first, lines in time order.
    for (from, to) in tree_moves(&old.join("journal"), "changes.ndjson", |url| {
        Some(state.journal_path(url))
    }) {
        if take(step(Action::Move, &from, &to), &mut summary)? {
            merge_into(&from, &to)?;
        }
    }
    let crawl = old.join("crawl");
    let remote = crawl.join("remote.ndjson");
    if remote.exists() || crawl.join("folders").exists() {
        let record = old_record(&crawl)?;
        summary.listings = record
            .iter()
            .map(|r| state.listing_path(&r.listed_on))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let to = state.folders().root().to_path_buf();
        if take(step(Action::Listings, &crawl, &to), &mut summary)? {
            state.write_record(record)?;
            let migrated = state.migrations_dir().join("remote.ndjson.migrated");
            if remote.exists() {
                rename(&remote, &migrated)?;
            }
            let overlays = crawl.join("folders");
            if overlays.exists() {
                std::fs::remove_dir_all(&overlays).map_err(MirrorError::io(&overlays))?;
            }
        }
    }
    let done = crawl.join("done");
    if done.exists() && !state.done_path().exists() {
        let to = state.done_path();
        if take(step(Action::Move, &done, &to), &mut summary)? {
            let at = mtime(&done).unwrap_or_else(Stamp::now);
            let text = serde_json::json!({ "completed_at": at }).to_string();
            crate::crawl::write_atomic(&to, text.as_bytes())?;
            std::fs::remove_file(&done).map_err(MirrorError::io(&done))?;
        }
    }
    if !dry_run {
        for gone in [old.join("lock"), old.join("tmp")] {
            let _ = std::fs::remove_file(&gone).or_else(|_| std::fs::remove_dir_all(&gone));
        }
        prune_empty(&old);
    }
    summary.left = files_under(&old);
    if dry_run {
        summary.left.retain(|p| !known(&old, p));
    }
    if let Some(max_entries) = records {
        // Old records to remove, by folder, once the new ones are on the
        // disk: one sync for many folders, each folder's unlinks together.
        let mut done: Vec<Vec<(PathBuf, PathBuf)>> = Vec::new();
        let mut waiting = 0;
        // An archive not there is an error, not an archive with nothing.
        std::fs::read_dir(archive.root()).map_err(MirrorError::io(archive.root()))?;
        for dir in folders(archive.root()) {
            let Some(found) = old_records(&dir, max_entries, &mut summary)? else {
                continue;
            };
            let mut here = Vec::with_capacity(found.len());
            for (v1, v2, p) in found {
                if take(step(Action::Record, &v1, &v2), &mut summary)? {
                    if let Some(p) = p {
                        std::fs::write(&v2, p.to_json()).map_err(MirrorError::io(&v2))?;
                    }
                    here.push((v1, v2));
                }
            }
            waiting += here.len();
            done.push(here);
            if waiting >= RECORDS_PER_SYNC {
                remove_old(&state, &mut done)?;
                waiting = 0;
            }
        }
        remove_old(&state, &mut done)?;
    }
    Ok(summary)
}

/// Old records written again before one sync hands them to the disk.
const RECORDS_PER_SYNC: usize = if cfg!(test) { 2 } else { 4096 };

/// Hands the new records, and the log of their steps, to the disk, then
/// removes the old ones beside them, folder by folder.
fn remove_old(state: &Mirror, done: &mut Vec<Vec<(PathBuf, PathBuf)>>) -> Result<(), MirrorError> {
    if done.iter().all(Vec::is_empty) {
        done.clear();
        return Ok(());
    }
    let log = state.migrations_dir().join("upgrade.ndjson");
    let written = done.iter().flatten().map(|(_, v2)| v2.as_path());
    crate::net::archive::batch_to_disk(written.chain([log.as_path()]))
        .map_err(|(p, e)| MirrorError::io(&p)(e))?;
    for (v1, _) in done.drain(..).flatten() {
        std::fs::remove_file(&v1).map_err(MirrorError::io(&v1))?;
    }
    Ok(())
}

/// Every folder under `root`, `root` first, from the folder listings
/// alone: on exFAT a `stat` of one name reads its whole folder.
fn folders(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        // A folder that does not read has nothing to upgrade.
        let mut subs: Vec<PathBuf> = std::fs::read_dir(&d)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| !e.file_name().to_string_lossy().starts_with("._"))
            .map(|e| e.path())
            .collect();
        subs.sort();
        stack.extend(subs.into_iter().rev());
        out.push(d);
    }
    out
}

/// The format-1 records in `dir` of files there: each with its format-2
/// path and the record to write there, none when a format-2 record that
/// reads is there already (a run cut off wrote it, or a later write did:
/// it is the one kept). One listing of the folder, then one read of each
/// record: on exFAT each look for a name not cached reads the folder. A
/// record that does not read is added to `unreadable` and left; a folder
/// of more than `max_entries` entries is added to `skipped`, and `None`.
type Found = (PathBuf, PathBuf, Option<utilake_provenance::Provenance>);
fn old_records(
    dir: &Path,
    max_entries: usize,
    summary: &mut UpgradeSummary,
) -> Result<Option<Vec<Found>>, MirrorError> {
    use utilake_provenance::Provenance;
    use utilake_provenance::archive::{OLD_RECORD, RECORD, is_noise};
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(MirrorError::io(dir))?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if names.len() > max_entries {
        summary.skipped.push((dir.to_path_buf(), names.len()));
        return Ok(None);
    }
    names.sort();
    let here: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    let read = |path: &Path| {
        std::fs::read(path)
            .ok()
            .and_then(|b| Provenance::from_json(&b).ok())
    };
    let mut out = Vec::new();
    for name in &names {
        let Some(data) = name.strip_suffix(OLD_RECORD) else {
            continue;
        };
        if name.ends_with(RECORD)
            || name.starts_with("._")
            || is_noise(name)
            || !here.contains(data)
        {
            continue;
        }
        let (v1, v2) = (dir.join(name), dir.join(format!("{data}{RECORD}")));
        if here.contains(format!("{data}{RECORD}").as_str()) && read(&v2).is_some() {
            out.push((v1, v2, None));
            continue;
        }
        match read(&v1) {
            Some(p) => out.push((v1, v2, Some(p))),
            None => summary.unreadable.push(v1),
        }
    }
    Ok(Some(out))
}

fn step(action: Action, from: &Path, to: &Path) -> Step {
    Step {
        at: Stamp::now(),
        action,
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    }
}

/// Each record of the old layout and where it goes.
fn moves(home: &Home, old: &Path, state: &Mirror) -> Vec<(PathBuf, PathBuf)> {
    let cache = state.cache_dir();
    vec![
        (old.join("config.toml"), home.config_file()),
        (old.join("peek.ndjson"), state.dir().join("peeks.ndjson")),
        (
            old.join("removed.ndjson"),
            state.dir().join("removals.ndjson"),
        ),
        (old.join("held.ndjson"), cache.join("held.ndjson")),
        (old.join("progress.json"), cache.join("status.json")),
        (old.join("map/map.ndjson"), cache.join("catalog.ndjson")),
        (old.join("map/tree.txt"), cache.join("catalog.txt")),
        (
            old.join("relayout.ndjson"),
            state.migrations_dir().join("relayout.ndjson"),
        ),
    ]
}

/// Whether `path` is one a run would move or remove.
fn known(old: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(old).unwrap_or(path);
    let first = rel
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    matches!(
        first.as_str(),
        "config.toml"
            | "peek.ndjson"
            | "removed.ndjson"
            | "held.ndjson"
            | "progress.json"
            | "map"
            | "relayout.ndjson"
            | "crawl"
            | "journal"
            | "lock"
            | "tmp"
    )
}

/// Every file named `name` under `root`, kept at its folder URL's path,
/// and where `to` puts that URL's file.
fn tree_moves(
    root: &Path,
    name: &str,
    to: impl Fn(&str) -> Option<PathBuf>,
) -> Vec<(PathBuf, PathBuf)> {
    files_under(root)
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n == name))
        .filter_map(|path| {
            let dir = path.parent()?.strip_prefix(root).ok()?;
            let rel: Vec<String> = dir
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let target = to(&format!("https://{}/", rel.join("/")))?;
            Some((path, target))
        })
        .collect()
}

/// The crawl record as the old layout kept it: the crawl's, with each
/// folder a watch read since in place of what the crawl read, and all
/// under each folder gone since out of it.
fn old_record(crawl: &Path) -> Result<Vec<Remote>, MirrorError> {
    let remote = crawl.join("remote.ndjson");
    let mut record: BTreeMap<String, Remote> = match remote.exists() {
        true => read_record(&remote)?,
        false => Vec::new(),
    }
    .into_iter()
    .map(|r| (r.url.clone(), r))
    .collect();
    let (mut listings, mut gone) = (Vec::new(), Vec::new());
    for path in files_under(&crawl.join("folders")) {
        match path.file_name().and_then(|n| n.to_str()) {
            Some("listing.json") => listings.push(path),
            Some("gone") => gone.push(path),
            // Nothing else was kept there; a stray file is left.
            _ => continue,
        }
    }
    for path in gone {
        let url = std::fs::read_to_string(&path).map_err(MirrorError::io(&path))?;
        record.retain(|u, _| u == &url || !u.starts_with(url.as_str()));
    }
    for path in listings {
        let text = std::fs::read_to_string(&path).map_err(MirrorError::io(&path))?;
        let read: FolderRead = serde_json::from_str(&text).map_err(|e| MirrorError::Record {
            path: path.display().to_string(),
            line: 1,
            message: e.to_string(),
        })?;
        record.retain(|_, r| r.listed_on != read.folder);
        record.extend(read.links.into_iter().map(|r| (r.url.clone(), r.derived())));
    }
    Ok(record.into_values().collect())
}

/// Every file under `dir`, in path order; none when it is not there.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(e.path());
            } else if !e.file_name().to_string_lossy().starts_with("._") {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// Removes every folder under `dir`, and `dir`, that holds nothing but
/// AppleDouble files, deepest first.
fn prune_empty(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            prune_empty(&e.path());
        }
    }
    let only_twins = std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten()
            .all(|e| e.file_name().to_string_lossy().starts_with("._"))
    });
    if only_twins {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Moves the journal `from` to `to`, its lines merged with those there in
/// time order (`at`, or `seen_at` as an older mirror wrote it).
fn merge_into(from: &Path, to: &Path) -> Result<(), MirrorError> {
    if !to.exists() {
        return rename(from, to);
    }
    let read = |p: &Path| std::fs::read_to_string(p).map_err(MirrorError::io(p));
    let (a, b) = (read(to)?, read(from)?);
    let mut lines: Vec<&str> = a
        .lines()
        .chain(b.lines())
        .filter(|l| !l.is_empty())
        .collect();
    let at = |l: &str| {
        serde_json::from_str::<serde_json::Value>(l)
            .ok()
            .and_then(|v| {
                let t = v
                    .get("at")
                    .or_else(|| v.get("seen_at"))?
                    .as_str()?
                    .to_owned();
                Stamp::parse(&t).ok()
            })
            .map_or(i64::MAX, Stamp::unix)
    };
    lines.sort_by_key(|l| at(l));
    crate::crawl::write_atomic(to, (lines.join("\n") + "\n").as_bytes())?;
    std::fs::remove_file(from).map_err(MirrorError::io(from))
}

/// Renames `from` to `to`, making `to`'s folder.
fn rename(from: &Path, to: &Path) -> Result<(), MirrorError> {
    let dir = to.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(MirrorError::io(dir))?;
    std::fs::rename(from, to).map_err(MirrorError::io(to))
}

/// When `path` was last written.
fn mtime(path: &Path) -> Option<Stamp> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(Stamp::from_unix(i64::try_from(secs).ok()?))
}

/// Appends `step` to the log and hands it to the disk.
fn log(state: &Mirror, step: &Step) -> Result<(), MirrorError> {
    let path = state.migrations_dir().join("upgrade.ndjson");
    let dir = state.migrations_dir();
    std::fs::create_dir_all(&dir).map_err(MirrorError::io(&dir))?;
    let mut line = serde_json::to_vec(step).expect("a Step always serialises");
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(MirrorError::io(&path))?;
    f.write_all(&line).map_err(MirrorError::io(&path))?;
    // A record's step is handed to the disk with its batch, by the sync
    // before the old records go: an fsync each takes seconds on exFAT.
    if step.action == Action::Record {
        return Ok(());
    }
    crate::net::archive::to_disk(&f).map_err(MirrorError::io(&path))
}

#[cfg(test)]
mod tests;
