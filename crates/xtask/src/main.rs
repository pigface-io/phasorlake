//! `cargo run -p xtask -- <command>`: the gates the hooks and CI run, from
//! [`phasor_gates`], and this repository's own schemas pull and check.
//!
//! Everything here is Rust so the repository needs no second toolchain.

mod schemas;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use phasor_gates::bench::{self, Targets};
use phasor_gates::errors_doc::{self, PHASOR_CATEGORIES, Spec};
use phasor_gates::layers::{self, Family};
use phasor_gates::{covergate, llm_check, ratchet};

/// Why every `query.*` code retired.
const QUERY_GONE: &str = "raised by phasor's read of the lake, which is gone: phasor writes the lake and reading it is the reader's own SQL";

const MMS_DECODE: &str = "phasorlake-schemas/crates/phasor-mms-decode/src/diagnose.rs";
const PLAIN_DECODE: &str = "phasorlake-schemas/crates/phasor-plain-decode/src/diagnose.rs";
const ZIP: &str = "phasorlake-schemas/crates/phasor-zip/src/diagnose.rs";

/// This repository's error catalogue: phasor's categories, the codes a
/// phasor-schemas crate raises on phasor's behalf, and the codes retired.
const ERRORS: Spec<'static> = Spec {
    product: "phasor",
    categories: &PHASOR_CATEGORIES,
    retired: &[
        (
            "zip.no_csv",
            "an archive with no .CSV entry; the walk takes what a decoder accepts, so it is `zip.no_entry`",
        ),
        (
            "zip.size_mismatch",
            "an entry that inflated past the size it declared; the zip reader refuses it itself since zip 9, as `zip.bad_entry`",
        ),
        (
            "listing.era_unsupported",
            "months before 2024 could not be listed; every archive month is now readable and a month the archive does not publish is `listing.month_missing`",
        ),
        (
            "schema.merge_version_conflict",
            "the schema bootstrap refused two different column lists under one version number; each list is now a variant of the version",
        ),
        (
            "schema.version_drift",
            "a block whose columns differed from its declared version; resolution is now against increments, and such a block is `schema.deviates`",
        ),
        (
            "schema.unknown_version",
            "an I-row version the table did not declare; increments decide now, and an unseen version is the warning `schema.unseen_version`",
        ),
        (
            "schema.version_aliased",
            "an undeclared version whose columns were those of a declared one; resolution is by increments and columns now",
        ),
        (
            "schema.compatible_drift",
            "a block missing declared columns; now `schema.deviates`",
        ),
        (
            "schema.unknown_columns",
            "a block with columns the table did not declare; now `schema.deviates`",
        ),
        (
            "schema.decimal_fallback",
            "decimal mode read a column without precision as a float; decimal mode is gone and every number is a double",
        ),
        (
            "schema.rule_conflict",
            "a hand-written rule routed a column the file also carried; rules are gone, fixes go in an increment's columns",
        ),
        (
            "schema.census_unresolved",
            "a census shape neither declared nor read by a rule; now `schema.census_deviates`",
        ),
        (
            "schema.breaking_change",
            "a table file changed in a way the lake does not survive; tables come from the histories phasor-schemas publishes, which hold their own changes",
        ),
        ("query.at_on_interval_table", QUERY_GONE),
        ("query.bad_filter_value", QUERY_GONE),
        ("query.bad_time", QUERY_GONE),
        ("query.filter_after_resolution", QUERY_GONE),
        ("query.lake_format", QUERY_GONE),
        ("query.order_nan", QUERY_GONE),
        ("query.plan_failed", QUERY_GONE),
        ("query.raw_only", QUERY_GONE),
        ("query.read_failed", QUERY_GONE),
        ("query.role_missing", QUERY_GONE),
        ("query.role_type", QUERY_GONE),
        ("query.roles_invalid", QUERY_GONE),
        ("query.source_unknown", QUERY_GONE),
        ("query.stamp_unknown", QUERY_GONE),
        ("query.unknown_column", QUERY_GONE),
        ("query.unknown_table", QUERY_GONE),
        ("query.window_empty", QUERY_GONE),
        ("query.window_ignored", QUERY_GONE),
        ("query.window_uncovered", QUERY_GONE),
        ("query.write_failed", QUERY_GONE),
    ],
    inherited: &[
        ("csv.block_mismatch", MMS_DECODE),
        ("csv.data_before_header", MMS_DECODE),
        ("csv.field_count_mismatch", MMS_DECODE),
        ("csv.header_malformed", MMS_DECODE),
        ("csv.header_missing", PLAIN_DECODE),
        ("csv.line_too_long", MMS_DECODE),
        ("csv.row_too_short", MMS_DECODE),
        ("csv.too_many_bad_rows", MMS_DECODE),
        ("csv.unknown_record", MMS_DECODE),
        ("json.not_wemde", PLAIN_DECODE),
        ("json.syntax", PLAIN_DECODE),
        ("zip.no_entry", ZIP),
        ("zip.spool_failed", ZIP),
    ],
};

/// What this repository benchmarks: the micro benches of the fast set,
/// the writer's with them in the full set, and `phasor-bench` end to end.
const BENCHES: Targets<'static> = Targets {
    fast: &[
        ("phasor-mms-conform", "scan"),
        ("phasor-registry", "resolve"),
        ("phasor-mms-conform", "values"),
        ("phasor-mms-conform", "plain"),
        ("phasor-acquire", "fetch"),
    ],
    full: &[
        ("phasor-mms-conform", "scan"),
        ("phasor-registry", "resolve"),
        ("phasor-mms-conform", "values"),
        ("phasor-mms-conform", "plain"),
        ("phasor-acquire", "fetch"),
        ("phasor-writer", "write"),
    ],
    e2e: Some("phasor-bench"),
};

#[derive(Parser)]
#[command(name = "xtask", about = "phasor repository tooling")]
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
    /// Copy phasor-schemas' tables and manifest at a commit into
    /// schemas/{market}/pulled/, every market it publishes.
    SchemasPull {
        /// The commit, or anything git names one by.
        rev: String,
        /// The phasor-schemas checkout to read it from.
        #[arg(long, default_value = "../phasorlake-schemas")]
        from: PathBuf,
    },
    /// Fail unless each schemas/{market}/pulled/ is its manifest's, of the commit
    /// Cargo.lock takes phasor-iceberg from.
    SchemasCheck,
    /// Fail on a dependency between the workspace's crates that layers.json
    /// does not declare, or one it declares that is gone.
    Layers,
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

/// The repository root: the parent of `crates/`.
fn repo_root() -> PathBuf {
    phasor_gates::repo_root(env!("CARGO_MANIFEST_DIR"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = repo_root();
    let result = match cli.cmd {
        Cmd::Covergate { lcov } => covergate::run(&root, &lcov),
        Cmd::Bench { set, tighten } => bench::run(&root, BENCHES, &set, tighten),
        Cmd::Ratchet { base } => ratchet::run(&root, &base),
        Cmd::SchemasPull { rev, from } => schemas::pull(&rev, &from),
        Cmd::SchemasCheck => schemas::check(),
        Cmd::Layers => layers::run(&root, &Family::PHASOR),
        Cmd::LlmCheck => llm_check::run(&root),
        Cmd::ErrorsDoc { write, check } => errors_doc::run(&root, &ERRORS, write, check),
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
