//! `schema`: the registry from the command line.

use std::path::Path;

use anyhow::Result;
use phasor_core::Diagnose;
use phasor_registry::Registry;

use crate::cli::SchemaCmd;
use crate::context::Context;
use crate::render;

/// `phasor schema ...`
pub async fn schema(ctx: &Context, cmd: &SchemaCmd) -> Result<i32> {
    let r = &ctx.registry;
    match cmd {
        SchemaCmd::List => {
            let mut rows: Vec<Vec<String>> = r
                .tables()
                .map(|(_, t)| {
                    vec![
                        t.name.clone(),
                        format!("{:?}", t.load_mode).to_ascii_lowercase(),
                        t.schemas.len().to_string(),
                        t.runs.len().to_string(),
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
                    render::json_line(&serde_json::json!({
                        "name": t.name,
                        "schemas": t.schemas.iter().map(|s| s.id).collect::<Vec<_>>(),
                        "runs": t.runs.len(),
                        "columns": t.columns.len(),
                    }));
                }
            } else {
                print!(
                    "{}",
                    render::table(
                        &["table", "mode", "schemas", "runs", "cols", "report pairs"],
                        &rows
                    )
                );
            }
            Ok(0)
        }
        SchemaCmd::Show { table } => {
            let Some(id) = r.table_by_name(table) else {
                anyhow::bail!("The registry has no table {table}; `phasor schema list` shows them.")
            };
            let t = r.table(id);
            if ctx.json {
                render::json_line(t);
            } else {
                print!("{}", phasor_registry::file_model::file::write_table(t));
                println!("\n# Arrow lake schema\n{}", r.lake_schema(id));
            }
            Ok(0)
        }
        SchemaCmd::History { table } => {
            let Some(h) = r.table_by_name(table).and_then(|id| r.history(id)) else {
                anyhow::bail!("The registry has no table {table}.")
            };
            if ctx.json {
                render::json_line(h);
                return Ok(0);
            }
            let t = r.table(r.table_by_name(table).expect("found above"));
            let mut prev: Option<Vec<&str>> = None;
            let schemas: Vec<Vec<String>> = t
                .schemas
                .iter()
                .map(|s| {
                    let cols = t.schema_columns(s);
                    let changes = prev
                        .as_deref()
                        .map_or_else(String::new, |p| changes(p, &cols));
                    let row = vec![s.id.to_string(), cols.len().to_string(), changes];
                    prev = Some(cols);
                    row
                })
                .collect();
            let runs: Vec<Vec<String>> = t
                .runs
                .iter()
                .map(|run| {
                    vec![
                        run.schema.to_string(),
                        format!("{}.{}", run.report.0, run.report.1),
                        run.version.to_string(),
                        run.from.to_string(),
                        run.to.to_string(),
                        run.kinds.join(","),
                        run.files.to_string(),
                        run.rows.to_string(),
                    ]
                })
                .collect();
            print!(
                "{}\n{}",
                render::table(&["schema", "cols", "changes"], &schemas),
                render::table(
                    &[
                        "schema", "report", "version", "from", "to", "kinds", "files", "rows"
                    ],
                    &runs
                )
            );
            Ok(0)
        }
        SchemaCmd::Plan {
            report_type,
            report_subtype,
            version,
            columns,
            month,
        } => plan(r, report_type, report_subtype, *version, columns, *month),
        SchemaCmd::Validate { dir } => match dir {
            None => {
                println!(
                    "embedded registry: {} tables, valid (validated at build time)",
                    r.tables().count()
                );
                Ok(0)
            }
            Some(d) => match Registry::load_dir(Path::new(d)) {
                Ok(reg) => {
                    println!("{}: {} tables, valid", d.display(), reg.tables().count());
                    Ok(0)
                }
                Err(e) => {
                    eprintln!("{}", e.diagnostic().render());
                    Ok(5)
                }
            },
        },
    }
}

/// The columns `next` adds to and drops from `prev`, and whether the
/// columns both have moved.
fn changes(prev: &[&str], next: &[&str]) -> String {
    let mut out: Vec<String> = next
        .iter()
        .filter(|c| !prev.contains(c))
        .map(|c| format!("+{c}"))
        .collect();
    out.extend(
        prev.iter()
            .filter(|c| !next.contains(c))
            .map(|c| format!("-{c}")),
    );
    let shared = |a: &[&str], b: &[&str]| -> Vec<String> {
        a.iter()
            .filter(|c| b.contains(c))
            .map(|c| (*c).to_owned())
            .collect()
    };
    if shared(prev, next) != shared(next, prev) {
        out.push("reordered".into());
    }
    out.join(" ")
}

/// `phasor schema plan <type> <subtype> <version> --columns ...`: the plan
/// a shape resolves to, permissively, as one JSON document.
fn plan(
    r: &Registry,
    report_type: &str,
    report_subtype: &str,
    version: u16,
    columns: &[String],
    month: Option<phasor_core::YearMonth>,
) -> Result<i32> {
    let Some(resolved) = r.resolve_names(report_type, report_subtype, version, columns, month)
    else {
        anyhow::bail!("A report type and subtype are names: not empty, no control characters.")
    };
    match resolved {
        Ok(res) => {
            render::json_line(&serde_json::json!({
                "resolution": res.label(),
                "version": res.version,
                "source": res.source.label(),
                "plan_hash": res.plan_hash().to_string(),
                "plan": &*res.plan,
            }));
            Ok(0)
        }
        Err(e) => {
            eprintln!("{}", e.diagnostic().render());
            Ok(5)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::changes;

    #[test]
    fn changes_between_schemas() {
        assert_eq!(changes(&["A", "B"], &["A", "B"]), "");
        assert_eq!(changes(&["A", "B"], &["A", "B", "C"]), "+C");
        assert_eq!(changes(&["A", "B"], &["B", "A", "C"]), "+C reordered");
        assert_eq!(changes(&["A", "B"], &["A"]), "-B");
    }
}
