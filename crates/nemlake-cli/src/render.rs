//! Tables, JSON and progress.

use std::fmt::Write as _;
use std::io::Write;

use nemlake_core::Diagnostic;
use nemlake_core::event::{Event, Summary};
use nemlake_core::report::Report;

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

/// Renders the end-of-run summary for humans.
pub fn summary_text(s: &Summary, report: &Report) -> String {
    let mut out = String::new();
    let failed: Vec<_> = report.failed().collect();
    if !failed.is_empty() {
        let rows: Vec<Vec<String>> = failed
            .iter()
            .map(|f| {
                vec![
                    f.source.clone(),
                    f.failed_with.as_deref().unwrap_or("").to_owned(),
                ]
            })
            .collect();
        out.push_str("Failed:\n");
        out.push_str(&table(&["source", "code"], &rows));
        out.push('\n');
    }
    for l in report.summary_lines() {
        out.push_str(&l);
        out.push('\n');
    }
    if !report.by_code.is_empty() {
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "{} listed, {} ingested, {} skipped, {} failed; {} rows ({} quarantined); {:.1} MB in, {:.1} MB out; {:.1}s, {:.0} MB/s",
        s.listed,
        s.ingested,
        s.skipped,
        s.failed,
        s.rows,
        s.bad_rows,
        s.bytes_in as f64 / 1e6,
        s.bytes_out as f64 / 1e6,
        s.elapsed_ms as f64 / 1000.0,
        s.mb_per_s()
    );
    out
}

/// Renders one diagnostic for humans.
pub fn diagnostic_text(d: &Diagnostic) -> String {
    format!("{} {}", d.severity, d.render())
}

/// Consumes events: JSON lines in `--json` mode, progress bars otherwise.
pub async fn consume_events(
    mut rx: tokio::sync::mpsc::Receiver<Event>,
    json: bool,
    progress: bool,
    verbose: bool,
) {
    let bars = progress.then(indicatif::MultiProgress::new);
    let spinner = bars.as_ref().map(|m| {
        let pb = m.add(indicatif::ProgressBar::new_spinner());
        pb.set_style(
            indicatif::ProgressStyle::with_template("{spinner} {msg}").expect("valid template"),
        );
        pb.enable_steady_tick(std::time::Duration::from_millis(120));
        pb
    });
    let mut done = 0usize;
    let mut total = 0usize;
    while let Some(ev) = rx.recv().await {
        if json {
            json_line(&ev);
            continue;
        }
        match &ev {
            Event::Listed { files } => {
                total = *files;
                if let Some(s) = &spinner {
                    s.set_message(format!("{files} files listed"));
                } else if verbose {
                    eprintln!("{files} files listed");
                }
            }
            Event::DownloadStarted { url, size } if verbose => {
                eprintln!(
                    "downloading {url} ({})",
                    size.map_or("unknown size".to_owned(), |s| format!(
                        "{:.1} MB",
                        s as f64 / 1e6
                    ))
                );
            }
            Event::SourceDone(o) => {
                done += 1;
                if let Some(s) = &spinner {
                    s.set_message(format!(
                        "{done}/{total} {}: {:?} {} rows",
                        short(&o.source),
                        o.status,
                        o.rows
                    ));
                } else if verbose {
                    eprintln!(
                        "{done}/{total} {} {:?} rows={} bad={}",
                        o.source, o.status, o.rows, o.bad_rows
                    );
                }
            }
            Event::Diagnostic(d) if d.is_error() || verbose => {
                let text = diagnostic_text(d);
                match &bars {
                    Some(m) => {
                        let _ = m.println(text);
                    }
                    None => eprintln!("{text}"),
                }
            }
            _ => {}
        }
    }
    if let Some(s) = spinner {
        s.finish_and_clear();
    }
}

fn short(s: &str) -> String {
    s.rsplit('/').next().unwrap_or(s).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns() {
        let t = table(
            &["a", "bbb"],
            &[
                vec!["x".into(), "y".into()],
                vec!["long".into(), "z".into()],
            ],
        );
        assert_eq!(t, "a     bbb\n----  ---\nx     y\nlong  z\n");
        assert_eq!(short("https://h/a/b.zip"), "b.zip");
        let s = Summary {
            listed: 1,
            ingested: 1,
            rows: 20,
            bytes_in: 1000,
            elapsed_ms: 1000,
            ..Default::default()
        };
        let text = summary_text(&s, &Report::new());
        assert!(text.contains("1 listed, 1 ingested"));
        let d = Diagnostic::error("net.offline", nemlake_core::Category::Net, "Offline.");
        assert!(diagnostic_text(&d).starts_with("error Offline."));
    }
}
