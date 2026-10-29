//! `import`, `ingest`, `verify`, `iceberg`: the pipeline over local and archived files, and checks.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use phasor_core::Diagnose;
use phasor_core::TimeRange;
use phasor_kit::{Query, Source};
use phasor_pipeline::{Input, Plan, RunOptions};

use crate::cli::{Cli, IngestArgs, IngestCmd};
use crate::context::Context;
use crate::render;

use super::common;

pub(crate) fn options(
    cli: &Cli,
    ctx: &Context,
    args: &IngestArgs,
    downloads: Option<u8>,
    dry_run: bool,
) -> RunOptions {
    RunOptions {
        jobs: cli.jobs.unwrap_or_else(|| ctx.config.effective_jobs()),
        downloads: downloads
            .map_or(ctx.config.downloads, usize::from)
            .clamp(1, 4),
        mode: if args.strict || ctx.config.strict {
            phasor_kit::Strictness::Strict
        } else {
            phasor_kit::Strictness::Permissive
        },
        max_bad_rows: args.max_bad_rows.unwrap_or(ctx.config.max_bad_rows),
        fail_fast: args.fail_fast,
        dry_run,
        strict_footer: args.strict || ctx.config.strict,
        batch_rows: phasor_pipeline::DEFAULT_BATCH_ROWS,
        ingest: true,
        layout: args.layout(ctx.config.lake),
    }
}

async fn run_plan(cli: &Cli, ctx: &Context, plan: Plan) -> Result<i32> {
    let (tx, rx) = tokio::sync::mpsc::channel(4096);
    let fetcher = ctx.fetcher(Some(tx.clone()))?;
    let (manifest, thread) = ctx.manifest()?;
    let deps = ctx.deps(fetcher, manifest);
    let interrupts = common::Interrupts::listen();
    let json = ctx.json;
    let progress = ctx.progress;
    let verbose = cli.verbose > 0;
    let consumer = tokio::spawn(render::consume_events(rx, json, progress, verbose));
    let report = phasor_pipeline::run(plan, deps, tx, interrupts.token()).await;
    interrupts.finish().await;
    let _ = consumer.await;
    drop(thread);
    refresh_iceberg(ctx, report.run_id).await?;
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

/// `phasor import <paths> [--kit K]`: zips or CSVs from disk into the lake.
pub async fn import(cli: &Cli, ctx: &Context, cmd: &crate::cli::ImportCmd) -> Result<i32> {
    let (paths, args) = (&cmd.paths, &cmd.ingest);
    if let Some(k) = &cmd.kit
        && ctx.kits.kit(k).is_none()
    {
        anyhow::bail!(
            "No kit named {k}; this binary reads with {}.",
            ctx.kits
                .kits()
                .iter()
                .map(|k| k.id)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
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
        "import {}",
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let input = match &cmd.kit {
        Some(kit) => Input::LocalIn {
            kit: kit.to_ascii_lowercase(),
            paths: files,
        },
        None => Input::Local(files),
    };
    let plan = Plan {
        request,
        input,
        options: options(cli, ctx, args, None, false),
    };
    run_plan(cli, ctx, plan).await
}

/// `phasor ingest`: archived files into the lake, no network. With a window, the
/// market's archived files over it; without, every source behind the current rules.
pub async fn ingest(cli: &Cli, ctx: &Context, args: &IngestCmd) -> Result<i32> {
    if args.tables.tables.is_empty() && args.tables.market.is_none() && !args.window.is_set() {
        if args.source.is_some() {
            anyhow::bail!("--source goes with a window: which archived files of it to read.");
        }
        let plan = Plan {
            request: "ingest".to_owned(),
            input: Input::Stale,
            options: options(cli, ctx, &args.ingest, None, args.dry_run),
        };
        return run_plan(cli, ctx, plan).await;
    }
    let sel = common::select(ctx, &args.tables)?;
    let Some(w) = common::window(&args.window, &sel.def)? else {
        anyhow::bail!("Name a window with the tables: --from and --to, or --day.");
    };
    let kit = ctx
        .kits
        .kit(&sel.kit)
        .ok_or_else(|| anyhow::anyhow!("This binary was assembled without the {} kit.", sel.kit))?;
    let reads = reads(&sel.kit, &kit.sources, &sel.names, args.source.as_deref())?;
    let mut code = 0;
    for (source, names) in reads {
        // A source cut by day is archived by day, one cut by month by month.
        let days = w.days(&sel.def);
        let range = if source.granularity() == phasor_kit::Granularity::Day {
            days
        } else {
            TimeRange::months(days.from.first_day().month(), days.to.last_day().month())
        };
        let plan = Plan {
            request: common::request("ingest", &sel, &w, &sel.def),
            input: Input::Archive {
                source,
                query: Query {
                    tables: names,
                    exclude: args
                        .tables
                        .exclude
                        .iter()
                        .map(|t| t.to_ascii_uppercase())
                        .collect(),
                    range,
                    instants: Some(w.instants),
                },
            },
            options: options(cli, ctx, &args.ingest, None, args.dry_run),
        };
        code = code.max(run_plan(cli, ctx, plan).await?);
    }
    Ok(code)
}

/// `phasor iceberg`: each table's Iceberg metadata from the manifest.
pub async fn iceberg(ctx: &Context, table: Option<&str>) -> Result<i32> {
    let (manifest, thread) = ctx.manifest()?;
    let outputs = manifest
        .live_outputs(table)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()));
    drop(manifest);
    thread.join();
    let (rows, code) = build_iceberg(ctx, &outputs?)?;
    if ctx.json {
        for row in &rows {
            render::json_line(&serde_json::json!({
                "table": row[0], "snapshots": row[1], "files": row[2], "rows": row[3],
            }));
        }
    } else {
        print!(
            "{}",
            render::table(&["table", "snapshots", "files", "rows"], &rows)
        );
    }
    Ok(code)
}

/// Each table's Iceberg metadata from its live files: a row per table; 1 when one has no history.
fn build_iceberg(
    ctx: &Context,
    outputs: &[phasor_writer::manifest::LiveOutput],
) -> Result<(Vec<Vec<String>>, i32)> {
    let mut by_table: std::collections::BTreeMap<&str, Vec<phasor_writer::manifest::LiveOutput>> =
        std::collections::BTreeMap::new();
    for o in outputs {
        by_table
            .entry(o.table.as_str())
            .or_default()
            .push(o.clone());
    }
    let r = &ctx.registry;
    let mut rows = Vec::new();
    let mut code = 0;
    for (name, outs) in by_table {
        let Some((id, h)) = r
            .table_by_name(name)
            .and_then(|id| Some((id, r.history(id)?)))
        else {
            rows.push(vec![
                name.to_owned(),
                "-".into(),
                "-".into(),
                "no history".into(),
            ]);
            code = 1;
            continue;
        };
        let published = r.table(id).published();
        let t = phasor_writer::iceberg::TableSchemas {
            table: name.to_owned(),
            uuid: phasor_writer::iceberg::table_uuid(name),
            schemas: h.schemas.clone(),
            properties: published.properties,
            docs: published.docs,
        };
        let built = phasor_writer::iceberg::build_table(
            &ctx.home.lake(),
            &r.table(id).market.to_string(),
            &r.table_dir(id),
            &t,
            &outs,
        )
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        rows.push(vec![
            name.to_owned(),
            built.snapshots.to_string(),
            built.files.to_string(),
            built.rows.to_string(),
        ]);
    }
    Ok((rows, code))
}

/// A source `ingest` reads, and the tables it reads of it.
type Read = (Arc<dyn Source>, Vec<String>);

/// The sources `ingest` reads and the tables of each: the named source, else
/// for each table the monthly source that publishes it, else the first that
/// does; with no tables, every feed.
fn reads(
    kit: &str,
    sources: &[Arc<dyn Source>],
    names: &[String],
    named: Option<&str>,
) -> Result<Vec<Read>> {
    if let Some(id) = named {
        let Some(source) = sources.iter().find(|s| s.id() == id) else {
            let have: Vec<&str> = sources.iter().map(|s| s.id()).collect();
            anyhow::bail!(
                "The {kit} kit has no source {id}; it has {}.",
                have.join(", ")
            );
        };
        return Ok(vec![(Arc::clone(source), names.to_vec())]);
    }
    let mut out = Vec::new();
    for g in phasor_pipeline::source_groups(sources, names) {
        let Some(source) = g.monthly.or(g.daily) else {
            let tables = g.tables.join(", ");
            anyhow::bail!("No source of the {kit} kit publishes {tables}; name one with --source.");
        };
        out.push((source, g.tables));
    }
    Ok(out)
}

/// After a run, rebuilds the Iceberg metadata of every table it ingested.
pub async fn refresh_iceberg(ctx: &Context, run_id: Option<i64>) -> Result<()> {
    let Some(run_id) = run_id else {
        return Ok(());
    };
    let (manifest, thread) = ctx.manifest()?;
    let mut outputs = Vec::new();
    let tables = manifest
        .tables_of_run(run_id)
        .await
        .map_err(|e| super::common::fail(&e));
    for t in tables.as_deref().unwrap_or_default() {
        let has_history = ctx
            .registry
            .table_by_name(t)
            .is_some_and(|id| ctx.registry.history(id).is_some());
        if has_history {
            outputs.extend(
                manifest
                    .live_outputs(Some(t))
                    .await
                    .map_err(|e| super::common::fail(&e))?,
            );
        }
    }
    drop(manifest);
    thread.join();
    tables?;
    build_iceberg(ctx, &outputs)?;
    Ok(())
}

/// `phasor verify --archive`: every archived file hashed again against its record.
pub fn verify_archive(ctx: &Context) -> Result<i32> {
    let store = ctx.archive();
    let entries = phasor_acquire::archive::entries_under(&store, None);
    let mut bad = 0usize;
    for e in &entries {
        if let Err(err) = phasor_acquire::archive::verify(e) {
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

/// `phasor verify`: every manifest output exists with its size; every quarantine file is current.
pub async fn verify(ctx: &Context, table: Option<&str>) -> Result<i32> {
    let (manifest, thread) = ctx.manifest()?;
    let report = phasor_writer::verify_lake(&manifest, &ctx.home.quarantine(), table)
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
