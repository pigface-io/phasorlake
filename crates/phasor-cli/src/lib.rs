//! The command: parse flags, assemble dependencies, call the library crates, render.

pub mod cli;
pub mod commands;
pub mod context;
pub mod kits;
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
