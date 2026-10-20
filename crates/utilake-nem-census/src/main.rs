//! `utilake-nem-census`: keep `schemas/` in step with AEMO's archive.
//!
//! `census` records every block of every archived file of a month, `sync`
//! brings the schemas up to the month's control files, and `fixtures` cuts
//! a month's test fixtures, all from the local archive that `utilake fetch
//! nem` fills:
//!
//! ```text
//! utilake-nem-census census --from 2024-01 --to 2024-12
//! utilake-nem-census sync --from 2024-01 --to 2024-12 [--check]
//! utilake-nem-census fixtures --month 2024-01 --out testdata/bootstrap/MMSDM_2024_01
//! utilake-nem-census models --from 2022-10 --to 2026-09
//! utilake-nem-census export --out ../nem-schemas [--check]
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use utilake_core::Diagnose;

#[derive(Parser)]
#[command(
    name = "utilake-nem-census",
    about = "Keep utilake schema files in step with AEMO's archive"
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
    /// download a month first with `utilake fetch nem`. A month already
    /// recorded is skipped, so a run that stops can be run again.
    Census {
        /// First month.
        #[arg(long, value_name = "YYYY-MM")]
        from: utilake_core::YearMonth,
        /// Last month (default: the first).
        #[arg(long, value_name = "YYYY-MM")]
        to: Option<utilake_core::YearMonth>,
        /// Take the census of months already recorded again.
        #[arg(long)]
        again: bool,
        /// The utilake home whose archive is read (default `~/.utilake`).
        #[arg(long, env = "UTILAKE_HOME")]
        home: Option<PathBuf>,
        /// The schemas directory to write into.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// Archive base URL, which names where the month's files are kept.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Record every block of every archived file under nemweb's report
    /// folders (`Reports/ARCHIVE`, `Reports/CURRENT`), per folder, month
    /// and shape, into `{schemas}/nem/census/reports/{ROOT}/{folder}.toml`,
    /// and add each shape to `{schemas}/nem/shapes/{TABLE}.toml` with the
    /// months it was seen. Blocks are typed by the control files of the
    /// MMSDM month they fall in. A folder already recorded is skipped.
    Reports {
        /// Report roots to read (default both).
        #[arg(long, value_name = "ROOT", default_values_t = utilake_nem_census::reports::ROOTS.map(String::from))]
        root: Vec<String>,
        /// Only folders whose path starts with this.
        #[arg(long)]
        folder: Option<String>,
        /// Take the census of folders already recorded again.
        #[arg(long)]
        again: bool,
        /// The utilake home whose archive is read (default `~/.utilake`).
        #[arg(long, env = "UTILAKE_HOME")]
        home: Option<PathBuf>,
        /// The schemas directory to write into.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// Archive base URL, which names where the files are kept.
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
        from: utilake_core::YearMonth,
        /// Last month (default: the first).
        #[arg(long, value_name = "YYYY-MM")]
        to: Option<utilake_core::YearMonth>,
        /// Report what would change, write nothing, and exit 3 if anything would.
        #[arg(long)]
        check: bool,
        /// Start every table's increments again from the months synced, in
        /// month order: for a first pass over the whole archive. Columns,
        /// ids and types stay; a table no month has keeps its increments.
        #[arg(long)]
        rebuild: bool,
        /// The utilake home whose archive is read (default `~/.utilake`).
        #[arg(long, env = "UTILAKE_HOME")]
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
        #[arg(long, default_value = "crates/utilake-nem-census/snapshot_tables.toml")]
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
        month: utilake_core::YearMonth,
        /// Output directory: `CTL/`, `heads/` and `README.txt` under it.
        #[arg(long)]
        out: PathBuf,
        /// D rows to keep per block.
        #[arg(long, default_value_t = 20)]
        rows: usize,
        /// The utilake home whose archive is read (default `~/.utilake`).
        #[arg(long, env = "UTILAKE_HOME")]
        home: Option<PathBuf>,
        /// Archive base URL, which names where the month's files are kept.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Read the data-model create scripts the archive carries
    /// (`DOCUMENTATION/MMS Data Model/v5.x/MMSDM_create_v5.x.zip`, from
    /// 2022-10 on) into `{schemas}/nem/models/{version}.toml`, one file per
    /// release with the months it shipped in, and `months.toml`. A version
    /// already recorded only widens its months.
    Models {
        /// First month.
        #[arg(long, value_name = "YYYY-MM")]
        from: utilake_core::YearMonth,
        /// Last month (default: the first).
        #[arg(long, value_name = "YYYY-MM")]
        to: Option<utilake_core::YearMonth>,
        /// The utilake home whose archive is read (default `~/.utilake`).
        #[arg(long, env = "UTILAKE_HOME")]
        home: Option<PathBuf>,
        /// The schemas directory to write into.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// Archive base URL, which names where the month's files are kept.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Write every table and shape as JSON Schema into a `nem-schemas`
    /// checkout: `nem/{TABLE}/{shape}.schema.json` and `index.json` per
    /// table, `manifest.json`, `provenance.json` and `LICENCE.md`, from the
    /// schemas, every census month and the data model. The checkout's
    /// `nem/` is owned whole: a shape no longer recorded is removed.
    Export {
        /// The checkout to write into.
        #[arg(long)]
        out: PathBuf,
        /// Compare instead of writing: list every file that is missing,
        /// differs or should not be there, and exit 3 if any.
        #[arg(long)]
        check: bool,
        /// The schemas directory.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// The data-model JSON, for packages, visibility and column comments.
        #[arg(
            long,
            default_value = "testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json"
        )]
        datamodel: PathBuf,
        /// Where the documents are published; each `$id` is this plus the file's path.
        #[arg(long, default_value = utilake_nem_census::export::DEFAULT_BASE_URL)]
        base_url: String,
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
        #[arg(long, default_value = utilake_nem_census::dmreport::REPORT_BASE)]
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

fn fail(d: &utilake_core::Diagnostic) -> ExitCode {
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
        Cmd::Reports {
            root,
            folder,
            again,
            home,
            schemas,
            base,
        } => reports(
            &root,
            folder.as_deref(),
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
        Cmd::Migrate { schemas, check } => {
            match utilake_nem_census::migrate::run(&schemas, check) {
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
            }
        }
        Cmd::PdrDiff { pdr, schemas } => {
            let tables = match utilake_mms_registry::file_model::blob::SchemaSet::read_dir(&schemas)
            {
                Ok(set) => set.tables,
                Err(e) => return fail(&e.diagnostic()),
            };
            match utilake_nem_census::pdr::report(&pdr, &schemas.join("nem"), &tables) {
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
        Cmd::Models {
            from,
            to,
            home,
            schemas,
            base,
        } => models(from, to.unwrap_or(from), home.as_deref(), &schemas, &base),
        Cmd::Export {
            out,
            check,
            schemas,
            datamodel,
            base_url,
        } => export(&out, check, &schemas, &datamodel, base_url),
        Cmd::Datamodel {
            out,
            interval_ms,
            base,
        } => {
            let fetcher = match utilake_nem_census::dmreport::report_fetcher(
                std::time::Duration::from_millis(interval_ms),
            ) {
                Ok(f) => f,
                Err(e) => return fail(&e.diagnostic()),
            };
            let report =
                match run_async(utilake_nem_census::dmreport::fetch_report(&fetcher, &base)) {
                    Ok(r) => r,
                    Err(e) => return fail(&e.diagnostic()),
                };
            let version = report.version.as_deref().unwrap_or("unversioned");
            let path = out.join(format!("mms_data_model_{version}.json"));
            let written = std::fs::create_dir_all(&out).and_then(|()| {
                std::fs::write(&path, utilake_nem_census::dmreport::to_json(&report))
            });
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

/// The archive of the utilake home at `home`, else `~/.utilake`, and the
/// month layout of the archive at `base`.
fn archive(
    home: Option<&Path>,
    base: &str,
) -> Result<(utilake_acquire::Archive, utilake_nem::MmsdmArchive), ExitCode> {
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(home) = utilake_core::paths::Home::resolve(home, user_home.as_deref()) else {
        eprintln!("error: no home directory; pass --home");
        return Err(ExitCode::from(2));
    };
    let nem = utilake_nem::MmsdmArchive::new(base).map_err(|e| fail(&e.diagnostic()))?;
    Ok((utilake_acquire::Archive::new(home.archive()), nem))
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
    from: utilake_core::YearMonth,
    to: utilake_core::YearMonth,
    again: bool,
    home: Option<&Path>,
    schemas: &Path,
    base: &str,
) -> ExitCode {
    use utilake_nem_census::census::{census_month, census_path, month_files, record};
    let (store, nem) = match archive(home, base) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let tmp = match scratch() {
        Ok(t) => t,
        Err(code) => return code,
    };
    let registry = utilake_mms_registry::Registry::load_embedded();
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

/// `reports`: one folder after another, each recorded once complete.
fn reports(
    roots: &[String],
    folder: Option<&str>,
    again: bool,
    home: Option<&Path>,
    schemas: &Path,
    base: &str,
) -> ExitCode {
    use utilake_nem_census::reports::{
        CtlSource, census_folder, folder_path, folders, record_folder, top_folders,
    };
    let (store, nem) = match archive(home, base) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let tmp = match scratch() {
        Ok(t) => t,
        Err(code) => return code,
    };
    let registry = utilake_mms_registry::Registry::load_embedded();
    let mut ctls = CtlSource::new(&store, &nem);
    let mut code = ExitCode::SUCCESS;
    for root in roots {
        let probe = store.path_for(&format!("{}/Reports/{root}/x", base.trim_end_matches('/')));
        let Some(dir) = probe.parent() else { continue };
        let tops = top_folders(dir);
        if tops.is_empty() {
            eprintln!("Reports/{root}: nothing archived under {}", dir.display());
        }
        for top in tops {
            let name = top
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if folder.is_some_and(|f| {
                let first = f.split('/').next().unwrap_or(f);
                !name.starts_with(first) && !first.starts_with(&name)
            }) {
                continue;
            }
            for (rel, files) in folders(&store, dir, &top) {
                if folder.is_some_and(|f| !rel.starts_with(f)) {
                    continue;
                }
                if !again && folder_path(schemas, root, &rel).is_file() {
                    eprintln!("Reports/{root}/{rel}: recorded already");
                    continue;
                }
                let (census, shapes) = census_folder(&registry, &mut ctls, &files, tmp.path());
                for u in &census.unread {
                    eprintln!("error: Reports/{root}/{rel}: {} {}", u.file, u.error);
                    code = ExitCode::from(3);
                }
                let added = match record_folder(schemas, &registry, root, &rel, &census, &shapes) {
                    Ok(n) => n,
                    Err(e) => return fail(&e.diagnostic()),
                };
                eprintln!(
                    "Reports/{root}/{rel}: {} files, {} CSVs, {} month-shape(s), {added} new shape(s), {} unread",
                    census.files,
                    census.csvs,
                    census.blocks.len(),
                    census.unread.len()
                );
            }
        }
    }
    code
}

/// `fixtures`: one month's fixtures from the local archive.
fn fixtures(
    month: utilake_core::YearMonth,
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
    let files = match utilake_nem_census::census::month_files(&store, &nem, month) {
        Ok(f) => f,
        Err(e) => return fail(&e.diagnostic()),
    };
    match utilake_nem_census::fixtures::write_fixtures(&files, tmp.path(), out, rows) {
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

/// `models`: one month after another, each release recorded once.
fn models(
    from: utilake_core::YearMonth,
    to: utilake_core::YearMonth,
    home: Option<&Path>,
    schemas: &Path,
    base: &str,
) -> ExitCode {
    use utilake_nem_census::models::{model_zips, record};
    let (store, nem) = match archive(home, base) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let mut code = ExitCode::SUCCESS;
    for month in from.range_inclusive(to) {
        let files = match utilake_nem_census::census::month_files(&store, &nem, month) {
            Ok(f) => f,
            Err(e @ utilake_nem_census::census::CensusError::NotArchived { .. }) => {
                eprintln!("note: {month} skipped: {e}");
                continue;
            }
            Err(e) => return fail(&e.diagnostic()),
        };
        let zips = model_zips(&files);
        let r = match record(schemas, month, &zips) {
            Ok(r) => r,
            Err(e) => return fail(&e.diagnostic()),
        };
        for d in &r.problems {
            eprintln!("error: {}", d.render());
            code = ExitCode::from(3);
        }
        eprintln!(
            "{month}: {} model zip(s), versions [{}], {} new",
            zips.len(),
            r.versions.join(", "),
            r.new_versions.len()
        );
    }
    code
}

/// `export`: render everything, then write it or compare it.
fn export(out: &Path, check: bool, schemas: &Path, datamodel: &Path, base_url: String) -> ExitCode {
    use utilake_nem_census::export::{Export, Inputs};
    let inputs = Inputs {
        schemas: schemas.to_path_buf(),
        datamodel: Some(datamodel.to_path_buf()),
        base_url,
    };
    let rendered = match Export::load(&inputs).and_then(|e| e.render()) {
        Ok(r) => r,
        Err(d) => return fail(&d),
    };
    for d in &rendered.problems {
        eprintln!("warning: {}", d.render());
    }
    let mut code = if rendered.problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(3)
    };
    if check {
        let stale = match utilake_nem_census::export::check(out, &rendered) {
            Ok(s) => s,
            Err(d) => return fail(&d),
        };
        for s in &stale {
            eprintln!("error: {}", s.diagnostic().render());
        }
        if stale.is_empty() {
            eprintln!(
                "{} file(s) current in {}",
                rendered.files.len(),
                out.display()
            );
        } else {
            eprintln!("{} file(s) stale in {}", stale.len(), out.display());
            code = ExitCode::from(3);
        }
        return code;
    }
    match utilake_nem_census::export::write(out, &rendered) {
        Ok(w) => {
            eprintln!(
                "{} file(s) written, {} kept, {} removed in {}",
                w.written,
                w.kept,
                w.removed,
                out.display()
            );
            code
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
) -> Result<utilake_nem_census::sync::Facts, Box<utilake_core::Diagnostic>> {
    let unreadable = |path: &Path, cause: &dyn std::fmt::Display| {
        Box::new(
            utilake_core::Diagnostic::error(
                "schema.standing_list",
                utilake_core::Category::Schema,
                format!(
                    "The list of standing-data tables {} could not be read: {cause}",
                    path.display()
                ),
            )
            .with_file(path.display().to_string()),
        )
    };
    let dm = utilake_nem_census::datamodel::DataModel::load(datamodel)
        .map_err(|e| Box::new(e.diagnostic()))?;
    let text = std::fs::read(readme).map_err(|e| unreadable(readme, &e))?;
    let mut snapshots: std::collections::BTreeSet<String> =
        utilake_nem_census::standing::readme_tables(&String::from_utf8_lossy(&text))
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
    Ok(utilake_nem_census::sync::Facts {
        datamodel: Some(dm),
        snapshots,
    })
}

/// `sync`: one month after another, oldest first.
fn sync(
    (from, to): (utilake_core::YearMonth, utilake_core::YearMonth),
    (check, rebuild): (bool, bool),
    home: Option<&Path>,
    schemas: &Path,
    base: &str,
    (datamodel, readme, snapshot_list): (&Path, &Path, &Path),
) -> ExitCode {
    use utilake_nem_census::sync::{Evidence, Sync, by_table, read_ctls};
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
    let shapes: Vec<_> = match utilake_mms_registry::file_model::blob::SchemaSet::read_dir(schemas)
    {
        Ok(set) => set.shapes.into_values().flatten().collect(),
        Err(e) => return fail(&e.diagnostic()),
    };
    if rebuild {
        s.rebuild();
    }
    let mut code = ExitCode::SUCCESS;
    for month in from.range_inclusive(to) {
        let files = match utilake_nem_census::census::month_files(&store, &nem, month) {
            Ok(f) => f,
            // A month not downloaded here says nothing either way.
            Err(e @ utilake_nem_census::census::CensusError::NotArchived { .. }) => {
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
        let evidence = match utilake_nem_census::census::read_month(schemas, month) {
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
