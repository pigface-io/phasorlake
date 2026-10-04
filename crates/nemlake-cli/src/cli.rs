//! The flag tree.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use nemlake_core::YearMonth;

/// AEMO market data into a local Parquet lake.
#[derive(Parser, Debug)]
#[command(name = "nemlake", version, about, max_term_width = 100)]
pub struct Cli {
    /// Home directory (default `~/.nemlake`).
    #[arg(long, global = true, env = "NEMLAKE_HOME")]
    pub home: Option<PathBuf>,
    /// Config file (default `{home}/config.toml`).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Machine-readable output: one JSON document, or NDJSON for streams.
    #[arg(long, global = true)]
    pub json: bool,
    /// More logging; twice for debug.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// No progress bars.
    #[arg(long, global = true)]
    pub no_progress: bool,
    /// Never touch the network; fail on a cache miss.
    #[arg(long, global = true)]
    pub offline: bool,
    /// Parse workers (default half the cores).
    #[arg(long, global = true)]
    pub jobs: Option<usize>,
    /// Emit Decimal128 for NUMBER(p,s) columns instead of f64.
    #[arg(long, global = true)]
    pub decimal: bool,
    /// Use only the tables built into nemlake; ignore `{home}/schemas/`.
    #[arg(long, global = true)]
    pub no_user_schemas: bool,
    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Download and ingest files from AEMO.
    Fetch {
        /// The market.
        #[command(subcommand)]
        market: FetchMarket,
    },
    /// Ingest local zips or CSVs; no network.
    Ingest {
        /// Files or directories.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Ingest flags.
        #[command(flatten)]
        ingest: IngestArgs,
    },
    /// Read again, from disk, every source whose lake files are behind:
    /// written by rules that have changed since (a table file added or
    /// edited, another type mode), left unfinished by a failed run, or
    /// gone. No network.
    Reingest {
        /// List the sources that would be read again and stop.
        #[arg(long)]
        dry_run: bool,
        /// Ingest flags.
        #[command(flatten)]
        ingest: IngestArgs,
    },
    /// Show a file's blocks, versions, columns and how the registry resolves them.
    Inspect {
        /// A zip or CSV.
        path: PathBuf,
        /// Infer a draft schema for blocks the registry does not know.
        #[arg(long)]
        infer: bool,
        /// Rows to sample per block for inference.
        #[arg(long, default_value_t = 1000)]
        sample_rows: u64,
    },
    /// List what the manifest knows.
    Ls {
        /// Only this table.
        #[arg(long)]
        table: Option<String>,
        /// Only failed ingests.
        #[arg(long)]
        failed: bool,
        /// Runs instead of ingests.
        #[arg(long)]
        runs: bool,
        /// At most this many rows.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Show errors and warnings recorded by past runs.
    Errors {
        /// Only this run id (default: all).
        #[arg(long)]
        run: Option<i64>,
        /// Only the latest run.
        #[arg(long)]
        last: bool,
        /// Only this table.
        #[arg(long)]
        table: Option<String>,
        /// Only this code.
        #[arg(long)]
        code: Option<String>,
        /// Only errors, not warnings.
        #[arg(long)]
        errors_only: bool,
        /// At most this many.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Check the lake against the manifest.
    Verify {
        /// Only this table.
        #[arg(long)]
        table: Option<String>,
    },
    /// The schema registry.
    Schema {
        /// The subcommand.
        #[command(subcommand)]
        cmd: SchemaCmd,
    },
    /// The archive of downloaded files: every file as the server sent it,
    /// with a record of how it was requested.
    Archive {
        /// The subcommand.
        #[command(subcommand)]
        cmd: ArchiveCmd,
    },
    /// Listings and scratch files, which can be thrown away.
    Cache {
        /// The subcommand.
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    /// Western Australia (WEM) data.
    Wem {
        /// The subcommand.
        #[command(subcommand)]
        cmd: WemCmd,
    },
}

/// Markets to fetch.
#[derive(Subcommand, Debug)]
pub enum FetchMarket {
    /// The National Electricity Market monthly archive.
    Nem {
        /// Tables, comma separated (default: every table the archive has
        /// for the months asked, declared in the registry or not).
        #[arg(long, value_delimiter = ',')]
        table: Vec<String>,
        /// Tables to leave out, comma separated.
        #[arg(long, value_delimiter = ',')]
        exclude: Vec<String>,
        /// First month, YYYY-MM.
        #[arg(long)]
        from: YearMonth,
        /// Last month, YYYY-MM (default: same as --from).
        #[arg(long)]
        to: Option<YearMonth>,
        /// List what would be fetched and stop.
        #[arg(long)]
        dry_run: bool,
        /// Concurrent downloads.
        #[arg(long)]
        downloads: Option<usize>,
        /// Re-download even when cached.
        #[arg(long)]
        refresh: bool,
        /// Ingest flags.
        #[command(flatten)]
        ingest: IngestArgs,
    },
}

/// Flags shared by fetch and ingest.
#[derive(Args, Debug, Clone, Default)]
pub struct IngestArgs {
    /// Bad rows tolerated per file before it fails (default from config, else 0).
    #[arg(long)]
    pub max_bad_rows: Option<u64>,
    /// Refuse what the registry does not declare: an unknown table, version
    /// or column list fails its file. Without this, everything a file
    /// carries is kept and what is undeclared is reported.
    #[arg(long)]
    pub strict: bool,
    /// Stop at the first failure.
    #[arg(long)]
    pub fail_fast: bool,
}

/// Registry commands.
#[derive(Subcommand, Debug)]
pub enum SchemaCmd {
    /// Every table the registry knows.
    List,
    /// One table in full.
    Show {
        /// Table name.
        table: String,
    },
    /// Columns added, removed or reordered between two versions.
    Diff {
        /// Table name.
        table: String,
        /// From version.
        from: u16,
        /// To version.
        to: u16,
    },
    /// Every version of a table with when it was seen.
    Lineage {
        /// Table name.
        table: String,
    },
    /// The conformance plan a shape is read with, as JSON: which lake
    /// column each file column feeds, how it is cast, what is written null.
    Plan {
        /// Report type, e.g. `DISPATCH`.
        report_type: String,
        /// Report subtype, e.g. `UNIT_SCADA`.
        report_subtype: String,
        /// The version the file's I row carries.
        version: u16,
        /// The file's column names, in order.
        #[arg(long, value_delimiter = ',', required = true)]
        columns: Vec<String>,
        /// The month of the source, for rules bounded by months.
        #[arg(long, value_name = "YYYY-MM")]
        month: Option<nemlake_core::YearMonth>,
    },
    /// Validate a schemas directory (default: the embedded registry).
    Validate {
        /// Directory.
        dir: Option<PathBuf>,
    },
    /// Draft what a file would add to the registry: a table for a report
    /// pair nobody declared, a version or a variant for a column list the
    /// table does not have yet.
    Infer {
        /// A zip or CSV.
        path: PathBuf,
        /// Write each draft to `{home}/schemas/` as `DRAFT_{TABLE}.toml`
        /// instead of printing it. A draft is ignored until it is renamed
        /// to `{TABLE}.toml`.
        #[arg(long)]
        write: bool,
        /// Rows to sample per block for the types of new columns.
        #[arg(long, default_value_t = 1000)]
        sample_rows: u64,
        /// A control file (`.ctl`) of the same table from AEMO's archive,
        /// to type new columns from; repeatable. It is used for a block
        /// that carries every column it names, and the sampled rows
        /// overrule it where a value does not fit.
        #[arg(long, value_name = "FILE")]
        ctl: Vec<PathBuf>,
    },
}

/// Archive commands.
#[derive(Subcommand, Debug)]
pub enum ArchiveCmd {
    /// Every archived download with when and how it was requested.
    Ls,
    /// Hash every archived file again and compare with its record.
    Verify,
}

/// Cache commands. Neither touches the archive of downloads.
#[derive(Subcommand, Debug)]
pub enum CacheCmd {
    /// Remove every cached listing, so the next run lists afresh.
    Clear,
    /// Remove leftovers of interrupted work: partial downloads nobody
    /// resumed, spooled files, stale listings, records without a file.
    Gc {
        /// Partial downloads are kept for a resume for this many days.
        #[arg(long, value_name = "DAYS", default_value_t = 7)]
        keep_partial: u64,
        /// List what would be removed and remove nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

/// WEM commands.
#[derive(Subcommand, Debug)]
pub enum WemCmd {
    /// Show a dataset's manifest.
    Manifest {
        /// Dataset name, e.g. facility-scada.
        dataset: String,
        /// First month.
        #[arg(long)]
        from: Option<YearMonth>,
        /// Last month.
        #[arg(long)]
        to: Option<YearMonth>,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn parses() {
        Cli::command().debug_assert();
        let c = Cli::try_parse_from([
            "nemlake",
            "fetch",
            "nem",
            "--table",
            "A,B",
            "--from",
            "2024-01",
            "--to",
            "2024-02",
            "--dry-run",
        ])
        .unwrap();
        match c.command {
            Command::Fetch {
                market:
                    FetchMarket::Nem {
                        table,
                        from,
                        to,
                        dry_run,
                        ..
                    },
            } => {
                assert_eq!(table, ["A", "B"]);
                assert_eq!(from.to_string(), "2024-01");
                assert_eq!(to.unwrap().to_string(), "2024-02");
                assert!(dry_run);
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["nemlake", "fetch", "nem", "--from", "2024-13"]).is_err());
        let c = Cli::try_parse_from(["nemlake", "--json", "-vv", "ingest", "a.zip", "--strict"])
            .unwrap();
        assert!(c.json);
        assert_eq!(c.verbose, 2);
        assert!(matches!(
            c.command,
            Command::Ingest {
                ingest: IngestArgs { strict: true, .. },
                ..
            }
        ));
        let c = Cli::try_parse_from(["nemlake", "reingest", "--dry-run"]).unwrap();
        assert!(matches!(c.command, Command::Reingest { dry_run: true, .. }));
    }
}
