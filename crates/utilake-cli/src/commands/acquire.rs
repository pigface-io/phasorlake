//! The fetch and ingest stages, run over the market's sources and reported.

use anyhow::Result;
use tokio_util::sync::CancellationToken;
use utilake_pipeline::{GetReport, GetRequest, RunOptions};
use utilake_writer::ManifestHandle;

use crate::cli::Cli;
use crate::context::Context;
use crate::render;

/// Plans `req` over the kit's sources and runs it: fetched into the
/// archive, and read into the lake unless `options.ingest` is off. Prints
/// the plan and each step's summary as it goes. A plan that cannot be made
/// (offline, a listing refused) is printed as its diagnostic and returned
/// as the exit code a run would have given: 4 for the network, else 1.
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
    let report = utilake_pipeline::run_get(req, deps, options, tx, cancel).await;
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
            // Offline is its own outcome; any other failure to plan, a
            // refusal or a server error included, is a failure.
            let code = if d.code == "net.offline" {
                i32::from(utilake_core::diag::exit::OFFLINE)
            } else {
                i32::from(utilake_core::diag::exit::FAILURE)
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
