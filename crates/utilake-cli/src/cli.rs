//! The flag tree.
//!
//! Three stores, one verb per arrow, and `get` for the whole:
//!
//! ```text
//! AEMO ──fetch──▶ archive ──ingest──▶ lake ──▶ output
//! disk ──import─▶ lake                        get = fetch + ingest + write
//! ```
//!
//! `fetch`, `ingest` and `get` take the same tables and window; `get` adds
//! how the rows are resolved and written. Every command's `--help` ends
//! with examples.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use utilake_core::YearMonth;

/// AEMO market data into a local Parquet lake.
#[derive(Parser, Debug)]
#[command(name = "utilake", version, about, max_term_width = 100, after_long_help = ABOUT)]
pub struct Cli {
    /// Home directory (default `~/.utilake`; the archive, the lake and the
    /// manifest live under it).
    #[arg(long, global = true, env = "UTILAKE_HOME")]
    pub home: Option<PathBuf>,
    /// Config file (default `{home}/config.toml`).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Machine-readable output: one JSON document, or NDJSON for streams.
    #[arg(long, global = true)]
    pub json: bool,
    /// More logging; twice (`-vv`) for debug.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// No progress bars.
    #[arg(long, global = true)]
    pub no_progress: bool,
    /// Never touch the network: `get` reads the lake as it is, `fetch`
    /// fails.
    #[arg(long, global = true)]
    pub offline: bool,
    /// Parse workers (default half the cores), e.g. `--jobs 4`.
    #[arg(long, global = true)]
    pub jobs: Option<usize>,
    /// Use only the tables built into utilake; ignore `{home}/schemas/`.
    #[arg(long, global = true)]
    pub no_user_schemas: bool,
    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

const ABOUT: &str = "\
Three stores: the archive (AEMO's files, each once, with a record of where and
when it came from), the lake (Parquet per source file, with a manifest) and the
output (one square frame per table). One verb per arrow, and `get` for the whole:

  AEMO --fetch--> archive --ingest--> lake --> output
  disk --import-> lake                         get = fetch + ingest + write

Examples:
  utilake get DISPATCHPRICE --from 2026-08-01 --to 2026-08-31
      August's prices, fetched where AEMO publishes them most finely, read into
      the lake, written as Parquet parts under ./utilake-get/nem/DISPATCHPRICE/.
  utilake get DUDETAIL --at 2026-08-15 --offline
      The version of every unit in force on the 15th, from the lake alone.
  utilake fetch DISPATCH_UNIT_SCADA --from 2026-07 --to 2026-08
      Two months of files into the archive, nothing else.
  utilake ingest
      Every archived file not yet in the lake, and every lake file behind its rules.
  utilake list --grep price
      Every table whose name or description says price.
";

/// Top-level commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// An interval of some tables, fetched, read into the lake and written
    /// as Parquet: one row per fact, the versions in force across the window.
    #[command(after_long_help = GET_EXAMPLES)]
    Get(GetCmd),
    /// Download the files that cover an interval into the archive, politely,
    /// each once. Nothing is read into the lake.
    #[command(after_long_help = FETCH_EXAMPLES)]
    Fetch(FetchCmd),
    /// Read zips or CSVs you already have on disk into the lake. No network.
    #[command(after_long_help = IMPORT_EXAMPLES)]
    Import(ImportCmd),
    /// Read archived files into the lake: with no arguments, every archived
    /// file not yet in it and every lake file behind its rules (a table file
    /// edited, another type mode, a failed run, a file gone). No network.
    #[command(after_long_help = INGEST_EXAMPLES)]
    Ingest(IngestCmd),
    /// What there is: every table, one table's columns, what the lake holds
    /// of it, the values of an entity column, the files and the archive.
    #[command(visible_alias = "ls", after_long_help = LIST_EXAMPLES)]
    List(ListCmd),
    /// Show a file's blocks, versions, columns and how the registry resolves them.
    #[command(
        after_long_help = "Examples:\n  utilake inspect PUBLIC_DVD_DISPATCHPRICE_202608010000.zip\n  utilake --json inspect prices.csv\n"
    )]
    Inspect {
        /// A zip or CSV, e.g. `PUBLIC_DVD_DISPATCHPRICE_202608010000.zip`.
        path: PathBuf,
    },
    /// Show errors and warnings recorded by past runs.
    #[command(
        after_long_help = "Examples:\n  utilake errors --last\n  utilake errors --table DISPATCHPRICE --errors-only\n  utilake errors --code conform.bad_row --limit 20\n"
    )]
    Errors {
        /// Only this run id, e.g. `--run 12` (default: all).
        #[arg(long)]
        run: Option<i64>,
        /// Only the latest run.
        #[arg(long)]
        last: bool,
        /// Only this table, e.g. `--table DISPATCHPRICE`.
        #[arg(long)]
        table: Option<String>,
        /// Only this code, e.g. `--code conform.bad_row`.
        #[arg(long)]
        code: Option<String>,
        /// Only errors, not warnings.
        #[arg(long)]
        errors_only: bool,
        /// At most this many, e.g. `--limit 20`.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Build each table's Iceberg metadata from the manifest and the
    /// census's history: one snapshot per run of one schema and version.
    #[command(
        after_long_help = "Examples:\n  utilake iceberg\n  utilake iceberg --table DISPATCHPRICE\n"
    )]
    Iceberg {
        /// Only this table, e.g. `--table DISPATCHPRICE`.
        #[arg(long)]
        table: Option<String>,
    },
    /// Check the lake against the manifest, or the archive against its records.
    #[command(
        after_long_help = "Examples:\n  utilake verify\n  utilake verify --table DISPATCHPRICE\n  utilake verify --archive\n"
    )]
    Verify {
        /// Only this table, e.g. `--table DISPATCHPRICE`.
        #[arg(long, conflicts_with = "archive")]
        table: Option<String>,
        /// Hash every archived file again and compare with its record,
        /// instead of checking the lake.
        #[arg(long)]
        archive: bool,
    },
    /// The schema registry.
    Schema {
        /// The subcommand.
        #[command(subcommand)]
        cmd: SchemaCmd,
    },
    /// Western Australia (WEM) data.
    Wem {
        /// The subcommand.
        #[command(subcommand)]
        cmd: WemCmd,
    },
}

/// The tables a command works on.
#[derive(Args, Debug, Clone, Default)]
pub struct TablesArg {
    /// Tables, comma separated, by name or report pair, e.g.
    /// `DISPATCHPRICE,DUDETAIL` or `DISPATCH.UNIT_SCADA`. The market is the
    /// tables'; all must be of one market.
    #[arg(value_delimiter = ',')]
    pub tables: Vec<String>,
    /// Tables to leave out, comma separated, e.g. `--exclude DUDETAILSUMMARY`.
    #[arg(long, value_delimiter = ',')]
    pub exclude: Vec<String>,
    /// The market, e.g. `--market nem`; only needed for a table the
    /// registry does not know.
    #[arg(long)]
    pub market: Option<String>,
}

/// When: a range, a trading day, or one instant, in the market's clock.
#[derive(Args, Debug, Clone, Default)]
pub struct WindowArgs {
    /// The start: a month `2026-08`, a day `2026-08-01`, a local time
    /// `2026-08-01T04:00`, or RFC 3339 with its own offset
    /// `2026-07-31T18:00:00Z`. Included.
    #[arg(long, requires = "to", conflicts_with_all = ["at", "day"])]
    pub from: Option<String>,
    /// The end, in the same forms. A month or a day means through its end
    /// (`--to 2026-08-31` includes the 31st); a time is where the window
    /// stops (`--to 2026-08-01T12:00` ends at noon).
    #[arg(long, requires = "from")]
    pub to: Option<String>,
    /// One instant, e.g. `--at 2026-08-15` or `--at 2026-08-15T14:30`: the
    /// versions in force then. Not for a table whose time ends an interval.
    #[arg(long, conflicts_with = "day")]
    pub at: Option<String>,
    /// One trading day, e.g. `--day 2026-08-01`: 04:00 that day to 04:00
    /// the next in the NEM, 08:00 to 08:00 in the WEM.
    #[arg(long)]
    pub day: Option<String>,
}

impl WindowArgs {
    /// Whether any window was given.
    pub fn is_set(&self) -> bool {
        self.from.is_some() || self.at.is_some() || self.day.is_some()
    }
}

/// Which rows a read returns.
#[derive(Args, Debug, Clone, Default)]
pub struct ResolveArgs {
    /// Keep only what was known by this time, e.g. `--known-at 2026-08-01T16:30`:
    /// the row's cutoff column (LASTCHANGED, RUN_DATETIME), else its
    /// source's publish time. Not with `--raw`.
    #[arg(long, conflicts_with = "raw")]
    pub known_at: Option<String>,
    /// Every revision of every fact, each once, instead of the newest.
    /// Not with `--raw` or `--latest`.
    #[arg(long, conflicts_with_all = ["raw", "latest"])]
    pub revisions: bool,
    /// One row per fact, the newest revision: the default, named for
    /// `--format sql`, which is raw unless told otherwise.
    #[arg(long, conflicts_with = "raw")]
    pub latest: bool,
    /// Every row of every file, as written, no resolution.
    #[arg(long)]
    pub raw: bool,
    /// The schema to read as, e.g. `--as-of 2` (default: the latest).
    #[arg(long)]
    pub as_of: Option<u32>,
    /// The columns to return, comma separated, e.g. `--columns REGIONID,RRP`
    /// (default: all). The role columns are always read.
    #[arg(long, value_delimiter = ',')]
    pub columns: Vec<String>,
    /// Keep rows whose column is one of the values, e.g.
    /// `--where REGIONID=NSW1,VIC1`. May repeat. On an entity column it
    /// runs before resolution; on any other, after.
    #[arg(long = "where", value_name = "COL=V,V")]
    pub filters: Vec<String>,
}

/// Where and how the rows are written.
#[derive(Args, Debug, Clone, Default)]
pub struct OutputArgs {
    /// Where: a directory of parts, e.g. `-o out/`, or one file when the
    /// extension names a format, e.g. `-o prices.parquet`; `-o -` prints
    /// (for `--format sql`). Default `./utilake-get`, laid out as
    /// `{out}/{market}/{TABLE}/`.
    #[arg(short, long)]
    pub out: Option<PathBuf>,
    /// The format, e.g. `--format csv`; `sql` writes the same read as a
    /// SQL query instead of running it.
    #[arg(long, value_enum, default_value_t = FormatArg::Parquet)]
    pub format: FormatArg,
    /// For `--format sql`, where the rows come from, e.g. `--dialect postgres`.
    #[arg(long, value_enum, default_value_t = DialectArg::Duckdb)]
    pub dialect: DialectArg,
    /// Parquet compression, e.g. `--compression snappy`.
    #[arg(long, value_enum, default_value_t = CodecArg::Zstd)]
    pub compression: CodecArg,
    /// Under a directory, a new part begins at this many bytes, e.g.
    /// `--target-file-bytes 268435456` (default 128 MiB).
    #[arg(long)]
    pub target_file_bytes: Option<u64>,
}

/// The output format.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormatArg {
    /// Parquet, with the lake's field ids and `utilake.*` footer keys.
    #[default]
    Parquet,
    /// CSV with a header row.
    Csv,
    /// An Arrow IPC file.
    Arrow,
    /// The read as one SQL query, written, not run.
    Sql,
}

/// Where a SQL query's rows come from.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialectArg {
    /// The lake's Parquet files, read in place.
    #[default]
    Duckdb,
    /// A relation named for the table whose rows carry `_nl_source`.
    Postgres,
}

/// Parquet compression.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecArg {
    /// zstd level 3: what the lake writes.
    #[default]
    Zstd,
    /// LZ4: larger files, the fastest to read.
    Lz4,
    /// Snappy: larger files, read by older tools too.
    Snappy,
    /// None.
    None,
}

impl CodecArg {
    /// The codec it names.
    pub const fn codec(self) -> utilake_core::config::Codec {
        use utilake_core::config::Codec;
        match self {
            Self::Zstd => Codec::Zstd,
            Self::Lz4 => Codec::Lz4,
            Self::Snappy => Codec::Snappy,
            Self::None => Codec::None,
        }
    }
}

/// How files are fetched.
#[derive(Args, Debug, Clone, Default)]
pub struct FetchArgs {
    /// Which side of the market's publication to read, e.g. `--source daily`.
    #[arg(long, value_enum, default_value_t = SourceArg::Auto)]
    pub source: SourceArg,
    /// Downloads in flight, e.g. `--downloads 2` (default from config, 2;
    /// at most 4: AEMO's server blocks clients that ask too fast).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=4))]
    pub downloads: Option<u8>,
}

/// Which side of a market's publication is read.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceArg {
    /// Whole published months from the monthly archive, the rest daily.
    #[default]
    Auto,
    /// Every month the interval touches, from the monthly archive.
    Monthly,
    /// Every day, from the report folders.
    Daily,
}

/// `get`: an interval of some tables, straight to Parquet.
#[derive(Args, Debug, Clone)]
pub struct GetCmd {
    /// The tables.
    #[command(flatten)]
    pub tables: TablesArg,
    /// The window.
    #[command(flatten)]
    pub window: WindowArgs,
    /// The resolution.
    #[command(flatten)]
    pub resolve: ResolveArgs,
    /// The output.
    #[command(flatten)]
    pub output: OutputArgs,
    /// The fetch.
    #[command(flatten)]
    pub fetch: FetchArgs,
    /// Plan what would be fetched and read, print it, write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Ingest flags.
    #[command(flatten)]
    pub ingest: IngestArgs,
}

const GET_EXAMPLES: &str = "\
Examples:
  utilake get DISPATCHPRICE --from 2026-08-01 --to 2026-08-31
      August's five-minute prices: whole months from the monthly archive, the
      rest from the report folders, one row per interval and region, under
      ./utilake-get/nem/DISPATCHPRICE/ with a sidecar naming every source.
  utilake get DISPATCHPRICE,DISPATCH_UNIT_SCADA --day 2026-08-01
      One trading day (04:00 to 04:00) of two tables.
  utilake get DUDETAIL --at 2026-08-15 --offline
      The version of every unit in force on the 15th, from the lake alone;
      _nl_valid_to says where each version stops.
  utilake get DISPATCHPRICE --from 2026-08-01T16:00 --to 2026-08-01T17:00 --known-at 2026-08-01T16:30
      The prices as they were known at half past four.
  utilake get DISPATCHPRICE --from 2026-08 --to 2026-08 --revisions --columns REGIONID,RRP --where REGIONID=NSW1,VIC1
      Every run of two regions, two columns.
  utilake get DISPATCHPRICE --from 2026-08-01 --to 2026-08-02 -o prices.csv
      One CSV file; the extension names the format.
  utilake get DISPATCHPRICE --from 2026-08-01 --to 2026-08-02 --format sql -o -
      The same read as a SQL query for DuckDB, printed.
  utilake get DISPATCHPRICE --from 2026-08 --to 2026-08 --dry-run
      What would be fetched (files, bytes) and read (lake files, units).
";

/// `fetch`: the files that cover an interval, into the archive.
#[derive(Args, Debug, Clone)]
pub struct FetchCmd {
    /// The tables.
    #[command(flatten)]
    pub tables: TablesArg,
    /// The window.
    #[command(flatten)]
    pub window: WindowArgs,
    /// The fetch.
    #[command(flatten)]
    pub fetch: FetchArgs,
    /// List what would be fetched and stop.
    #[arg(long)]
    pub dry_run: bool,
}

const FETCH_EXAMPLES: &str = "\
Examples:
  utilake fetch DISPATCHPRICE --from 2026-07 --to 2026-08
      The monthly archive's files of two months.
  utilake fetch DISPATCH_UNIT_SCADA --from 2026-08-28 --to 2026-08-30
      Three days from the report folders (the month is not published yet).
  utilake fetch DISPATCHPRICE --from 2026-08 --to 2026-08 --source daily --dry-run
      The daily files a month would take, listed, none fetched.
";

/// `import`: local files into the lake.
#[derive(Args, Debug, Clone)]
pub struct ImportCmd {
    /// Zips or CSVs, or directories of them, e.g.
    /// `PUBLIC_DVD_DISPATCHPRICE_202608010000.zip` or `downloads/`.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// The kit that reads every file, e.g. `--kit gas`, whatever its name
    /// or place says (default: the kit whose source knows the file, else
    /// the NEM's).
    #[arg(long)]
    pub kit: Option<String>,
    /// Ingest flags.
    #[command(flatten)]
    pub ingest: IngestArgs,
}

const IMPORT_EXAMPLES: &str = "\
Examples:
  utilake import PUBLIC_DVD_DISPATCHPRICE_202608010000.zip
      One monthly file from disk.
  utilake import downloads/ --strict
      Every zip and CSV in a directory; a block no table declares fails its file.
  utilake import ~/Downloads/Day01.zip --kit gas
      A file whose name says nothing of its market, read by the gas kit.
";

/// `ingest`: archived files into the lake.
#[derive(Args, Debug, Clone)]
pub struct IngestCmd {
    /// The tables (default: every table the archive holds for the window).
    #[command(flatten)]
    pub tables: TablesArg,
    /// The window (default: whatever is archived and not yet in the lake).
    #[command(flatten)]
    pub window: WindowArgs,
    /// Which archived source to read, e.g. `--source nem-reports` or
    /// `--source gas-sttm` (default: the market's monthly source that
    /// publishes the tables, else its first that does).
    #[arg(long)]
    pub source: Option<String>,
    /// List what would be read and stop.
    #[arg(long)]
    pub dry_run: bool,
    /// Ingest flags.
    #[command(flatten)]
    pub ingest: IngestArgs,
}

const INGEST_EXAMPLES: &str = "\
Examples:
  utilake ingest
      Every archived file not yet in the lake, and every lake file behind its
      rules, read again.
  utilake ingest --dry-run
      The same, listed only.
  utilake ingest DISPATCHPRICE --from 2026-07 --to 2026-08
      Two archived months of one table.
  utilake ingest DISPATCH_UNIT_SCADA --from 2026-08-28 --to 2026-08-30 --source nem-reports
      Three archived days from the report folders.
";

/// Flags of ingest.
#[derive(Args, Debug, Clone, Default)]
pub struct IngestArgs {
    /// Bad rows tolerated per file before it fails, e.g. `--max-bad-rows 10`
    /// (default from config, else 0).
    #[arg(long)]
    pub max_bad_rows: Option<u64>,
    /// Refuse what the registry does not declare: an unknown table, or a
    /// block no run of its table's history fits, fails its file.
    /// Without this, everything a file carries is kept and what deviates
    /// is reported.
    #[arg(long)]
    pub strict: bool,
    /// Stop at the first failure.
    #[arg(long)]
    pub fail_fast: bool,
    /// Close a lake file for the next at about this size, e.g.
    /// `--lake-file-size-mb 256` (default from `[lake]` in config, else 128).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4096))]
    pub lake_file_size_mb: Option<u32>,
    /// Rows per row group of a lake file, e.g. `--lake-row-group-rows 131072`
    /// (default 262144).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1024..=16_777_216))]
    pub lake_row_group_rows: Option<u64>,
    /// Data page size of a lake file in kilobytes, e.g. `--lake-page-kb 512`
    /// (default 1024).
    #[arg(long, value_parser = clap::value_parser!(u32).range(8..=65_536))]
    pub lake_page_kb: Option<u32>,
    /// Compression of the lake's files, e.g. `--lake-compression lz4`
    /// (default zstd).
    #[arg(long, value_enum)]
    pub lake_compression: Option<CodecArg>,
    /// The zstd level of the lake's files, 1 to 22, e.g. `--lake-zstd-level 9`
    /// (default 3).
    #[arg(long, value_parser = clap::value_parser!(i32).range(1..=22))]
    pub lake_zstd_level: Option<i32>,
}

impl IngestArgs {
    /// The lake layout: `[lake]` from config, each flag given over it.
    pub fn layout(&self, config: utilake_core::config::Lake) -> utilake_core::config::Lake {
        utilake_core::config::Lake {
            file_size_mb: self.lake_file_size_mb.unwrap_or(config.file_size_mb),
            row_group_rows: self
                .lake_row_group_rows
                .map_or(config.row_group_rows, |r| r as usize),
            page_kb: self.lake_page_kb.unwrap_or(config.page_kb),
            compression: self
                .lake_compression
                .map_or(config.compression, CodecArg::codec),
            zstd_level: self.lake_zstd_level.unwrap_or(config.zstd_level),
            ..config
        }
    }
}

/// `list`: what there is.
#[derive(Args, Debug, Clone, Default)]
pub struct ListCmd {
    /// One table, e.g. `DISPATCHPRICE`: its columns, with each one's type,
    /// role, field id and AEMO's words for it. Without one, every table.
    pub table: Option<String>,
    /// Only this market, e.g. `--market nem`.
    #[arg(long)]
    pub market: Option<String>,
    /// Only this package of AEMO's data model, e.g. `--package DISPATCH`.
    #[arg(long)]
    pub package: Option<String>,
    /// Only tables whose valid time is of this kind, e.g. `--kind from`
    /// (`interval_end`, `instant`, `day`, `from`, `timeless`).
    #[arg(long)]
    pub kind: Option<String>,
    /// Only tables whose name or description has this text, e.g. `--grep price`.
    #[arg(long)]
    pub grep: Option<String>,
    /// For one table, the months the lake holds, the gaps, and which side
    /// of the publication each month came from.
    #[arg(long, requires = "table", conflicts_with_all = ["values", "files"])]
    pub coverage: bool,
    /// For one table, the distinct values of an entity column with their
    /// row counts and spans, e.g. `--values DUID`.
    #[arg(long, requires = "table", value_name = "COL")]
    pub values: Option<String>,
    /// The lake's files (of one table, or all), with rows, span and source.
    #[arg(long)]
    pub files: bool,
    /// Every archived download, with where and when it came from.
    #[arg(long, conflicts_with_all = ["table", "files", "ingests", "runs"])]
    pub archive: bool,
    /// The manifest's ingests (of one table, or all), latest first.
    #[arg(long, conflicts_with_all = ["files", "runs"])]
    pub ingests: bool,
    /// With `--ingests`, only the failed ones.
    #[arg(long, requires = "ingests")]
    pub failed: bool,
    /// The latest run.
    #[arg(long)]
    pub runs: bool,
    /// At most this many rows of ingests or values, e.g. `--limit 20`.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

const LIST_EXAMPLES: &str = "\
Examples:
  utilake list
      Every table: market, kind, entity columns, rows and span in the lake, what it is.
  utilake list --grep price --json
      The ones about prices, as JSON.
  utilake list --kind from
      The standing-data tables, in force from an effective date.
  utilake list DUDETAIL
      Its columns: type, role, field id, AEMO's words.
  utilake list DISPATCHPRICE --coverage
      The months the lake holds, the gaps, where each month came from.
  utilake list DUDETAIL --values DUID
      Every unit, with its rows and span.
  utilake list DISPATCHPRICE --files
      The lake's files of the table.
  utilake list --archive
      Every downloaded file and its record.
  utilake list --ingests --failed
      The ingests that failed.
";

/// Registry commands.
#[derive(Subcommand, Debug)]
pub enum SchemaCmd {
    /// Every table the registry knows.
    List,
    /// One table in full.
    Show {
        /// Table name, e.g. `DISPATCHPRICE`.
        table: String,
    },
    /// A table's history: its schemas, with the columns each adds or
    /// drops, and the runs of files of each (`--json`: the Iceberg history).
    History {
        /// Table name, e.g. `DISPATCHPRICE`.
        table: String,
    },
    /// The conformance plan a shape is read with, as JSON: which lake
    /// column each file column feeds, how it is cast, what is written null.
    Plan {
        /// Report type, e.g. `DISPATCH`.
        report_type: String,
        /// Report subtype, e.g. `UNIT_SCADA`.
        report_subtype: String,
        /// The version the file's I row carries, e.g. `1`.
        version: u16,
        /// The file's column names, in order, e.g.
        /// `--columns SETTLEMENTDATE,DUID,SCADAVALUE`.
        #[arg(long, value_delimiter = ',', required = true)]
        columns: Vec<String>,
        /// The month the source was released in, e.g. `--month 2026-08`,
        /// which picks among the runs that fit.
        #[arg(long, value_name = "YYYY-MM")]
        month: Option<utilake_core::YearMonth>,
    },
    /// Validate a schemas directory (default: the embedded registry).
    Validate {
        /// Directory, e.g. `~/.utilake/schemas`.
        dir: Option<PathBuf>,
    },
}

/// WEM commands.
#[derive(Subcommand, Debug)]
pub enum WemCmd {
    /// Show a dataset's manifest.
    Manifest {
        /// Dataset name, e.g. `facility-scada`.
        dataset: String,
        /// First month, e.g. `--from 2026-01`.
        #[arg(long)]
        from: Option<YearMonth>,
        /// Last month, e.g. `--to 2026-08`.
        #[arg(long)]
        to: Option<YearMonth>,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn layout_flags_go_over_the_config() {
        use utilake_core::config::{Codec, Lake};
        let c = Cli::try_parse_from([
            "utilake",
            "import",
            "a.zip",
            "--lake-file-size-mb",
            "256",
            "--lake-row-group-rows",
            "131072",
            "--lake-page-kb",
            "512",
            "--lake-compression",
            "lz4",
            "--lake-zstd-level",
            "9",
        ])
        .unwrap();
        let Command::Import(ImportCmd { ingest, .. }) = c.command else {
            panic!("import");
        };
        let config = Lake {
            page_rows: 5000,
            ..Lake::default()
        };
        let l = ingest.layout(config);
        assert_eq!(
            (l.file_size_mb, l.row_group_rows, l.page_kb, l.zstd_level),
            (256, 131_072, 512, 9)
        );
        assert_eq!(l.compression, Codec::Lz4);
        // What no flag sets stays the config's.
        assert_eq!(l.page_rows, 5000);
        assert_eq!(IngestArgs::default().layout(config), config);
        for (arg, codec) in [
            ("zstd", Codec::Zstd),
            ("snappy", Codec::Snappy),
            ("none", Codec::None),
        ] {
            let c = Cli::try_parse_from(["utilake", "import", "a.zip", "--lake-compression", arg])
                .unwrap();
            let Command::Import(ImportCmd { ingest, .. }) = c.command else {
                panic!("import");
            };
            assert_eq!(ingest.layout(config).compression, codec);
        }
        assert!(
            Cli::try_parse_from(["utilake", "import", "a.zip", "--lake-page-kb", "4"]).is_err()
        );
    }

    use super::*;

    #[test]
    fn parses() {
        Cli::command().debug_assert();
        let c = Cli::try_parse_from([
            "utilake",
            "fetch",
            "A,B",
            "--from",
            "2024-01",
            "--to",
            "2024-02",
            "--dry-run",
        ])
        .unwrap();
        match c.command {
            Command::Fetch(f) => {
                assert_eq!(f.tables.tables, ["A", "B"]);
                assert_eq!(f.window.from.as_deref(), Some("2024-01"));
                assert_eq!(f.window.to.as_deref(), Some("2024-02"));
                assert!(f.dry_run);
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["utilake", "fetch", "A", "--from", "2024-01"]).is_err());
        let c = Cli::try_parse_from(["utilake", "--json", "-vv", "import", "a.zip", "--strict"])
            .unwrap();
        assert!(c.json);
        assert_eq!(c.verbose, 2);
        assert!(matches!(
            c.command,
            Command::Import(ImportCmd {
                ingest: IngestArgs { strict: true, .. },
                ..
            })
        ));
        let c = Cli::try_parse_from(["utilake", "ingest", "--dry-run"]).unwrap();
        match c.command {
            Command::Ingest(i) => {
                assert!(i.dry_run && !i.window.is_set() && i.tables.tables.is_empty());
            }
            other => panic!("{other:?}"),
        }
        let c = Cli::try_parse_from([
            "utilake",
            "get",
            "DUDETAIL",
            "--at",
            "2024-01-15",
            "--where",
            "DUID=A,B",
            "--columns",
            "DUID,VERSIONNO",
            "-o",
            "d.csv",
            "--compression",
            "snappy",
            "--offline",
        ])
        .unwrap();
        assert!(c.offline);
        match c.command {
            Command::Get(g) => {
                assert_eq!(g.window.at.as_deref(), Some("2024-01-15"));
                assert_eq!(g.resolve.filters, ["DUID=A,B"]);
                assert_eq!(g.resolve.columns, ["DUID", "VERSIONNO"]);
                assert_eq!(g.output.compression, CodecArg::Snappy);
                assert_eq!(g.output.format, FormatArg::Parquet);
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["utilake", "get", "T", "--from", "2024-01-01"]).is_err());
        assert!(
            Cli::try_parse_from([
                "utilake",
                "get",
                "T",
                "--at",
                "2024-01-01",
                "--raw",
                "--revisions"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "utilake",
                "get",
                "T",
                "--day",
                "2024-01-01",
                "--at",
                "2024-01-01"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["utilake", "get", "T", "--day", "x", "--downloads", "9"]).is_err()
        );
        let c = Cli::try_parse_from(["utilake", "ls", "DUDETAIL", "--values", "DUID"]).unwrap();
        match c.command {
            Command::List(l) => assert_eq!(l.values.as_deref(), Some("DUID")),
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["utilake", "list", "--coverage"]).is_err());
        assert!(Cli::try_parse_from(["utilake", "list", "--archive", "--files"]).is_err());
        let c = Cli::try_parse_from(["utilake", "verify", "--archive"]).unwrap();
        assert!(matches!(c.command, Command::Verify { archive: true, .. }));
    }

    #[test]
    fn every_command_has_examples_and_every_option_one() {
        fn walk(cmd: &clap::Command, path: &str) {
            let subs: Vec<_> = cmd.get_subcommands().collect();
            let has_examples = cmd
                .get_after_long_help()
                .is_some_and(|h| h.to_string().contains("Examples:"));
            assert!(
                has_examples
                    || !subs.is_empty()
                    || path.starts_with("utilake schema")
                    || path.starts_with("utilake wem"),
                "{path}: --help ends with no Examples:"
            );
            for a in cmd.get_arguments() {
                if a.is_positional() && a.get_id() == "command" {
                    continue;
                }
                let help = a
                    .get_long_help()
                    .or(a.get_help())
                    .map(std::string::ToString::to_string)
                    .unwrap_or_default();
                let flag = !a.is_positional() && a.get_action().takes_values();
                assert!(
                    !help.is_empty() && (!flag || help.contains('`')),
                    "{path} --{}: an option that takes a value shows an example in backticks; help was {help:?}",
                    a.get_id()
                );
            }
            for s in subs {
                walk(s, &format!("{path} {}", s.get_name()));
            }
        }
        walk(&Cli::command(), "utilake");
    }
}
