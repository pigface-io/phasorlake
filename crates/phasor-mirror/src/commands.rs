//! Each command: building what it needs from the flags, the home and the
//! config, and printing what it did.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Duration;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use phasor_mirror::config::{Config, Home};
use phasor_mirror::net::{
    Archive, FetchError, Fetcher, FetcherConfig, Fetchers, HostFetcher, OfflineTransport,
    ReqwestTransport, Transport, per_host,
};
use std::cell::RefCell;

use phasor_mirror::{Diagnose, Mirror, MirrorError, PeekCount, Status, Url};

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
    /// What every fetcher's requests go through: the network, or none.
    transport: Arc<dyn Transport>,
}

impl Context {
    /// The home and config of the flags; no network.
    pub fn new(cli: &Cli) -> Result<Self> {
        let user_home = std::env::var_os("HOME").map(PathBuf::from);
        let home = Home::resolve(cli.home.as_deref(), user_home.as_deref())
            .context("No home directory: pass --home or set PHASOR_MIRROR_HOME.")?;
        let mut config = Config::load(&home.config_file())
            .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        config.keep_from = cli.keep_from.or(config.keep_from);
        let progress = cli.progress
            || (!cli.no_progress
                && !cli.json
                && std::io::IsTerminal::is_terminal(&std::io::stderr()));
        let transport: Arc<dyn Transport> = if cli.offline {
            Arc::new(OfflineTransport)
        } else {
            let ua = format!(
                "phasor-mirror/{} (+{}){}",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_REPOSITORY"),
                config.user_agent_suffix
            );
            Arc::new(ReqwestTransport::new(
                &ua,
                Duration::from_secs(30),
                Duration::from_secs(config.read_timeout_secs),
            )?)
        };
        Ok(Self {
            home,
            config,
            json: cli.json,
            progress,
            transport,
        })
    }

    /// The archive of downloads.
    pub fn archive(&self) -> Archive {
        Archive::new(self.home.archive())
    }

    /// A fetcher per host, over the network or offline.
    pub fn fetchers(&self) -> Result<Fetchers, MirrorError> {
        let hosts = self.config.hosts()?;
        Ok(per_host(&hosts, &self.archive(), |h| {
            self.fetcher_parts(h.request_interval)
        }))
    }

    /// The fetchers with each host that refused given a new fetcher.
    pub fn renewed(&self, fetchers: Fetchers) -> Fetchers {
        fetchers.renewed(|h: &HostFetcher| {
            let (transport, cfg) = self.fetcher_parts(h.fetcher.min_interval());
            Fetcher::new(transport, self.archive(), cfg)
        })
    }

    /// The transport and the knobs of one fetcher, paced at `interval`.
    fn fetcher_parts(&self, interval: Duration) -> (Arc<dyn Transport>, FetcherConfig) {
        let transport = self.transport.clone();
        let backoff = Duration::from_millis(self.config.backoff_ms);
        let cfg = FetcherConfig {
            attempts: self.config.attempts,
            backoff,
            max_backoff: backoff * 60,
            min_interval: interval,
        };
        (transport, cfg)
    }
}

/// Runs the command; the exit code.
pub async fn run_command(cli: &Cli) -> Result<i32> {
    let ctx = &Context::new(cli)?;
    let cmd = &cli.command;
    let mirror = ctx.home.records().keeping(ctx.config.keep_from);
    let result = match cmd {
        Command::Crawl { roots, fresh } => {
            let pass = async {
                let fetcher = ctx.fetchers()?;
                crawl(ctx, &fetcher, &mirror, roots, *fresh).await
            };
            locked(&mirror, pass).await
        }
        Command::Download { only } => {
            let pass = async {
                let fetcher = ctx.fetchers()?;
                download(ctx, &fetcher, &mirror, only, None, false, None)
                    .await
                    .map(|(code, _)| code)
            };
            locked(&mirror, pass).await
        }
        Command::Peek => {
            let pass = async {
                let fetcher = ctx.fetchers()?;
                peek(ctx, &fetcher, &mirror, false, None)
                    .await
                    .map(|(code, _)| code)
            };
            locked(&mirror, pass).await
        }
        Command::Run {
            roots,
            fresh,
            refusals,
            wait_minutes,
            delete_duplicates,
        } => {
            let wait = Duration::from_secs(wait_minutes * 60);
            let plan = RunPlan {
                given: roots,
                fresh: *fresh,
                refusals: *refusals,
                wait,
                delete_duplicates: *delete_duplicates,
            };
            locked(&mirror, run(ctx, &mirror, &plan)).await
        }
        Command::Watch {
            roots,
            rounds,
            refusals,
            wait_minutes,
            delete_duplicates,
        } => {
            let plan = WatchPlan {
                given: roots,
                rounds: *rounds,
                refusals: *refusals,
                wait: Duration::from_secs(wait_minutes * 60),
                delete_duplicates: *delete_duplicates,
            };
            locked(&mirror, watch(ctx, &mirror, &plan)).await
        }
        Command::Versions { prune: true } => locked(&mirror, versions(ctx, &mirror, true)).await,
        Command::Versions { prune: false } => versions(ctx, &mirror, false).await,
        Command::Map { roots } => {
            let jobs = cli.jobs.unwrap_or_else(|| ctx.config.effective_jobs());
            map(ctx, &mirror, roots, jobs)
        }
        Command::Prune { yes } => locked(&mirror, async { prune(ctx, &mirror, *yes) }).await,
        Command::Hash { all } => hash(ctx, *all),
        Command::Compare { verify: true, .. } => verify(ctx, &mirror),
        Command::Compare { delete, .. } => compare(ctx, &mirror, *delete),
        Command::Summary { out, check } => summary(ctx, &mirror, out, *check),
        Command::Status { roots, full } => status(ctx, &mirror, roots, *full),
        Command::Reindex => {
            let _lock = mirror.lock()?;
            let n = mirror.reindex(&ctx.archive())?;
            if ctx.json {
                render::json_line(&serde_json::json!({"event": "reindex_done", "lines": n}));
            } else {
                println!("{n} held lines written to {}", mirror.held_path().display());
            }
            Ok(0)
        }
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

fn report(ctx: &Context, d: &phasor_mirror::Diagnostic) {
    if ctx.json {
        render::json_line(d);
    } else {
        eprintln!("{}", d.render());
    }
}

/// The roots given, else every host's.
fn roots(ctx: &Context, given: &[String]) -> Result<Vec<Url>, MirrorError> {
    if given.is_empty() {
        Ok(ctx
            .config
            .hosts()?
            .into_iter()
            .flat_map(|h| h.roots)
            .collect())
    } else {
        given.iter().map(|r| phasor_mirror::root(r)).collect()
    }
}

/// What `run` was asked to do.
struct RunPlan<'a> {
    given: &'a [String],
    fresh: bool,
    refusals: u32,
    wait: Duration,
    delete_duplicates: bool,
}

/// Crawls, then downloads; a refusal waits `wait` and starts both again
/// with a new fetcher, at most `refusals` times.
async fn run(ctx: &Context, mirror: &Mirror, plan: &RunPlan<'_>) -> Result<i32, MirrorError> {
    let (given, refusals, wait, delete_duplicates) =
        (plan.given, plan.refusals, plan.wait, plan.delete_duplicates);
    let mut fresh = plan.fresh;
    let mut refused = 0;
    let mut fetchers = Some(ctx.fetchers()?);
    loop {
        let (tx, rx) = tokio::sync::mpsc::channel(4096);
        let fetcher = fetchers
            .take()
            .expect("kept for the next pass")
            .with_events(&tx);
        let attempt = async {
            crawl(ctx, &fetcher, mirror, given, fresh).await?;
            // The counts `status` reads while this runs: counted once,
            // kept up to date from what peek and download do.
            let tracker = RefCell::new(Tracker::start(ctx, mirror, given, rx)?);
            // One fetcher paces both: peek's small requests fit between
            // the downloads.
            let (peeked, downloaded) = tokio::join!(
                peek(ctx, &fetcher, mirror, true, Some(&tracker)),
                download(
                    ctx,
                    &fetcher,
                    mirror,
                    &[],
                    None,
                    delete_duplicates,
                    Some(&tracker)
                )
            );
            tracker.into_inner().write();
            let ((p_code, p_net), (d_code, d_net)) = (peeked?, downloaded?);
            Ok::<_, MirrorError>((p_code.max(d_code), p_net || d_net))
        };
        let d = match attempt.await {
            Ok((code, false)) => return Ok(code),
            // Files failed on the network: another pass fetches only those.
            Ok((_, true)) => phasor_mirror::Diagnostic::error(
                "mirror.files_failed",
                phasor_mirror::diag::MIRROR,
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
        // The crawl moved aside already; a second move would lose it. Only
        // the host that refused gets a new fetcher.
        fetchers = Some(ctx.renewed(fetcher));
        fresh = false;
    }
}

/// What `watch` was asked to do.
struct WatchPlan<'a> {
    given: &'a [String],
    rounds: Option<u64>,
    refusals: u32,
    wait: Duration,
    delete_duplicates: bool,
}

/// Reads each folder's listing again when its turn comes and downloads what
/// is new, until `rounds` rounds; a failure waits `wait`, at most `refusals` in a row.
async fn watch(ctx: &Context, mirror: &Mirror, plan: &WatchPlan<'_>) -> Result<i32, MirrorError> {
    let roots = roots(ctx, plan.given)?;
    if !mirror.done_path().exists() {
        return Err(MirrorError::CrawlIncomplete {
            path: mirror.done_path().display().to_string(),
        });
    }
    let folders = mirror
        .kept()?
        .into_iter()
        .filter(|r| r.kind == phasor_mirror::RemoteKind::Dir)
        .map(|r| r.url)
        .chain(roots.iter().map(ToString::to_string));
    let cadence = ctx.config.cadence()?;
    let least = cadence.least;
    let mut schedule = phasor_mirror::Schedule::new(cadence, folders, std::time::Instant::now());
    // Rounds are no closer than the least interval: each one reads the
    // crawl record and, when anything changed, writes it again.
    let mut not_before = std::time::Instant::now();
    let archive = ctx.archive();
    let mut fetcher = ctx.fetchers()?;
    let (mut rounds, mut refused) = (0, 0);
    while plan.rounds.is_none_or(|n| rounds < n) {
        let now = std::time::Instant::now();
        let due = schedule.due(now);
        if due.is_empty() || now < not_before {
            let next = schedule.next().unwrap_or(now + plan.wait).max(not_before);
            tokio::time::sleep_until(next.into()).await;
            continue;
        }
        rounds += 1;
        not_before = now + least;
        // A link to an archive on a drive not mounted.
        let root = archive.root();
        if root.symlink_metadata().is_ok() && !root.exists() {
            event(ctx, "archive_missing", &root.display().to_string());
            tokio::time::sleep(plan.wait).await;
            continue;
        }
        let round = async {
            let r = phasor_mirror::refresh(&fetcher, mirror, &roots, &due).await?;
            let now = std::time::Instant::now();
            for f in &r.removed {
                schedule.remove(f);
            }
            for f in &r.added {
                schedule.add(f.clone(), now);
            }
            for (f, changed) in &r.folders {
                schedule.checked(f, *changed, now);
            }
            let changed = r.folders.iter().filter(|(_, c)| *c).count();
            if ctx.json {
                render::json_line(&serde_json::json!({
                    "event": "watch_round",
                    "read": r.folders.len(),
                    "changed": changed,
                    "added": r.added,
                    "removed": r.removed,
                    "files": r.files.len(),
                    "folders": schedule.len(),
                }));
            } else if changed > 0 {
                println!(
                    "{} folders read, {changed} changed, {} added, {} gone; {} files new or rewritten",
                    r.folders.len(),
                    r.added.len(),
                    r.removed.len(),
                    r.files.len(),
                );
            }
            // Every file the archive lacks, changed this round or not, so one
            // a failed or stopped download missed is fetched next time.
            if !r.files.is_empty() {
                let files: std::collections::HashSet<&str> =
                    r.files.iter().map(String::as_str).collect();
                let paired = phasor_mirror::pairs(mirror, &ctx.archive(), false)?;
                if paired.iter().any(|p| files.contains(p.zip.as_str())) {
                    peek(ctx, &fetcher, mirror, true, None).await?;
                }
            }
            let read: Vec<String> = r.folders.into_iter().map(|(f, _)| f).collect();
            download(
                ctx,
                &fetcher,
                mirror,
                &[],
                Some(&read),
                plan.delete_duplicates,
                None,
            )
            .await
        };
        let d = match round.await {
            Ok(_) => {
                refused = 0;
                continue;
            }
            Err(MirrorError::Fetch(e)) if waits_out(&e) => e.diagnostic(),
            Err(other) => return Err(other),
        };
        if fetcher.succeeded() > 0 {
            refused = 0;
        }
        report(ctx, &d);
        if refused >= plan.refusals {
            return Ok(1);
        }
        refused += 1;
        event(ctx, "waiting", &format!("{} seconds", plan.wait.as_secs()));
        tokio::time::sleep(plan.wait).await;
        fetcher = ctx.renewed(fetcher);
    }
    Ok(0)
}

async fn versions(ctx: &Context, mirror: &Mirror, prune: bool) -> Result<i32, MirrorError> {
    let json = ctx.json;
    let s = phasor_mirror::versions(mirror, &ctx.archive(), prune, |e| {
        if json {
            render::json_line(&serde_json::json!({ "event": "version_same", "line": e }));
        }
    })
    .await?;
    if json {
        render::json_line(&serde_json::json!({ "event": "versions_done", "summary": s }));
    } else {
        println!(
            "{} files with {} versions: {} the same bytes as one before, {} zips packed again; {} removed ({:.1} MB)",
            s.files,
            s.versions,
            s.same,
            s.repacked,
            s.removed,
            s.bytes as f64 / 1e6,
        );
    }
    Ok(0)
}

fn prune(ctx: &Context, mirror: &Mirror, yes: bool) -> Result<i32, MirrorError> {
    let s = phasor_mirror::prune(mirror, &ctx.archive(), yes)?;
    if ctx.json {
        render::json_line(&serde_json::json!({ "event": "prune_done", "summary": s }));
        return Ok(0);
    }
    let before = s.before.map(|m| m.to_string()).unwrap_or_default();
    println!(
        "{} {} files listed before {before} ({:.1} GB); {} held ({:.1} GB); {} folders whole",
        if s.applied { "deleted" } else { "would delete" },
        s.files,
        s.listed_bytes as f64 / 1e9,
        s.held_files,
        s.held_bytes as f64 / 1e9,
        s.folders.len(),
    );
    if !s.applied {
        println!("pass --yes to delete; this cannot be undone");
    }
    Ok(0)
}

/// A line of what a watch does: NDJSON, or for people on stderr.
fn event(ctx: &Context, name: &str, detail: &str) {
    if ctx.json {
        render::json_line(&serde_json::json!({ "event": name, "detail": detail }));
    } else {
        eprintln!("{name}: {detail}");
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
    fetcher: &Fetchers,
    mirror: &Mirror,
    given: &[String],
    fresh: bool,
) -> Result<i32, MirrorError> {
    let roots = roots(ctx, given)?;
    let json = ctx.json;
    let progress = ctx.progress;
    let summary = phasor_mirror::crawl(fetcher, mirror, &roots, fresh, |p| {
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

/// Peeks the zips not yet downloaded; the exit code, and whether a zip failed on the network.
async fn peek(
    ctx: &Context,
    fetcher: &Fetchers,
    mirror: &Mirror,
    only_paired: bool,
    tracker: Option<&RefCell<Tracker>>,
) -> Result<(i32, bool), MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let summary = phasor_mirror::peek(fetcher, mirror, only_paired, |z| {
        if let Some(t) = tracker {
            t.borrow_mut().peeked(z.done, z.total);
        }
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
            "{} zips peeked: {} archived, {} read, {} not zips or damaged, {} failed",
            summary.zips,
            summary.archived,
            summary.read,
            summary.unreadable,
            summary.failed.len(),
        );
    }
    let network = summary.failed.iter().any(passes_with_time);
    Ok((i32::from(!summary.failed.is_empty()), network))
}

/// Whether a failure on the network may pass if asked again later.
fn passes_with_time(d: &phasor_mirror::Diagnostic) -> bool {
    matches!(
        d.code.as_ref(),
        "net.retries_exhausted" | "net.timeout" | "net.connect" | "net.body"
    )
}

/// Downloads the crawl's files; the exit code, and whether a file failed on the network.
async fn download(
    ctx: &Context,
    fetcher: &Fetchers,
    mirror: &Mirror,
    only: &[String],
    folders: Option<&[String]>,
    delete_duplicates: bool,
    tracker: Option<&RefCell<Tracker>>,
) -> Result<(i32, bool), MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let options = phasor_mirror::DownloadOptions {
        downloads: ctx.config.downloads,
        only,
        folders,
        delete_duplicates,
    };
    let summary = phasor_mirror::download(fetcher, mirror, &options, |f| {
        if let Some(t) = tracker {
            t.borrow_mut().downloaded(f);
        }
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
    let offsets = phasor_mirror::Offsets::of(&ctx.config.hosts()?);
    let s = phasor_mirror::map(mirror, &ctx.archive(), &roots, jobs, &offsets)?;
    if ctx.json {
        render::json_line(&serde_json::json!({ "event": "map_done", "summary": s }));
    } else {
        println!(
            "{} folders, {} files, {} zips; {:.1} GB archived, {} not downloaded, {} removed as duplicates, {} no longer on nemweb, {} later versions, {} unreadable; written to {}",
            s.dirs,
            s.files,
            s.zips,
            s.archived_bytes as f64 / 1e9,
            s.not_archived,
            s.removed,
            s.gone_upstream,
            s.versions,
            s.errors,
            mirror.cache_dir().display(),
        );
    }
    Ok(i32::from(s.errors > 0))
}

fn hash(ctx: &Context, all: bool) -> Result<i32, MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let s = phasor_mirror::hash(&ctx.archive(), all, |f| {
        if json {
            render::json_line(&serde_json::json!({ "event": "file_hashed", "file": f }));
        } else if progress {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r{}/{} files read   ", f.done, f.total);
            let _ = err.flush();
        }
    })?;
    for d in &s.failed {
        report(ctx, d);
    }
    if json {
        render::json_line(&serde_json::json!({ "event": "hash_done", "summary": s }));
    } else {
        if progress {
            eprintln!();
        }
        println!(
            "{} downloaded files: {} read, {} CRC-32s recorded, {} not as recorded",
            s.files,
            s.read,
            s.recorded,
            s.failed.len()
        );
    }
    Ok(i32::from(!s.failed.is_empty()))
}

fn verify(ctx: &Context, mirror: &Mirror) -> Result<i32, MirrorError> {
    let results = phasor_mirror::verify(mirror, &ctx.archive())?;
    let mut rows = Vec::new();
    for v in &results {
        if ctx.json {
            render::json_line(&serde_json::json!({ "event": "verified", "verified": v }));
        } else {
            rows.push(vec![
                v.zip.rsplit('/').next().unwrap_or(&v.zip).to_owned(),
                if v.holds() { "holds" } else { "BROKEN" }.to_owned(),
                v.checked.to_string(),
                v.changed.len().to_string(),
                v.missing.len().to_string(),
                if v.no_listing { "no listing" } else { "" }.to_owned(),
            ]);
            for f in v.changed.iter().chain(&v.missing).take(5) {
                eprintln!("{}: {f} is no longer the zip's", v.zip);
            }
        }
    }
    let broken = results.iter().filter(|v| !v.holds()).count();
    if !ctx.json {
        print!(
            "{}",
            render::table(
                &["zip", "folder", "checked", "changed", "missing", ""],
                &rows
            )
        );
        println!(
            "{} removed zips checked, {broken} no longer held by their folders",
            results.len()
        );
    }
    Ok(i32::from(broken > 0))
}

fn compare(ctx: &Context, mirror: &Mirror, delete: bool) -> Result<i32, MirrorError> {
    use phasor_mirror::Verdict;
    let archive = ctx.archive();
    let offsets = phasor_mirror::Offsets::of(&ctx.config.hosts()?);
    // Read once for the whole pass: each removal keeps its zip's listing.
    let record = mirror.record()?;
    let current = phasor_mirror::peek::current_in(mirror, &record)?;
    let mut known = phasor_mirror::peek::Known {
        record,
        peeked: current.keys().cloned().collect(),
    };
    let listings: std::collections::HashMap<String, Vec<phasor_mirror::zipdir::Entry>> = current
        .into_values()
        .filter_map(|p| Some((p.url, p.entries?)))
        .collect();
    let tmp = mirror.tmp_dir();
    let (mut compared, mut waiting, mut freed, mut spared) = (0usize, 0usize, 0u64, 0u64);
    let mut rows = Vec::new();
    for pair in phasor_mirror::pairs(mirror, &archive, false)? {
        let listing = listings.get(&pair.zip).map(Vec::as_slice);
        let cmp = phasor_mirror::compare(&archive, &pair, listing, &tmp)?;
        if cmp.verdict == Verdict::NotArchived {
            waiting += 1;
            continue;
        }
        compared += 1;
        let mut action = "kept";
        if delete && cmp.verdict == Verdict::Same {
            let offset = offsets.offset_of(&cmp.zip);
            let r = phasor_mirror::remove(mirror, &archive, &cmp, offset, Some(&mut known))?;
            if r.downloaded {
                freed += r.size;
            } else {
                spared += r.size;
            }
            action = "removed";
        } else if delete {
            // A rollup kept whole: its files held from CURRENT still gain
            // the address inside it.
            phasor_mirror::note_entries(&cmp)?;
        }
        if ctx.json {
            render::json_line(&serde_json::json!({
                "event": "compared", "comparison": cmp, "action": action,
            }));
        } else {
            let name = cmp.zip.rsplit('/').next().unwrap_or(&cmp.zip).to_owned();
            let verdict = serde_json::to_value(cmp.verdict).expect("a verdict serialises");
            rows.push(vec![
                name,
                verdict.as_str().unwrap_or_default().to_owned(),
                if cmp.from_listing { "listing" } else { "zip" }.to_owned(),
                cmp.same.to_string(),
                cmp.repacked.to_string(),
                cmp.only_in_zip.len().to_string(),
                cmp.different.len().to_string(),
                cmp.unsure.len().to_string(),
                cmp.ancillary.len().to_string(),
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
    if ctx.json {
        render::json_line(&serde_json::json!({
            "event": "compare_done",
            "compared": compared,
            "waiting": waiting,
            "freed_bytes": freed,
            "not_downloaded_bytes": spared,
        }));
    } else {
        print!(
            "{}",
            render::table(
                &[
                    "zip",
                    "verdict",
                    "from",
                    "same",
                    "repacked",
                    "only in zip",
                    "differ",
                    "unsure",
                    "pages",
                    "only in folder",
                    "action"
                ],
                &rows,
            )
        );
        println!(
            "{compared} compared, {waiting} waiting for downloads, {:.1} GB freed, {:.1} GB not to download",
            freed as f64 / 1e9,
            spared as f64 / 1e9
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
    let text = phasor_mirror::summary(&mirror.cache_dir().join("catalog.ndjson"))?;
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

/// A run's counts, written to the mirror every few seconds for `status`.
pub struct Tracker {
    mirror: Mirror,
    status: Status,
    peeked_before: usize,
    written: std::time::Instant,
    every: Duration,
    /// The fetcher's progress, and the bytes so far of each file in flight.
    events: tokio::sync::mpsc::Receiver<phasor_mirror::net::Event>,
    flying: std::collections::HashMap<String, u64>,
}

impl Tracker {
    /// Counts everything once and writes it.
    fn start(
        ctx: &Context,
        mirror: &Mirror,
        given: &[String],
        events: tokio::sync::mpsc::Receiver<phasor_mirror::net::Event>,
    ) -> Result<Self, MirrorError> {
        let roots = roots(ctx, given)?;
        let mut status = phasor_mirror::status(mirror, &ctx.archive(), &roots)?;
        // Listings that still hold: older ones are peeked again this pass.
        let peeked_before = phasor_mirror::peek::current(mirror)?.len();
        status.peek = Some(PeekCount {
            done: peeked_before,
            total: peeked_before,
        });
        let t = Self {
            mirror: mirror.clone(),
            status,
            peeked_before,
            written: std::time::Instant::now(),
            every: Duration::from_secs(ctx.config.progress_every_secs),
            events,
            flying: std::collections::HashMap::new(),
        };
        t.write();
        Ok(t)
    }

    fn downloaded(&mut self, f: &phasor_mirror::FileDone<'_>) {
        // Its last progress may still be queued: take it before it is done.
        self.drain();
        self.flying.remove(f.url);
        if !f.cached {
            self.status.add_download(f.url, f.bytes);
        }
        self.tick();
    }

    fn peeked(&mut self, done: usize, total: usize) {
        self.status.peek = Some(PeekCount {
            done: self.peeked_before + done,
            total: self.peeked_before + total,
        });
        self.tick();
    }

    /// Takes the fetcher's progress so far.
    fn drain(&mut self) {
        use phasor_mirror::net::Event;
        while let Ok(e) = self.events.try_recv() {
            // A fetcher reports only its downloads.
            if let Event::DownloadStarted { url, .. } = &e {
                self.flying.insert(url.clone(), 0);
            }
            if let Event::DownloadProgress { url, bytes } = e {
                self.flying.insert(url, bytes);
            }
        }
    }

    fn tick(&mut self) {
        self.drain();
        self.status.downloading = Some(self.flying.values().sum());
        if self.written.elapsed() >= self.every {
            self.write();
            self.written = std::time::Instant::now();
        }
    }

    /// Counts that cannot be written leave `status` to count for itself.
    fn write(&self) {
        let _ = self.mirror.write_progress(&self.status);
    }
}

fn status(
    ctx: &Context,
    mirror: &Mirror,
    given: &[String],
    full: bool,
) -> Result<i32, MirrorError> {
    // While a run holds the lock, its counts; else count now.
    let kept = if !full && mirror.running() {
        mirror.read_progress()?
    } else {
        None
    };
    let s = match kept {
        Some(mut s) => {
            s.running = true;
            s
        }
        None => phasor_mirror::status(mirror, &ctx.archive(), &roots(ctx, given)?)?,
    };
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
    if let Some(at) = &s.as_of {
        let _ = writeln!(
            out,
            "counts kept by the run, as of {at} (status --full counts now)"
        );
    }
    if let Some(p) = s.peek {
        let _ = writeln!(out, "zips peeked: {}/{}", p.done, p.total);
    }
    if let Some(b) = s.downloading.filter(|b| *b > 0) {
        let _ = writeln!(out, "downloading now: {:.2} GB so far", b as f64 / 1e9);
    }
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
