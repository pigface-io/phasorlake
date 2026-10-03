//! `fetch`, `ingest`, `verify`: the pipeline and the lake.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use nemlake_core::Diagnose;
use nemlake_fetch::{MmsdmArchive, Query};
use nemlake_pipeline::{Input, Plan, RunOptions};
use nemlake_schema::Mode;
use tokio_util::sync::CancellationToken;

use crate::cli::{Cli, FetchMarket, IngestArgs};
use crate::context::Context;
use crate::render;

fn options(
    cli: &Cli,
    ctx: &Context,
    args: &IngestArgs,
    downloads: Option<usize>,
    dry_run: bool,
) -> RunOptions {
    RunOptions {
        jobs: cli.jobs.unwrap_or_else(|| ctx.config.effective_jobs()),
        downloads: downloads.unwrap_or(ctx.config.downloads).max(1),
        mode: if args.strict || ctx.config.strict {
            Mode::Strict
        } else {
            Mode::Permissive
        },
        max_bad_rows: args.max_bad_rows.unwrap_or(ctx.config.max_bad_rows),
        fail_fast: args.fail_fast,
        skip_unknown: !args.no_skip_unknown,
        dry_run,
        strict_footer: args.strict || ctx.config.strict,
        batch_rows: nemlake_mms::arrow_sink::DEFAULT_BATCH_ROWS,
    }
}

async fn run_plan(
    cli: &Cli,
    ctx: &Context,
    plan: Plan,
    refresh: bool,
    downloads: Option<usize>,
) -> Result<i32> {
    let (tx, rx) = tokio::sync::mpsc::channel(4096);
    let fetcher = ctx.fetcher(downloads, refresh, Some(tx.clone()))?;
    let (manifest, thread) = ctx.manifest()?;
    let deps = ctx.deps(fetcher, manifest);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("interrupted; finishing the files in flight");
            c2.cancel();
        }
    });
    let json = ctx.json;
    let progress = ctx.progress;
    let verbose = cli.verbose > 0;
    let consumer = tokio::spawn(render::consume_events(rx, json, progress, verbose));
    let report = nemlake_pipeline::run(plan, deps, tx, cancel).await;
    let _ = consumer.await;
    drop(thread);
    if json {
        render::json_line(&serde_json::json!({
            "type": "run",
            "run_id": report.run_id,
            "exit_code": report.exit_code,
            "summary": report.summary,
            "failed": report.report.failed().collect::<Vec<_>>(),
            "listed": report.listed,
        }));
    } else {
        if !report.listed.is_empty()
            && report.summary.ingested + report.summary.skipped + report.summary.failed == 0
        {
            let rows: Vec<Vec<String>> = report
                .listed
                .iter()
                .map(|f| {
                    vec![
                        f.table.clone().unwrap_or_default(),
                        f.period.map(|p| p.to_string()).unwrap_or_default(),
                        f.size
                            .map(|s| format!("{:.1} MB", s as f64 / 1e6))
                            .unwrap_or_default(),
                        f.url.to_string(),
                    ]
                })
                .collect();
            print!(
                "{}",
                render::table(&["table", "period", "size", "url"], &rows)
            );
        }
        print!("{}", render::summary_text(&report.summary, &report.report));
    }
    Ok(report.exit_code)
}

/// `nemlake fetch nem ...`
pub async fn fetch(cli: &Cli, ctx: &Context, market: &FetchMarket) -> Result<i32> {
    match market {
        FetchMarket::Nem {
            table,
            from,
            to,
            dry_run,
            downloads,
            refresh,
            ingest,
        } => {
            let source = Arc::new(
                MmsdmArchive::new(&ctx.config.sources.nem_base_url)
                    .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?,
            );
            let tables: Vec<String> = if table.is_empty() {
                ctx.registry
                    .tables()
                    .filter(|(_, t)| !t.archive_files.is_empty())
                    .flat_map(|(_, t)| t.archive_files.clone())
                    .collect()
            } else {
                table.iter().map(|t| t.to_ascii_uppercase()).collect()
            };
            let to = to.unwrap_or(*from);
            if to < *from {
                anyhow::bail!("--to {to} is before --from {from}.");
            }
            let request = format!(
                "fetch nem {} {from}..{to}",
                if table.is_empty() {
                    "*".to_owned()
                } else {
                    table.join(",")
                }
            );
            let plan = Plan {
                request,
                input: Input::Remote {
                    source,
                    query: Query {
                        tables,
                        from: *from,
                        to,
                    },
                },
                options: options(cli, ctx, ingest, *downloads, *dry_run),
            };
            run_plan(cli, ctx, plan, *refresh, *downloads).await
        }
    }
}

/// `nemlake ingest <paths>`
pub async fn ingest(cli: &Cli, ctx: &Context, paths: &[PathBuf], args: &IngestArgs) -> Result<i32> {
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(p)?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|e| {
                    e.extension().is_some_and(|x| {
                        x.eq_ignore_ascii_case("zip") || x.eq_ignore_ascii_case("csv")
                    })
                })
                .collect();
            entries.sort();
            files.extend(entries);
        } else {
            files.push(p.clone());
        }
    }
    let request = format!(
        "ingest {}",
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let plan = Plan {
        request,
        input: Input::Local(files),
        options: options(cli, ctx, args, None, false),
    };
    run_plan(cli, ctx, plan, false, None).await
}

/// `nemlake verify`: every output the manifest lists exists with its size.
pub async fn verify(ctx: &Context, table: Option<&str>) -> Result<i32> {
    let (manifest, thread) = ctx.manifest()?;
    let ingests = manifest
        .list_ingests(table.map(str::to_owned), false, 100_000)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
    drop(thread);
    let mut problems: Vec<Vec<String>> = Vec::new();
    let mut files = 0usize;
    for i in &ingests {
        for o in &i.outputs {
            files += 1;
            match std::fs::metadata(Path::new(&o.path)) {
                Ok(m) if m.len() == o.bytes => {}
                Ok(m) => problems.push(vec![
                    o.path.clone(),
                    "verify.size_mismatch".into(),
                    format!("manifest says {} bytes, disk has {}", o.bytes, m.len()),
                ]),
                Err(_) => problems.push(vec![
                    o.path.clone(),
                    "verify.missing_file".into(),
                    "listed in the manifest but not on disk".into(),
                ]),
            }
        }
    }
    if ctx.json {
        render::json_line(&serde_json::json!({ "files": files, "problems": problems }));
    } else if problems.is_empty() {
        println!("{files} file(s) verified, no problems");
    } else {
        print!("{}", render::table(&["path", "code", "problem"], &problems));
    }
    Ok(if problems.is_empty() { 0 } else { 1 })
}
