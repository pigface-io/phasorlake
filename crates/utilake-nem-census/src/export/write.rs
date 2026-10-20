//! Writing the rendered files, or checking a directory against them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use utilake_core::{Category, Diagnose, Diagnostic};

use super::{IoError, Rendered};

/// The directory the generator owns whole: files there it did not render
/// are removed, or reported stale.
const OWNED: &str = "nem";

/// What one write did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Written {
    /// Files whose bytes changed or were new.
    pub written: usize,
    /// Files already current.
    pub kept: usize,
    /// Files under `nem/` the render did not produce, removed.
    pub removed: usize,
}

/// One file a check found not as rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    /// Relative path.
    pub path: String,
    /// `missing`, `differs` or `extra`.
    pub why: &'static str,
}

impl Stale {
    /// The diagnostic the check prints for it.
    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error(
            "schema.export_stale",
            Category::Schema,
            format!("{} is {}; run `just schema-export`.", self.path, self.why),
        )
        .with_file(self.path.clone())
    }
}

/// Writes every rendered file and the provenance under `out`, through a
/// temporary file and a rename, and removes what it did not render under
/// `nem/`.
pub fn write(out: &Path, r: &Rendered) -> Result<Written, Box<Diagnostic>> {
    let mut done = Written::default();
    for (rel, text) in &r.files {
        let path = out.join(rel);
        if std::fs::read_to_string(&path).is_ok_and(|now| now == *text) {
            done.kept += 1;
            continue;
        }
        put(&path, text)?;
        done.written += 1;
    }
    put(&out.join("provenance.json"), &r.provenance)?;
    for path in extras(out, &r.files)? {
        std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
        done.removed += 1;
        prune(path.parent(), out);
    }
    Ok(done)
}

/// Compares `out` with the render: every rendered file must be there with
/// the same bytes, and nothing else may be under `nem/`.
pub fn check(out: &Path, r: &Rendered) -> Result<Vec<Stale>, Box<Diagnostic>> {
    let extra = extras(out, &r.files)?;
    let mut stale = Vec::new();
    for (rel, text) in &r.files {
        let why = match std::fs::read_to_string(out.join(rel)) {
            Ok(now) if now == *text => continue,
            Ok(_) => "differs",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
            Err(e) => return Err(io(&out.join(rel), e)),
        };
        stale.push(Stale {
            path: rel.clone(),
            why,
        });
    }
    for path in &extra {
        let rel = path.strip_prefix(out).unwrap_or(path);
        stale.push(Stale {
            path: rel.to_string_lossy().replace('\\', "/"),
            why: "extra",
        });
    }
    Ok(stale)
}

fn put(path: &Path, text: &str) -> Result<(), Box<Diagnostic>> {
    let dir = path.parent().expect("a rendered path has a directory");
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io(path, e))
}

/// Files under `out/nem/` that the render did not produce, sorted.
fn extras(out: &Path, files: &BTreeMap<String, String>) -> Result<Vec<PathBuf>, Box<Diagnostic>> {
    let root = out.join(OWNED);
    let mut found = Vec::new();
    walk(&root, &mut found)?;
    found.retain(|p| {
        let rel = p
            .strip_prefix(out)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        !files.contains_key(&rel)
    });
    found.sort();
    Ok(found)
}

fn walk(dir: &Path, into: &mut Vec<PathBuf>) -> Result<(), Box<Diagnostic>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io(dir, e)),
    };
    for entry in entries {
        let path = entry.map_err(|e| io(dir, e))?.path();
        if path.is_dir() {
            walk(&path, into)?;
        } else {
            into.push(path);
        }
    }
    Ok(())
}

/// Removes directories left empty by a removal, up to `nem/` itself.
fn prune(mut dir: Option<&Path>, out: &Path) {
    let stop = out.join(OWNED);
    while let Some(d) = dir {
        if d == stop || std::fs::remove_dir(d).is_err() {
            return;
        }
        dir = d.parent();
    }
}

fn io(path: &Path, e: std::io::Error) -> Box<Diagnostic> {
    Box::new(IoError::new(path, e).diagnostic())
}
