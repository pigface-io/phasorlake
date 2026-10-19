//! `archive` and `wem`.

use anyhow::Result;
use utilake_acquire::{Query, Source, WemDatafiles};
use utilake_core::{Diagnose, YearMonth};

use crate::cli::{ArchiveCmd, WemCmd};
use crate::context::Context;
use crate::render;

/// `utilake archive ...`
pub fn archive(ctx: &Context, cmd: &ArchiveCmd) -> Result<i32> {
    let store = ctx.archive();
    let entries = store.entries_under(None);
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
                        vec![
                            format!("{:.1} MB", e.sidecar.size as f64 / 1e6),
                            e.sidecar.fetched_at.clone(),
                            e.sidecar.last_modified.clone().unwrap_or_default(),
                            e.sidecar
                                .last_modified_at
                                .map(|s| s.rfc3339())
                                .unwrap_or_default(),
                            e.sidecar.url.clone(),
                        ]
                    })
                    .collect();
                print!(
                    "{}",
                    render::table(
                        &["size", "fetched", "published", "published_at", "url"],
                        &rows,
                    )
                );
                println!(
                    "{} file(s), {:.1} MB, under {}",
                    entries.len(),
                    entries.iter().map(|e| e.sidecar.size).sum::<u64>() as f64 / 1e6,
                    store.root().display()
                );
            }
            Ok(0)
        }
        ArchiveCmd::Verify => {
            let mut bad = 0usize;
            for e in &entries {
                let result = utilake_acquire::Archive::verify(e);
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

/// `utilake wem ...`
pub async fn wem(ctx: &Context, cmd: &WemCmd) -> Result<i32> {
    match cmd {
        WemCmd::Manifest { dataset, from, to } => {
            let w = WemDatafiles::new(&ctx.config.sources.wem_base_url, vec![dataset.clone()])
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            let fetcher = ctx.fetcher(None)?;
            let from = from.unwrap_or(YearMonth::new(2000, 1).expect("valid"));
            let to = to.unwrap_or(YearMonth::new(2100, 12).expect("valid"));
            let files = w
                .list(
                    &Query {
                        exclude: Vec::new(),
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
                            f.modified_at.map(|s| s.rfc3339()).unwrap_or_default(),
                            f.url.to_string(),
                        ]
                    })
                    .collect();
                print!(
                    "{}",
                    render::table(&["period", "generated", "generated_at", "url"], &rows)
                );
                println!(
                    "{} file(s); downloading WEM data is not implemented yet",
                    files.len()
                );
            }
            Ok(0)
        }
    }
}
