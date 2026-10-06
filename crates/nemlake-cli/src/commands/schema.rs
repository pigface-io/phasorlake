//! `schema`: the registry from the command line.

use std::path::Path;

use anyhow::Result;
use nemlake_core::Diagnose;
use nemlake_registry::Registry;

use crate::cli::SchemaCmd;
use crate::context::Context;
use crate::render;

/// `nemlake schema ...`
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
                        t.increments
                            .iter()
                            .map(|i| i.n.to_string())
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
                    render::json_line(&serde_json::json!({
                        "name": t.name,
                        "increments": t.increments.iter().map(|i| i.n).collect::<Vec<_>>(),
                        "columns": t.columns.len(),
                    }));
                }
            } else {
                print!(
                    "{}",
                    render::table(
                        &["table", "mode", "increments", "cols", "report pairs"],
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
        SchemaCmd::Increments { table } => {
            let Some(incs) = r.increments(table) else {
                anyhow::bail!("The registry has no table {table}.")
            };
            let mut prev: Option<&[String]> = None;
            let rows: Vec<Vec<String>> = incs
                .iter()
                .map(|i| {
                    let changes = prev.map_or_else(String::new, |p| changes(p, &i.columns));
                    prev = Some(&i.columns);
                    vec![
                        i.n.to_string(),
                        i.from.to_string(),
                        i.aemo_ctl.clone().unwrap_or_default(),
                        i.aemo_versions
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                        i.columns.len().to_string(),
                        changes,
                    ]
                })
                .collect();
            if ctx.json {
                render::json_line(&incs);
            } else {
                print!(
                    "{}",
                    render::table(
                        &[
                            "n",
                            "from",
                            "aemo control file",
                            "versions",
                            "cols",
                            "changes"
                        ],
                        &rows
                    )
                );
            }
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

/// The columns `next` adds to and drops from `prev`, and whether the
/// columns both have moved.
fn changes(prev: &[String], next: &[String]) -> String {
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
    let shared = |a: &[String], b: &[String]| -> Vec<String> {
        a.iter().filter(|c| b.contains(c)).cloned().collect()
    };
    if shared(prev, next) != shared(next, prev) {
        out.push("reordered".into());
    }
    out.join(" ")
}

/// `nemlake schema plan <type> <subtype> <version> --columns ...`: the plan
/// a shape resolves to, permissively, as one JSON document.
fn plan(
    r: &Registry,
    report_type: &str,
    report_subtype: &str,
    version: u16,
    columns: &[String],
    month: Option<nemlake_core::YearMonth>,
) -> Result<i32> {
    let norm = |s: &str| nemlake_core::normalize_name(s, &mut String::new()).map(str::to_owned);
    let (Some(rt), Some(st)) = (norm(report_type), norm(report_subtype)) else {
        anyhow::bail!("A report type and subtype are names: not empty, no control characters.")
    };
    let names: Vec<String> = columns.iter().filter_map(|c| norm(c)).collect();
    let cols: Vec<&str> = names.iter().map(String::as_str).collect();
    let irow = nemlake_core::IRow {
        report_type: &rt,
        report_subtype: &st,
        version,
        columns: &cols,
    };
    match r.resolve_in(&irow, nemlake_registry::ResolveOptions::default(), month) {
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
    fn changes_between_increments() {
        let v = |s: &[&str]| s.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>();
        assert_eq!(changes(&v(&["A", "B"]), &v(&["A", "B"])), "");
        assert_eq!(changes(&v(&["A", "B"]), &v(&["A", "B", "C"])), "+C");
        assert_eq!(
            changes(&v(&["A", "B"]), &v(&["B", "A", "C"])),
            "+C reordered"
        );
        assert_eq!(changes(&v(&["A", "B"]), &v(&["A"])), "-B");
    }
}
