//! `nemlake-census`: keep `schemas/` in step with AEMO's archive.
//!
//! `census` records every block of every archived file of a month, and
//! `fixtures` cuts a month's test fixtures, both from the local archive
//! that `nemlake fetch nem` fills:
//!
//! ```text
//! nemlake-census census --from 2024-01 --to 2024-12
//! nemlake-census fixtures --month 2024-01 --out testdata/bootstrap/MMSDM_2024_01
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use nemlake_core::Diagnose;

#[derive(Parser)]
#[command(
    name = "nemlake-census",
    about = "Keep nemlake schema files in step with AEMO's archive"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record every block of every archived file of each month: its shape
    /// and rows, into `{schemas}/nem/census/{YYYY-MM}.toml` and
    /// `{schemas}/nem/shapes/{TABLE}.toml`. Reads the local archive only;
    /// download a month first with `nemlake fetch nem`. A month already
    /// recorded is skipped, so a run that stops can be run again.
    Census {
        /// First month.
        #[arg(long, value_name = "YYYY-MM")]
        from: nemlake_core::YearMonth,
        /// Last month (default: the first).
        #[arg(long, value_name = "YYYY-MM")]
        to: Option<nemlake_core::YearMonth>,
        /// Take the census of months already recorded again.
        #[arg(long)]
        again: bool,
        /// The nemlake home whose archive is read (default `~/.nemlake`).
        #[arg(long, env = "NEMLAKE_HOME")]
        home: Option<PathBuf>,
        /// The schemas directory to write into.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// Archive base URL, which names where the month's files are kept.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Cut the fixtures of a month from the local archive: its control
    /// files, its README and the head of every data file (each block's I
    /// row and first D rows).
    Fixtures {
        /// Archive month.
        #[arg(long)]
        month: nemlake_core::YearMonth,
        /// Output directory: `CTL/`, `heads/` and `README.txt` under it.
        #[arg(long)]
        out: PathBuf,
        /// D rows to keep per block.
        #[arg(long, default_value_t = 20)]
        rows: usize,
        /// The nemlake home whose archive is read (default `~/.nemlake`).
        #[arg(long, env = "NEMLAKE_HOME")]
        home: Option<PathBuf>,
        /// Archive base URL, which names where the month's files are kept.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Read AEMO's MMS Data Model Report and write it as JSON, which a new
    /// table takes its key and description from (network: one request per package and per
    /// table, a few hundred in all).
    Datamodel {
        /// Output directory; the file is `mms_data_model_{version}.json`.
        #[arg(long, default_value = "testdata/bootstrap/datamodel")]
        out: PathBuf,
        /// Milliseconds between two requests.
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        /// Where the report's pages live.
        #[arg(long, default_value = nemlake_census::dmreport::REPORT_BASE)]
        base: String,
    },
}

fn run_async<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(f)
}

fn fail(d: &nemlake_core::Diagnostic) -> ExitCode {
    eprintln!("error: {}", d.render());
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Census {
            from,
            to,
            again,
            home,
            schemas,
            base,
        } => census(
            from,
            to.unwrap_or(from),
            again,
            home.as_deref(),
            &schemas,
            &base,
        ),
        Cmd::Fixtures {
            month,
            out,
            rows,
            home,
            base,
        } => fixtures(month, &out, rows, home.as_deref(), &base),
        Cmd::Datamodel {
            out,
            interval_ms,
            base,
        } => {
            let fetcher = match nemlake_census::dmreport::report_fetcher(
                std::time::Duration::from_millis(interval_ms),
            ) {
                Ok(f) => f,
                Err(e) => return fail(&e.diagnostic()),
            };
            let report = match run_async(nemlake_census::dmreport::fetch_report(&fetcher, &base)) {
                Ok(r) => r,
                Err(e) => return fail(&e.diagnostic()),
            };
            let version = report.version.as_deref().unwrap_or("unversioned");
            let path = out.join(format!("mms_data_model_{version}.json"));
            let written = std::fs::create_dir_all(&out)
                .and_then(|()| std::fs::write(&path, nemlake_census::dmreport::to_json(&report)));
            match written {
                Ok(()) => {
                    eprintln!(
                        "{} table(s) of data model {version} written to {}",
                        report.tables.len(),
                        path.display()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {} could not be written: {e}", path.display());
                    ExitCode::from(1)
                }
            }
        }
    }
}

/// The archive of the nemlake home at `home`, else `~/.nemlake`, and the
/// month layout of the archive at `base`.
fn archive(
    home: Option<&Path>,
    base: &str,
) -> Result<(nemlake_acquire::Archive, nemlake_acquire::MmsdmArchive), ExitCode> {
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(home) = nemlake_core::paths::Home::resolve(home, user_home.as_deref()) else {
        eprintln!("error: no home directory; pass --home");
        return Err(ExitCode::from(2));
    };
    let nem = nemlake_acquire::MmsdmArchive::new(base).map_err(|e| fail(&e.diagnostic()))?;
    Ok((nemlake_acquire::Archive::new(home.archive()), nem))
}

/// A scratch directory for nested zips, removed when dropped.
fn scratch() -> Result<tempfile::TempDir, ExitCode> {
    tempfile::tempdir().map_err(|e| {
        eprintln!("error: no scratch directory: {e}");
        ExitCode::from(1)
    })
}

/// `census`: one month after another, each recorded once complete.
fn census(
    from: nemlake_core::YearMonth,
    to: nemlake_core::YearMonth,
    again: bool,
    home: Option<&Path>,
    schemas: &Path,
    base: &str,
) -> ExitCode {
    use nemlake_census::census::{census_month, census_path, month_files, record};
    let (store, nem) = match archive(home, base) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let tmp = match scratch() {
        Ok(t) => t,
        Err(code) => return code,
    };
    let registry = nemlake_registry::Registry::load_embedded();
    let mut code = ExitCode::SUCCESS;
    for month in from.range_inclusive(to) {
        if !again && census_path(schemas, month).is_file() {
            eprintln!("{month}: recorded already");
            continue;
        }
        let files = match month_files(&store, &nem, month) {
            Ok(f) => f,
            Err(e) => return fail(&e.diagnostic()),
        };
        let census = census_month(&registry, month, &files, tmp.path());
        for d in &census.problems {
            eprintln!("error: {}", d.render());
            code = ExitCode::from(3);
        }
        let added = match record(schemas, &registry, &census) {
            Ok(n) => n,
            Err(e) => return fail(&e.diagnostic()),
        };
        let blocks: usize = census.files.values().map(Vec::len).sum();
        eprintln!(
            "{month}: {} files, {blocks} blocks, {added} new shape(s), {} file(s) unread",
            census.files.len(),
            census.problems.len()
        );
    }
    code
}

/// `fixtures`: one month's fixtures from the local archive.
fn fixtures(
    month: nemlake_core::YearMonth,
    out: &Path,
    rows: usize,
    home: Option<&Path>,
    base: &str,
) -> ExitCode {
    let (store, nem) = match archive(home, base) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let tmp = match scratch() {
        Ok(t) => t,
        Err(code) => return code,
    };
    let files = match nemlake_census::census::month_files(&store, &nem, month) {
        Ok(f) => f,
        Err(e) => return fail(&e.diagnostic()),
    };
    match nemlake_census::fixtures::write_fixtures(&files, tmp.path(), out, rows) {
        Ok(w) => {
            for d in &w.problems {
                eprintln!("error: {}", d.render());
            }
            eprintln!(
                "{month}: {} head(s), {} control file(s){} written to {}",
                w.heads,
                w.ctls,
                if w.readme { " and the README" } else { "" },
                out.display()
            );
            if w.problems.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(3)
            }
        }
        Err(d) => fail(&d),
    }
}
