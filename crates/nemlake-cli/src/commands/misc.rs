//! `cache` and `wem`.

use anyhow::Result;
use nemlake_core::{Diagnose, YearMonth};
use nemlake_fetch::cache::DiskCache;
use nemlake_fetch::{Query, Source, WemDatafiles};

use crate::cli::{CacheCmd, WemCmd};
use crate::context::Context;
use crate::render;

/// `nemlake cache ...`
pub async fn cache(ctx: &Context, cmd: &CacheCmd) -> Result<i32> {
    let c = DiskCache::new(ctx.home.cache());
    match cmd {
        CacheCmd::Ls => {
            let entries = c
                .entries()
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            if ctx.json {
                for e in &entries {
                    render::json_line(
                        &serde_json::json!({ "path": e.path, "url": e.sidecar.url, "size": e.sidecar.size, "sha256": e.sidecar.sha256, "fetched_at": e.sidecar.fetched_at }),
                    );
                }
            } else {
                let rows: Vec<Vec<String>> = entries
                    .iter()
                    .map(|e| {
                        vec![
                            format!("{:.1} MB", e.sidecar.size as f64 / 1e6),
                            e.sidecar.fetched_at.clone(),
                            e.sidecar.url.clone(),
                        ]
                    })
                    .collect();
                print!("{}", render::table(&["size", "fetched", "url"], &rows));
                println!(
                    "{} file(s), {:.1} MB",
                    entries.len(),
                    entries.iter().map(|e| e.sidecar.size).sum::<u64>() as f64 / 1e6
                );
            }
            Ok(0)
        }
        CacheCmd::Clear => {
            let entries = c
                .entries()
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            let n = entries.len();
            for e in &entries {
                c.remove(e)
                    .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            }
            let listings = ctx.home.listings();
            if listings.is_dir() {
                std::fs::remove_dir_all(&listings)?;
                std::fs::create_dir_all(&listings)?;
            }
            println!("removed {n} cached download(s) and every cached listing");
            Ok(0)
        }
        CacheCmd::Gc {
            ingested,
            older_than,
            keep_partial,
            dry_run,
        } => {
            // (path, bytes, reason) of everything to remove.
            let mut doomed: Vec<(std::path::PathBuf, u64, String)> = c
                .leftovers(
                    i64::try_from(*keep_partial)
                        .unwrap_or(i64::MAX)
                        .saturating_mul(86_400),
                )
                .into_iter()
                .map(|l| (l.path, l.bytes, l.why.to_owned()))
                .collect();
            let entries = c
                .entries()
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            let now = nemlake_core::clock::unix_now();
            let manifest = if *ingested {
                Some(ctx.manifest()?)
            } else {
                None
            };
            let mut blobs = 0usize;
            for e in &entries {
                let old = older_than.is_some_and(|days| {
                    let fetched = nemlake_fetch::cache::parse_rfc3339_secs(&e.sidecar.fetched_at)
                        .unwrap_or(0);
                    now - fetched
                        > i64::try_from(days)
                            .unwrap_or(i64::MAX)
                            .saturating_mul(86_400)
                });
                let in_lake = match &manifest {
                    Some((m, _)) => m
                        .is_ingested(&e.sidecar.url, &e.sidecar.sha256)
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?,
                    None => false,
                };
                let why = match (in_lake, old) {
                    (true, _) => "download already in the lake",
                    (false, true) => "download older than asked",
                    (false, false) => continue,
                };
                blobs += 1;
                for p in [e.path.clone(), DiskCache::sidecar_path(&e.path)] {
                    let bytes = std::fs::metadata(&p).map_or(0, |m| m.len());
                    doomed.push((p, bytes, why.to_owned()));
                }
            }
            drop(manifest);
            let total: u64 = doomed.iter().map(|d| d.1).sum();
            if !*dry_run {
                for (p, _, _) in &doomed {
                    if p.exists() {
                        std::fs::remove_file(p)?;
                    }
                }
            }
            if ctx.json {
                for (p, bytes, why) in &doomed {
                    render::json_line(
                        &serde_json::json!({ "path": p, "bytes": bytes, "why": why, "removed": !*dry_run }),
                    );
                }
            } else {
                if *dry_run {
                    let rows: Vec<Vec<String>> = doomed
                        .iter()
                        .map(|(p, bytes, why)| {
                            vec![
                                format!("{:.1} MB", *bytes as f64 / 1e6),
                                why.clone(),
                                p.display().to_string(),
                            ]
                        })
                        .collect();
                    if !rows.is_empty() {
                        print!("{}", render::table(&["size", "why", "path"], &rows));
                    }
                }
                println!(
                    "{} {} file(s), {:.1} MB: {blobs} download(s), {} leftover(s)",
                    if *dry_run { "would remove" } else { "removed" },
                    doomed.len(),
                    total as f64 / 1e6,
                    doomed.len() - blobs * 2
                );
            }
            Ok(0)
        }
    }
}

/// `nemlake wem ...`
pub async fn wem(ctx: &Context, cmd: &WemCmd) -> Result<i32> {
    match cmd {
        WemCmd::Manifest { dataset, from, to } => {
            let w = WemDatafiles::new(&ctx.config.sources.wem_base_url, vec![dataset.clone()])
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            let fetcher = ctx.fetcher(None, false, None)?;
            let from = from.unwrap_or(YearMonth::new(2000, 1).expect("valid"));
            let to = to.unwrap_or(YearMonth::new(2100, 12).expect("valid"));
            let files = w
                .list(
                    &Query {
                        tables: vec![],
                        from,
                        to,
                    },
                    &fetcher,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            if ctx.json {
                for f in &files {
                    render::json_line(f);
                }
            } else {
                let rows: Vec<Vec<String>> = files
                    .iter()
                    .map(|f| {
                        vec![
                            f.period.map(|p| p.to_string()).unwrap_or_default(),
                            f.modified.clone().unwrap_or_default(),
                            f.url.to_string(),
                        ]
                    })
                    .collect();
                print!("{}", render::table(&["period", "generated", "url"], &rows));
                println!(
                    "{} file(s); downloading WEM data is not implemented yet",
                    files.len()
                );
            }
            Ok(0)
        }
    }
}
