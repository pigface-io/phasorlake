//! Benchmarks held to `bench/budgets.json`: criterion medians and the `phasor-bench`
//! harness under `/usr/bin/time`. `--tighten` moves a budget only tighter.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Budgets {
    /// Which machine the numbers were taken on.
    pub machine: String,
    /// Slack before a breach counts.
    pub tolerance: Tolerance,
    /// Per benchmark: key -> number. Keys start with `max_` or `min_`.
    pub benches: BTreeMap<String, BTreeMap<String, f64>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct Tolerance {
    /// Percent slack on time and throughput.
    pub time_pct: f64,
    /// Percent slack on memory.
    pub mem_pct: f64,
}

/// A measurement set: bench -> key -> value, using the same keys as budgets.
pub type Measurements = BTreeMap<String, BTreeMap<String, f64>>;

fn is_memory_key(key: &str) -> bool {
    key.contains("rss") || key.contains("heap") || key.contains("alloc")
}

/// Compares measurements to budgets; returns breaches and a table of rows.
/// A measurement in the unit its budget is kept in: criterion's time is
/// `max_ns` under a millisecond and `max_ms` above, so a bench that crosses
/// that line still meets its budget.
fn in_budget_unit<'k>(
    budget: &BTreeMap<String, f64>,
    key: &'k str,
    value: f64,
) -> (std::borrow::Cow<'k, str>, f64) {
    match key {
        "max_ns" if !budget.contains_key(key) && budget.contains_key("max_ms") => {
            ("max_ms".into(), value / 1e6)
        }
        "max_ms" if !budget.contains_key(key) && budget.contains_key("max_ns") => {
            ("max_ns".into(), value * 1e6)
        }
        _ => (key.into(), value),
    }
}

pub fn compare(budgets: &Budgets, measured: &Measurements) -> (Vec<String>, Vec<String>) {
    let mut breaches = Vec::new();
    let mut rows = Vec::new();
    for (bench, keys) in measured {
        let Some(budget) = budgets.benches.get(bench) else {
            breaches.push(format!("{bench}: no budget in bench/budgets.json; add one"));
            continue;
        };
        for (key, value) in keys {
            let (key, value) = in_budget_unit(budget, key, *value);
            let (key, value) = (key.as_ref(), &value);
            let Some(limit) = budget.get(key) else {
                continue;
            };
            let pct = if is_memory_key(key) {
                budgets.tolerance.mem_pct
            } else {
                budgets.tolerance.time_pct
            };
            let slack = limit * pct / 100.0;
            let ok = if key.starts_with("min_") {
                *value >= limit - slack
            } else {
                *value <= limit + slack
            };
            rows.push(format!(
                "{:<4} {bench:<32} {key:<22} {value:>14.3}  budget {limit:>12.3}",
                if ok { "ok" } else { "FAIL" }
            ));
            if !ok {
                breaches.push(format!(
                    "{bench}.{key} = {value:.3} breaks budget {limit:.3} (tolerance {pct}%)"
                ));
            }
        }
    }
    (breaches, rows)
}

/// Moves each budget to the measurement when that is tighter.
pub fn tighten(budgets: &mut Budgets, measured: &Measurements) -> usize {
    let mut n = 0;
    for (bench, keys) in measured {
        let b = budgets.benches.entry(bench.clone()).or_default();
        for (key, value) in keys {
            let (key, value) = in_budget_unit(b, key, *value);
            let (key, value) = (key.as_ref(), &value);
            let slack = if is_memory_key(key) { 1.10 } else { 1.15 };
            let proposed = if key.starts_with("min_") {
                value / slack
            } else {
                value * slack
            };
            let proposed = round3(proposed);
            match b.get(key) {
                Some(cur)
                    if (key.starts_with("min_") && proposed <= *cur)
                        || (!key.starts_with("min_") && proposed >= *cur) => {}
                _ => {
                    b.insert(key.to_owned(), proposed);
                    n += 1;
                }
            }
        }
    }
    n
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[derive(Deserialize)]
struct Estimates {
    median: Estimate,
}
#[derive(Deserialize)]
struct Estimate {
    point_estimate: f64,
}
#[derive(Deserialize)]
struct BenchmarkMeta {
    #[serde(default)]
    throughput: Option<Throughput>,
}
#[derive(Deserialize)]
struct Throughput {
    #[serde(rename = "Bytes", default)]
    bytes: Option<u64>,
    #[serde(rename = "Elements", default)]
    elements: Option<u64>,
}

/// Reads criterion's output for every bench under `target/criterion`.
pub fn read_criterion(target: &Path) -> Result<Measurements> {
    let mut out: Measurements = BTreeMap::new();
    let root = target.join("criterion");
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let new = p.join("new");
            if new.is_dir() {
                let est: Estimates =
                    serde_json::from_str(&std::fs::read_to_string(new.join("estimates.json"))?)?;
                let meta: BenchmarkMeta =
                    serde_json::from_str(&std::fs::read_to_string(new.join("benchmark.json"))?)?;
                let name = p
                    .strip_prefix(&root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                let ns = est.median.point_estimate;
                let keys = out.entry(name).or_default();
                let (bytes, elements) = meta
                    .throughput
                    .map_or((None, None), |t| (t.bytes, t.elements));
                if let Some(bytes) = bytes {
                    keys.insert(
                        "min_mb_per_s".into(),
                        round3(bytes as f64 / 1e6 / (ns / 1e9)),
                    );
                } else if let Some(rows) = elements {
                    keys.insert("min_rows_per_s".into(), round3(rows as f64 / (ns / 1e9)));
                } else if ns > 1e6 {
                    keys.insert("max_ms".into(), round3(ns / 1e6));
                } else {
                    keys.insert("max_ns".into(), round3(ns));
                }
            } else {
                stack.push(p);
            }
        }
    }
    Ok(out)
}

/// Runs of the end-to-end benchmark; each metric keeps its best, so one slow run does not fail.
const E2E_RUNS: usize = 3;

fn run_e2e(root: &Path) -> Result<Measurements> {
    let status = Command::new("cargo")
        .args(["build", "--release", "-q", "-p", "phasor-bench"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("building phasor-bench failed");
    }
    let bin = root.join("target/release/phasor-bench");
    let mut best: Measurements = BTreeMap::new();
    for _ in 0..E2E_RUNS {
        for (bench, keys) in run_e2e_once(root, &bin)? {
            let into = best.entry(bench).or_default();
            for (key, value) in keys {
                let better = |old: f64| {
                    if key.starts_with("min_") {
                        value.max(old)
                    } else {
                        value.min(old)
                    }
                };
                let v = into.get(&key).map_or(value, |&old| better(old));
                into.insert(key, v);
            }
        }
    }
    Ok(best)
}

fn run_e2e_once(root: &Path, bin: &Path) -> Result<Measurements> {
    let out = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/time")
            .arg("-l")
            .arg(bin)
            .current_dir(root)
            .output()?
    } else {
        Command::new("/usr/bin/time")
            .arg("-v")
            .arg(bin)
            .current_dir(root)
            .output()?
    };
    if !out.status.success() {
        bail!(
            "phasor-bench failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut measured: Measurements = serde_json::from_str(stdout.trim())
        .context("phasor-bench did not print JSON measurements")?;
    if let Some(rss) = parse_rss_mb(&stderr) {
        for keys in measured.values_mut() {
            keys.insert("max_rss_mb".into(), round3(rss));
        }
    }
    Ok(measured)
}

/// Peak resident set size in MB from `/usr/bin/time -l` (macOS, bytes) or `-v` (GNU, KiB).
pub fn parse_rss_mb(time_output: &str) -> Option<f64> {
    for line in time_output.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_suffix("maximum resident set size") {
            return rest.trim().parse::<f64>().ok().map(|b| b / 1e6);
        }
        if let Some(rest) = l.strip_prefix("Maximum resident set size (kbytes):") {
            return rest.trim().parse::<f64>().ok().map(|k| k * 1024.0 / 1e6);
        }
    }
    None
}

pub fn run(set: &str, do_tighten: bool) -> Result<bool> {
    let root = crate::repo_root();
    let budgets_path = root.join("bench/budgets.json");
    let mut budgets: Budgets = serde_json::from_str(
        &std::fs::read_to_string(&budgets_path).context("reading bench/budgets.json")?,
    )?;
    // Named explicitly: `--benches` would also run unit tests, which reject criterion flags.
    let targets: &[(&str, &str)] = match set {
        "fast" => &[
            ("phasor-mms-conform", "scan"),
            ("phasor-registry", "resolve"),
            ("phasor-mms-conform", "values"),
            ("phasor-mms-conform", "plain"),
            ("phasor-acquire", "fetch"),
        ],
        "full" => &[
            ("phasor-mms-conform", "scan"),
            ("phasor-registry", "resolve"),
            ("phasor-mms-conform", "values"),
            ("phasor-mms-conform", "plain"),
            ("phasor-acquire", "fetch"),
            ("phasor-writer", "write"),
        ],
        other => bail!("unknown set {other}; use fast or full"),
    };
    let target_dir = root.join("target");
    let _ = std::fs::remove_dir_all(target_dir.join("criterion"));
    for (p, target) in targets {
        let status = Command::new("cargo")
            .args([
                "bench", "-q", "-p", p, "--bench", target, "--", "--quick", "--noplot",
            ])
            .current_dir(&root)
            .status()?;
        if !status.success() {
            bail!("cargo bench -p {p} failed");
        }
    }
    let mut measured = read_criterion(&target_dir)?;
    if set == "full" {
        measured.extend(run_e2e(&root)?);
    }
    if do_tighten {
        let n = tighten(&mut budgets, &measured);
        std::fs::write(
            &budgets_path,
            serde_json::to_string_pretty(&budgets)? + "\n",
        )?;
        println!("tightened {n} budget(s)");
    }
    let (breaches, rows) = compare(&budgets, &measured);
    for r in &rows {
        println!("{r}");
    }
    for b in &breaches {
        eprintln!("{b}");
    }
    Ok(breaches.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budgets() -> Budgets {
        let mut benches = BTreeMap::new();
        benches.insert(
            "parse/x".to_owned(),
            [
                ("min_mb_per_s".to_owned(), 100.0),
                ("max_ns".to_owned(), 10.0),
            ]
            .into_iter()
            .collect(),
        );
        Budgets {
            machine: "test".into(),
            tolerance: Tolerance {
                time_pct: 10.0,
                mem_pct: 5.0,
            },
            benches,
        }
    }

    #[test]
    fn compares_with_tolerance() {
        let b = budgets();
        let mut m: Measurements = BTreeMap::new();
        m.insert(
            "parse/x".into(),
            [
                ("min_mb_per_s".to_owned(), 91.0),
                ("max_ns".to_owned(), 10.9),
            ]
            .into_iter()
            .collect(),
        );
        let (breaches, rows) = compare(&b, &m);
        assert!(breaches.is_empty(), "{breaches:?}");
        assert_eq!(rows.len(), 2);
        m.get_mut("parse/x")
            .unwrap()
            .insert("min_mb_per_s".into(), 80.0);
        m.get_mut("parse/x").unwrap().insert("max_ns".into(), 12.0);
        m.insert(
            "new/bench".into(),
            [("max_ms".to_owned(), 1.0)].into_iter().collect(),
        );
        let (breaches, _) = compare(&b, &m);
        assert_eq!(breaches.len(), 3, "{breaches:?}");
    }

    #[test]
    fn a_time_under_a_millisecond_meets_a_budget_kept_in_ms() {
        let mut b = budgets();
        b.benches.insert(
            "store/split".into(),
            [("max_ms".to_owned(), 2.0)].into_iter().collect(),
        );
        let mut m: Measurements = BTreeMap::new();
        m.insert(
            "store/split".into(),
            [("max_ns".to_owned(), 600_000.0)].into_iter().collect(),
        );
        let (breaches, rows) = compare(&b, &m);
        assert!(breaches.is_empty(), "{breaches:?}");
        assert_eq!(rows.len(), 1, "checked, not skipped");
        m.get_mut("store/split")
            .unwrap()
            .insert("max_ns".into(), 3e6);
        assert_eq!(compare(&b, &m).0.len(), 1);
        assert_eq!(tighten(&mut b, &m), 0);
        m.get_mut("store/split")
            .unwrap()
            .insert("max_ns".into(), 1e6);
        assert_eq!(tighten(&mut b, &m), 1);
        assert_eq!(b.benches["store/split"]["max_ms"], 1.15);
        assert!(!b.benches["store/split"].contains_key("max_ns"));
        // The other way: a budget in ns, a measurement in ms.
        b.benches.insert(
            "parse/y".into(),
            [("max_ns".to_owned(), 2e6)].into_iter().collect(),
        );
        m.insert(
            "parse/y".into(),
            [("max_ms".to_owned(), 1.5)].into_iter().collect(),
        );
        assert!(compare(&b, &m).0.is_empty());
    }

    #[test]
    fn tightens_only_tighter() {
        let mut b = budgets();
        let mut m: Measurements = BTreeMap::new();
        m.insert(
            "parse/x".into(),
            [
                ("min_mb_per_s".to_owned(), 200.0),
                ("max_ns".to_owned(), 20.0),
                ("max_rss_mb".to_owned(), 50.0),
            ]
            .into_iter()
            .collect(),
        );
        let n = tighten(&mut b, &m);
        assert_eq!(n, 2, "{b:?}");
        assert!((b.benches["parse/x"]["min_mb_per_s"] - 173.913).abs() < 0.01);
        assert_eq!(
            b.benches["parse/x"]["max_ns"], 10.0,
            "a looser measurement leaves the budget alone"
        );
        assert_eq!(b.benches["parse/x"]["max_rss_mb"], 55.0);
    }

    #[test]
    fn parses_time_output() {
        assert_eq!(
            parse_rss_mb("  1.00 real\n   123456789  maximum resident set size\n"),
            Some(123.456_789)
        );
        assert_eq!(
            parse_rss_mb("\tMaximum resident set size (kbytes): 2048\n"),
            Some(2.097_152)
        );
        assert_eq!(parse_rss_mb("nothing"), None);
    }
}
