//! `cache` and `wem`.

use anyhow::Result;
use nemlake_core::{Diagnose, YearMonth};
use nemlake_fetch::cache::DiskCache;
use nemlake_fetch::{Query, Source, WemDatafiles};

use crate::cli::{CacheCmd, WemCmd};
use crate::context::Context;
use crate::render;

/// `nemlake cache ...`
pub fn cache(ctx: &Context, cmd: &CacheCmd) -> Result<i32> {
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
                    "{} file(s); downloading WEM data arrives in a later milestone",
                    files.len()
                );
            }
            Ok(0)
        }
    }
}
