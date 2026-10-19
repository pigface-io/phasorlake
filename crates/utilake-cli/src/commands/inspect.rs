//! `inspect`: what is in a file and how the registry sees it.

use std::path::Path;

use anyhow::Result;
use utilake_core::Diagnose;
use utilake_kit::container::walk_csv_entries;
use utilake_mms_conform::inspect::{BlockSummary, InspectSink};
use utilake_mms_decode::scan::{ScanOptions, scan};
use utilake_mms_registry::{Mode, ResolveOptions};

use crate::context::Context;
use crate::render;

/// Every block of every CSV in `path` (a CSV, or a zip walked through
/// nested zips), each with the entry it came from, resolved strictly so a
/// status other than `conforms` names what the registry lacks. Entries that
/// are not CSVs are reported on stderr.
fn scan_blocks(ctx: &Context, path: &Path) -> Result<Vec<(String, BlockSummary)>> {
    let opts = ResolveOptions { mode: Mode::Strict };
    let mut blocks: Vec<(String, BlockSummary)> = Vec::new();
    let mut handle = |entry: &str, reader: &mut dyn std::io::Read| -> Result<()> {
        let mut sink = InspectSink::new(&ctx.registry, opts);
        let stats = scan(
            reader,
            &mut sink,
            &ScanOptions {
                max_bad_rows: u64::MAX,
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        blocks.extend(sink.blocks.into_iter().map(|b| (entry.to_owned(), b)));
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
            skipped.push(o.name);
        })
        .map_err(|e| match e {
            utilake_kit::container::WalkError::Archive(a) => {
                anyhow::anyhow!("{}", a.diagnostic().render())
            }
            utilake_kit::container::WalkError::Callback(c) => c,
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
    Ok(blocks)
}

/// `utilake inspect <file>`
pub fn inspect(ctx: &Context, path: &Path) -> Result<i32> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut json_blocks: Vec<serde_json::Value> = Vec::new();
    for (entry, b) in &scan_blocks(ctx, path)? {
        rows.push(vec![
            entry.clone(),
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
    }
    if ctx.json {
        render::json_line(
            &serde_json::json!({ "file": path.display().to_string(), "blocks": json_blocks }),
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
    }
    let unresolved = rows.iter().filter(|r| r[7].starts_with("schema.")).count();
    Ok(if unresolved > 0 { 5 } else { 0 })
}
