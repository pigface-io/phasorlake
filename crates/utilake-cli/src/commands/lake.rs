//! `fetch`, `ingest`, `verify`: the pipeline and the lake.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio_util::sync::CancellationToken;
use utilake_acquire::{Query, Source};
use utilake_core::Diagnose;
use utilake_core::TimeRange;
use utilake_pipeline::{Input, Plan, RunOptions};

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
            utilake_kit::Strictness::Strict
        } else {
            utilake_kit::Strictness::Permissive
        },
        max_bad_rows: args.max_bad_rows.unwrap_or(ctx.config.max_bad_rows),
        fail_fast: args.fail_fast,
        dry_run,
        strict_footer: args.strict || ctx.config.strict,
        batch_rows: utilake_pipeline::DEFAULT_BATCH_ROWS,
        ingest: true,
    }
}

async fn run_plan(cli: &Cli, ctx: &Context, plan: Plan) -> Result<i32> {
    let (tx, rx) = tokio::sync::mpsc::channel(4096);
    let fetcher = ctx.fetcher(Some(tx.clone()))?;
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
    let report = utilake_pipeline::run(plan, deps, tx, cancel).await;
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
            let known: u64 = report.listed.iter().filter_map(|f| f.size).sum();
            println!(
                "{} file(s), {:.2} GB to download where the listing gives a size",
                report.listed.len(),
                known as f64 / 1e9
            );
        }
        print!("{}", render::summary_text(&report.summary, &report.report));
    }
    Ok(report.exit_code)
}

/// The NEM archive, as the kits were assembled with it.
fn nem_source(ctx: &Context) -> Result<Arc<dyn Source>> {
    ctx.kits
        .source(utilake_acquire::mmsdm::SOURCE_ID)
        .map(|(_, s)| s.clone())
        .ok_or_else(|| anyhow::anyhow!("This binary was assembled without the NEM kit."))
}

/// `utilake fetch nem ...`
pub async fn fetch(cli: &Cli, ctx: &Context, market: &FetchMarket) -> Result<i32> {
    match market {
        FetchMarket::Nem {
            table,
            exclude,
            from,
            to,
            dry_run,
            downloads,
        } => {
            let source = nem_source(ctx)?;
            // No table named: every file the archive has for the months,
            // whether the registry declares its table or not.
            let tables: Vec<String> = table.iter().map(|t| t.to_ascii_uppercase()).collect();
            let exclude: Vec<String> = exclude.iter().map(|t| t.to_ascii_uppercase()).collect();
            let to = to.unwrap_or(*from);
            if to < *from {
                anyhow::bail!("--to {to} is before --from {from}.");
            }
            let request = format!(
                "fetch nem {}{} {from}..{to}",
                if table.is_empty() {
                    "*".to_owned()
                } else {
                    table.join(",")
                },
                if exclude.is_empty() {
                    String::new()
                } else {
                    format!(" except {}", exclude.join(","))
                }
            );
            let plan = Plan {
                request,
                input: Input::Remote {
                    source,
                    query: Query {
                        exclude,
                        tables,
                        range: TimeRange::months(*from, to),
                    },
                },
                options: RunOptions {
                    ingest: false,
                    ..options(cli, ctx, &IngestArgs::default(), *downloads, *dry_run)
                },
            };
            run_plan(cli, ctx, plan).await
        }
    }
}

/// `utilake ingest <paths>`
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
    run_plan(cli, ctx, plan).await
}

/// `utilake ingest --from ...`: the archived files of the months, as
/// `fetch nem` downloaded them; no network.
pub async fn ingest_archived(
    cli: &Cli,
    ctx: &Context,
    from: utilake_core::YearMonth,
    to: utilake_core::YearMonth,
    tables: &[String],
    exclude: &[String],
    args: &IngestArgs,
) -> Result<i32> {
    if to < from {
        anyhow::bail!("--to {to} is before --from {from}.");
    }
    let source = nem_source(ctx)?;
    let upper = |v: &[String]| v.iter().map(|t| t.to_ascii_uppercase()).collect::<Vec<_>>();
    let plan = Plan {
        request: format!(
            "ingest nem {} {from}..{to}",
            if tables.is_empty() {
                "*".to_owned()
            } else {
                tables.join(",")
            }
        ),
        input: Input::Archive {
            source,
            query: Query {
                tables: upper(tables),
                exclude: upper(exclude),
                range: TimeRange::months(from, to),
            },
        },
        options: options(cli, ctx, args, None, false),
    };
    run_plan(cli, ctx, plan).await
}

/// `utilake reingest`: every recorded source whose lake files are behind
/// the current rules, read again from where it is kept.
pub async fn reingest(cli: &Cli, ctx: &Context, dry_run: bool, args: &IngestArgs) -> Result<i32> {
    let plan = Plan {
        request: "reingest".to_owned(),
        input: Input::Stale,
        options: options(cli, ctx, args, None, dry_run),
    };
    run_plan(cli, ctx, plan).await
}

/// `utilake verify`: every output the manifest lists exists with its size,
/// and every quarantine file belongs to a current ingest.
pub async fn verify(ctx: &Context, table: Option<&str>) -> Result<i32> {
    let (manifest, thread) = ctx.manifest()?;
    let report = utilake_writer::verify_lake(&manifest, &ctx.home.quarantine(), table)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
    drop(manifest);
    thread.join();
    if ctx.json {
        render::json_line(&serde_json::to_value(&report)?);
    } else if report.is_clean() {
        println!("{} file(s) verified, no problems", report.files);
    } else {
        let rows: Vec<Vec<String>> = report
            .problems
            .iter()
            .map(|p| vec![p.path.clone(), p.code.clone(), p.problem.clone()])
            .collect();
        print!("{}", render::table(&["path", "code", "problem"], &rows));
    }
    Ok(if report.is_clean() { 0 } else { 1 })
}
