//! `layers.json` declares, for each crate of the workspace, the crates of
//! the family (`utilake-*`) it may depend on outside its tests. The gate
//! fails on an edge the file does not list, so a crate cannot start leaning
//! on another without the layering saying so, and on a listed edge that is
//! gone, so the file never claims more than the code does.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

const FILE: &str = "layers.json";

/// Each crate's family dependencies.
pub type Graph = BTreeMap<String, BTreeSet<String>>;

/// What is wrong with `actual` against `declared`, one line each.
pub fn problems(declared: &Graph, actual: &Graph) -> Vec<String> {
    let mut out = Vec::new();
    for (krate, deps) in actual {
        let Some(allowed) = declared.get(krate) else {
            out.push(format!("{krate}: not in {FILE}"));
            continue;
        };
        for d in deps.difference(allowed) {
            out.push(format!("{krate} -> {d}: not allowed by {FILE}"));
        }
        for d in allowed.difference(deps) {
            out.push(format!(
                "{krate} -> {d}: listed in {FILE} but gone; remove it"
            ));
        }
    }
    for krate in declared.keys().filter(|k| !actual.contains_key(*k)) {
        out.push(format!("{krate}: in {FILE} but not a workspace crate"));
    }
    out
}

/// The workspace's family edges, from `cargo metadata`: normal and build
/// dependencies, not dev ones.
fn actual(root: &Path) -> Result<Graph> {
    let out = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()
        .context("cargo metadata")?;
    anyhow::ensure!(
        out.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let mut graph = Graph::new();
    for p in meta["packages"].as_array().context("packages")? {
        let name = p["name"].as_str().context("name")?.to_owned();
        let deps = p["dependencies"]
            .as_array()
            .context("dependencies")?
            .iter()
            .filter(|d| d["kind"].as_str() != Some("dev"))
            .filter_map(|d| d["name"].as_str())
            .filter(|d| d.starts_with("utilake-"))
            .map(str::to_owned)
            .collect();
        graph.insert(name, deps);
    }
    Ok(graph)
}

pub fn run(root: &Path) -> Result<bool> {
    let path = root.join(FILE);
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let declared: Graph = serde_json::from_str(&text).with_context(|| FILE.to_owned())?;
    let found = problems(&declared, &actual(root)?);
    for p in &found {
        eprintln!("layers: {p}");
    }
    Ok(found.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> Graph {
        edges
            .iter()
            .map(|(k, ds)| {
                (
                    (*k).to_owned(),
                    ds.iter().map(|d| (*d).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn an_edge_not_declared_or_declared_and_gone_fails() {
        let declared = graph(&[("a", &["core"]), ("core", &[]), ("old", &[])]);
        let actual = graph(&[("a", &["kit"]), ("core", &[]), ("new", &[])]);
        assert_eq!(
            problems(&declared, &actual),
            [
                "a -> kit: not allowed by layers.json",
                "a -> core: listed in layers.json but gone; remove it",
                "new: not in layers.json",
                "old: in layers.json but not a workspace crate",
            ]
        );
        assert!(problems(&actual, &actual).is_empty());
    }
}
