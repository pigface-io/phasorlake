//! `cache` and `wem`.

use anyhow::Result;
use nemlake_core::{Diagnose, YearMonth};
use nemlake_fetch::{Query, Source, WemDatafiles};

use crate::cli::{ArchiveCmd, CacheCmd, WemCmd};
use crate::context::Context;
use crate::render;

/// `nemlake archive ...`
pub fn archive(ctx: &Context, cmd: &ArchiveCmd) -> Result<i32> {
    let store = ctx.store();
    let entries = store
        .entries()
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
    match cmd {
        ArchiveCmd::Ls => {
            if ctx.json {
                for e in &entries {
                    render::json_line(&serde_json::json!({ "path": e.path, "record": e.sidecar }));
                }
            } else {
                let rows: Vec<Vec<String>> = entries
                    .iter()
                    .map(|e| {
                        let how = e.sidecar.request.as_ref().map_or_else(String::new, |r| {
                            match (&r.member, &r.range) {
                                (Some(_), _) => "member of a zip".to_owned(),
                                (None, Some(range)) => format!("GET {range}"),
                                (None, None) => "GET".to_owned(),
                            }
                        });
                        vec![
                            format!("{:.1} MB", e.sidecar.size as f64 / 1e6),
                            e.sidecar.fetched_at.clone(),
                            how,
                            e.sidecar
                                .request
                                .as_ref()
                                .map_or_else(String::new, |r| r.by.clone()),
                            e.sidecar.url.clone(),
                        ]
                    })
                    .collect();
                print!(
                    "{}",
                    render::table(&["size", "fetched", "how", "requested by", "url"], &rows)
                );
                println!(
                    "{} file(s), {:.1} MB, under {}",
                    entries.len(),
                    entries.iter().map(|e| e.sidecar.size).sum::<u64>() as f64 / 1e6,
                    store.archive_root().display()
                );
            }
            Ok(0)
        }
        ArchiveCmd::Verify => {
            let mut bad = 0usize;
            for e in &entries {
                let result = store.verify(e);
                if let Err(err) = &result {
                    bad += 1;
                    let d = err.diagnostic();
                    if ctx.json {
                        render::json_line(&d);
                    } else {
                        eprintln!("{}", d.render());
                    }
                }
            }
            if !ctx.json {
                println!(
                    "{} file(s) checked, {bad} do not match their record",
                    entries.len()
                );
            }
            Ok(i32::from(bad > 0))
        }
    }
}

/// `nemlake cache ...`
pub fn cache(ctx: &Context, cmd: &CacheCmd) -> Result<i32> {
    let store = ctx.store();
    match cmd {
        CacheCmd::Clear => {
            let listings = ctx.home.listings();
            let n = std::fs::read_dir(&listings).map_or(0, |d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_none_or(|x| x != "json"))
                    .count()
            });
            if listings.is_dir() {
                std::fs::remove_dir_all(&listings)?;
                std::fs::create_dir_all(&listings)?;
            }
            println!("removed {n} cached listing(s); the archive of downloads is untouched");
            Ok(0)
        }
        CacheCmd::Gc {
            keep_partial,
            dry_run,
        } => {
            let doomed = store.leftovers(
                i64::try_from(*keep_partial)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(86_400),
            );
            let total: u64 = doomed.iter().map(|l| l.bytes).sum();
            if !*dry_run {
                for l in &doomed {
                    if l.path.exists() {
                        std::fs::remove_file(&l.path)?;
                    }
                }
            }
            if ctx.json {
                for l in &doomed {
                    render::json_line(
                        &serde_json::json!({ "path": l.path, "bytes": l.bytes, "why": l.why, "removed": !*dry_run }),
                    );
                }
            } else {
                if *dry_run && !doomed.is_empty() {
                    let rows: Vec<Vec<String>> = doomed
                        .iter()
                        .map(|l| {
                            vec![
                                format!("{:.1} MB", l.bytes as f64 / 1e6),
                                l.why.to_owned(),
                                l.path.display().to_string(),
                            ]
                        })
                        .collect();
                    print!("{}", render::table(&["size", "why", "path"], &rows));
                }
                println!(
                    "{} {} leftover file(s), {:.1} MB; archived downloads are never removed",
                    if *dry_run { "would remove" } else { "removed" },
                    doomed.len(),
                    total as f64 / 1e6
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
            let fetcher = ctx.fetcher(&format!("wem manifest {dataset}"), false, None)?;
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
