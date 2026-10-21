//! One module per command family; `dispatch` routes the parsed CLI.

mod inspect;
mod lake;
mod manifest;
mod misc;
mod schema;

use anyhow::Result;

use crate::cli::{Cli, Command};
use crate::context::Context;

/// Runs the command; returns the exit code.
pub async fn dispatch(cli: Cli) -> Result<i32> {
    init_logging(cli.verbose, cli.json);
    let ctx = Context::new(&cli)?;
    match &cli.command {
        Command::Fetch { market } => lake::fetch(&cli, &ctx, market).await,
        Command::Ingest {
            paths,
            from,
            to,
            table,
            exclude,
            ingest,
        } => match from {
            Some(from) => {
                lake::ingest_archived(
                    &cli,
                    &ctx,
                    *from,
                    to.unwrap_or(*from),
                    table,
                    exclude,
                    ingest,
                )
                .await
            }
            None => lake::ingest(&cli, &ctx, paths, ingest).await,
        },
        Command::Reingest { dry_run, ingest } => lake::reingest(&cli, &ctx, *dry_run, ingest).await,
        Command::Verify { table } => lake::verify(&ctx, table.as_deref()).await,
        Command::Iceberg { table } => lake::iceberg(&ctx, table.as_deref()).await,
        Command::Sql {
            table,
            from,
            to,
            as_of,
        } => lake::sql(&ctx, table, (from, to), *as_of).await,
        Command::Inspect { path } => inspect::inspect(&ctx, path),
        Command::Ls {
            table,
            failed,
            runs,
            limit,
        } => manifest::ls(&ctx, table.clone(), *failed, *runs, *limit).await,
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
        Command::Archive { cmd } => misc::archive(&ctx, cmd),
        Command::Wem { cmd } => misc::wem(&ctx, cmd).await,
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
