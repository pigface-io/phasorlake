//! `nemlake-census`: keep `schemas/` in step with AEMO's archive.
//!
//! `census` records every block of every archived file of a month, `sync`
//! brings the schemas up to the month's control files, and `fixtures` cuts
//! a month's test fixtures, all from the local archive that `nemlake fetch
//! nem` fills:
//!
//! ```text
//! nemlake-census census --from 2024-01 --to 2024-12
//! nemlake-census sync --from 2024-01 --to 2024-12 [--check]
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
    /// Bring `{schemas}/nem/` up to the control files of each archived
    /// month, oldest first: a table a month's control file lays out anew
    /// gains an increment, a table the schemas lack is started. Nothing
    /// there is changed. Take the census of a month first: a new table's
    /// report pairs and every increment's versions come from it.
    Sync {
        /// First month.
        #[arg(long, value_name = "YYYY-MM")]
        from: nemlake_core::YearMonth,
        /// Last month (default: the first).
        #[arg(long, value_name = "YYYY-MM")]
        to: Option<nemlake_core::YearMonth>,
        /// Report what would change, write nothing, and exit 3 if anything would.
        #[arg(long)]
        check: bool,
        /// Start every table's increments again from the months synced, in
        /// month order: for a first pass over the whole archive. Columns,
        /// ids and types stay; a table no month has keeps its increments.
        #[arg(long)]
        rebuild: bool,
        /// The nemlake home whose archive is read (default `~/.nemlake`).
        #[arg(long, env = "NEMLAKE_HOME")]
        home: Option<PathBuf>,
        /// The schemas directory.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// Archive base URL, which names where the month's files are kept.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
        /// The data-model JSON a new table takes its key and description from.
        #[arg(
            long,
            default_value = "testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json"
        )]
        datamodel: PathBuf,
        /// AEMO's archive README, which lists the standing-data tables.
        #[arg(long, default_value = "testdata/bootstrap/MMSDM_2026_08/README.txt")]
        readme: PathBuf,
        /// Standing-data tables the README does not name.
        #[arg(long, default_value = "crates/nemlake-census/snapshot_tables.toml")]
        snapshot_list: PathBuf,
    },
    /// Write every table file again in the current `schema_format`,
    /// deriving what a new format adds (format 3: `[temporal]`). Changes
    /// no column, id, type or increment.
    Migrate {
        /// The schemas directory.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// Report the files that would change, write nothing, and exit 3 if any would.
        #[arg(long)]
        check: bool,
    },
    /// Report, per table, where AEMO's PDR control files differ from the
    /// control files of our increments: columns only one of them loads,
    /// loader types that differ, tables only the PDR has. Report only.
    PdrDiff {
        /// The PDR control files.
        #[arg(long, default_value = "reference/pdr")]
        pdr: PathBuf,
        /// The schemas directory.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
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
        Cmd::Sync {
            from,
            to,
            check,
            rebuild,
            home,
            schemas,
            base,
            datamodel,
            readme,
            snapshot_list,
        } => sync(
            (from, to.unwrap_or(from)),
            (check, rebuild),
            home.as_deref(),
            &schemas,
            &base,
            (&datamodel, &readme, &snapshot_list),
        ),
        Cmd::Migrate { schemas, check } => match nemlake_census::migrate::run(&schemas, check) {
            Ok(changed) if check && !changed.is_empty() => {
                for p in &changed {
                    println!("would rewrite {}", p.display());
                }
                ExitCode::from(3)
            }
            Ok(changed) => {
                println!(
                    "{} table file(s) {}",
                    changed.len(),
                    if check { "out of date" } else { "rewritten" }
                );
                ExitCode::SUCCESS
            }
            Err(d) => fail(&d),
        },
        Cmd::PdrDiff { pdr, schemas } => {
            let tables = match nemlake_registry::file_model::blob::SchemaSet::read_dir(&schemas) {
                Ok(set) => set.tables,
                Err(e) => return fail(&e.diagnostic()),
            };
            match nemlake_census::pdr::report(&pdr, &schemas.join("nem"), &tables) {
                Ok(lines) => {
                    for l in &lines {
                        println!("{l}");
                    }
                    eprintln!("{} difference(s)", lines.len());
                    ExitCode::SUCCESS
                }
                Err(d) => fail(&d),
            }
        }
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

#[derive(serde::Deserialize)]
struct SnapshotList {
    observed: Vec<String>,
}

/// What a new table takes from outside its control file: the data model,
/// and the standing-data tables of the README and the list beside it.
fn facts(
    datamodel: &Path,
    readme: &Path,
    snapshot_list: &Path,
) -> Result<nemlake_census::sync::Facts, Box<nemlake_core::Diagnostic>> {
    let unreadable = |path: &Path, cause: &dyn std::fmt::Display| {
        Box::new(
            nemlake_core::Diagnostic::error(
                "schema.standing_list",
                nemlake_core::Category::Schema,
                format!(
                    "The list of standing-data tables {} could not be read: {cause}",
                    path.display()
                ),
            )
            .with_file(path.display().to_string()),
        )
    };
    let dm = nemlake_census::datamodel::DataModel::load(datamodel)
        .map_err(|e| Box::new(e.diagnostic()))?;
    let text = std::fs::read(readme).map_err(|e| unreadable(readme, &e))?;
    let mut snapshots: std::collections::BTreeSet<String> =
        nemlake_census::standing::readme_tables(&String::from_utf8_lossy(&text))
            .into_iter()
            .collect();
    if snapshots.is_empty() {
        return Err(unreadable(readme, &"it names no table"));
    }
    let list = std::fs::read_to_string(snapshot_list)
        .map_err(|e| unreadable(snapshot_list, &e))
        .and_then(|t| {
            toml::from_str::<SnapshotList>(&t).map_err(|e| unreadable(snapshot_list, &e))
        })?;
    snapshots.extend(list.observed);
    Ok(nemlake_census::sync::Facts {
        datamodel: Some(dm),
        snapshots,
    })
}

/// `sync`: one month after another, oldest first.
fn sync(
    (from, to): (nemlake_core::YearMonth, nemlake_core::YearMonth),
    (check, rebuild): (bool, bool),
    home: Option<&Path>,
    schemas: &Path,
    base: &str,
    (datamodel, readme, snapshot_list): (&Path, &Path, &Path),
) -> ExitCode {
    use nemlake_census::sync::{Evidence, Sync, by_table, read_ctls};
    let facts = match facts(datamodel, readme, snapshot_list) {
        Ok(f) => f,
        Err(d) => return fail(&d),
    };
    let (store, nem) = match archive(home, base) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let tmp = match scratch() {
        Ok(t) => t,
        Err(code) => return code,
    };
    let mut s = match Sync::load(schemas) {
        Ok(s) => s,
        Err(d) => return fail(&d),
    };
    let shapes: Vec<_> = match nemlake_registry::file_model::blob::SchemaSet::read_dir(schemas) {
        Ok(set) => set.shapes.into_values().flatten().collect(),
        Err(e) => return fail(&e.diagnostic()),
    };
    if rebuild {
        s.rebuild();
    }
    let mut code = ExitCode::SUCCESS;
    for month in from.range_inclusive(to) {
        let files = match nemlake_census::census::month_files(&store, &nem, month) {
            Ok(f) => f,
            // A month not downloaded here says nothing either way.
            Err(e @ nemlake_census::census::CensusError::NotArchived { .. }) => {
                eprintln!("note: {month} skipped: {e}");
                continue;
            }
            Err(e) => return fail(&e.diagnostic()),
        };
        let (found, mut problems) = read_ctls(&files, tmp.path());
        let (ctls, unparsed) = by_table(found);
        problems.extend(unparsed);
        for d in &problems {
            eprintln!("error: {}", d.render());
            code = ExitCode::from(3);
        }
        let evidence = match nemlake_census::census::read_month(schemas, month) {
            Ok(Some(files)) => Evidence::new(&files, &shapes),
            Ok(None) => {
                eprintln!(
                    "note: {month} has no census; new tables are skipped and no version is recorded (`just census {month}` first)"
                );
                Evidence::default()
            }
            Err(e) => return fail(&e.diagnostic()),
        };
        let said = s.month(month, &ctls, &evidence, &facts);
        for line in &said {
            println!("{month}: {line}");
        }
        eprintln!(
            "{month}: {} control file(s), {} change(s)",
            ctls.len(),
            said.len()
        );
    }
    for line in s.finish() {
        println!("{line}");
    }
    let changed = s.changed();
    if check {
        if changed > 0 {
            eprintln!("{changed} table(s) would change; run `just schema-sync` for the months");
            return ExitCode::from(3);
        }
        eprintln!("no table would change");
        return code;
    }
    match s.write() {
        Ok(n) => {
            eprintln!("{n} table(s) written");
            code
        }
        Err(d) => fail(&d),
    }
}
