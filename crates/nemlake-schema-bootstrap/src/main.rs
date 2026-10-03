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
        /// Directory of `PUBLIC_DVD_*.ctl` files.
        #[arg(long)]
        ctl: PathBuf,
        /// The scraped data-model JSON.
        #[arg(long)]
        datamodel: PathBuf,
        /// Directories of `.CSV`/`.zip` files to sample I rows from; repeatable.
        #[arg(long, required = true)]
        samples: Vec<PathBuf>,
        /// Curated list of snapshot tables.
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
    /// Fetch the head of every archive zip of a month as `{stem}.CSV` fixtures (network).
    Heads {
        /// Archive month.
        #[arg(long)]
        month: nemlake_core::YearMonth,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        /// D rows to keep per block.
        #[arg(long, default_value_t = 20)]
        rows: usize,
        /// Concurrent requests.
        #[arg(long, default_value_t = 6)]
        parallel: usize,
        /// Archive base URL.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
    /// Fetch every CTL of a month (network).
    Ctls {
        /// Archive month.
        #[arg(long)]
        month: nemlake_core::YearMonth,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        /// Archive base URL.
        #[arg(long, default_value = "https://www.nemweb.com.au")]
        base: String,
    },
}

fn tmp_cache() -> PathBuf {
    std::env::temp_dir().join(format!("nemlake-bootstrap-{}", std::process::id()))
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
    snapshot: Vec<String>,
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
            snapshot_list,
            only,
            check,
        } => {
            match regen(
                &schemas,
                &ctl,
                &datamodel,
                &samples,
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
            rows,
            parallel,
            base,
        } => {
            let archive = match nemlake_fetch::MmsdmArchive::new(&base) {
                Ok(a) => a,
                Err(e) => return fail(&e.diagnostic()),
            };
            let transport = match nemlake_schema_bootstrap::remote::nemweb_transport() {
                Ok(t) => t,
                Err(e) => return fail(&e.diagnostic()),
            };
            let cache = tmp_cache();
            let result = run_async(nemlake_schema_bootstrap::remote::fetch_heads(
                transport, &archive, &cache, &out, month, rows, parallel,
            ));
            let _ = std::fs::remove_dir_all(&cache);
            match result {
                Ok(outcomes) => {
                    let mut failed = 0;
                    for o in &outcomes {
                        match (&o.path, &o.error) {
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
        Cmd::Ctls { month, out, base } => {
            let archive = match nemlake_fetch::MmsdmArchive::new(&base) {
                Ok(a) => a,
                Err(e) => return fail(&e.diagnostic()),
            };
            let transport = match nemlake_schema_bootstrap::remote::nemweb_transport() {
                Ok(t) => t,
                Err(e) => return fail(&e.diagnostic()),
            };
            let cache = tmp_cache();
            let result = run_async(nemlake_schema_bootstrap::remote::fetch_ctls(
                transport, &archive, &cache, &out, month,
            ));
            let _ = std::fs::remove_dir_all(&cache);
            match result {
                Ok(paths) => {
                    eprintln!("{} CTL file(s) written to {}", paths.len(), out.display());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&e.diagnostic()),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn regen(
    schemas: &Path,
    ctl_dir: &Path,
    datamodel: &Path,
    sample_dirs: &[PathBuf],
    snapshot_list: &Path,
    only: &[String],
    check: bool,
) -> Result<usize, Box<nemlake_core::Diagnostic>> {
    let dm = DataModel::load(datamodel).map_err(|e| Box::new(e.diagnostic()))?;
    let snapshots: BTreeSet<String> = std::fs::read_to_string(snapshot_list)
        .ok()
        .and_then(|t| toml::from_str::<SnapshotList>(&t).ok())
        .map(|l| l.snapshot.into_iter().collect())
        .unwrap_or_default();

    // CTLs by the table they load, plus the archive stem each one reads.
    let mut ctls: BTreeMap<String, (CtlFile, String)> = BTreeMap::new();
    let mut by_infile: BTreeMap<String, String> = BTreeMap::new();
    let mut ctl_paths: Vec<_> = std::fs::read_dir(ctl_dir)
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
    ctl_paths.sort();
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
        ctls.entry(name).or_insert((f, rel.display().to_string()));
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
            ctl: ctl.map(|(c, _)| c),
            dm: dm.table(&name),
            samples,
            existing: existing.as_ref(),
            snapshot: snapshots.contains(&name),
            archive_files,
            ctl_path: ctl.map(|(_, p)| p.clone()),
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
