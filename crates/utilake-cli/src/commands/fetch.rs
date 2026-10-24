//! `fetch`: the files that cover an interval, into the archive, politely.

use anyhow::{Result, bail};
use utilake_pipeline::{GetRequest, RunOptions, SourceChoice};

use crate::cli::{Cli, FetchCmd, IngestArgs, SourceArg};
use crate::context::Context;
use crate::render;

use super::common;

/// The pipeline's choice of side.
pub fn source_choice(s: SourceArg) -> SourceChoice {
    match s {
        SourceArg::Auto => SourceChoice::Auto,
        SourceArg::Monthly => SourceChoice::Monthly,
        SourceArg::Daily => SourceChoice::Daily,
    }
}

/// `utilake fetch TABLES --from A --to B`
pub async fn fetch(cli: &Cli, ctx: &Context, args: &FetchCmd) -> Result<i32> {
    let sel = common::select(ctx, &args.tables)?;
    let Some(w) = common::window(&args.window, &sel.def)? else {
        bail!("fetch needs a window: --from and --to, or --day.");
    };
    let request = common::request("fetch", &sel, Some(&w), &sel.def);
    let req = GetRequest {
        kit: sel.kit.clone(),
        tables: sel.names.clone(),
        exclude: args
            .tables
            .exclude
            .iter()
            .map(|t| t.to_ascii_uppercase())
            .collect(),
        range: w.days(&sel.def),
        instants: w.instants,
        source: source_choice(args.fetch.source),
        request: request.clone(),
    };
    let options = RunOptions {
        ingest: false,
        ..super::lake::options(
            cli,
            ctx,
            &IngestArgs::default(),
            args.fetch.downloads,
            args.dry_run,
        )
    };
    let (manifest, thread) = ctx.manifest()?;
    let report = super::acquire::run(cli, ctx, manifest, req, options).await;
    thread.join();
    let report = match report? {
        Ok(r) => r,
        Err(code) => return Ok(code),
    };
    if ctx.json {
        render::json_line(&super::acquire::json("fetch", &request, &report));
    }
    Ok(report.exit_code)
}
