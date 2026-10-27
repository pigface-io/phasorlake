//! `inspect`: what is in a file and how the registry sees it.

use std::path::Path;

use anyhow::Result;
use phasor_core::Diagnose as _;
use phasor_mms_conform::inspect::{self, BlockSummary, InspectError};

use crate::context::Context;
use crate::render;

/// Every block of every CSV in `path`, each with the entry it came from;
/// what could not be read is reported on stderr.
fn scan_blocks(ctx: &Context, path: &Path) -> Result<Vec<(String, BlockSummary)>> {
    let found = inspect::file(&ctx.registry, path, &ctx.home.tmp()).map_err(|e| match e {
        InspectError::Open(e) => anyhow::Error::from(e),
        InspectError::Scan(d) => anyhow::anyhow!("{}", d.render()),
        InspectError::Archive(a) => anyhow::anyhow!("{}", a.diagnostic().render()),
    })?;
    for (entry, declared, read) in &found.footers {
        eprintln!("warning: {entry}: footer says {declared} lines but {read} were read");
    }
    for a in &found.failed {
        eprintln!("{}", a.diagnostic().render());
    }
    let skipped = &found.skipped;
    if !skipped.is_empty() && !ctx.json {
        eprintln!(
            "skipped {} non-CSV entr{}: {}",
            skipped.len(),
            if skipped.len() == 1 { "y" } else { "ies" },
            skipped.join(", ")
        );
    }
    Ok(found.blocks)
}

/// `phasor inspect <file>`
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
