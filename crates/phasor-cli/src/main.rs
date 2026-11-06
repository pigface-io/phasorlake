//! `phasor`: AEMO market data into a local Parquet lake.

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = phasor_cli::Cli::parse();
    // The pool every blocking task shares: a run keeps `jobs` ingests in
    // flight and hashes the next files on the same pool, with the spool
    // and the manifest's callers beside them, so two threads a job plus
    // slack. `jobs` here is the flag's number or the pipeline's default
    // (a setting in the config, read later, can only lower it).
    let jobs = cli
        .jobs
        .unwrap_or_else(phasor_core::config::default_jobs)
        .max(1);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(2 * jobs + 8)
        .build()
        .expect("the tokio runtime builds on every supported platform");
    let code = runtime.block_on(phasor_cli::run(cli));
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
