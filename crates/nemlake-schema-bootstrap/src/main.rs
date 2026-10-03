//! `nemlake-schema-bootstrap`: regenerate `schemas/nem/mms/*.toml`.
//!
//! ```text
//! nemlake-schema-bootstrap regen --schemas schemas --ctl testdata/bootstrap/MMSDM_2024_01/CTL \
//!     --datamodel testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json --samples testdata/csv
//! ```
//!
//! Every table that has a CTL or a sample is (re)written; files are only
//! touched when their content changes, so a no-op regen leaves git clean.
//!
//! `heads` and `ctls` gather the fixtures from nemweb (network):
//!
//! ```text
//! nemlake-schema-bootstrap heads --month 2024-01 --out testdata/bootstrap/MMSDM_2024_01/heads
//! nemlake-schema-bootstrap ctls  --month 2024-01 --out testdata/bootstrap/MMSDM_2024_01/CTL
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use nemlake_core::Diagnose;
use nemlake_schema_bootstrap::ctl::{self, CtlFile};
use nemlake_schema_bootstrap::datamodel::DataModel;
use nemlake_schema_bootstrap::merge::{Inputs, merge};
use nemlake_schema_bootstrap::sample::{SampledBlock, index_by_pair, sample_dir, table_from_stem};
use nemlake_schema_model::TableDef;
use nemlake_schema_model::blob::SchemaSet;
use nemlake_schema_model::file::{read_table, write_table};

#[derive(Parser)]
#[command(
    name = "nemlake-schema-bootstrap",
    about = "Regenerate nemlake schema files from AEMO sources"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Regenerate table files from CTLs, the data model and sampled files.
    Regen {
        /// The `schemas/` directory to write into.
        #[arg(long, default_value = "schemas")]
        schemas: PathBuf,
        /// Directories of `PUBLIC_DVD_*.ctl` files; repeatable. When two
        /// directories have a control file for the same table the first wins,
        /// so name the newest month first.
        #[arg(long, required = true)]
        ctl: Vec<PathBuf>,
        /// The data-model JSON, as the `datamodel` subcommand writes it.
        #[arg(long)]
        datamodel: PathBuf,
        /// Directories of `.CSV`/`.zip` files to sample I rows from; repeatable.
        #[arg(long, required = true)]
        samples: Vec<PathBuf>,
        /// AEMO's archive README, which lists the standing-data tables.
        #[arg(long)]
        readme: PathBuf,
        /// Standing-data tables the README does not name, each borne out
        /// by the samples.
        #[arg(
            long,
            default_value = "crates/nemlake-schema-bootstrap/snapshot_tables.toml"
        )]
        snapshot_list: PathBuf,
        /// Only these tables (comma separated).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Report what would change without writing.
        #[arg(long)]
        check: bool,
    },
    /// Fetch the head of every archive zip of a month as fixtures (network).
    Heads {
        /// Archive month.
        #[arg(long)]
        month: nemlake_core::YearMonth,
        /// Output directory: one `{stem}.CSV` per zip.
        #[arg(long, required_unless_present = "bundle", conflicts_with = "bundle")]
        out: Option<PathBuf>,
        /// Output file: every head of the month in one multi-block CSV.
        #[arg(long)]
        bundle: Option<PathBuf>,
        /// D rows to keep per block.
        #[arg(long, default_value_t = 20)]
        rows: usize,
        /// Bytes to request of each zip.
        #[arg(long, default_value_t = nemlake_schema_bootstrap::remote::HEAD_BYTES)]
        bytes: u64,
        /// Milliseconds between two requests; nemweb blocks fast clients.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// Where listings and zip indexes are cached between runs.
        #[arg(long, default_value = "target/bootstrap-cache")]
        cache: PathBuf,
        /// Archive base URL.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Fetch the control files of a month (network).
    Ctls {
        /// Archive month.
        #[arg(long)]
        month: nemlake_core::YearMonth,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        /// Skip tables that already have a control file in `--out` or in
        /// one of these directories; repeatable.
        #[arg(long)]
        skip_known: Vec<PathBuf>,
        /// Milliseconds between two requests; nemweb blocks fast clients.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// Where listings and zip indexes are cached between runs.
        #[arg(long, default_value = "target/bootstrap-cache")]
        cache: PathBuf,
        /// Archive base URL.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Read AEMO's MMS Data Model Report and write it as the JSON `regen`
    /// takes with `--datamodel` (network: one request per package and per
    /// table, a few hundred in all).
    Datamodel {
        /// Output directory; the file is `mms_data_model_{version}.json`.
        #[arg(long, default_value = "testdata/bootstrap/datamodel")]
        out: PathBuf,
        /// Milliseconds between two requests.
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        /// Where fetched pages are cached between runs.
        #[arg(long, default_value = "target/bootstrap-cache")]
        cache: PathBuf,
        /// Where the report's pages live.
        #[arg(long, default_value = nemlake_schema_bootstrap::dmreport::REPORT_BASE)]
        base: String,
    },
    /// Fetch the README of a month's archive, which lists the tables AEMO
    /// publishes as standing data (network: one request).
    Readme {
        /// Archive month.
        #[arg(long)]
        month: nemlake_core::YearMonth,
        /// Output directory; the file is `README.txt`.
        #[arg(long)]
        out: PathBuf,
        /// Milliseconds between two requests; nemweb blocks fast clients.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// Where listings and zip indexes are cached between runs.
        #[arg(long, default_value = "target/bootstrap-cache")]
        cache: PathBuf,
        /// Archive base URL.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Gather what historical months add: a bundle of heads per month and
    /// the control files that type columns later months dropped (network).
    History {
        /// Archive months, in the order to visit them; newest first keeps
        /// the newest control file for each table.
        #[arg(long, required = true, value_delimiter = ',')]
        months: Vec<nemlake_core::YearMonth>,
        /// Output directory: `MMSDM_{YYYY}_{MM}.heads.csv` and `CTL/`.
        #[arg(long)]
        dir: PathBuf,
        /// Directories of control files already on hand; repeatable.
        #[arg(long)]
        ctl: Vec<PathBuf>,
        /// D rows to keep per block.
        #[arg(long, default_value_t = 1)]
        rows: usize,
        /// Bytes to request of each zip.
        #[arg(long, default_value_t = 65_536)]
        bytes: u64,
        /// Milliseconds between two requests; nemweb blocks fast clients.
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        /// Where listings and zip indexes are cached between runs.
        #[arg(long, default_value = "target/bootstrap-cache")]
        cache: PathBuf,
        /// Archive base URL.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
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

#[derive(serde::Deserialize)]
struct SnapshotList {
    observed: Vec<String>,
}

fn fail(d: &nemlake_core::Diagnostic) -> ExitCode {
    eprintln!("error: {}", d.render());
    ExitCode::from(1)
}

/// Maps an archive file stem's table part to the table it loads.
///
/// `PREDISPATCHREGIONSUM_D` loads `PREDISPATCHREGIONSUM` (the CTL says so);
/// `BIDPEROFFER1` is a split of `BIDPEROFFER` (the data model knows the
/// shorter name). Anything else is itself.
fn canonical_table(
    stem_table: &str,
    by_infile: &BTreeMap<String, String>,
    dm: &DataModel,
) -> String {
    if let Some(t) = by_infile.get(stem_table) {
        return t.clone();
    }
    if dm.table(stem_table).is_some() {
        return stem_table.to_owned();
    }
    let trimmed = stem_table.trim_end_matches(|c: char| c.is_ascii_digit());
    if !trimmed.is_empty() && dm.table(trimmed).is_some() {
        trimmed.to_owned()
    } else {
        stem_table.to_owned()
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Regen {
            schemas,
            ctl,
            datamodel,
            samples,
            readme,
            snapshot_list,
            only,
            check,
        } => {
            match regen(
                &schemas,
                &ctl,
                &datamodel,
                &samples,
                &readme,
                &snapshot_list,
                &only,
                check,
            ) {
                Ok(changed) => {
                    if check && changed > 0 {
                        eprintln!("{changed} schema file(s) would change; run `just schema-regen`");
                        return ExitCode::from(3);
                    }
                    ExitCode::SUCCESS
                }
                Err(d) => fail(&d),
            }
        }
        Cmd::Heads {
            month,
            out,
            bundle,
            rows,
            bytes,
            interval_ms,
            cache,
            base,
        } => {
            use nemlake_schema_bootstrap::remote::{HeadDest, HeadOptions};
            let dest = match (out, bundle) {
                (_, Some(f)) => HeadDest::Bundle(f),
                (Some(d), None) => HeadDest::Dir(d),
                (None, None) => unreachable!("clap requires --out or --bundle"),
            };
            let opts = HeadOptions { rows, bytes };
            let (archive, fetcher) = match network(&base, &cache, interval_ms) {
                Ok(n) => n,
                Err(code) => return code,
            };
            let result = run_async(nemlake_schema_bootstrap::remote::fetch_heads(
                &fetcher, &archive, &dest, month, opts,
            ));
            match result {
                Ok(outcomes) => {
                    let mut failed = 0;
                    for o in &outcomes {
                        match (&o.path, &o.error) {
                            (Some(_), _) if matches!(dest, HeadDest::Bundle(_)) => {}
                            (Some(p), _) => eprintln!("wrote {}", p.display()),
                            (None, Some(e)) => {
                                failed += 1;
                                eprintln!("failed {}: {e}", o.url);
                            }
                            (None, None) => {}
                        }
                    }
                    eprintln!("{} head(s), {failed} failed", outcomes.len());
                    if failed > 0 {
                        ExitCode::from(3)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(e) => fail(&e.diagnostic()),
            }
        }
        Cmd::Ctls {
            month,
            out,
            skip_known,
            interval_ms,
            cache,
            base,
        } => {
            let (archive, fetcher) = match network(&base, &cache, interval_ms) {
                Ok(n) => n,
                Err(code) => return code,
            };
            let result = run_async(nemlake_schema_bootstrap::remote::fetch_ctls(
                &fetcher,
                &archive,
                &out,
                month,
                &skip_known,
            ));
            match result {
                Ok(paths) => {
                    eprintln!("{} CTL file(s) written to {}", paths.len(), out.display());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&e.diagnostic()),
            }
        }
        Cmd::Readme {
            month,
            out,
            interval_ms,
            cache,
            base,
        } => {
            let (archive, fetcher) = match network(&base, &cache, interval_ms) {
                Ok(n) => n,
                Err(code) => return code,
            };
            match run_async(nemlake_schema_bootstrap::remote::fetch_readme(
                &fetcher, &archive, &out, month,
            )) {
                Ok(path) => {
                    eprintln!("README written to {}", path.display());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&e.diagnostic()),
            }
        }
        Cmd::Datamodel {
            out,
            interval_ms,
            cache,
            base,
        } => {
            let transport = match nemlake_schema_bootstrap::remote::nemweb_transport() {
                Ok(t) => t,
                Err(e) => return fail(&e.diagnostic()),
            };
            let fetcher = nemlake_schema_bootstrap::remote::paced_fetcher(
                transport,
                &cache,
                std::time::Duration::from_millis(interval_ms),
            );
            let report = match run_async(nemlake_schema_bootstrap::dmreport::fetch_report(
                &fetcher, &base,
            )) {
                Ok(r) => r,
                Err(e) => return fail(&e.diagnostic()),
            };
            let version = report.version.as_deref().unwrap_or("unversioned");
            let path = out.join(format!("mms_data_model_{version}.json"));
            let written = std::fs::create_dir_all(&out).and_then(|()| {
                std::fs::write(&path, nemlake_schema_bootstrap::dmreport::to_json(&report))
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
        Cmd::History {
            months,
            dir,
            ctl,
            rows,
            bytes,
            interval_ms,
            cache,
            base,
        } => {
            let (archive, fetcher) = match network(&base, &cache, interval_ms) {
                Ok(n) => n,
                Err(code) => return code,
            };
            let opts = nemlake_schema_bootstrap::remote::HeadOptions { rows, bytes };
            for month in months {
                let result = run_async(nemlake_schema_bootstrap::remote::fetch_history_month(
                    &fetcher, &archive, &dir, &ctl, month, opts,
                ));
                match result {
                    Ok(m) => eprintln!(
                        "{month}: {} head(s){}, {} control file(s) fetched",
                        m.heads,
                        if m.fetched_bundle {
                            ""
                        } else {
                            " (already here)"
                        },
                        m.ctls.len()
                    ),
                    // Stop at the first failure: a refusal must not be followed by more requests.
                    Err(e) => return fail(&e.diagnostic()),
                }
            }
            ExitCode::SUCCESS
        }
    }
}

/// The archive and the paced fetcher every network subcommand uses.
fn network(
    base: &str,
    cache: &Path,
    interval_ms: u64,
) -> Result<(nemlake_fetch::MmsdmArchive, nemlake_fetch::Fetcher), ExitCode> {
    let archive = nemlake_fetch::MmsdmArchive::new(base).map_err(|e| fail(&e.diagnostic()))?;
    let transport =
        nemlake_schema_bootstrap::remote::nemweb_transport().map_err(|e| fail(&e.diagnostic()))?;
    let fetcher = nemlake_schema_bootstrap::remote::paced_fetcher(
        transport,
        cache,
        std::time::Duration::from_millis(interval_ms),
    );
    Ok((archive, fetcher))
}

#[allow(clippy::too_many_arguments)]
fn regen(
    schemas: &Path,
    ctl_dirs: &[PathBuf],
    datamodel: &Path,
    sample_dirs: &[PathBuf],
    readme: &Path,
    snapshot_list: &Path,
    only: &[String],
    check: bool,
) -> Result<usize, Box<nemlake_core::Diagnostic>> {
    let dm = DataModel::load(datamodel).map_err(|e| Box::new(e.diagnostic()))?;
    // Standing-data tables: the ones AEMO's README names, and the ones the
    // samples show to behave the same. A list that cannot be read is an
    // error, not an empty list: it would turn every such table into one of
    // monthly rows without a word.
    let unreadable = |path: &Path, what: &str, cause: &dyn std::fmt::Display| {
        Box::new(
            nemlake_core::Diagnostic::error(
                "schema.standing_list",
                nemlake_core::Category::Schema,
                format!("{what} {} could not be read: {cause}", path.display()),
            )
            .with_file(path.display().to_string()),
        )
    };
    let readme_text =
        std::fs::read(readme).map_err(|e| unreadable(readme, "The archive README", &e))?;
    let mut snapshots: BTreeSet<String> =
        nemlake_schema_bootstrap::standing::readme_tables(&String::from_utf8_lossy(&readme_text))
            .into_iter()
            .collect();
    if snapshots.is_empty() {
        return Err(unreadable(
            readme,
            "The archive README",
            &"it lists no standing-data table",
        ));
    }
    let list_text = std::fs::read_to_string(snapshot_list).map_err(|e| {
        unreadable(
            snapshot_list,
            "The list of observed standing-data tables",
            &e,
        )
    })?;
    let list: SnapshotList = toml::from_str(&list_text).map_err(|e| {
        unreadable(
            snapshot_list,
            "The list of observed standing-data tables",
            &e,
        )
    })?;
    snapshots.extend(list.observed);

    // CTLs by the table they load, plus the archive stem each one reads.
    // Every control file of a table, to be sorted newest month first.
    let mut ctls: BTreeMap<String, Vec<(Option<nemlake_core::YearMonth>, CtlFile, String)>> =
        BTreeMap::new();
    let mut by_infile: BTreeMap<String, String> = BTreeMap::new();
    let mut ctl_paths: Vec<PathBuf> = Vec::new();
    for ctl_dir in ctl_dirs {
        let mut in_dir: Vec<_> = std::fs::read_dir(ctl_dir)
            .map_err(|e| {
                Box::new(
                    nemlake_core::Diagnostic::error(
                        "schema.ctl_dir",
                        nemlake_core::Category::Schema,
                        format!("The CTL directory {} could not be read.", ctl_dir.display()),
                    )
                    .caused_by(&e),
                )
            })?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "ctl"))
            .collect();
        in_dir.sort();
        ctl_paths.extend(in_dir);
    }
    for p in ctl_paths {
        let text = std::fs::read_to_string(&p).map_err(|e| {
            Box::new(
                nemlake_core::Diagnostic::error(
                    "schema.ctl_read",
                    nemlake_core::Category::Schema,
                    format!("The CTL {} could not be read.", p.display()),
                )
                .caused_by(&e),
            )
        })?;
        let f =
            ctl::parse(&p.display().to_string(), &text).map_err(|e| Box::new(e.diagnostic()))?;
        let name = f.table.to_ascii_uppercase();
        if let Some((stem_table, _)) = table_from_stem(f.infile.trim_end_matches(".CSV")) {
            by_infile.insert(stem_table, name.clone());
        }
        let rel = p
            .components()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<PathBuf>();
        let month = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(nemlake_core::archive_name::parse)
            .map(|n| n.month);
        ctls.entry(name)
            .or_default()
            .push((month, f, rel.display().to_string()));
    }
    for list in ctls.values_mut() {
        // Stable: files of one month keep directory and name order.
        list.sort_by_key(|(m, _, _)| std::cmp::Reverse(*m));
    }

    // Samples: by archive-stem table when the file name has one, else by
    // report pair looked up through the CTL (which names both).
    let mut files = Vec::new();
    for d in sample_dirs {
        files.extend(sample_dir(d, 1000).map_err(|e| Box::new(e.diagnostic()))?);
    }
    let pair_index = index_by_pair(&files);
    let mut pair_to_table: BTreeMap<(String, String), String> = BTreeMap::new();
    for f in &files {
        if let Some(stem_table) = &f.table {
            let t = canonical_table(stem_table, &by_infile, &dm);
            for b in &f.blocks {
                pair_to_table.insert((b.report_type.clone(), b.report_subtype.clone()), t.clone());
            }
        }
    }
    // Fixture CSVs are named `{TABLE}_v{N}_head.csv`; use that too.
    for d in sample_dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            for p in rd.filter_map(Result::ok).map(|e| e.path()) {
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if let Some(table_part) = stem
                    .strip_suffix("_head")
                    .and_then(|s| s.rsplit_once("_v").map(|(t, _)| t))
                {
                    let t = canonical_table(table_part, &by_infile, &dm);
                    if let Ok(s) = nemlake_schema_bootstrap::sample::sample_path(&p, 1) {
                        for b in &s.blocks {
                            pair_to_table
                                .entry((b.report_type.clone(), b.report_subtype.clone()))
                                .or_insert(t.clone());
                        }
                    }
                }
            }
        }
    }
    let mut samples_by_table: BTreeMap<
        String,
        Vec<(Option<nemlake_core::YearMonth>, SampledBlock)>,
    > = BTreeMap::new();
    for (pair, versions) in &pair_index {
        let Some(table) = pair_to_table.get(pair) else {
            eprintln!(
                "note: report pair {pair:?} maps to no known table; skipped (a live-feed-only table?)"
            );
            continue;
        };
        for blocks in versions.values() {
            for (m, b) in blocks {
                samples_by_table
                    .entry(table.clone())
                    .or_default()
                    .push((*m, b.clone()));
            }
        }
    }

    let mut tables: BTreeSet<String> = ctls.keys().cloned().collect();
    tables.extend(samples_by_table.keys().cloned());
    let out_dir = schemas.join("nem").join("mms");
    std::fs::create_dir_all(&out_dir).map_err(|e| {
        Box::new(
            nemlake_core::Diagnostic::error(
                "schema.out_dir",
                nemlake_core::Category::Schema,
                format!(
                    "The output directory {} could not be created.",
                    out_dir.display()
                ),
            )
            .caused_by(&e),
        )
    })?;
    let mut changed = 0;
    let mut written: Vec<TableDef> = Vec::new();
    for name in tables {
        if !only.is_empty() && !only.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let path = out_dir.join(format!("{name}.toml"));
        let existing = if path.is_file() {
            Some(read_table(&path).map_err(|e| Box::new(e.diagnostic()))?)
        } else {
            None
        };
        let ctl = ctls.get(&name);
        let samples = samples_by_table
            .get(&name)
            .map(|v| v.iter().map(|(m, b)| (*m, b)).collect())
            .unwrap_or_default();
        // Every archive stem that loads this table, split digits removed.
        let mut archive_files: Vec<String> = by_infile
            .iter()
            .filter(|(_, t)| **t == name)
            .map(|(s, _)| s.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned())
            .collect();
        archive_files.sort();
        archive_files.dedup();
        if archive_files.is_empty() {
            archive_files.push(name.clone());
        }
        let inputs = Inputs {
            name: name.clone(),
            ctls: ctl
                .map(|list| list.iter().map(|(_, c, _)| c).collect())
                .unwrap_or_default(),
            dm: dm.table(&name),
            samples,
            existing: existing.as_ref(),
            snapshot: snapshots.contains(&name),
            archive_files,
            ctl_path: ctl.and_then(|list| list.first()).map(|(_, _, p)| p.clone()),
        };
        let table = match merge(&inputs) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skip: {}", e.diagnostic().render());
                continue;
            }
        };
        let text = write_table(&table);
        let current = std::fs::read_to_string(&path).ok();
        if current.as_deref() != Some(text.as_str()) {
            changed += 1;
            if check {
                eprintln!("would write {}", path.display());
            } else {
                std::fs::write(&path, text).map_err(|e| {
                    Box::new(
                        nemlake_core::Diagnostic::error(
                            "schema.write",
                            nemlake_core::Category::Schema,
                            format!("The schema file {} could not be written.", path.display()),
                        )
                        .caused_by(&e),
                    )
                })?;
                eprintln!("wrote {}", path.display());
            }
        }
        written.push(table);
    }
    // Validate what is on disk now so a bad regen is caught here, not at build time.
    let set = SchemaSet::read_dir(schemas).map_err(|e| Box::new(e.diagnostic()))?;
    let issues = set.validate();
    for i in &issues {
        eprintln!("invalid: {i}");
    }
    if !issues.is_empty() {
        return Err(Box::new(nemlake_core::Diagnostic::error(
            "schema.file_invalid",
            nemlake_core::Category::Schema,
            format!(
                "{} schema file issue(s) after regen; see above.",
                issues.len()
            ),
        )));
    }
    eprintln!(
        "{} table(s) processed, {changed} changed, {} on disk",
        written.len(),
        set.tables.len()
    );
    Ok(changed)
}
