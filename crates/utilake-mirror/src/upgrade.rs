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
/// every format-1 provenance record in `archive`. `on_step` sees each
/// step before it is taken.
pub fn upgrade(
    home: &Home,
    archive: &Archive,
    dry_run: bool,
    records: bool,
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
    })
    .into_iter()
    .chain(tree_moves(&old.join("journal"), "changes.ndjson", |url| {
        Some(state.journal_path(url))
    })) {
        if !to.exists() && take(step(Action::Move, &from, &to), &mut summary)? {
            rename(&from, &to)?;
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
    if records {
        for path in Archive::walk(archive.root())? {
            let mut v1 = path.as_os_str().to_owned();
            v1.push(utilake_provenance::archive::OLD_RECORD);
            let v1 = PathBuf::from(v1);
            if !v1.exists() || Archive::record_path(&path).exists() {
                continue;
            }
            let Ok(Some(p)) = Archive::read(&path) else {
                summary.unreadable.push(v1);
                continue;
            };
            let to = Archive::record_path(&path);
            if take(step(Action::Record, &v1, &to), &mut summary)? {
                Archive::write(&path, &p)?;
            }
        }
    }
    Ok(summary)
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
    crate::net::archive::to_disk(&f).map_err(MirrorError::io(&path))
}

#[cfg(test)]
mod tests;
