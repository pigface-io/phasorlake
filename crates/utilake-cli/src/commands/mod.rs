//! One module per command family; `dispatch` routes the parsed CLI.

mod acquire;
mod common;
mod fetch;
mod get;
mod inspect;
mod lake;
mod list;
mod manifest;
mod schema;

use anyhow::Result;

use crate::cli::{Cli, Command};
use crate::context::Context;

/// Runs the command; returns the exit code.
pub async fn dispatch(cli: Cli) -> Result<i32> {
    init_logging(cli.verbose, cli.json);
    let ctx = Context::new(&cli)?;
    match &cli.command {
        Command::Get(args) => get::get(&cli, &ctx, args).await,
        Command::Fetch(args) => fetch::fetch(&cli, &ctx, args).await,
        Command::Import(args) => lake::import(&cli, &ctx, &args.paths, &args.ingest).await,
        Command::Ingest(args) => lake::ingest(&cli, &ctx, args).await,
        Command::List(args) => list::list(&ctx, args).await,
        Command::Verify { table, archive } => {
            if *archive {
                lake::verify_archive(&ctx)
            } else {
                lake::verify(&ctx, table.as_deref()).await
            }
        }
        Command::Iceberg { table } => lake::iceberg(&ctx, table.as_deref()).await,
        Command::Inspect { path } => inspect::inspect(&ctx, path),
        Command::Errors {
            run,
            last,
            table,
            code,
            errors_only,
            limit,
        } => {
            manifest::errors(
                &ctx,
                *run,
                *last,
                table.clone(),
                code.clone(),
                *errors_only,
                *limit,
            )
            .await
        }
        Command::Schema { cmd } => schema::schema(&ctx, cmd).await,
        Command::Wem { cmd } => manifest::wem(&ctx, cmd).await,
    }
}

fn init_logging(verbose: u8, json: bool) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("utilake={level}")));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    let _ = if json {
        builder.json().try_init()
    } else {
        builder.try_init()
    };
}
