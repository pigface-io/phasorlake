//! Every source file is held to the floor of the longest matching prefix in
//! `coverage.json`; a file under no floor fails, so new code never arrives
//! without a decision about how well it is tested. Lines, not percent of
//! files: 100 means every line.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    /// Path prefixes the gate ignores.
    pub exclude: Vec<String>,
    /// Path prefix to minimum line coverage, in percent. The longest wins.
    pub floors: BTreeMap<String, f64>,
}

/// Per-file (hit, total) from an lcov profile, paths relative to the root.
pub fn parse_lcov(text: &str, root: &Path) -> BTreeMap<String, (u64, u64)> {
    let mut out: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("SF:") {
            let p = Path::new(path);
            let rel = p
                .strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            current = Some(rel);
        } else if let Some(rest) = line.strip_prefix("DA:")
            && let Some(file) = &current
        {
            let mut parts = rest.split(',');
            let _line_no = parts.next();
            let hits: u64 = parts.next().and_then(|h| h.parse().ok()).unwrap_or(0);
            let e = out.entry(file.clone()).or_insert((0, 0));
            e.1 += 1;
            if hits > 0 {
                e.0 += 1;
            }
        } else if line == "end_of_record" {
            current = None;
        }
    }
    out
}

/// Checks files against the config; returns every failure message.
pub fn check(config: &Config, files: &BTreeMap<String, (u64, u64)>) -> Vec<String> {
    let mut failures = Vec::new();
    for (file, (hit, total)) in files {
        if !file.starts_with("crates/") || config.exclude.iter().any(|x| file.starts_with(x)) {
            continue;
        }
        let floor = config
            .floors
            .iter()
            .filter(|(prefix, _)| file.starts_with(prefix.as_str()))
            .max_by_key(|(p, _)| p.len());
        match floor {
            None => failures.push(format!(
                "{file}: no coverage floor covers this file; add one to coverage.json"
            )),
            Some((prefix, floor)) => {
                let pct = if *total == 0 {
                    100.0
                } else {
                    *hit as f64 * 100.0 / *total as f64
                };
                if pct + 1e-9 < *floor {
                    failures.push(format!("{file}: {pct:.1}% of {total} lines covered; the floor for {prefix} is {floor}%"));
                }
            }
        }
    }
    failures
}

pub fn run(lcov: &Path) -> Result<bool> {
    let root = crate::repo_root();
    let config: Config = serde_json::from_str(
        &std::fs::read_to_string(root.join("coverage.json")).context("reading coverage.json")?,
    )?;
    let text =
        std::fs::read_to_string(lcov).with_context(|| format!("reading {}", lcov.display()))?;
    let files = parse_lcov(&text, &root);
    let failures = check(&config, &files);
    for f in &failures {
        eprintln!("{f}");
    }
    let (hit, total) = files
        .iter()
        .filter(|(f, _)| f.starts_with("crates/"))
        .fold((0u64, 0u64), |(h, t), (_, (fh, ft))| (h + fh, t + ft));
    println!(
        "coverage: {hit}/{total} lines ({:.1}%) over {} files; {} failure(s)",
        if total == 0 {
            100.0
        } else {
            hit as f64 * 100.0 / total as f64
        },
        files.len(),
        failures.len()
    );
    Ok(failures.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_gates() {
        let lcov = "SF:/r/crates/a/src/x.rs\nDA:1,1\nDA:2,0\nend_of_record\nSF:/r/crates/b/src/y.rs\nDA:1,3\nend_of_record\nSF:/r/other/z.rs\nDA:1,0\nend_of_record\n";
        let files = parse_lcov(lcov, Path::new("/r"));
        assert_eq!(files["crates/a/src/x.rs"], (1, 2));
        assert_eq!(files["crates/b/src/y.rs"], (1, 1));
        let config = Config {
            exclude: vec![],
            floors: [("crates/a/src".to_owned(), 60.0)].into_iter().collect(),
        };
        let f = check(&config, &files);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[0].contains("50.0% of 2 lines"));
        assert!(f[1].contains("no coverage floor"));
        let config = Config {
            exclude: vec!["crates/b/".into()],
            floors: [("crates/a/src".to_owned(), 50.0)].into_iter().collect(),
        };
        assert!(check(&config, &files).is_empty());
    }
}
