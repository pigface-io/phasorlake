//! The flag tree.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use utilake_core::YearMonth;

/// AEMO market data into a local Parquet lake.
#[derive(Parser, Debug)]
#[command(name = "utilake", version, about, max_term_width = 100)]
pub struct Cli {
    /// Home directory (default `~/.utilake`).
    #[arg(long, global = true, env = "UTILAKE_HOME")]
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
    /// Use only the tables built into utilake; ignore `{home}/schemas/`.
    #[arg(long, global = true)]
    pub no_user_schemas: bool,
    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Download files from AEMO into the archive, each once.
    Fetch {
        /// The market.
        #[command(subcommand)]
        market: FetchMarket,
    },
    /// Write Parquet from archived months (`--from`) or from local zips or
    /// CSVs; no network.
    Ingest {
        /// Files or directories.
        #[arg(required_unless_present = "from", conflicts_with = "from")]
        paths: Vec<PathBuf>,
        /// First archived month, YYYY-MM.
        #[arg(long)]
        from: Option<YearMonth>,
        /// Last archived month, YYYY-MM (default: same as --from).
        #[arg(long, requires = "from")]
        to: Option<YearMonth>,
        /// Tables, comma separated (default: every table archived).
        #[arg(long, value_delimiter = ',', requires = "from")]
        table: Vec<String>,
        /// Tables to leave out, comma separated.
        #[arg(long, value_delimiter = ',', requires = "from")]
        exclude: Vec<String>,
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
    /// with a record of where and when it came from.
    Archive {
        /// The subcommand.
        #[command(subcommand)]
        cmd: ArchiveCmd,
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
    Nem(utilake_nem::cli::FetchNem),
}

/// Flags of ingest.
#[derive(Args, Debug, Clone, Default)]
pub struct IngestArgs {
    /// Bad rows tolerated per file before it fails (default from config, else 0).
    #[arg(long)]
    pub max_bad_rows: Option<u64>,
    /// Refuse what the registry does not declare: an unknown table, or a
    /// block whose columns are not its increment's, fails its file.
    /// Without this, everything a file carries is kept and what deviates
    /// is reported.
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
    /// A table's increments: from which month, the AEMO control file and
    /// versions, and the columns each adds or drops.
    Increments {
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
        /// The month of the source, which decides the increment.
        #[arg(long, value_name = "YYYY-MM")]
        month: Option<utilake_core::YearMonth>,
    },
    /// Validate a schemas directory (default: the embedded registry).
    Validate {
        /// Directory.
        dir: Option<PathBuf>,
    },
}

/// Archive commands.
#[derive(Subcommand, Debug)]
pub enum ArchiveCmd {
    /// Every archived download with where and when it came from.
    Ls,
    /// Hash every archived file again and compare with its record.
    Verify,
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
            "utilake",
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
                market: FetchMarket::Nem(args),
            } => {
                assert_eq!(args.table, ["A", "B"]);
                assert_eq!(args.from.to_string(), "2024-01");
                assert_eq!(args.to.unwrap().to_string(), "2024-02");
                assert!(args.dry_run);
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["utilake", "fetch", "nem", "--from", "2024-13"]).is_err());
        let c = Cli::try_parse_from(["utilake", "--json", "-vv", "ingest", "a.zip", "--strict"])
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
        let c = Cli::try_parse_from(["utilake", "reingest", "--dry-run"]).unwrap();
        assert!(matches!(c.command, Command::Reingest { dry_run: true, .. }));
    }
}
