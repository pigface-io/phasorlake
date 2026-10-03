//! Budgets only tighten and floors only rise. Against the base ref's copy of
//! `bench/budgets.json` and `coverage.json`, any loosened value fails unless
//! a commit since the base explains it with a `bench-budget:` or
//! `coverage-floor:` line.

use std::collections::BTreeMap;
use std::process::Command;

use anyhow::{Context, Result};

fn git_show(base: &str, path: &str) -> Result<Option<String>> {
    let out = Command::new("git").args(["show", &format!("{base}:{path}")]).current_dir(crate::repo_root()).output().context("running git show")?;
    if out.status.success() { Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned())) } else { Ok(None) }
}

fn commit_messages_since(base: &str) -> Result<String> {
    let out = Command::new("git").args(["log", "--format=%B", &format!("{base}..HEAD")]).current_dir(crate::repo_root()).output().context("running git log")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Budget keys where a *higher* value is tighter.
fn higher_is_tighter(key: &str) -> bool {
    key.starts_with("min_")
}

/// Compares two budget documents; returns every loosening.
pub fn budget_loosenings(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(o), Some(n)) = (old.get("benches").and_then(|b| b.as_object()), new.get("benches").and_then(|b| b.as_object())) else {
        return out;
    };
    for (bench, ob) in o {
        let Some(nb) = n.get(bench).and_then(|b| b.as_object()) else {
            out.push(format!("bench/budgets.json: bench {bench} was removed"));
            continue;
        };
        let Some(ob) = ob.as_object() else { continue };
        for (key, ov) in ob {
            let (Some(ov), Some(nv)) = (ov.as_f64(), nb.get(key).and_then(|v| v.as_f64())) else { continue };
            let loosened = if higher_is_tighter(key) { nv < ov } else { nv > ov };
            if loosened {
                out.push(format!("bench/budgets.json: {bench}.{key} loosened from {ov} to {nv}"));
            }
        }
    }
    out
}

/// Compares two coverage documents; returns every dropped floor.
pub fn floor_drops(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(o), Some(n)) = (old.get("floors").and_then(|f| f.as_object()), new.get("floors").and_then(|f| f.as_object())) else {
        return out;
    };
    for (prefix, ov) in o {
        match n.get(prefix).and_then(|v| v.as_f64()) {
            None => out.push(format!("coverage.json: floor for {prefix} was removed")),
            Some(nv) if nv < ov.as_f64().unwrap_or(0.0) => out.push(format!("coverage.json: floor for {prefix} dropped from {ov} to {nv}")),
            _ => {}
        }
    }
    let empty = BTreeMap::new();
    let _ = &empty;
    out
}

pub fn run(base: &str) -> Result<bool> {
    let root = crate::repo_root();
    let mut problems = Vec::new();
    let messages = commit_messages_since(base).unwrap_or_default();
    if let Some(old) = git_show(base, "bench/budgets.json")? {
        let old: serde_json::Value = serde_json::from_str(&old).unwrap_or_default();
        let new: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("bench/budgets.json"))?)?;
        let l = budget_loosenings(&old, &new);
        if !l.is_empty() && !messages.lines().any(|l| l.trim_start().starts_with("bench-budget:")) {
            problems.extend(l.into_iter().map(|m| format!("{m}; explain it with a `bench-budget:` line in a commit message")));
        }
    }
    if let Some(old) = git_show(base, "coverage.json")? {
        let old: serde_json::Value = serde_json::from_str(&old).unwrap_or_default();
        let new: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("coverage.json"))?)?;
        let d = floor_drops(&old, &new);
        if !d.is_empty() && !messages.lines().any(|l| l.trim_start().starts_with("coverage-floor:")) {
            problems.extend(d.into_iter().map(|m| format!("{m}; explain it with a `coverage-floor:` line in a commit message")));
        }
    }
    for p in &problems {
        eprintln!("{p}");
    }
    if problems.is_empty() {
        println!("ratchet: no budget loosened and no floor dropped against {base}");
    }
    Ok(problems.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_loosening_and_drops() {
        let old = serde_json::json!({ "benches": { "parse/x": { "min_mb_per_s": 100, "max_ms": 10 }, "gone": { "max_ms": 1 } } });
        let new = serde_json::json!({ "benches": { "parse/x": { "min_mb_per_s": 90, "max_ms": 12 } } });
        let l = budget_loosenings(&old, &new);
        assert_eq!(l.len(), 3, "{l:?}");
        let tighter = serde_json::json!({ "benches": { "parse/x": { "min_mb_per_s": 200, "max_ms": 5 }, "gone": { "max_ms": 1 } } });
        assert!(budget_loosenings(&old, &tighter).is_empty());
        let oc = serde_json::json!({ "floors": { "a": 90, "b": 80 } });
        let nc = serde_json::json!({ "floors": { "a": 85 } });
        let d = floor_drops(&oc, &nc);
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(floor_drops(&oc, &serde_json::json!({ "floors": { "a": 95, "b": 80 } })).is_empty());
        assert!(budget_loosenings(&serde_json::json!({}), &new).is_empty());
    }
}
