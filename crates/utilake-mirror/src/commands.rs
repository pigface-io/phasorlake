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
use std::cell::RefCell;

use nemweb_mirror::{Diagnose, Mirror, MirrorError, PeekCount, Status, Url};

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
        Command::Download { only } => {
            let fetcher = ctx.fetcher();
            let pass = async {
                download(ctx, &fetcher, &mirror, only, false, None)
                    .await
                    .map(|(code, _)| code)
            };
            locked(&mirror, pass).await
        }
        Command::Peek => {
            let fetcher = ctx.fetcher();
            let pass = async {
                peek(ctx, &fetcher, &mirror, None)
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
        Command::Map { roots } => {
            let jobs = cli.jobs.unwrap_or_else(|| ctx.config.effective_jobs());
            map(ctx, &mirror, roots, jobs)
        }
        Command::Hash { all } => hash(ctx, *all),
        Command::Compare { verify: true, .. } => verify(ctx, &mirror),
        Command::Compare { delete, .. } => compare(ctx, &mirror, *delete),
        Command::Summary { out, check } => summary(ctx, &mirror, out, *check),
        Command::Status { roots, full } => status(ctx, &mirror, roots, *full),
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
/// What `run` was asked to do.
struct RunPlan<'a> {
    given: &'a [String],
    fresh: bool,
    refusals: u32,
    wait: Duration,
    delete_duplicates: bool,
}

async fn run(ctx: &Context, mirror: &Mirror, plan: &RunPlan<'_>) -> Result<i32, MirrorError> {
    let (given, refusals, wait, delete_duplicates) =
        (plan.given, plan.refusals, plan.wait, plan.delete_duplicates);
    let mut fresh = plan.fresh;
    let mut refused = 0;
    loop {
        let (tx, rx) = tokio::sync::mpsc::channel(4096);
        let fetcher = ctx.fetcher().with_events(tx);
        let attempt = async {
            crawl(ctx, &fetcher, mirror, given, fresh).await?;
            // The counts `status` reads while this runs: counted once,
            // kept up to date from what peek and download do.
            let tracker = RefCell::new(Tracker::start(ctx, mirror, given, rx)?);
            // One fetcher paces both: peek's small requests fit between
            // the downloads.
            let (peeked, downloaded) = tokio::join!(
                peek(ctx, &fetcher, mirror, Some(&tracker)),
                download(
                    ctx,
                    &fetcher,
                    mirror,
                    &[],
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
    tracker: Option<&RefCell<Tracker>>,
) -> Result<(i32, bool), MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let summary = nemweb_mirror::peek(fetcher, mirror, |z| {
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
    only: &[String],
    delete_duplicates: bool,
    tracker: Option<&RefCell<Tracker>>,
) -> Result<(i32, bool), MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let options = nemweb_mirror::DownloadOptions {
        downloads: ctx.config.downloads,
        only,
        delete_duplicates,
    };
    let summary = nemweb_mirror::download(fetcher, mirror, &options, |f| {
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

fn hash(ctx: &Context, all: bool) -> Result<i32, MirrorError> {
    let json = ctx.json;
    let progress = ctx.progress;
    let s = nemweb_mirror::hash(&ctx.archive(), all, |f| {
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
    let results = nemweb_mirror::verify(mirror, &ctx.archive())?;
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
    use nemweb_mirror::Verdict;
    let archive = ctx.archive();
    let removed = mirror.removed()?;
    let listings: std::collections::HashMap<String, Vec<nemweb_mirror::zipdir::Entry>> =
        nemweb_mirror::peek::current(mirror)?
            .into_values()
            .filter_map(|p| Some((p.url, p.entries?)))
            .collect();
    let tmp = mirror.dir().join("tmp");
    let (mut compared, mut waiting, mut freed, mut spared) = (0usize, 0usize, 0u64, 0u64);
    let mut rows = Vec::new();
    for pair in nemweb_mirror::pairs(mirror)? {
        if removed.contains_key(&pair.zip) {
            continue;
        }
        let listing = listings.get(&pair.zip).map(Vec::as_slice);
        let cmp = nemweb_mirror::compare(&archive, &pair, listing, &tmp)?;
        if cmp.verdict == Verdict::NotArchived {
            waiting += 1;
            continue;
        }
        compared += 1;
        let mut action = "kept";
        if delete && cmp.verdict == Verdict::Same {
            let r = nemweb_mirror::remove(mirror, &archive, &cmp)?;
            if r.downloaded {
                freed += r.size;
            } else {
                spared += r.size;
            }
            action = "removed";
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

/// A run's counts, written to the mirror every few seconds for `status`.
pub struct Tracker {
    mirror: Mirror,
    status: Status,
    peeked_before: usize,
    written: std::time::Instant,
    every: Duration,
    /// The fetcher's progress, and the bytes so far of each file in flight.
    events: tokio::sync::mpsc::Receiver<nemweb_mirror::net::Event>,
    flying: std::collections::HashMap<String, u64>,
}

impl Tracker {
    /// Counts everything once and writes it.
    fn start(
        ctx: &Context,
        mirror: &Mirror,
        given: &[String],
        events: tokio::sync::mpsc::Receiver<nemweb_mirror::net::Event>,
    ) -> Result<Self, MirrorError> {
        let roots = roots(ctx, given)?;
        let mut status = nemweb_mirror::status(mirror, &ctx.archive(), &roots)?;
        // Listings that still hold: older ones are peeked again this pass.
        let peeked_before = nemweb_mirror::peek::current(mirror)?.len();
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

    fn downloaded(&mut self, f: &nemweb_mirror::FileDone<'_>) {
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
        use nemweb_mirror::net::Event;
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::DownloadStarted { url, .. } => self.flying.insert(url, 0),
                Event::DownloadProgress { url, bytes } => self.flying.insert(url, bytes),
            };
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
        None => nemweb_mirror::status(mirror, &ctx.archive(), &roots(ctx, given)?)?,
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
