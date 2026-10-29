//! `errors`: the manifest's diagnostics; and `wem`, a dataset's file list.

use anyhow::Result;
use phasor_core::{Diagnose, Severity, YearMonth};
use phasor_kit::{Query, Source};
use phasor_wem::WemDatafiles;
use phasor_writer::manifest::DiagFilter;

use crate::cli::WemCmd;
use crate::context::Context;
use crate::render;

/// `phasor errors`
#[allow(clippy::too_many_arguments)]
pub async fn errors(
    ctx: &Context,
    run: Option<i64>,
    last: bool,
    table: Option<String>,
    code: Option<String>,
    errors_only: bool,
    limit: usize,
) -> Result<i32> {
    let (m, thread) = ctx.manifest()?;
    let run_id = if last {
        m.latest_run()
            .await
            .map_err(|e| super::common::fail(&e))?
            .map(|r| r.id)
    } else {
        run
    };
    let filter = DiagFilter {
        run_id,
        table,
        code,
        severity: errors_only.then_some(Severity::Error),
        limit,
    };
    let diags = m
        .list_diagnostics(filter)
        .await
        .map_err(|e| super::common::fail(&e))?;
    drop(thread);
    // JSON lines either way: newest first with `--json`, oldest first without.
    if ctx.json {
        for (run, ingest, d) in &diags {
            render::json_line(
                &serde_json::json!({ "run_id": run, "ingest_id": ingest, "diagnostic": d }),
            );
        }
    } else if diags.is_empty() {
        println!("no diagnostics");
    } else {
        for (run, ingest, d) in diags.iter().rev() {
            render::json_line(
                &serde_json::json!({ "run_id": run, "ingest_id": ingest, "diagnostic": d }),
            );
        }
    }
    Ok(0)
}

/// `phasor wem ...`
pub async fn wem(ctx: &Context, cmd: &WemCmd) -> Result<i32> {
    match cmd {
        WemCmd::Manifest { dataset, from, to } => {
            let base = phasor_wem::WemConfig::from_sources(&ctx.config.sources)
                .map_err(|e| anyhow::anyhow!("{e} (in the config file)"))?
                .base_url;
            let w = WemDatafiles::new(&base, vec![dataset.clone()])
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            let fetcher = ctx.fetcher(None)?;
            let from = from.unwrap_or(YearMonth::new(2000, 1).expect("valid"));
            let to = to.unwrap_or(YearMonth::new(2100, 12).expect("valid"));
            let files = w
                .list(
                    &Query {
                        exclude: Vec::new(),
                        tables: vec![],
                        range: phasor_core::TimeRange::months(from, to),
                        instants: None,
                    },
                    fetcher.as_ref(),
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
