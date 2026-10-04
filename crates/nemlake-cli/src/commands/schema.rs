//! `schema`: the registry from the command line.

use std::path::Path;

use anyhow::Result;
use nemlake_core::Diagnose;
use nemlake_registry::{Registry, SchemaVersion, TypeOptions};

use crate::cli::SchemaCmd;
use crate::context::Context;
use crate::render;

/// `nemlake schema ...`
pub fn schema(ctx: &Context, cmd: &SchemaCmd) -> Result<i32> {
    let r = &ctx.registry;
    match cmd {
        SchemaCmd::List => {
            let mut rows: Vec<Vec<String>> = r
                .tables()
                .map(|(_, t)| {
                    vec![
                        t.name.clone(),
                        t.package.clone(),
                        format!("{:?}", t.load_mode).to_ascii_lowercase(),
                        t.versions
                            .iter()
                            .map(|v| v.n.to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                        t.columns.len().to_string(),
                        t.report_pairs
                            .iter()
                            .map(|(a, b)| format!("{a}.{b}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                    ]
                })
                .collect();
            rows.sort();
            if ctx.json {
                for (_, t) in r.tables() {
                    render::json_line(
                        &serde_json::json!({ "name": t.name, "package": t.package, "versions": t.versions.iter().map(|v| v.n.0).collect::<Vec<_>>(), "columns": t.columns.len() }),
                    );
                }
            } else {
                print!(
                    "{}",
                    render::table(
                        &[
                            "table",
                            "package",
                            "mode",
                            "versions",
                            "cols",
                            "report pairs"
                        ],
                        &rows
                    )
                );
            }
            Ok(0)
        }
        SchemaCmd::Show { table } => {
            let Some(id) = r.table_by_name(table) else {
                anyhow::bail!(
                    "The registry has no table {table}; `nemlake schema list` shows them."
                )
            };
            let t = r.table(id);
            if ctx.json {
                render::json_line(t);
            } else {
                print!("{}", nemlake_registry::file_model::file::write_table(t));
                println!("\n# Arrow lake schema\n{}", r.lake_schema(id));
            }
            Ok(0)
        }
        SchemaCmd::Diff { table, from, to } => {
            let Some(changes) = r.diff(table, SchemaVersion(*from), SchemaVersion(*to)) else {
                anyhow::bail!(
                    "Table {table} has no versions {from} and {to} both declared; `nemlake schema lineage {table}` lists them."
                )
            };
            if ctx.json {
                render::json_line(&changes.iter().map(ToString::to_string).collect::<Vec<_>>());
            } else if changes.is_empty() {
                println!("no changes between version {from} and {to}");
            } else {
                for c in &changes {
                    println!("{c}");
                }
            }
            Ok(0)
        }
        SchemaCmd::Lineage { table } => {
            let Some(lin) = r.lineage(table) else {
                anyhow::bail!("The registry has no table {table}.")
            };
            let rows: Vec<Vec<String>> = lin
                .iter()
                .map(|e| {
                    vec![
                        e.version.to_string(),
                        e.first_seen.clone().unwrap_or_default(),
                        e.last_seen.clone().unwrap_or_default(),
                        e.variants.to_string(),
                        e.changes
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; "),
                    ]
                })
                .collect();
            if ctx.json {
                render::json_line(&rows);
            } else {
                print!(
                    "{}",
                    render::table(
                        &["version", "first seen", "last seen", "variants", "changes"],
                        &rows
                    )
                );
            }
            Ok(0)
        }
        SchemaCmd::Infer {
            path,
            write,
            sample_rows,
            ctl,
        } => infer(ctx, path, *write, *sample_rows, ctl),
        SchemaCmd::Validate { dir } => match dir {
            None => {
                println!(
                    "embedded registry: {} tables, valid (validated at build time)",
                    r.tables().count()
                );
                Ok(0)
            }
            Some(d) => match Registry::load_dir(Path::new(d), TypeOptions::default()) {
                Ok(reg) => {
                    let breaks = r.check_compat(&reg);
                    for (t, m) in &breaks {
                        println!("breaking: {t}: {m}");
                    }
                    println!(
                        "{}: {} tables, valid{}",
                        d.display(),
                        reg.tables().count(),
                        if breaks.is_empty() {
                            ""
                        } else {
                            ", with breaking changes against the embedded registry"
                        }
                    );
                    Ok(if breaks.is_empty() { 0 } else { 5 })
                }
                Err(e) => {
                    eprintln!("{}", e.diagnostic().render());
                    Ok(5)
                }
            },
        },
    }
}

/// `nemlake schema infer <file>`
fn infer(
    ctx: &Context,
    path: &Path,
    write: bool,
    sample_rows: u64,
    ctl_paths: &[std::path::PathBuf],
) -> Result<i32> {
    use std::collections::BTreeMap;

    use nemlake_registry::file_model::ctl::CtlFile;
    use nemlake_registry::infer::{apply_ctl, draft_table, extend_table};

    let r = &ctx.registry;
    // The control files on offer, each with the name it was given under.
    let mut ctls: Vec<(String, CtlFile)> = Vec::new();
    for p in ctl_paths {
        let text = std::fs::read_to_string(p).map_err(|e| {
            anyhow::anyhow!("The control file {} could not be read: {e}", p.display())
        })?;
        let parsed = nemlake_registry::file_model::ctl::parse(&p.display().to_string(), &text)
            .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        let name = p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        ctls.push((name, parsed));
    }
    // The control file for a block: one whose every column the block
    // carries, the widest if several do.
    let ctl_for = |irow: &nemlake_core::IRow<'_>| {
        ctls.iter()
            .filter(|(_, c)| {
                c.data_columns().next().is_some()
                    && c.data_columns()
                        .all(|k| irow.columns.iter().any(|n| n.eq_ignore_ascii_case(&k.name)))
            })
            .max_by_key(|(_, c)| c.data_columns().count())
    };
    // What the rows overruled, by table.
    let mut overruled: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Tables this file changes, by name, and the draft that claims each
    // report pair nobody declared.
    let mut changed: BTreeMap<String, nemlake_registry::TableDef> = BTreeMap::new();
    let mut drafted: BTreeMap<(String, String), String> = BTreeMap::new();
    let blocks = super::inspect::scan_blocks(ctx, path, sample_rows)?;
    for (_, b) in &blocks {
        if b.status == "exact" {
            continue;
        }
        let cols: Vec<&str> = b.columns.iter().map(String::as_str).collect();
        let irow = super::inspect::irow_of(b, &cols);
        let pair = (b.report_type.clone(), b.report_subtype.clone());
        let known = b.table.clone().or_else(|| drafted.get(&pair).cloned());
        match known {
            None => {
                let mut draft = draft_table(nemlake_registry::Market::Nem, &irow, &b.sampler);
                if let Some((file, ctl)) = ctl_for(&irow) {
                    let notes = apply_ctl(&mut draft, &irow, &b.sampler, ctl);
                    // The control file knows the table's own name.
                    let name = ctl.table.to_ascii_uppercase();
                    if r.table_by_name(&name).is_none() && !changed.contains_key(&name) {
                        draft.name = name;
                    }
                    draft.provenance.ctl = Some(file.clone());
                    draft.provenance.note = Some(
                        "draft; columns typed from the control file where the sampled rows agree, inferred otherwise".into(),
                    );
                    overruled
                        .entry(draft.name.clone())
                        .or_default()
                        .extend(notes);
                }
                drafted.insert(pair, draft.name.clone());
                changed.insert(draft.name.clone(), draft);
            }
            Some(name) => {
                let base = match changed.get(&name) {
                    Some(t) => t.clone(),
                    None => match r.table_by_name(&name) {
                        Some(id) => r.table(id).clone(),
                        None => continue,
                    },
                };
                if let Some(mut t) = extend_table(&base, &irow, &b.sampler) {
                    if let Some((_, ctl)) = ctl_for(&irow) {
                        let notes = apply_ctl(&mut t, &irow, &b.sampler, ctl);
                        overruled.entry(name.clone()).or_default().extend(notes);
                    }
                    changed.insert(name, t);
                }
            }
        }
    }
    if changed.is_empty() {
        if ctx.json {
            render::json_line(
                &serde_json::json!({ "file": path.display().to_string(), "drafts": [] }),
            );
        } else {
            println!(
                "every one of the {} block(s) is already declared; nothing to infer",
                blocks.len()
            );
        }
        return Ok(0);
    }
    let mut out = Vec::new();
    for (name, t) in &changed {
        let toml = nemlake_registry::file_model::file::write_table(t);
        let issues = nemlake_registry::file_model::validate::validate_structure(t);
        let target = ctx
            .home
            .schemas()
            .join(t.market.as_str())
            .join("mms")
            .join(format!("DRAFT_{name}.toml"));
        if write {
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&target, &toml)?;
        }
        let notes = overruled.get(name).cloned().unwrap_or_default();
        out.push((name, toml, issues, target, notes));
    }
    if ctx.json {
        let drafts: Vec<serde_json::Value> = out
            .iter()
            .map(|(name, toml, issues, target, notes)| {
                serde_json::json!({
                    "table": name,
                    "toml": toml,
                    "path": if write { Some(target.display().to_string()) } else { None },
                    "issues": issues.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "notes": notes,
                })
            })
            .collect();
        render::json_line(
            &serde_json::json!({ "file": path.display().to_string(), "drafts": drafts }),
        );
    } else {
        for (name, toml, issues, target, notes) in &out {
            if write {
                println!("wrote {}", target.display());
            } else {
                println!("# draft for {name}\n{toml}");
            }
            for i in issues {
                println!("# to fix by hand: {i}");
            }
            for n in notes {
                println!("# note: {n}");
            }
        }
        if write {
            println!(
                "{} draft(s). Review each one: the types of new columns are guesses from the rows sampled. Rename DRAFT_X.toml to X.toml to put it in effect.",
                out.len()
            );
        }
    }
    Ok(0)
}
