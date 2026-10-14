//! `mirror`: a copy of every file under nemweb's archive folders.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Duration;

use anyhow::Result;
use nemlake_acquire::{FetchError, Fetcher};
use nemlake_core::Diagnose;
use nemlake_mirror::{Mirror, MirrorError, Status, Url};

use crate::cli::{Cli, MirrorCmd};
use crate::context::Context;
use crate::render;

/// `nemlake mirror ...`
pub async fn mirror(cli: &Cli, ctx: &Context, cmd: &MirrorCmd) -> Result<i32> {
    let mirror = Mirror::new(ctx.home.root().join("mirror"));
    let result = match cmd {
        MirrorCmd::Crawl { roots, fresh } => {
            let fetcher = ctx.fetcher(None)?;
            match mirror.lock() {
                Ok(_lock) => crawl(ctx, &fetcher, &mirror, roots, *fresh).await,
                Err(e) => Err(e),
            }
        }
        MirrorCmd::Download => {
            let fetcher = ctx.fetcher(None)?;
            match mirror.lock() {
                Ok(_lock) => download(ctx, &fetcher, &mirror).await.map(|(code, _)| code),
                Err(e) => Err(e),
            }
        }
        MirrorCmd::Run {
            roots,
            fresh,
            refusals,
            wait_minutes,
        } => {
            let wait = Duration::from_secs(wait_minutes * 60);
            match mirror.lock() {
                Ok(_lock) => run(ctx, &mirror, roots, *fresh, *refusals, wait).await?,
                Err(e) => Err(e),
            }
        }
        MirrorCmd::Map { roots } => {
            let jobs = cli.jobs.unwrap_or_else(|| ctx.config.effective_jobs());
            map(ctx, &mirror, roots, jobs)
        }
        MirrorCmd::Compare { delete } => compare(ctx, &mirror, *delete),
        MirrorCmd::Summary { out, check } => summary(ctx, &mirror, out, *check),
        MirrorCmd::Status { roots } => status(ctx, &mirror, roots),
    };
    match result {
        Ok(code) => Ok(code),
        Err(e) => {
            report(ctx, &e.diagnostic());
            Ok(1)
        }
    }
}

fn report(ctx: &Context, d: &nemlake_core::Diagnostic) {
    if ctx.json {
        render::json_line(d);
    } else {
        eprintln!("{}", d.render());
    }
}

fn roots(ctx: &Context, given: &[String]) -> Result<Vec<Url>, MirrorError> {
    if given.is_empty() {
        nemlake_mirror::default_roots(&ctx.config.sources.nem_base_url)
    } else {
        given.iter().map(|r| nemlake_mirror::root(r)).collect()
    }
}

/// Crawls, then downloads; a refusal waits `wait` and starts both again
/// with a new fetcher, at most `refusals` times. The crawl carries on from
/// its kept pages and the download skips archived files, so starting again
/// repeats no request that succeeded.
async fn run(
    ctx: &Context,
    mirror: &Mirror,
    given: &[String],
    fresh: bool,
    refusals: u32,
    wait: Duration,
) -> Result<Result<i32, MirrorError>> {
    let mut fresh = fresh;
    let mut refused = 0;
    loop {
        let fetcher = ctx.fetcher(None)?;
        let started = std::time::Instant::now();
        let attempt = async {
            crawl(ctx, &fetcher, mirror, given, fresh).await?;
            download(ctx, &fetcher, mirror).await
        };
        let d = match attempt.await {
            Ok((code, false)) => return Ok(Ok(code)),
            // Files failed on the network: another pass fetches only those.
            Ok((_, true)) => nemlake_core::Diagnostic::error(
                "mirror.files_failed",
                nemlake_core::Category::Mirror,
                "Some files failed on the network; the next pass fetches only those.",
            ),
            Err(MirrorError::Fetch(e)) if waits_out(&e) => e.diagnostic(),
            Err(other) => return Ok(Err(other)),
        };
        // A run that got somewhere before failing starts the count again.
        if started.elapsed() > wait {
            refused = 0;
        }
        if refused >= refusals {
            report(ctx, &d);
            return Ok(Ok(1));
        }
        refused += 1;
        report(ctx, &d);
        if ctx.json {
            render::json_line(&serde_json::json!({
                "event": "waiting",
                "refusals": refused,
                "seconds": wait.as_secs(),
            }));
        } else {
            eprintln!(
                "nemweb refused a request or the network failed ({refused} of {refusals} in a row); waiting {} minutes",
                wait.as_secs() / 60
            );
        }
        tokio::time::sleep(wait).await;
        // The crawl moved aside already; a second move would lose it.
        fresh = false;
    }
}

/// Failures that pass with time: a refusal by nemweb, and the network.
fn waits_out(e: &FetchError) -> bool {
    matches!(
        e,
        FetchError::Blocked { .. }
            | FetchError::RetriesExhausted { .. }
            | FetchError::Timeout { .. }
            | FetchError::Connect { .. }
            | FetchError::Body { .. }
    )
}

async fn crawl(
    ctx: &Context,
    fetcher: &Fetcher,
    mirror: &Mirror,
    given: &[String],
    fresh: bool,
) -> Result<i32, MirrorError> {
    let roots = roots(ctx, given)?;
    let json = ctx.json;
    let progress = ctx.progress;
    let summary = nemlake_mirror::crawl(fetcher, mirror, &roots, fresh, |p| {
        if json {
            render::json_line(&serde_json::json!({ "event": "page_read", "page": p }));
        } else if progress {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r{} pages read, {} queued   ", p.pages, p.queued);
            let _ = err.flush();
        }
    })
    .await?;
    if json {
        render::json_line(&serde_json::json!({ "event": "crawl_done", "summary": summary }));
    } else {
        if progress {
            eprintln!();
        }
        if summary.already_done {
            println!("The crawl is complete already; pass --fresh to read every page again.");
        }
        println!(
            "{} folders, {} files ({:.1} GB), {} links out of the roots; {} pages read, {} of them from nemweb",
            summary.dirs,
            summary.files,
            summary.bytes as f64 / 1e9,
            summary.external,
            summary.pages,
            summary.pages_fetched,
        );
    }
    Ok(0)
}

/// Downloads the crawl's files; the exit code, and whether a file failed
/// on the network, which passes with time.
async fn download(
    ctx: &Context,
    fetcher: &Fetcher,
    mirror: &Mirror,
) -> Result<(i32, bool), MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let summary = nemlake_mirror::download(fetcher, mirror, ctx.config.downloads, |f| {
        if json {
            render::json_line(&serde_json::json!({ "event": "file_done", "file": f }));
        } else if progress {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r{}/{} files   ", f.done, f.total);
            let _ = err.flush();
        }
    })
    .await?;
    for d in &summary.failed {
        report(ctx, d);
    }
    if json {
        render::json_line(&serde_json::json!({ "event": "download_done", "summary": summary }));
    } else {
        if progress {
            eprintln!();
        }
        println!(
            "{} files: {} archived already, {} removed as duplicates, {} downloaded ({:.1} GB), {} failed",
            summary.files,
            summary.cached,
            summary.removed,
            summary.downloaded,
            summary.bytes as f64 / 1e9,
            summary.failed.len(),
        );
    }
    let network = summary.failed.iter().any(|d| {
        matches!(
            d.code.as_ref(),
            "net.retries_exhausted" | "net.timeout" | "net.connect" | "net.body"
        )
    });
    Ok((i32::from(!summary.failed.is_empty()), network))
}

fn map(ctx: &Context, mirror: &Mirror, given: &[String], jobs: usize) -> Result<i32, MirrorError> {
    let roots = roots(ctx, given)?;
    let s = nemlake_mirror::map(mirror, &ctx.archive(), &roots, jobs)?;
    if ctx.json {
        render::json_line(&serde_json::json!({ "event": "map_done", "summary": s }));
    } else {
        println!(
            "{} folders, {} files, {} zips; {:.1} GB archived, {} not downloaded, {} removed as duplicates, {} no longer on nemweb, {} unreadable; written to {}",
            s.dirs,
            s.files,
            s.zips,
            s.archived_bytes as f64 / 1e9,
            s.not_archived,
            s.removed,
            s.gone_upstream,
            s.errors,
            mirror.dir().join("map").display(),
        );
    }
    Ok(i32::from(s.errors > 0))
}

fn compare(ctx: &Context, mirror: &Mirror, delete: bool) -> Result<i32, MirrorError> {
    use nemlake_mirror::Verdict;
    let archive = ctx.archive();
    let removed = mirror.removed()?;
    let tmp = mirror.dir().join("tmp");
    let (mut waiting, mut freed, mut rows) = (0usize, 0u64, Vec::new());
    for pair in nemlake_mirror::pairs(mirror)? {
        if removed.contains_key(&pair.zip) {
            continue;
        }
        let cmp = nemlake_mirror::compare(&archive, &pair, &tmp)?;
        if cmp.verdict == Verdict::NotArchived {
            waiting += 1;
            continue;
        }
        let mut action = "kept";
        if delete && cmp.verdict == Verdict::Same {
            let r = nemlake_mirror::remove(mirror, &archive, &cmp)?;
            freed += r.size;
            action = "removed";
        }
        if ctx.json {
            render::json_line(&serde_json::json!({
                "event": "compared", "comparison": cmp, "action": action,
            }));
        } else {
            let name = cmp.zip.rsplit('/').next().unwrap_or(&cmp.zip).to_owned();
            rows.push(vec![
                name,
                format!("{:?}", cmp.verdict).to_lowercase(),
                cmp.same.to_string(),
                cmp.repacked.to_string(),
                cmp.only_in_zip.len().to_string(),
                cmp.different.len().to_string(),
                cmp.only_in_folder.len().to_string(),
                action.to_owned(),
            ]);
            for f in cmp.only_in_zip.iter().take(5) {
                eprintln!("{}: only in the zip: {f}", cmp.zip);
            }
            for f in cmp.different.iter().take(5) {
                eprintln!("{}: differs from the folder: {f}", cmp.zip);
            }
        }
    }
    if !ctx.json {
        print!(
            "{}",
            render::table(
                &[
                    "zip",
                    "verdict",
                    "same",
                    "repacked",
                    "only in zip",
                    "differ",
                    "only in folder",
                    "action"
                ],
                &rows,
            )
        );
        println!(
            "{} compared, {waiting} waiting for downloads, {:.1} GB freed",
            rows.len(),
            freed as f64 / 1e9
        );
    }
    Ok(0)
}

fn summary(
    ctx: &Context,
    mirror: &Mirror,
    out: &std::path::Path,
    check: bool,
) -> Result<i32, MirrorError> {
    let text = nemlake_mirror::summary(&mirror.dir().join("map/map.ndjson"))?;
    if check {
        if std::fs::read_to_string(out).ok().as_deref() != Some(text.as_str()) {
            return Err(MirrorError::SummaryStale {
                path: out.display().to_string(),
            });
        }
    } else {
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| MirrorError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }
        std::fs::write(out, &text).map_err(|e| MirrorError::Io {
            path: out.display().to_string(),
            source: e,
        })?;
    }
    if ctx.json {
        render::json_line(&serde_json::json!({
            "event": "summary_done",
            "path": out,
            "lines": text.lines().count(),
            "checked": check,
        }));
    } else if check {
        println!("{} is the summary of the current map", out.display());
    } else {
        println!(
            "{} lines written to {}",
            text.lines().count(),
            out.display()
        );
    }
    Ok(0)
}

fn status(ctx: &Context, mirror: &Mirror, given: &[String]) -> Result<i32, MirrorError> {
    let roots = roots(ctx, given)?;
    let s = nemlake_mirror::status(mirror, &ctx.archive(), &roots)?;
    if ctx.json {
        render::json_line(&s);
    } else {
        print!("{}", status_text(&s));
    }
    Ok(0)
}

fn status_text(s: &Status) -> String {
    let run = if s.running { "running" } else { "not running" };
    let crawl = if s.crawl_done {
        "complete"
    } else {
        "in progress"
    };
    let mut out = format!("mirror {run}; crawl {crawl}: {} pages read\n", s.pages);
    if !s.crawl_done {
        return out;
    }
    let rows: Vec<Vec<String>> = s
        .roots
        .iter()
        .map(|r| {
            vec![
                r.root.clone(),
                if r.removed_files > 0 {
                    format!(
                        "{}/{} ({} removed)",
                        r.archived_files, r.files, r.removed_files
                    )
                } else {
                    format!("{}/{}", r.archived_files, r.files)
                },
                format!(
                    "{:.1}/{:.1}",
                    r.archived_bytes as f64 / 1e9,
                    r.bytes as f64 / 1e9
                ),
            ]
        })
        .collect();
    out.push_str(&render::table(&["root", "files", "GB"], &rows));
    if s.external > 0 {
        let _ = writeln!(out, "{} links out of the roots, not followed", s.external);
    }
    out
}
