//! `phasor-mirror`: a copy of every file under nemweb's archive folders, and
//! the catalog of it.

mod cli;
mod commands;
mod render;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the tokio runtime builds on every supported platform");
    match runtime.block_on(commands::run_command(&cli)) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::from(2)
        }
    }
}
