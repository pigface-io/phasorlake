//! `schema`: the registry from the command line.

use std::path::Path;

use anyhow::Result;
use nemlake_core::Diagnose;
use nemlake_schema::{Registry, SchemaVersion, TypeOptions};

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
                print!("{}", nemlake_schema::file::write_table(t));
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
                    render::table(&["version", "first seen", "last seen", "changes"], &rows)
                );
            }
            Ok(0)
        }
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
