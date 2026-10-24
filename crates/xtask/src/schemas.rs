//! The tables utilake reads are the ones utilake-schemas publishes: each
//! table's Iceberg table metadata, `{market}/<TABLE>/metadata/v1.metadata.json`,
//! and the manifest of their hashes. `pull` copies them at a commit into
//! `schemas/{market}/pulled/` for every market published and records the
//! commit in each `SOURCE`; `check` holds each copy to the manifest and the
//! commit to the one `Cargo.lock` takes `utilake-iceberg` from, so the
//! histories and the crate that reads them are always of one commit. A
//! market with no table published has no `pulled/`.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The markets utilake-schemas may publish, each a directory of its own.
pub const MARKETS: [&str; 3] = ["nem", "wem", "gas"];

/// Where a market's copy lives, under the repository root.
fn pulled_rel(market: &str) -> String {
    format!("schemas/{market}/pulled")
}

/// The file that names the commit the copy was taken at.
const SOURCE: &str = "SOURCE";

/// The manifest of the copy, as utilake-schemas publishes it.
const MANIFEST: &str = "manifest.json";
/// Which report folder carries each block, beside the tables.
const FOLDERS: &str = "folders.json";

/// Where utilake-schemas publishes a market's folders file.
fn folders_source(market: &str) -> String {
    format!("{market}/{FOLDERS}")
}

/// The market whose folders file a manifest path is, if it is one.
fn folders_market(path: &str) -> Option<&str> {
    match path.split('/').collect::<Vec<_>>().as_slice() {
        [market, FOLDERS] if MARKETS.contains(market) => Some(market),
        _ => None,
    }
}

/// A table's metadata file, under its directory.
const METADATA: &str = "metadata/v1.metadata.json";

fn pulled_dir(market: &str) -> PathBuf {
    crate::repo_root().join(pulled_rel(market))
}

/// The market and table a manifest path names, when it is a
/// `{market}/<TABLE>/metadata/v1.metadata.json`.
fn table_of(path: &str) -> Option<(&str, &str)> {
    match path.split('/').collect::<Vec<_>>().as_slice() {
        [market, table, "metadata", "v1.metadata.json"]
            if MARKETS.contains(market) && !table.is_empty() =>
        {
            Some((market, table))
        }
        _ => None,
    }
}

fn git(checkout: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git -C {} {}: {}",
            checkout.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The bytes of each `paths` at `rev`, from one `git cat-file --batch`.
fn blobs(checkout: &Path, rev: &str, paths: &[String]) -> Result<Vec<Vec<u8>>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("running git cat-file")?;
    let mut stdin = child.stdin.take().context("git cat-file has a stdin")?;
    let request: String = paths.iter().map(|p| format!("{rev}:{p}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(request.as_bytes()));
    let mut out = BufReader::new(child.stdout.take().context("git cat-file has a stdout")?);
    let mut found = Vec::with_capacity(paths.len());
    for p in paths {
        let mut header = String::new();
        out.read_line(&mut header)?;
        let size: usize = match header.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_, "blob", size] => size.parse()?,
            _ => bail!("{rev}:{p} is not a file: {}", header.trim()),
        };
        let mut bytes = vec![0; size + 1];
        out.read_exact(&mut bytes)?;
        bytes.pop();
        found.push(bytes);
    }
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("the writer to git cat-file panicked"))??;
    child.wait()?;
    Ok(found)
}

/// Copies every market's tables' metadata, its folders file and the
/// manifest of `rev` in `checkout` into `schemas/{market}/pulled/`,
/// whatever the checkout's working tree holds, and writes the commit to
/// each `SOURCE`. A market the commit publishes no table of has no copy.
pub fn pull(rev: &str, checkout: &Path) -> Result<bool> {
    let full = git(
        checkout,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?
    .trim()
    .to_owned();
    let mut args = vec![
        "ls-tree",
        "-r",
        "--name-only",
        full.as_str(),
        "--",
        MANIFEST,
    ];
    args.extend(MARKETS);
    let listing = git(checkout, &args)?;
    let paths: Vec<String> = listing
        .lines()
        .filter(|p| *p == MANIFEST || folders_market(p).is_some() || table_of(p).is_some())
        .map(str::to_owned)
        .collect();
    if !paths.iter().any(|p| p == MANIFEST) {
        bail!(
            "{full} has no {MANIFEST}; is {} utilake-schemas?",
            checkout.display()
        );
    }
    let bytes = blobs(checkout, &full, &paths)?;
    let manifest = paths
        .iter()
        .position(|p| p == MANIFEST)
        .map(|i| bytes[i].clone())
        .expect("the manifest is among the paths");
    let mut report = Vec::new();
    for market in MARKETS {
        let dir = pulled_dir(market);
        let tables = paths
            .iter()
            .filter(|p| table_of(p).is_some_and(|(m, _)| m == market))
            .count();
        if tables == 0 {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            continue;
        }
        let tmp = dir.with_extension("tmp");
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?;
        }
        std::fs::create_dir_all(&tmp)?;
        let mut folders = false;
        for (p, b) in paths.iter().zip(&bytes) {
            let to = match (table_of(p), folders_market(p)) {
                (Some((m, t)), _) if m == market => tmp.join(t).join(METADATA),
                (_, Some(m)) if m == market => {
                    folders = true;
                    tmp.join(FOLDERS)
                }
                _ => continue,
            };
            std::fs::create_dir_all(to.parent().expect("a file in a directory"))?;
            std::fs::write(&to, b).with_context(|| format!("writing {}", to.display()))?;
        }
        std::fs::write(tmp.join(MANIFEST), &manifest)?;
        std::fs::write(tmp.join(SOURCE), format!("{full}\n"))?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::rename(&tmp, &dir)?;
        report.push(format!(
            "{tables} {market} tables{}",
            if folders { " with folders.json" } else { "" }
        ));
    }
    println!(
        "schemas-pull: {} of utilake-schemas {full} into schemas/{{market}}/pulled",
        report.join(", ")
    );
    Ok(true)
}

/// The commit `Cargo.lock` takes `utilake-iceberg` from.
pub fn locked_rev(lock: &str) -> Option<&str> {
    let mut in_package = false;
    for line in lock.lines() {
        if line.starts_with("[[package]]") {
            in_package = false;
        } else if line == "name = \"utilake-iceberg\"" {
            in_package = true;
        } else if in_package && let Some(source) = line.strip_prefix("source = ") {
            return source
                .trim_matches('"')
                .rsplit_once('#')
                .map(|(_, rev)| rev);
        }
    }
    None
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// What is wrong with `market`'s copy in `dir`, held to its manifest and
/// to the commit `lock` names. Empty when it is right.
pub fn problems(dir: &Path, lock: &str, market: &str) -> Vec<String> {
    let pulled = pulled_rel(market);
    let mut out = Vec::new();
    let source = std::fs::read_to_string(dir.join(SOURCE)).unwrap_or_default();
    let source = source.trim();
    match locked_rev(lock) {
        None => out.push("Cargo.lock takes utilake-iceberg from no git commit".to_owned()),
        Some(rev) if rev != source => out.push(format!(
            "{pulled}/{SOURCE} is {source:?} but Cargo.lock takes utilake-iceberg from {rev}; run `just schemas-pull {rev}`"
        )),
        Some(_) => {}
    }
    let manifest: serde_json::Value = match std::fs::read(dir.join(MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        Some(m) => m,
        None => {
            out.push(format!("{pulled}/{MANIFEST} is missing or not JSON"));
            return out;
        }
    };
    let want: BTreeMap<String, String> = manifest
        .get("sha256")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(path, sha)| {
            let (m, table) = table_of(path)?;
            if m != market {
                return None;
            }
            Some((table.to_owned(), sha.as_str()?.to_owned()))
        })
        .collect();
    if want.is_empty() {
        out.push(format!("{pulled}/{MANIFEST} lists no {market} table"));
    }
    // folders.json is held to the manifest too, when published.
    let folders_sha = manifest
        .get("sha256")
        .and_then(|m| m.get(folders_source(market)))
        .and_then(serde_json::Value::as_str);
    match (folders_sha, std::fs::read(dir.join(FOLDERS))) {
        (Some(sha), Ok(bytes)) if sha256(&bytes) != sha => out.push(format!(
            "{FOLDERS} has SHA-256 {} where the manifest says {sha}; pulled files are not edited",
            sha256(&bytes)
        )),
        (Some(_), Err(_)) => out.push(format!("{FOLDERS} is in the manifest but not pulled")),
        (None, Ok(_)) => out.push(format!("{FOLDERS} is pulled but the manifest lists none")),
        _ => {}
    }
    for (table, sha) in &want {
        let path = dir.join(table).join(METADATA);
        match std::fs::read(&path) {
            Err(_) => out.push(format!("{table}/{METADATA} is in the manifest but not pulled")),
            Ok(bytes) if sha256(&bytes) != *sha => out.push(format!(
                "{table}/{METADATA} has SHA-256 {} where the manifest says {sha}; pulled files are not edited",
                sha256(&bytes)
            )),
            Ok(_) => {}
        }
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for p in rd.filter_map(Result::ok).map(|e| e.path()) {
            let rel = p
                .strip_prefix(dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned();
            let known = match rel.split('/').collect::<Vec<_>>().as_slice() {
                [SOURCE | MANIFEST | FOLDERS] => true,
                [table] => p.is_dir() && want.contains_key(*table),
                [table, "metadata"] => p.is_dir() && want.contains_key(*table),
                [table, "metadata", "v1.metadata.json"] => want.contains_key(*table),
                _ => false,
            };
            if !known {
                out.push(format!("{rel} is not a file the manifest lists"));
            } else if p.is_dir() {
                stack.push(p);
            }
        }
    }
    out.sort();
    out
}

/// `schemas-check`: every market's copy is the manifest's, and of the
/// locked commit; a market without a copy has nothing to check.
pub fn check() -> Result<bool> {
    let lock = std::fs::read_to_string(crate::repo_root().join("Cargo.lock"))
        .context("reading Cargo.lock")?;
    let mut ok = true;
    for market in MARKETS {
        let dir = pulled_dir(market);
        if !dir.exists() {
            continue;
        }
        let problems = problems(&dir, &lock, market);
        for p in &problems {
            println!("schemas-check: {p}");
        }
        if problems.is_empty() {
            println!(
                "schemas-check: {} is the manifest's, of the commit Cargo.lock pins",
                pulled_rel(market)
            );
        }
        ok &= problems.is_empty();
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = "[[package]]\nname = \"other\"\nsource = \"git+ssh://x?rev=1#1\"\n\n[[package]]\nname = \"utilake-iceberg\"\nversion = \"0.1.0\"\nsource = \"git+ssh://git@github.com/g0itb4/utilake-schemas.git?rev=abc#abc\"\n";

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("REGION").join("metadata");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("v1.metadata.json"), b"{}").unwrap();
        std::fs::write(dir.path().join(FOLDERS), b"{\"folders\":[]}").unwrap();
        let manifest = serde_json::json!({"sha256": {
            "nem/REGION/metadata/v1.metadata.json": sha256(b"{}"),
            "nem/REGION/schema-1.json": "00",
            "nem/REGION/metadata/snap-1-1-u.avro": "00",
            "nem/folders.json": sha256(b"{\"folders\":[]}"),
        }});
        std::fs::write(dir.path().join(MANIFEST), manifest.to_string()).unwrap();
        std::fs::write(dir.path().join(SOURCE), "abc\n").unwrap();
        dir
    }

    #[test]
    fn a_copy_of_the_locked_commit_has_no_problem() {
        assert_eq!(locked_rev(LOCK), Some("abc"));
        assert_eq!(locked_rev("[[package]]\nname = \"x\"\n"), None);
        assert_eq!(
            table_of("nem/REGION/metadata/v1.metadata.json"),
            Some(("nem", "REGION"))
        );
        assert_eq!(
            table_of("gas/GBB_FACILITIES/metadata/v1.metadata.json"),
            Some(("gas", "GBB_FACILITIES"))
        );
        assert_eq!(table_of("nem/REGION/schema-1.json"), None);
        assert_eq!(table_of("nem/REGION/table.json"), None);
        assert_eq!(table_of("mars/X/metadata/v1.metadata.json"), None);
        assert_eq!(folders_market("gas/folders.json"), Some("gas"));
        assert_eq!(folders_market("gas/x/folders.json"), None);
        let dir = tree();
        assert_eq!(problems(dir.path(), LOCK, "nem"), Vec::<String>::new());
        // The same copy read as another market's: its tables are not that
        // market's, and its folders file is not listed for it.
        let p = problems(dir.path(), LOCK, "gas");
        assert!(p.iter().any(|m| m.contains("lists no gas table")), "{p:?}");
    }

    #[test]
    fn each_problem_is_named() {
        let dir = tree();
        std::fs::write(dir.path().join("REGION/metadata/v1.metadata.json"), b"{ }").unwrap();
        std::fs::write(dir.path().join("stray.txt"), b"").unwrap();
        std::fs::create_dir_all(dir.path().join("OTHER")).unwrap();
        std::fs::write(dir.path().join(FOLDERS), b"{\"folders\":[1]}").unwrap();
        let p = problems(dir.path(), &LOCK.replace("abc", "def"), "nem");
        assert_eq!(p.len(), 5, "{p:?}");
        assert!(p.iter().any(|m| m.starts_with("folders.json has SHA-256")));
        assert!(p.iter().any(|m| m.contains("not edited")));
        assert!(p.iter().any(|m| m.starts_with("stray.txt")));
        assert!(p.iter().any(|m| m.starts_with("OTHER")));
        assert!(p.iter().any(|m| m.contains("just schemas-pull def")));
        std::fs::remove_file(dir.path().join("REGION/metadata/v1.metadata.json")).unwrap();
        let p = problems(dir.path(), "", "nem");
        assert!(p.iter().any(|m| m.contains("but not pulled")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("from no git commit")), "{p:?}");
        std::fs::write(dir.path().join(MANIFEST), "{}").unwrap();
        assert!(
            problems(dir.path(), LOCK, "nem")
                .iter()
                .any(|m| m.contains("lists no nem table"))
        );
        std::fs::remove_file(dir.path().join(MANIFEST)).unwrap();
        assert!(
            problems(dir.path(), LOCK, "nem")
                .iter()
                .any(|m| m.contains("missing"))
        );
        // folders.json pulled without a manifest entry, or listed but not
        // pulled.
        let dir = tree();
        let mut m: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(MANIFEST)).unwrap()).unwrap();
        m["sha256"]
            .as_object_mut()
            .unwrap()
            .remove("nem/folders.json");
        std::fs::write(dir.path().join(MANIFEST), m.to_string()).unwrap();
        let p = problems(dir.path(), LOCK, "nem");
        assert_eq!(p, ["folders.json is pulled but the manifest lists none"]);
        let dir = tree();
        std::fs::remove_file(dir.path().join(FOLDERS)).unwrap();
        let p = problems(dir.path(), LOCK, "nem");
        assert_eq!(p, ["folders.json is in the manifest but not pulled"]);
    }
}
