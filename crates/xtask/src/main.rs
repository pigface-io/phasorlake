//! `cargo run -p xtask -- <command>`: the gates the hooks and CI run.
//!
//! Everything here is Rust so the repository needs no second toolchain.

mod bench;
mod errors_doc;
mod llm_check;
mod schemas;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use utilake_gates::{covergate, ratchet};

#[derive(Parser)]
#[command(name = "xtask", about = "utilake repository tooling")]
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
    /// Copy utilake-schemas' NEM tables and manifest at a commit into
    /// schemas/nem/pulled/.
    SchemasPull {
        /// The commit, or anything git names one by.
        rev: String,
        /// The utilake-schemas checkout to read it from.
        #[arg(long, default_value = "../utilake-schemas")]
        from: PathBuf,
    },
    /// Fail unless schemas/nem/pulled/ is its manifest's, of the commit
    /// Cargo.lock takes utilake-iceberg from.
    SchemasCheck,
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
        Cmd::Covergate { lcov } => covergate::run(&repo_root(), &lcov),
        Cmd::Bench { set, tighten } => bench::run(&set, tighten),
        Cmd::Ratchet { base } => ratchet::run(&repo_root(), &base),
        Cmd::SchemasPull { rev, from } => schemas::pull(&rev, &from),
        Cmd::SchemasCheck => schemas::check(),
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
