//! The command: parse flags, assemble dependencies, call the pipeline or the
//! registry, render. Nothing here decides what a table means or where a row
//! goes; those decisions live in the library crates.

pub mod cli;
pub mod commands;
pub mod context;
pub mod render;

pub use cli::Cli;

/// Runs the parsed command and returns the exit code.
pub async fn run(cli: Cli) -> i32 {
    match commands::dispatch(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}
