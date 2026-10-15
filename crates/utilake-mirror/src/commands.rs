//! Each command: building what it needs from the flags, the home and the
//! config, and printing what it did.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Duration;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use nemweb_mirror::config::{Config, Home};
use nemweb_mirror::net::{
    Archive, FetchError, Fetcher, FetcherConfig, OfflineTransport, ReqwestTransport, Transport,
};
use nemweb_mirror::{Diagnose, Mirror, MirrorError, Status, Url};

use crate::cli::{Cli, Command};
use crate::render;

/// What every command may need: the home, the config, and how to print.
pub struct Context {
    /// The home.
    pub home: Home,
    /// The config.
    pub config: Config,
    /// NDJSON output.
    pub json: bool,
    /// A progress line on stderr.
    pub progress: bool,
    /// No network.
    pub offline: bool,
}

impl Context {
    /// The home and config of the flags; no network.
    pub fn new(cli: &Cli) -> Result<Self> {
        let user_home = std::env::var_os("HOME").map(PathBuf::from);
        let home = Home::resolve(cli.home.as_deref(), user_home.as_deref())
            .context("No home directory: pass --home or set NEMWEB_MIRROR_HOME.")?;
        let config = Config::load(&home.config_file())
            .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        let progress = cli.progress
            || (!cli.no_progress
                && !cli.json
                && std::io::IsTerminal::is_terminal(&std::io::stderr()));
        Ok(Self {
            home,
            config,
            json: cli.json,
            progress,
            offline: cli.offline,
        })
    }

    /// The archive of downloads.
    pub fn archive(&self) -> Archive {
        Archive::new(self.home.archive())
    }

    /// A fetcher over the network, or offline, its requests the configured
    /// interval apart.
    pub fn fetcher(&self) -> Fetcher {
        let ua = format!(
            "nemweb-mirror/{} (+https://github.com/g0itb4/nemweb-mirror){}",
            env!("CARGO_PKG_VERSION"),
            self.config.user_agent_suffix
        );
        let transport: Arc<dyn Transport> = if self.offline {
            Arc::new(OfflineTransport)
        } else {
            Arc::new(ReqwestTransport::new(
                &ua,
                Duration::from_secs(30),
                Duration::from_secs(self.config.read_timeout_secs),
            ))
        };
        let backoff = Duration::from_millis(self.config.backoff_ms);
        let cfg = FetcherConfig {
            attempts: self.config.attempts,
            backoff,
            max_backoff: backoff * 60,
            min_interval: Duration::from_millis(self.config.request_interval_ms),
        };
        Fetcher::new(transport, self.archive(), cfg)
    }
}

/// Runs the command; the exit code.
pub async fn run_command(cli: &Cli) -> Result<i32> {
    let ctx = &Context::new(cli)?;
    let cmd = &cli.command;
    let mirror = Mirror::new(ctx.home.mirror());
    let result = match cmd {
        Command::Crawl { roots, fresh } => {
            let fetcher = ctx.fetcher();
            locked(&mirror, crawl(ctx, &fetcher, &mirror, roots, *fresh)).await
        }
        Command::Download => {
            let fetcher = ctx.fetcher();
            let pass = async { download(ctx, &fetcher, &mirror).await.map(|(code, _)| code) };
            locked(&mirror, pass).await
        }
        Command::Peek => {
            let fetcher = ctx.fetcher();
            let pass = async { peek(ctx, &fetcher, &mirror).await.map(|(code, _)| code) };
            locked(&mirror, pass).await
        }
        Command::Run {
            roots,
            fresh,
            refusals,
            wait_minutes,
        } => {
            let wait = Duration::from_secs(wait_minutes * 60);
            locked(&mirror, run(ctx, &mirror, roots, *fresh, *refusals, wait)).await
        }
        Command::Map { roots } => {
            let jobs = cli.jobs.unwrap_or_else(|| ctx.config.effective_jobs());
            map(ctx, &mirror, roots, jobs)
        }
        Command::Compare { delete } => compare(ctx, &mirror, *delete),
        Command::Summary { out, check } => summary(ctx, &mirror, out, *check),
        Command::Status { roots } => status(ctx, &mirror, roots),
    };
    match result {
        Ok(code) => Ok(code),
        Err(e) => {
            report(ctx, &e.diagnostic());
            Ok(1)
        }
    }
}

/// `work`, with the run lock held; the work does not start without it.
async fn locked(
    mirror: &Mirror,
    work: impl Future<Output = Result<i32, MirrorError>>,
) -> Result<i32, MirrorError> {
    let _lock = mirror.lock()?;
    work.await
}

fn report(ctx: &Context, d: &nemweb_mirror::Diagnostic) {
    if ctx.json {
        render::json_line(d);
    } else {
        eprintln!("{}", d.render());
    }
}

fn roots(ctx: &Context, given: &[String]) -> Result<Vec<Url>, MirrorError> {
    if given.is_empty() {
        nemweb_mirror::default_roots(&ctx.config.base_url)
    } else {
        given.iter().map(|r| nemweb_mirror::root(r)).collect()
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
) -> Result<i32, MirrorError> {
    let mut fresh = fresh;
    let mut refused = 0;
    loop {
        let fetcher = ctx.fetcher();
        let attempt = async {
            crawl(ctx, &fetcher, mirror, given, fresh).await?;
            // One fetcher paces both: peek's small requests fit between
            // the downloads.
            let (peeked, downloaded) =
                tokio::join!(peek(ctx, &fetcher, mirror), download(ctx, &fetcher, mirror));
            let ((p_code, p_net), (d_code, d_net)) = (peeked?, downloaded?);
            Ok::<_, MirrorError>((p_code.max(d_code), p_net || d_net))
        };
        let d = match attempt.await {
            Ok((code, false)) => return Ok(code),
            // Files failed on the network: another pass fetches only those.
            Ok((_, true)) => nemweb_mirror::Diagnostic::error(
                "mirror.files_failed",
                nemweb_mirror::Category::Mirror,
                "Some files failed on the network; the next pass fetches only those.",
            ),
            Err(MirrorError::Fetch(e)) if waits_out(&e) => e.diagnostic(),
            Err(other) => return Err(other),
        };
        // An attempt that got somewhere before failing starts the count again.
        if fetcher.succeeded() > 0 {
            refused = 0;
        }
        if refused >= refusals {
            report(ctx, &d);
            return Ok(1);
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
    let summary = nemweb_mirror::crawl(fetcher, mirror, &roots, fresh, |p| {
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

/// Peeks the zips not yet downloaded; the exit code, and whether a zip
/// failed on the network, which passes with time.
async fn peek(
    ctx: &Context,
    fetcher: &Fetcher,
    mirror: &Mirror,
) -> Result<(i32, bool), MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let summary = nemweb_mirror::peek(fetcher, mirror, |z| {
        if json {
            render::json_line(&serde_json::json!({ "event": "zip_peeked", "zip": z }));
        } else if progress {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r{}/{} zips peeked   ", z.done, z.total);
            let _ = err.flush();
        }
    })
    .await?;
    for d in &summary.failed {
        report(ctx, d);
    }
    if json {
        render::json_line(&serde_json::json!({ "event": "peek_done", "summary": summary }));
    } else {
        if progress {
            eprintln!();
        }
        println!(
            "{} zips peeked: {} read, {} not zips or damaged, {} failed",
            summary.zips,
            summary.read,
            summary.unreadable,
            summary.failed.len(),
        );
    }
    let network = summary.failed.iter().any(passes_with_time);
    Ok((i32::from(!summary.failed.is_empty()), network))
}

/// Whether a failure on the network may pass if asked again later.
fn passes_with_time(d: &nemweb_mirror::Diagnostic) -> bool {
    matches!(
        d.code.as_ref(),
        "net.retries_exhausted" | "net.timeout" | "net.connect" | "net.body"
    )
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
    let summary = nemweb_mirror::download(fetcher, mirror, ctx.config.downloads, |f| {
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
    let network = summary.failed.iter().any(passes_with_time);
    Ok((i32::from(!summary.failed.is_empty()), network))
}

fn map(ctx: &Context, mirror: &Mirror, given: &[String], jobs: usize) -> Result<i32, MirrorError> {
    let roots = roots(ctx, given)?;
    let s = nemweb_mirror::map(mirror, &ctx.archive(), &roots, jobs)?;
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
    use nemweb_mirror::Verdict;
    let archive = ctx.archive();
    let removed = mirror.removed()?;
    let tmp = mirror.dir().join("tmp");
    let (mut waiting, mut freed, mut rows) = (0usize, 0u64, Vec::new());
    for pair in nemweb_mirror::pairs(mirror)? {
        if removed.contains_key(&pair.zip) {
            continue;
        }
        let cmp = nemweb_mirror::compare(&archive, &pair, &tmp)?;
        if cmp.verdict == Verdict::NotArchived {
            waiting += 1;
            continue;
        }
        let mut action = "kept";
        if delete && cmp.verdict == Verdict::Same {
            let r = nemweb_mirror::remove(mirror, &archive, &cmp)?;
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
    let text = nemweb_mirror::summary(&mirror.dir().join("map/map.ndjson"))?;
    if check {
        if std::fs::read_to_string(out).ok().as_deref() != Some(text.as_str()) {
            return Err(MirrorError::SummaryStale {
                path: out.display().to_string(),
            });
        }
    } else {
        let parent = out.parent().unwrap_or(std::path::Path::new(""));
        std::fs::create_dir_all(parent).map_err(|e| MirrorError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
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
    let s = nemweb_mirror::status(mirror, &ctx.archive(), &roots)?;
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
