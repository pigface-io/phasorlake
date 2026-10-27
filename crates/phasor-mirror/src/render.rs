//! Tables and JSON lines.

use std::io::Write;

/// Aligns rows of cells into columns.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(c.chars().count());
            }
        }
    }
    let mut out = String::new();
    let line = |cells: &[String], out: &mut String| {
        let mut parts = Vec::with_capacity(cells.len());
        for (i, c) in cells.iter().enumerate() {
            let w = widths.get(i).copied().unwrap_or(0);
            parts.push(format!("{c:<w$}"));
        }
        out.push_str(parts.join("  ").trim_end());
        out.push('\n');
    };
    line(
        &headers.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>(),
        &mut out,
    );
    line(
        &widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>(),
        &mut out,
    );
    for r in rows {
        line(r, &mut out);
    }
    out
}

/// Prints one value as JSON on its own line.
pub fn json_line<T: serde::Serialize>(v: &T) {
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, v);
    let _ = out.write_all(b"\n");
}
