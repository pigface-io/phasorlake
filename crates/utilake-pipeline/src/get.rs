//! `utilake get`: an interval of some tables, planned across a market's
//! sources, fetched and read into the lake.
//!
//! A kit may publish a table two ways: a monthly archive (settled, a month
//! at a time, some weeks after the month) and report folders (live, a day
//! or a five-minute file at a time). The plan takes each whole month the
//! interval covers from the monthly source where that month is published,
//! and every other day from the daily source: the months the archive has
//! not published yet, and the partial months at either end. A table no
//! report folder carries comes from the archive whole months. `--source`
//! forces one side. The plan lists each source once; the runs read what it
//! listed.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use utilake_acquire::FetchError;
use utilake_core::event::Event;
use utilake_core::{CivilDay, Diagnose as _, Diagnostic, Period, TimeRange, YearMonth};
use utilake_kit::{Granularity, Query, RemoteFile, Source};

use crate::plan::{Deps, Input, Plan, RunOptions};
use crate::report::RunReport;

/// Which side of a market's publication to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceChoice {
    /// Whole published months from the monthly source, the rest daily.
    #[default]
    Auto,
    /// Every month the interval touches, from the monthly source.
    Monthly,
    /// Every day, from the daily source.
    Daily,
}

/// What `get` asks for.
#[derive(Debug, Clone)]
pub struct GetRequest {
    /// The kit, named by its market: `nem`.
    pub kit: String,
    /// Tables, upper-case.
    pub tables: Vec<String>,
    /// Tables left out.
    pub exclude: Vec<String>,
    /// The days the interval touches.
    pub range: TimeRange,
    /// The interval, `[from, to)` as microseconds since the epoch.
    pub instants: (i64, i64),
    /// Which side to read.
    pub source: SourceChoice,
    /// The request, as the manifest records it.
    pub request: String,
}

/// One source's part of a plan.
pub struct Step {
    /// The source.
    pub source: Arc<dyn Source>,
    /// What it is asked for.
    pub query: Query,
    /// The files it listed for that.
    pub files: Vec<RemoteFile>,
}

impl Step {
    /// Bytes the listing gave, where it gave any.
    pub fn bytes(&self) -> u64 {
        self.files.iter().filter_map(|f| f.size).sum()
    }
}

/// The plan: each source's step, and why the interval was cut so.
#[derive(Default)]
pub struct GetPlan {
    /// The steps, monthly first.
    pub steps: Vec<Step>,
    /// What decided the cut: months the archive has not published, tables
    /// no report folder carries, a side forced.
    pub notes: Vec<String>,
}

/// The months wholly inside the days of `range`.
fn whole_months(range: &TimeRange) -> Vec<YearMonth> {
    let (first, last) = (range.from.first_day(), range.to.last_day());
    range
        .months_iter()
        .filter(|m| {
            let p = Period::Month(*m);
            p.first_day() >= first && p.last_day() <= last
        })
        .collect()
}

/// The days of `range` not in `months`, as runs of consecutive days.
fn other_days(range: &TimeRange, months: &[YearMonth]) -> Vec<TimeRange> {
    let mut runs: Vec<TimeRange> = Vec::new();
    for d in range.days_iter().filter(|d| !months.contains(&d.month())) {
        match runs.last_mut() {
            Some(r) if r.to.last_day().next() == d => r.to = Period::Day(d),
            _ => runs.push(TimeRange::days(d, d)),
        }
    }
    runs
}

/// A monthly query over `months` (contiguous).
fn monthly_query(req: &GetRequest, from: YearMonth, to: YearMonth) -> Query {
    Query {
        tables: req.tables.clone(),
        exclude: req.exclude.clone(),
        range: TimeRange::months(from, to),
        instants: None,
    }
}

/// A daily query over the days of `range`, within the request's instants.
fn daily_query(req: &GetRequest, tables: &[String], range: TimeRange) -> Query {
    Query {
        tables: tables.to_vec(),
        exclude: req.exclude.clone(),
        range,
        instants: Some(req.instants),
    }
}

fn no_kit(req: &GetRequest) -> Box<Diagnostic> {
    Box::new(
        Diagnostic::error(
            "config.no_decoder",
            utilake_core::Category::Config,
            format!("No kit named {} was assembled into this binary.", req.kit),
        )
        .with_request(req.request.clone()),
    )
}

fn no_source(req: &GetRequest, side: &str) -> Box<Diagnostic> {
    Box::new(
        Diagnostic::error(
            "config.no_decoder",
            utilake_core::Category::Config,
            format!("The kit {} has no {side} source to read from.", req.kit),
        )
        .with_request(req.request.clone()),
    )
}

/// Lists `months` from the monthly source, dropping trailing months the
/// archive has not published (`listing.month_missing`) until it answers.
/// Returns the step, when any month is published, and the first month
/// that is not.
async fn list_months(
    req: &GetRequest,
    source: &Arc<dyn Source>,
    deps: &Deps,
    from: YearMonth,
    mut to: YearMonth,
    notes: &mut Vec<String>,
) -> Result<(Option<Step>, Option<YearMonth>), Box<Diagnostic>> {
    let mut unpublished = None;
    loop {
        let query = monthly_query(req, from, to);
        match source.list(&query, deps.fetcher.as_ref()).await {
            Ok(files) => {
                return Ok((
                    Some(Step {
                        source: source.clone(),
                        query,
                        files,
                    }),
                    unpublished,
                ));
            }
            Err(FetchError::MonthMissing { period, .. }) => {
                // The archive publishes months in order: this one and the
                // ones after it are not out yet.
                // A month the request did not ask for, or no month at all:
                // not the archive's way of saying "not yet"; reported.
                let missing = period
                    .parse::<YearMonth>()
                    .ok()
                    .filter(|m| *m >= from && *m <= to)
                    .ok_or_else(|| {
                        Box::new(
                            FetchError::MonthMissing {
                                period: period.clone(),
                                url: String::new(),
                                available: format!("{from}..{to} were asked for"),
                            }
                            .diagnostic()
                            .with_request(req.request.clone()),
                        )
                    })?;
                notes.push(format!(
                    "{period} is not published in the monthly archive yet"
                ));
                unpublished = Some(missing);
                if missing == from {
                    return Ok((None, unpublished));
                }
                to = missing.prev();
            }
            Err(e) => return Err(Box::new(e.diagnostic().with_request(req.request.clone()))),
        }
    }
}

/// Plans the request over the kit's sources, listing each once.
pub async fn plan_get(req: &GetRequest, deps: &Deps) -> Result<GetPlan, Box<Diagnostic>> {
    let kit = deps.kits.kit(&req.kit).ok_or_else(|| no_kit(req))?;
    // The first source of each granularity that serves every table asked
    // for: a kit of several feeds (gas) is read from the table's own.
    let serving = |g: Granularity| {
        kit.sources
            .iter()
            .find(|s| s.granularity() == g && req.tables.iter().all(|t| s.serves(t)))
            .cloned()
    };
    let monthly = serving(Granularity::Month);
    let daily = serving(Granularity::Day);
    let mut plan = GetPlan::default();
    let (first, last) = (req.range.first_month(), req.range.last_month());
    // Which days go to the daily side, and with which tables.
    let mut day_runs: Vec<TimeRange> = Vec::new();
    let mut daily_tables: Vec<String> = req.tables.clone();
    match req.source {
        SourceChoice::Monthly => {
            let source = monthly.clone().ok_or_else(|| no_source(req, "monthly"))?;
            plan.notes
                .push("every month from the monthly archive, as asked".into());
            let (step, _) = list_months(req, &source, deps, first, last, &mut plan.notes).await?;
            plan.steps.extend(step);
        }
        SourceChoice::Daily => {
            daily.as_ref().ok_or_else(|| no_source(req, "daily"))?;
            plan.notes
                .push("every day from the report folders, as asked".into());
            day_runs.push(req.range);
        }
        SourceChoice::Auto => {
            let months = whole_months(&req.range);
            day_runs = other_days(&req.range, &months);
            match (&monthly, months.first().copied(), months.last().copied()) {
                (Some(source), Some(m0), Some(m1)) => {
                    let (step, unpublished) =
                        list_months(req, source, deps, m0, m1, &mut plan.notes).await?;
                    plan.steps.extend(step);
                    if let Some(u) = unpublished {
                        // The unpublished months' days go daily.
                        let days = TimeRange::days(
                            Period::Month(u).first_day(),
                            Period::Month(m1).last_day(),
                        );
                        day_runs.push(days);
                        day_runs.sort_by_key(|r| r.from.first_day());
                    }
                }
                (None, Some(_), Some(_)) => {
                    plan.notes
                        .push("no monthly source: every day from the report folders".into());
                    day_runs = vec![req.range];
                }
                _ => {}
            }
        }
    }
    if day_runs.is_empty() {
        return Ok(plan);
    }
    let Some(daily) = daily else {
        // No daily source (a forced daily side was refused above): the
        // archive's whole months cover those days.
        return monthly_fallback(
            req,
            deps,
            monthly.as_ref(),
            &day_runs,
            &mut plan,
            "the kit has no daily source",
        )
        .await;
    };
    // Tables no report folder carries fall back to the archive.
    let probe = TimeRange::one(day_runs[0].from.first_day());
    let mut fallback: Vec<String> = Vec::new();
    for t in &req.tables {
        let q = daily_query(req, std::slice::from_ref(t), probe);
        match daily.list(&q, deps.fetcher.as_ref()).await {
            Ok(_) => {}
            Err(FetchError::NoMatch { nearest, .. }) if nearest.contains("monthly archive") => {
                plan.notes.push(format!(
                    "{t}: no report folder carries it; the monthly archive does"
                ));
                fallback.push(t.clone());
                daily_tables.retain(|d| d != t);
            }
            Err(e) => return Err(Box::new(e.diagnostic().with_request(req.request.clone()))),
        }
    }
    if !daily_tables.is_empty() {
        for run in &day_runs {
            let query = daily_query(req, &daily_tables, *run);
            let files = daily
                .list(&query, deps.fetcher.as_ref())
                .await
                .map_err(|e| Box::new(e.diagnostic().with_request(req.request.clone())))?;
            plan.steps.push(Step {
                source: daily.clone(),
                query,
                files,
            });
        }
    }
    if fallback.is_empty() {
        return Ok(plan);
    }
    let sub = GetRequest {
        tables: fallback,
        ..req.clone()
    };
    monthly_fallback(
        &sub,
        deps,
        monthly.as_ref(),
        &day_runs,
        &mut plan,
        "no report folder carries them",
    )
    .await
}

/// Adds the monthly steps that cover `day_runs` for the request's tables.
async fn monthly_fallback(
    req: &GetRequest,
    deps: &Deps,
    monthly: Option<&Arc<dyn Source>>,
    day_runs: &[TimeRange],
    plan: &mut GetPlan,
    why: &str,
) -> Result<GetPlan, Box<Diagnostic>> {
    let source = monthly.ok_or_else(|| no_source(req, "monthly"))?;
    let mut months: Vec<YearMonth> = day_runs
        .iter()
        .flat_map(|r| r.months_iter().collect::<Vec<_>>())
        .collect();
    months.sort();
    months.dedup();
    // `day_runs` is never empty here, so neither is `months`.
    let (m0, m1) = (months[0], months[months.len() - 1]);
    plan.notes.push(format!(
        "{} from the monthly archive for {}: {why}",
        TimeRange::months(m0, m1),
        req.tables.join(",")
    ));
    let (step, _) = list_months(req, source, deps, m0, m1, &mut plan.notes).await?;
    plan.steps.extend(step);
    Ok(std::mem::take(plan))
}

/// What `get` did: one run per step.
pub struct GetReport {
    /// The plan.
    pub plan: GetPlan,
    /// Each step's run, in step order.
    pub runs: Vec<RunReport>,
    /// The worst exit code of the runs.
    pub exit_code: i32,
}

/// Plans and runs the request: each step a run over the files its source
/// listed, into the lake.
pub async fn run_get(
    req: GetRequest,
    deps: Deps,
    options: RunOptions,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
) -> Result<GetReport, Box<Diagnostic>> {
    let plan = plan_get(&req, &deps).await?;
    let mut runs = Vec::new();
    let mut exit_code = 0;
    for step in &plan.steps {
        let run = crate::run::run(
            Plan {
                request: format!("{} [{}]", req.request, step.source.id()),
                input: Input::Listed {
                    source: step.source.clone(),
                    query: step.query.clone(),
                    files: step.files.clone(),
                },
                options: options.clone(),
            },
            deps.clone(),
            events.clone(),
            cancel.clone(),
        )
        .await;
        exit_code = exit_code.max(run.exit_code);
        runs.push(run);
        if cancel.is_cancelled() {
            break;
        }
    }
    Ok(GetReport {
        plan,
        runs,
        exit_code,
    })
}

/// The days a request touches, from its instants in a market's clock:
/// the day of `from` to the day holding the last instant before `to`.
pub fn days_of(instants: (i64, i64), offset_secs: i32) -> TimeRange {
    let day_of = |us: i64| {
        let local = us.div_euclid(1_000_000) + i64::from(offset_secs);
        CivilDay::from_days(local.div_euclid(86_400))
    };
    let (from, to) = instants;
    TimeRange::days(day_of(from), day_of((to - 1).max(from)))
}
