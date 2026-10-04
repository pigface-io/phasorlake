//! `ls` and `errors`: reading the manifest.

use anyhow::Result;
use nemlake_core::{Diagnose, Severity};
use nemlake_writer::manifest::DiagFilter;

use crate::context::Context;
use crate::render;

fn fail(e: &nemlake_writer::error::ManifestError) -> anyhow::Error {
    anyhow::anyhow!("{}", e.diagnostic().render())
}

/// `nemlake ls`
pub async fn ls(
    ctx: &Context,
    table: Option<String>,
    failed: bool,
    runs: bool,
    limit: usize,
) -> Result<i32> {
    let (m, thread) = ctx.manifest()?;
    if runs {
        let latest = m.latest_run().await.map_err(|e| fail(&e))?;
        drop(thread);
        if ctx.json {
            render::json_line(&latest);
        } else if let Some(r) = latest {
            print!(
                "{}",
                render::table(
                    &["run", "started", "finished", "exit", "argv"],
                    &[vec![
                        r.id.to_string(),
                        r.started_at,
                        r.finished_at.unwrap_or_default(),
                        r.exit_code.map(|c| c.to_string()).unwrap_or_default(),
                        r.argv
                    ]]
                )
            );
        } else {
            println!("no runs yet");
        }
        return Ok(0);
    }
    let ingests = m
        .list_ingests(table, failed, limit)
        .await
        .map_err(|e| fail(&e))?;
    drop(thread);
    if ctx.json {
        for i in &ingests {
            render::json_line(i);
        }
    } else {
        let rows: Vec<Vec<String>> = ingests
            .iter()
            .map(|i| {
                vec![
                    i.id.to_string(),
                    i.table.clone(),
                    i.schema_version.to_string(),
                    format!("{:?}", i.status).to_ascii_lowercase(),
                    i.rows.to_string(),
                    i.bad_rows.to_string(),
                    i.outputs.len().to_string(),
                    i.error_code.clone().unwrap_or_default(),
                    format!("{}!{}", i.url.rsplit('/').next().unwrap_or(&i.url), i.entry),
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(
                &[
                    "id", "table", "ver", "status", "rows", "bad", "files", "error", "source"
                ],
                &rows
            )
        );
    }
    Ok(0)
}

/// `nemlake errors`
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
        m.latest_run().await.map_err(|e| fail(&e))?.map(|r| r.id)
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
    let diags = m.list_diagnostics(filter).await.map_err(|e| fail(&e))?;
    drop(thread);
    // Diagnostics are JSON either way: one object a line, newest first
    // with `--json`, oldest first without, where an empty list says so.
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
