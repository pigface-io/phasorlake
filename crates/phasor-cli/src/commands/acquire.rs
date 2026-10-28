//! The fetch and ingest stages, run over the market's sources and reported.

use anyhow::Result;
use phasor_pipeline::{GetReport, GetRequest, RunOptions};
use phasor_writer::ManifestHandle;
use tokio_util::sync::CancellationToken;

use crate::cli::Cli;
use crate::context::Context;
use crate::render;

/// Plans `req` and runs it: fetched, and ingested unless `options.ingest` is off.
/// A plan that cannot be made prints its diagnostic and returns 4 for the network, else 1.
pub async fn run(
    cli: &Cli,
    ctx: &Context,
    manifest: ManifestHandle,
    req: GetRequest,
    options: RunOptions,
) -> Result<Result<GetReport, i32>> {
    let (tx, rx) = tokio::sync::mpsc::channel(4096);
    let fetcher = ctx.fetcher(Some(tx.clone()))?;
    let deps = ctx.deps(fetcher, manifest);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("interrupted; finishing the files in flight");
            c2.cancel();
        }
    });
    let consumer = tokio::spawn(render::consume_events(
        rx,
        ctx.json,
        ctx.progress,
        cli.verbose > 0,
    ));
    let report = phasor_pipeline::run_get(req, deps, options, tx, cancel).await;
    let _ = consumer.await;
    if let Ok(r) = &report {
        for run in &r.runs {
            super::lake::refresh_iceberg(ctx, run.run_id).await?;
        }
    }
    let report = match report {
        Ok(r) => r,
        Err(d) => {
            if ctx.json {
                render::json_line(&d);
            } else {
                eprintln!("error: {}", d.render());
            }
            // Offline is its own outcome; any other failure to plan is a failure.
            let code = if d.code == "net.offline" {
                i32::from(phasor_core::diag::exit::OFFLINE)
            } else {
                i32::from(phasor_core::diag::exit::FAILURE)
            };
            return Ok(Err(code));
        }
    };
    if !ctx.json {
        for n in &report.plan.notes {
            println!("plan: {n}");
        }
        for (step, run) in report.plan.steps.iter().zip(&report.runs) {
            println!(
                "{}: {} file(s) over {}, {:.1} MB listed",
                step.source.id(),
                step.files.len(),
                step.query.range,
                step.bytes() as f64 / 1e6
            );
            print!("{}", render::summary_text(&run.summary, &run.report));
        }
    }
    Ok(Ok(report))
}

/// The plan and the runs as JSON.
pub fn json(kind: &str, request: &str, report: &GetReport) -> serde_json::Value {
    serde_json::json!({
        "type": kind,
        "request": request,
        "notes": report.plan.notes,
        "steps": report.plan.steps.iter().map(|s| serde_json::json!({
            "source": s.source.id(), "range": s.query.range.to_string(),
            "files": s.files, "bytes": s.bytes(),
        })).collect::<Vec<_>>(),
        "runs": report.runs.iter().map(|r| serde_json::json!({
            "run_id": r.run_id, "exit_code": r.exit_code, "summary": r.summary,
        })).collect::<Vec<_>>(),
        "exit_code": report.exit_code,
    })
}
