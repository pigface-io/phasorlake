//! Moving an archive kept with each URL as its path to the layout of
//! `utilake_plain_decode::layout`: one folder per report folder, the
//! listing segment (`CURRENT`, `ARCHIVE`, `current`, `previous`) dropped.
//!
//! Every file with a sidecar in a folder under a listing moves, with its
//! sidecar, to the path of its sidecar's URL; a rename on the one volume,
//! nothing copied. A file already at the target with the same bytes keeps
//! its place, and the moving file's URL joins its sidecar's `also`
//! (`how = "same"`); the moving file is dropped. A target with other bytes
//! is never written over: the moving file is kept beside it as
//! `{name}@{at}`, `at` the time the listing gave it, else when it was
//! downloaded. The journals of a folder's two listings are merged into
//! its one journal, and `held.ndjson` lines whose file is now a version
//! are rewritten.
//!
//! Every step is logged to `mirror/relayout.ndjson` before it is taken, so
//! a run cut off is finished by the next. A second run finds nothing to
//! move.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crawl::Mirror;
use crate::download::Held;
use crate::error::MirrorError;
use crate::net::{Archive, How, Sidecar, is_apple_double};
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
    /// Files whose sidecar does not read: left where they are.
    pub unreadable: Vec<PathBuf>,
    /// Journals merged into their folder's journal.
    pub journals: usize,
    /// `held.ndjson` lines rewritten.
    pub held: usize,
    /// Empty folders of the old layout removed.
    pub folders_removed: usize,
    /// Whether nothing was changed.
    pub dry_run: bool,
}

impl Mirror {
    /// The log of relayout steps.
    pub fn relayout_path(&self) -> PathBuf {
        self.dir().join("relayout.ndjson")
    }
}

/// Moves the archive (and the mirror's journals) to the layout of
/// `utilake_plain_decode::layout`. With `dry_run`, only says what it would
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
    let mut placed: HashMap<PathBuf, String> = HashMap::new();
    let mut versions: HashMap<String, Option<String>> = HashMap::new();
    for dir in &stale {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for path in files {
            if !is_data_file(&path) {
                continue;
            }
            let sidecar = match Archive::read_sidecar(&path) {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(_) => {
                    summary.unreadable.push(path);
                    continue;
                }
            };
            let Some(step) = plan(archive, &path, &sidecar, &placed) else {
                continue;
            };
            on_step(&step);
            match step.action {
                Action::Move => summary.moved += 1,
                Action::Version => summary.versions += 1,
                Action::Same => summary.same += 1,
            }
            if step.action != Action::Same {
                placed.insert(step.to.clone(), sidecar.sha256.clone());
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
                log(mirror, &step)?;
                take(&step, sidecar)?;
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

/// Whether `path` is a data file a sidecar may describe: not a sidecar, a
/// download in progress, a sidecar being written, or an AppleDouble file.
fn is_data_file(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    !is_apple_double(path)
        && !name.ends_with(".json")
        && !name.ends_with(".part")
        && !name.ends_with(".tmp")
        && path.is_file()
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
            .map(|e| e.path())
            .filter(|p| !is_apple_double(p) && p.is_dir())
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

/// Where the file at `path` goes, and how; none when it is in place.
fn plan(
    archive: &Archive,
    path: &Path,
    sidecar: &Sidecar,
    placed: &HashMap<PathBuf, String>,
) -> Option<Step> {
    let base = archive.path_for(&sidecar.url);
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
        .listed_modified_at
        .or_else(|| Stamp::parse(&sidecar.fetched_at).ok())
        .unwrap_or_else(Stamp::now);
    let candidates = [
        target.clone(),
        Archive::version_path(&base, at),
        Archive::version_path(&base, Stamp::now()),
    ];
    let mut action = Action::Move;
    for to in candidates {
        let held = placed.get(&to).cloned().or_else(|| {
            to.exists().then(|| {
                Archive::read_sidecar(&to)
                    .ok()
                    .flatten()
                    .map_or_else(String::new, |s| s.sha256)
            })
        });
        match held {
            None => {
                return Some(Step {
                    at: Stamp::now(),
                    url: sidecar.url.clone(),
                    from: path.to_path_buf(),
                    to,
                    action,
                });
            }
            Some(sha) if sha == sidecar.sha256 => {
                return Some(Step {
                    at: Stamp::now(),
                    url: sidecar.url.clone(),
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

/// Appends `step` to the log and hands it to the disk.
fn log(mirror: &Mirror, step: &Step) -> Result<(), MirrorError> {
    let path = mirror.relayout_path();
    std::fs::create_dir_all(mirror.dir()).map_err(MirrorError::io(mirror.dir()))?;
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

/// Takes one step.
fn take(step: &Step, sidecar: Sidecar) -> Result<(), MirrorError> {
    let (from_sc, to_sc) = (
        Archive::sidecar_path(&step.from),
        Archive::sidecar_path(&step.to),
    );
    match step.action {
        Action::Move | Action::Version => {
            let dir = step.to.parent().expect("an archived path is in a folder");
            std::fs::create_dir_all(dir).map_err(MirrorError::io(dir))?;
            std::fs::rename(&step.from, &step.to).map_err(MirrorError::io(&step.to))?;
            std::fs::rename(&from_sc, &to_sc).map_err(MirrorError::io(&to_sc))?;
        }
        Action::Same => {
            let kept = Archive::read_sidecar(&step.to)?.map(|mut kept| {
                kept.add_listed(
                    &sidecar.url,
                    How::Same,
                    Stamp::now(),
                    sidecar.listed_modified_at,
                );
                for a in sidecar.also {
                    if !kept.names(&a.url) {
                        kept.also.push(a);
                    }
                }
                kept
            });
            kept.map(|k| Archive::write_sidecar(&step.to, &k))
                .transpose()?;
            std::fs::remove_file(&step.from).map_err(MirrorError::io(&step.from))?;
            std::fs::remove_file(&from_sc).map_err(MirrorError::io(&from_sc))?;
        }
    }
    Ok(())
}

/// Finishes the step a run before logged last, when it was cut off
/// between moving a file and its sidecar.
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
    let from_sc = Archive::sidecar_path(&step.from);
    if step.from.exists() || !from_sc.exists() {
        return Ok(());
    }
    match step.action {
        Action::Same => std::fs::remove_file(&from_sc).map_err(MirrorError::io(&from_sc)),
        Action::Move | Action::Version => {
            let to_sc = Archive::sidecar_path(&step.to);
            std::fs::rename(&from_sc, &to_sc).map_err(MirrorError::io(&to_sc))
        }
    }
}

/// Merges each journal kept under a listing into its folder's journal,
/// lines in time order. How many journals were (or would be) merged.
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
        let new = journals.path_for(&url).join("changes.ndjson");
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
                    v.get("seen_at")?
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
        match serde_json::from_str::<Held>(line) {
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
        let sc: Sidecar = serde_json::from_value(serde_json::json!({
            "url": url, "size": bytes.len(), "sha256": sha256_hex(bytes),
            "fetched_at": "2026-10-05T00:00:00Z",
        }))
        .unwrap();
        Archive::write_sidecar(&p, &sc).unwrap();
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
        Archive::add_also(&s_cur, "https://h/elsewhere/s.csv", How::Same, Stamp::now()).unwrap();
        Archive::add_also(&s_cur, &format!("{ARC}s.csv"), How::Same, Stamp::now())
            .unwrap_or_default();
        // A file whose URL is kept where it already is, under a listing.
        let odd = format!("{CUR}z.csv").replace("/CURRENT/", "/CURRENT/CURRENT/");
        let in_place = old(&archive, &odd, None, b"z");
        let z = archive.url_path(&format!("{CUR}z.csv"));
        std::fs::rename(&in_place, &z).unwrap();
        std::fs::rename(Archive::sidecar_path(&in_place), Archive::sidecar_path(&z)).unwrap();
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
        std::fs::write(Archive::sidecar_path(&bad), b"{").unwrap();
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
        std::fs::create_dir_all(mirror.dir()).unwrap();
        std::fs::write(mirror.held_path(), &held).unwrap();

        let mut steps = Vec::new();
        let dry = relayout(&mirror, &archive, true, |s| steps.push(s.clone())).unwrap();
        assert!(dry.dry_run);
        assert_eq!(
            (dry.moved, dry.versions, dry.same, dry.journals),
            (4, 1, 1, 2),
            "{steps:?}"
        );
        assert!(
            archive.url_path(&format!("{CUR}a.csv")).exists(),
            "a dry run moves nothing"
        );
        assert!(!mirror.relayout_path().exists());

        let s = relayout(&mirror, &archive, false, |_| {}).unwrap();
        assert_eq!(
            (s.moved, s.versions, s.same, s.journals, s.held),
            (4, 1, 1, 2, 1),
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
        let same = Archive::read_sidecar(&folder.join("s.csv"))
            .unwrap()
            .unwrap();
        assert_eq!(same.url, format!("{ARC}s.csv"));
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
        let first: Held = serde_json::from_str(lines.lines().next().unwrap()).unwrap();
        assert_eq!(
            first.file.as_deref(),
            version.path.file_name().unwrap().to_str()
        );
        assert!(lines.contains("not json"));
        let log = std::fs::read_to_string(mirror.relayout_path()).unwrap();
        assert_eq!(log.lines().count(), 6);

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
    fn a_step_cut_off_is_finished_by_the_next_run() {
        let home = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(home.path().join("mirror"));
        let archive = Archive::new(home.path().join("archive"));
        // No log yet; a log without a step.
        relayout(&mirror, &archive, false, |_| {}).unwrap();
        std::fs::create_dir_all(mirror.dir()).unwrap();
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
        log(&mirror, &step(Action::Move, &to)).unwrap();
        relayout(&mirror, &archive, false, |_| {}).unwrap();
        assert!(Archive::sidecar_path(&to).exists() && !Archive::sidecar_path(&from).exists());
        // Finished already: nothing to do.
        finish_last(&mirror).unwrap();
        // A drop cut off after the file, before its sidecar.
        let from2 = old(&archive, &format!("{CUR}a.csv"), None, b"a");
        std::fs::remove_file(&from2).unwrap();
        log(&mirror, &step(Action::Same, &to)).unwrap();
        finish_last(&mirror).unwrap();
        assert!(!Archive::sidecar_path(&from2).exists());
    }

    #[test]
    fn a_target_taken_every_way_is_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let archive = Archive::new(home.path().join("archive"));
        let from = old(&archive, &format!("{CUR}t.csv"), None, b"mine");
        let sc = Archive::read_sidecar(&from).unwrap().unwrap();
        let base = archive.path_for(&sc.url);
        let mut placed = HashMap::new();
        placed.insert(base.clone(), "other".to_owned());
        placed.insert(
            Archive::version_path(&base, Stamp::parse(&sc.fetched_at).unwrap()),
            "other".to_owned(),
        );
        // The third, at the time now, taken too: a file without a sidecar.
        let now = Archive::version_path(&base, Stamp::now());
        std::fs::create_dir_all(now.parent().unwrap()).unwrap();
        std::fs::write(&now, b"x").unwrap();
        let p = plan(&archive, &from, &sc, &placed);
        // The second may tick over; either way nothing is written over.
        assert!(p.is_none() || p.unwrap().to != now);
    }
}
