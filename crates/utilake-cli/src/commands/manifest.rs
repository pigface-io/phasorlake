//! `errors`: the diagnostics the manifest recorded.

use anyhow::Result;
use utilake_core::{Diagnose, Severity};
use utilake_writer::manifest::DiagFilter;

use crate::context::Context;
use crate::render;

fn fail(e: &utilake_writer::error::ManifestError) -> anyhow::Error {
    anyhow::anyhow!("{}", e.diagnostic().render())
}

/// `utilake errors`
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
