//! `cargo run -p xtask -- <command>`: the gates the hooks and CI run.
//!
//! Everything here is Rust so the repository needs no second toolchain.

mod bench;
mod covergate;
mod errors_doc;
mod llm_check;
mod ratchet;
mod schema_compat;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "nemlake repository tooling")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Hold every source file to the floor of its directory in coverage.json.
    Covergate {
        /// The lcov profile.
        lcov: PathBuf,
    },
    /// Run the benchmarks and hold them to bench/budgets.json.
    Bench {
        /// `fast` (micro only) or `full` (plus end to end).
        #[arg(long, default_value = "fast")]
        set: String,
        /// Rewrite budgets to the measurements where they are tighter.
        #[arg(long)]
        tighten: bool,
    },
    /// Fail if a budget loosened or a floor dropped against a git ref.
    Ratchet {
        /// The base ref.
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
    /// Fail if a table file changed in a way the lake does not survive.
    SchemaCompat {
        /// The base ref.
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
    /// Check llm/index.md: size, and every path and recipe it names exists.
    LlmCheck,
    /// Regenerate or check docs/errors.md from the diagnostic codes in the source.
    ErrorsDoc {
        /// Write the file.
        #[arg(long)]
        write: bool,
        /// Fail if the file would change.
        #[arg(long)]
        check: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Covergate { lcov } => covergate::run(&lcov),
        Cmd::Bench { set, tighten } => bench::run(&set, tighten),
        Cmd::Ratchet { base } => ratchet::run(&base),
        Cmd::SchemaCompat { base } => schema_compat::run(&base),
        Cmd::LlmCheck => llm_check::run(),
        Cmd::ErrorsDoc { write, check } => errors_doc::run(write, check),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("xtask: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// The repository root: the parent of `crates/`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}
