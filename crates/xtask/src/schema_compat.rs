//! The registry only changes in ways the lake survives. Every table file
//! under `schemas/` at the base ref is compared with the working tree by
//! the rules of `nemlake_registry::file_model::compat`: no column id reused or
//! moved, no type narrowed, no declared version or variant dropped, no
//! table deleted. A change that breaks one of them on purpose is explained
//! by a `schema-break:` line in a commit message since the base.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use nemlake_registry::file_model::TableDef;
use nemlake_registry::file_model::blob::SchemaSet;
use nemlake_registry::file_model::compat::breaking_changes;
use nemlake_registry::file_model::file::parse_table;

fn git(args: &[&str]) -> Result<Option<String>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(crate::repo_root())
        .output()
        .context("running git")?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Whether a path under `schemas/` is a table file the registry reads.
fn is_table_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.ends_with(".toml")
        && name != "market.toml"
        && !name.starts_with('_')
        && !name.starts_with("DRAFT_")
}

/// Every breaking change from `old` tables to `new` ones, as
/// `(table, what)`.
pub fn breaks(old: &[TableDef], new: &[TableDef]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for o in old {
        match new.iter().find(|n| n.name == o.name) {
            None => out.push((
                o.name.clone(),
                "table was deleted; mark it deprecated instead".to_owned(),
            )),
            Some(n) => out.extend(
                breaking_changes(o, n)
                    .into_iter()
                    .map(|m| (o.name.clone(), m)),
            ),
        }
    }
    out
}

pub fn run(base: &str) -> Result<bool> {
    let Some(listing) = git(&["ls-tree", "-r", "--name-only", base, "--", "schemas"])? else {
        println!("schema-compat: base ref {base} not found; nothing to compare against");
        return Ok(true);
    };
    let mut old = Vec::new();
    for path in listing.lines().filter(|p| is_table_file(p)) {
        let text = git(&["show", &format!("{base}:{path}")])?
            .with_context(|| format!("reading {path} at {base}"))?;
        let table = parse_table(Path::new(path), &text)
            .map_err(|e| anyhow::anyhow!("{path} at {base} does not parse: {e}"))?;
        old.push(table);
    }
    let new = SchemaSet::read_dir(&crate::repo_root().join("schemas"))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .tables;
    let found = breaks(&old, &new);
    if found.is_empty() {
        println!(
            "schema-compat: {} table(s) at {base}, {} now, no breaking change",
            old.len(),
            new.len()
        );
        return Ok(true);
    }
    for (table, what) in &found {
        eprintln!("breaking: {table}: {what}");
    }
    let explained = git(&["log", "--format=%B", &format!("{base}..HEAD")])?
        .unwrap_or_default()
        .lines()
        .any(|l| l.trim_start().starts_with("schema-break:"));
    if explained {
        println!(
            "schema-compat: {} breaking change(s), explained by a `schema-break:` line",
            found.len()
        );
        return Ok(true);
    }
    eprintln!(
        "{} breaking change(s) against {base}; undo them, or explain them with a `schema-break:` line in a commit message",
        found.len()
    );
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_files_and_breaks() {
        assert!(is_table_file("schemas/nem/mms/REGION.toml"));
        assert!(!is_table_file("schemas/nem/market.toml"));
        assert!(!is_table_file("schemas/nem/mms/DRAFT_X.toml"));
        assert!(!is_table_file("schemas/nem/mms/_conflicts.md"));
        assert!(!is_table_file("schemas/nem/mms/_notes.toml"));

        let now = SchemaSet::read_dir(&crate::repo_root().join("schemas"))
            .unwrap()
            .tables;
        assert!(breaks(&now, &now).is_empty());
        // A deleted table and a deleted column are both breaking.
        let mut fewer = now.clone();
        let gone = fewer.remove(0);
        let found = breaks(&now, &fewer);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, gone.name);
        assert!(found[0].1.contains("deleted"));
        let mut narrower = now.clone();
        let dropped = narrower[0].columns.pop().unwrap();
        let found = breaks(&now, &narrower);
        assert!(
            found
                .iter()
                .any(|(t, m)| *t == now[0].name && m.contains(&dropped.name)),
            "{found:?}"
        );
        // Adding a table is not.
        assert!(breaks(&fewer, &now).is_empty());
        // The working tree is compatible with itself at HEAD or, where HEAD
        // has no schemas yet, there is nothing to compare.
        assert!(run("refs/heads/no-such-branch-for-this-test").unwrap());
    }
}
