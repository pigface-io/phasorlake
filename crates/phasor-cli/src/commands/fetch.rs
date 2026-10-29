//! `fetch`: the files covering an interval into the archive, ingested unless `--no-ingest`.

use anyhow::{Result, bail};
use phasor_pipeline::{GetRequest, RunOptions, SourceChoice};

use crate::cli::{Cli, FetchCmd, SourceArg};
use crate::context::Context;
use crate::render;

use super::common;

/// `phasor fetch --documents`: every document fetched again, a new version kept.
async fn documents(ctx: &Context) -> Result<i32> {
    let nem = phasor_nem::NemConfig::from_sources(&ctx.config.sources)
        .map_err(|e| anyhow::anyhow!("{e} (in the config file)"))?;
    let fetcher = ctx.fetcher(None)?;
    let mut code = 0;
    for url in &nem.documents {
        let remote = phasor_kit::RemoteFile {
            url: url::Url::parse(url)?,
            source: "aemo-documents",
            table: None,
            period: None,
            size: None,
            modified: None,
            modified_at: None,
        };
        match fetcher.fetch_document(&remote).await {
            Ok(local) => {
                let kept = if local.cached {
                    "seen again"
                } else {
                    "new version"
                };
                if ctx.json {
                    render::json_line(&serde_json::json!({
                        "type": "document", "url": url, "path": local.path,
                        "sha256": local.sha256, "new": !local.cached,
                    }));
                } else {
                    println!("{kept}: {} ({})", local.path.display(), &local.sha256[..12]);
                }
            }
            Err(e) => {
                eprintln!("error: {url}: {e}");
                code = if code == 0 { 4 } else { code };
            }
        }
    }
    Ok(code)
}

/// The pipeline's choice of side.
pub fn source_choice(s: SourceArg) -> SourceChoice {
    match s {
        SourceArg::Auto => SourceChoice::Auto,
        SourceArg::Monthly => SourceChoice::Monthly,
        SourceArg::Daily => SourceChoice::Daily,
    }
}

/// `phasor fetch TABLES --from A --to B [--no-ingest]`
pub async fn fetch(cli: &Cli, ctx: &Context, args: &FetchCmd) -> Result<i32> {
    if args.documents {
        return documents(ctx).await;
    }
    let sel = common::select(ctx, &args.tables)?;
    let Some(w) = common::window(&args.window, &sel.def)? else {
        bail!("fetch needs a window: --from and --to, or --day.");
    };
    let request = common::request("fetch", &sel, &w, &sel.def);
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
        ingest: !args.no_ingest,
        ..super::lake::options(cli, ctx, &args.ingest, args.fetch.downloads, args.dry_run)
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
