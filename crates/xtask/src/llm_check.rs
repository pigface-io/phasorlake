//! Holds `llm/index.md` short and true: within its size limit, every path and recipe real.

use std::collections::BTreeSet;
use std::process::Command;

use anyhow::Result;

const ENTRY: &str = "CLAUDE.md";
const INDEX: &str = "llm/index.md";
const ENTRY_BUDGET: (usize, usize) = (20, 1024);
const INDEX_BUDGET: (usize, usize) = (90, 9 * 1024);

/// Code spans that look like paths: no spaces, a slash or an extension.
pub fn path_spans(markdown: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = markdown;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else { break };
        let span = &after[..end];
        if !span.contains(' ')
            && !span.contains('\n')
            && (span.contains('/')
                || span.rsplit_once('.').is_some_and(|(_, ext)| {
                    ext.chars().all(char::is_alphanumeric) && !ext.is_empty()
                }))
        {
            out.insert(span.to_owned());
        }
        rest = &after[end + 1..];
    }
    out
}

/// `just <recipe>` spans.
pub fn recipe_spans(markdown: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = markdown;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else { break };
        let span = &after[..end];
        if let Some(r) = span.strip_prefix("just ") {
            let recipe: String = r
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !recipe.is_empty() {
                out.insert(recipe);
            }
        }
        rest = &after[end + 1..];
    }
    out
}

pub fn run() -> Result<bool> {
    let root = crate::repo_root();
    let mut failures = Vec::new();
    let read = |name: &str, budget: (usize, usize), failures: &mut Vec<String>| -> String {
        let Ok(text) = std::fs::read_to_string(root.join(name)) else {
            failures.push(format!("{name} is missing"));
            return String::new();
        };
        let lines = text.trim_end().lines().count();
        if lines > budget.0 {
            failures.push(format!(
                "{name} has {lines} lines; the budget is {}",
                budget.0
            ));
        }
        if text.len() > budget.1 {
            failures.push(format!(
                "{name} has {} bytes; the budget is {}",
                text.len(),
                budget.1
            ));
        }
        text
    };
    let entry = read(ENTRY, ENTRY_BUDGET, &mut failures);
    if !entry.is_empty() && !entry.contains(INDEX) {
        failures.push(format!(
            "{ENTRY} does not point at the index: it must name {INDEX}"
        ));
    }
    if entry.lines().any(|l| l.trim() == format!("@{INDEX}")) {
        failures.push(format!("{ENTRY} imports the index with @{INDEX}; it must only point at it, so the index is read when needed, not on every session"));
    }
    let index = read(INDEX, INDEX_BUDGET, &mut failures);
    for p in path_spans(&index) {
        let path = root.join(&p);
        let ignored = Command::new("git")
            .args(["check-ignore", "-q", &p])
            .current_dir(&root)
            .status()
            .is_ok_and(|s| s.success());
        if !path.exists() && !ignored {
            failures.push(format!("{INDEX} names `{p}`, which does not exist"));
        }
    }
    let summary = Command::new("just")
        .arg("--summary")
        .current_dir(&root)
        .output();
    match summary {
        Ok(out) if out.status.success() => {
            let recipes: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            for r in recipe_spans(&index) {
                if !recipes.contains(&r) {
                    failures.push(format!("{INDEX} names `just {r}`, which is not a recipe"));
                }
            }
        }
        _ => failures.push(
            "`just --summary` failed, so the recipes named in the index could not be checked"
                .into(),
        ),
    }
    for f in &failures {
        eprintln!("{f}");
    }
    if failures.is_empty() {
        println!(
            "{INDEX}: {} lines, {} bytes, every path and recipe exists",
            index.trim_end().lines().count(),
            index.len()
        );
    }
    Ok(failures.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans() {
        let md = "Run `just cover` and read `crates/a/src/x.rs`, `README.md`, `WatchFleet` and `cargo test`.";
        let p = path_spans(md);
        assert!(p.contains("crates/a/src/x.rs") && p.contains("README.md"));
        assert!(!p.contains("WatchFleet") && !p.contains("cargo test"));
        assert_eq!(recipe_spans(md).into_iter().collect::<Vec<_>>(), ["cover"]);
    }
}
