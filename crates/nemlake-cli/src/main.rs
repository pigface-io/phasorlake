//! `nemlake`: AEMO market data into a local Parquet lake.

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = nemlake_cli::Cli::parse();
    let jobs = cli.jobs.unwrap_or(0).max(1);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(jobs + 8)
        .build()
        .expect("the tokio runtime builds on every supported platform");
    let code = runtime.block_on(nemlake_cli::run(cli));
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
