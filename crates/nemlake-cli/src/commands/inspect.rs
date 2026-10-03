//! `inspect`: what is in a file and how the registry sees it.

use std::path::Path;

use anyhow::Result;
use nemlake_core::{Diagnose, IRow};
use nemlake_mms::archive::walk_csv_entries;
use nemlake_mms::inspect::InspectSink;
use nemlake_mms::scan::{ScanOptions, scan};
use nemlake_schema::{Mode, ResolveOptions};

use crate::context::Context;
use crate::render;

/// `nemlake inspect <file>`
pub fn inspect(ctx: &Context, path: &Path, infer: bool, sample_rows: u64) -> Result<i32> {
    let opts = ResolveOptions { mode: Mode::Strict };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut drafts: Vec<serde_json::Value> = Vec::new();
    let mut json_blocks: Vec<serde_json::Value> = Vec::new();
    let mut handle = |entry: &str, reader: &mut dyn std::io::Read| -> Result<()> {
        let mut sink = InspectSink::new(&ctx.registry, opts, sample_rows);
        let stats = scan(
            reader,
            &mut sink,
            &ScanOptions {
                max_bad_rows: u64::MAX,
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        for b in &sink.blocks {
            rows.push(vec![
                entry.to_owned(),
                b.line.to_string(),
                format!("{}.{}", b.report_type, b.report_subtype),
                b.version.to_string(),
                b.columns.len().to_string(),
                b.rows.to_string(),
                b.table.clone().unwrap_or_default(),
                b.status.clone(),
                b.fingerprint.to_string(),
            ]);
            json_blocks.push(serde_json::json!({
                "entry": entry, "line": b.line, "report_type": b.report_type, "report_subtype": b.report_subtype,
                "version": b.version, "columns": b.columns, "rows": b.rows, "table": b.table, "status": b.status,
                "fingerprint": b.fingerprint.to_string(),
            }));
            if infer && b.table.is_none() {
                let cols: Vec<&str> = b.columns.iter().map(String::as_str).collect();
                let irow = IRow {
                    report_type: &b.report_type,
                    report_subtype: &b.report_subtype,
                    version: b.version,
                    columns: &cols,
                };
                let draft = nemlake_schema::infer::draft_table(
                    nemlake_schema::Market::Nem,
                    &irow,
                    &b.sampler,
                );
                let toml = nemlake_schema_model_write(&draft);
                drafts.push(serde_json::json!({ "table": draft.name, "toml": toml }));
            }
        }
        if stats.footer_mismatch() {
            eprintln!(
                "warning: {entry}: footer says {} lines but {} were read",
                stats.footer_declared.unwrap_or(0),
                stats.lines
            );
        }
        Ok(())
    };
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
    {
        let mut f = std::fs::File::open(path)?;
        handle(
            &path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            &mut f,
        )?;
    } else {
        let tmp = ctx.home.tmp();
        let mut skipped = Vec::new();
        walk_csv_entries(path, &tmp, &mut |e| handle(&e.name, e.reader), &mut |o| {
            skipped.push(o.to_owned());
        })
        .map_err(|e| match e {
            nemlake_mms::archive::WalkError::Archive(a) => {
                anyhow::anyhow!("{}", a.diagnostic().render())
            }
            nemlake_mms::archive::WalkError::Callback(c) => c,
        })?;
        if !skipped.is_empty() && !ctx.json {
            eprintln!(
                "skipped {} non-CSV entr{}: {}",
                skipped.len(),
                if skipped.len() == 1 { "y" } else { "ies" },
                skipped.join(", ")
            );
        }
    }
    if ctx.json {
        render::json_line(
            &serde_json::json!({ "file": path.display().to_string(), "blocks": json_blocks, "drafts": drafts }),
        );
    } else {
        print!(
            "{}",
            render::table(
                &[
                    "entry",
                    "line",
                    "report",
                    "ver",
                    "cols",
                    "rows",
                    "table",
                    "status",
                    "fingerprint"
                ],
                &rows
            )
        );
        for d in &drafts {
            println!(
                "\n# draft for {}\n{}",
                d["table"].as_str().unwrap_or(""),
                d["toml"].as_str().unwrap_or("")
            );
        }
    }
    let unresolved = rows.iter().filter(|r| r[7].starts_with("schema.")).count();
    Ok(if unresolved > 0 { 5 } else { 0 })
}

fn nemlake_schema_model_write(t: &nemlake_schema::TableDef) -> String {
    nemlake_schema::file::write_table(t)
}
