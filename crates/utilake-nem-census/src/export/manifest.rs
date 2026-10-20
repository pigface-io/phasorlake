//! `manifest.json` (content only) and `provenance.json` (who wrote it).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::json;
use utilake_core::YearMonth;
use utilake_mms_registry::file_model::ctl::sha256;

use super::{FORMAT, doc::text};

/// What the manifest counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Shape files, declared or not.
    pub tables: usize,
    /// Of those, tables with a `table.toml`.
    pub declared_tables: usize,
    /// Documents, one per shape id.
    pub shapes: usize,
}

/// The manifest: format, months covered, counts, and the SHA-256 of every
/// file under `nem/`. Nothing in it changes unless a document does.
pub fn render(
    base_url: &str,
    months: &BTreeSet<YearMonth>,
    counts: Counts,
    files: &BTreeMap<String, String>,
) -> String {
    let hashes: BTreeMap<&str, String> = files
        .iter()
        .filter(|(p, _)| p.starts_with("nem/"))
        .map(|(p, t)| (p.as_str(), sha256(t.as_bytes())))
        .collect();
    let span = months.first().zip(months.last()).map(
        |(a, b)| json!({ "first": a.to_string(), "last": b.to_string(), "count": months.len() }),
    );
    text(&json!({
        "format": FORMAT,
        "market": "nem",
        "base_url": base_url,
        "months": span,
        "tables": counts.tables,
        "declared_tables": counts.declared_tables,
        "shapes": counts.shapes,
        "files": hashes.len(),
        "sha256": hashes,
    }))
}

/// The provenance: the commit and tree the schemas were read at, whether
/// they were clean, the data model, and the time. Varies run to run, so it
/// is written and never compared.
pub fn provenance(schemas: &Path, datamodel: Option<&Path>) -> String {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(schemas)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let commit = git(&["rev-parse", "HEAD"]);
    let prefix = git(&["rev-parse", "--show-prefix"]);
    let tree = prefix
        .as_deref()
        .and_then(|p| git(&["rev-parse", &format!("HEAD:{}", p.trim_end_matches('/'))]));
    let dirty = git(&["status", "--porcelain", "--", "."]).map(|s| !s.is_empty());
    text(&json!({
        "generated_at": utilake_core::clock::now_utc_rfc3339(),
        "generator": "utilake-nem-census export",
        "utilake_commit": commit,
        "utilake_dirty": dirty,
        "schemas_tree": tree,
        "datamodel": datamodel.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()),
    }))
}
