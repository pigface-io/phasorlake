//! The run: list, fetch, ingest, commit.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use phasor_core::Diagnose;
use phasor_core::event::{Event, Summary};
use phasor_core::report::{DiagSink, Flow, Report, SourceOutcome, SourceStatus};
use phasor_core::{Category, Diagnostic};
use phasor_kit::{KitRegistry, SourceKit, Transform};
use phasor_kit::{LocalFile, Query, RemoteFile, Source};
use phasor_writer::manifest::{FileRecord, IngestEnd, IngestShape, IngestStart, IngestStatus};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::ingest_file::{FileReport, IngestInput, ingest_file};
use crate::plan::{Deps, Input, Plan};
use crate::report::{EXIT_FAILURE, EXIT_INTERRUPTED, EXIT_OFFLINE, RunReport, exit_code};

fn emit(tx: &mpsc::Sender<Event>, e: Event) {
    let _ = tx.try_send(e);
}

/// A local path's `file://` URL, a mirror version `{name}@{stamp}` with the
/// stamp as its fragment; the name without it; and the file's size.
fn local_url(path: &std::path::Path) -> Option<(Url, String, Option<u64>)> {
    let abs = std::path::absolute(path).ok()?;
    let kept = abs.file_name()?.to_string_lossy().into_owned();
    let (name, stamp) = phasor_kit::version_of(&kept);
    let name = name.to_owned();
    let mut url = Url::from_file_path(abs.with_file_name(&name)).ok()?;
    url.set_fragment(stamp);
    let size = std::fs::metadata(&abs).ok().map(|m| m.len());
    Some((url, name, size))
}

/// A local path as a `RemoteFile` with a `file://` URL, and the kit that reads it.
///
/// `None` when no kit reads such a file. A mirror version `{name}@{stamp}`
/// gets the stamp as its URL fragment.
pub fn local_remote(
    kits: &KitRegistry,
    path: &std::path::Path,
) -> Option<(&'static str, RemoteFile)> {
    let (url, name, size) = local_url(path)?;
    if let Some((kit, mut remote)) = kits.claim(&url) {
        remote.source = "local";
        remote.size = size;
        return Some((kit.id, remote));
    }
    // A zip's own name says nothing; any other file needs a decoder for its name.
    let is_zip = phasor_zip::has_extension(&name, ".zip");
    let kit = kits
        .kits()
        .iter()
        .rev()
        .find(|k| is_zip || k.decoder_for(&name).is_some())?;
    Some((
        kit.id,
        RemoteFile {
            url,
            source: "local",
            table: None,
            period: None,
            size,
            modified: None,
            modified_at: None,
        },
    ))
}

/// A local path as a `RemoteFile` read by the kit `kit`; `None` when it cannot read it.
pub fn local_remote_in(
    kits: &KitRegistry,
    kit: &str,
    path: &std::path::Path,
) -> Option<(&'static str, RemoteFile)> {
    let k = kits.kit(kit)?;
    let (url, name, size) = local_url(path)?;
    let is_zip = phasor_zip::has_extension(&name, ".zip");
    if !is_zip && k.decoder_for(&name).is_none() {
        return None;
    }
    let mut remote = k
        .sources
        .iter()
        .find_map(|s| s.describe(&url))
        .unwrap_or(RemoteFile {
            url,
            source: "local",
            table: None,
            period: None,
            size: None,
            modified: None,
            modified_at: None,
        });
    remote.source = "local";
    remote.size = size;
    Some((k.id, remote))
}

/// Hashes a local file into a `LocalFile`; blocking.
pub fn local_file(remote: RemoteFile, path: &std::path::Path) -> std::io::Result<LocalFile> {
    let (sha256, size) = phasor_acquire::sha256_file(path)?;
    Ok(LocalFile {
        remote,
        path: path.to_path_buf(),
        sha256,
        size,
        cached: true,
        etag: None,
        last_modified: None,
        last_modified_at: None,
        fetched_at: None,
    })
}

/// [`local_file`] on a blocking thread.
async fn hash_local(remote: RemoteFile, path: std::path::PathBuf) -> std::io::Result<LocalFile> {
    tokio::task::spawn_blocking(move || local_file(remote, &path))
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
}

/// A recorded source as the file to read it from again, with its kit; `None` when unreadable.
pub fn recorded_remote(kits: &KitRegistry, rec: &FileRecord) -> Option<(&'static str, RemoteFile)> {
    let path = std::path::Path::new(&rec.cache_path);
    let (kit, source) = match kits.source(&rec.source) {
        Some((kit, source)) => (Some(kit.id), source.id()),
        None => (None, "local"),
    };
    let (claimed, mut remote) = local_remote(kits, path)?;
    remote.url = Url::parse(&rec.url).ok()?;
    remote.source = source;
    Some((kit.unwrap_or(claimed), remote))
}

/// The kit whose source listed a file, else the fallback kit.
fn kit_of<'k>(kits: &'k KitRegistry, source_id: &str) -> Option<&'k SourceKit> {
    kits.source(source_id)
        .map(|(k, _)| k)
        .or_else(|| kits.fallback())
}

fn transforms(shapes: &[IngestShape]) -> Vec<Transform> {
    shapes.iter().map(|s| s.transform.clone()).collect()
}

/// A file to fetch or read.
struct Listed {
    kit: &'static str,
    remote: RemoteFile,
    path: Option<std::path::PathBuf>,
    known: Option<FileRecord>,
    /// An archived file, known with its hash already.
    archived: Option<LocalFile>,
    /// A zip to read by its directory first: the source that picks entries, and its query.
    picks: Option<(Arc<dyn Source>, Query)>,
}

enum Fetched {
    Ok(&'static str, LocalFile),
    Failed(RemoteFile, Box<Diagnostic>),
}

/// Runs a plan to completion.
pub async fn run(
    plan: Plan,
    deps: Deps,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
) -> RunReport {
    let started = Instant::now();
    let mut report = if plan.options.fail_fast {
        Report::fail_fast()
    } else {
        Report::new()
    };
    let mut summary = Summary::default();
    let request = plan.request.clone();

    let run_id = match deps.manifest.start_run(&request).await {
        Ok(id) => Some(id),
        Err(e) => {
            let d = e.diagnostic().with_request(request.clone());
            emit(&events, Event::Diagnostic(Box::new(d.clone())));
            report.emit(d);
            None
        }
    };

    // 1. List. Two URLs the archive keeps at one path are fetched once.
    let remote_listed = |source: &Arc<dyn Source>, query: &Query, files: Vec<RemoteFile>| {
        let mut seen = std::collections::HashSet::new();
        files
            .into_iter()
            .filter(|r| seen.insert(deps.fetcher.archive().path_for(r.url.as_str())))
            .map(|remote| Listed {
                kit: kit_of(&deps.kits, source.id()).map_or("none", |k| k.id),
                picks: source
                    .wants_ranged(&remote, query)
                    .then(|| (source.clone(), query.clone())),
                remote,
                path: None,
                known: None,
                archived: None,
            })
            .collect::<Vec<Listed>>()
    };
    let listed: Vec<Listed> = match &plan.input {
        Input::Listed {
            source,
            query,
            files,
        } => remote_listed(source, query, files.clone()),
        Input::Remote { source, query } => match source.list(query, deps.fetcher.as_ref()).await {
            Ok(files) => remote_listed(source, query, files),
            Err(e) => {
                let d = e.diagnostic().with_request(request.clone());
                emit(&events, Event::Diagnostic(Box::new(d.clone())));
                let code = if d.code == "net.offline" {
                    EXIT_OFFLINE
                } else {
                    EXIT_FAILURE
                };
                report.emit(d);
                return finish(
                    deps,
                    run_id,
                    report,
                    summary,
                    vec![],
                    code,
                    started,
                    &events,
                )
                .await;
            }
        },
        Input::Archive { source, query } => match source.archived(deps.fetcher.archive(), query) {
            Ok(files) => files
                .into_iter()
                .map(|l| Listed {
                    kit: kit_of(&deps.kits, source.id()).map_or("none", |k| k.id),
                    picks: None,
                    remote: l.remote.clone(),
                    path: Some(l.path.clone()),
                    known: None,
                    archived: Some(l),
                })
                .collect(),
            Err(e) => {
                let d = e.diagnostic().with_request(request.clone());
                emit(&events, Event::Diagnostic(Box::new(d.clone())));
                report.emit(d);
                Vec::new()
            }
        },
        Input::Local(_) | Input::LocalIn { .. } => {
            let (paths, kit) = match &plan.input {
                Input::LocalIn { kit, paths } => (paths, Some(kit.as_str())),
                Input::Local(paths) => (paths, None),
                _ => unreachable!("matched above"),
            };
            let mut out = Vec::new();
            for p in paths {
                let found = match kit {
                    Some(k) => local_remote_in(&deps.kits, k, p),
                    None => local_remote(&deps.kits, p),
                };
                match found {
                    Some((kit, remote)) => out.push(Listed {
                        kit,
                        picks: None,
                        remote,
                        path: Some(p.clone()),
                        known: None,
                        archived: None,
                    }),
                    None => {
                        let d = Diagnostic::error(
                            "config.bad_path",
                            Category::CONFIG,
                            match kit {
                                Some(k) => format!(
                                    "The path {} is not a file the {k} kit reads.",
                                    p.display()
                                ),
                                None => format!(
                                    "The path {} is not a file any kit of phasor reads.",
                                    p.display()
                                ),
                            },
                        )
                        .with_file(p.display().to_string())
                        .with_request(request.clone());
                        emit(&events, Event::Diagnostic(Box::new(d.clone())));
                        report.emit(d);
                    }
                }
            }
            out
        }
        Input::Stale => {
            let files = match deps.manifest.list_files().await {
                Ok(files) => files,
                Err(e) => {
                    let d = e.diagnostic().with_request(request.clone());
                    emit(&events, Event::Diagnostic(Box::new(d.clone())));
                    report.emit(d);
                    vec![]
                }
            };
            let mut out = Vec::new();
            for rec in files {
                // A source wholly in the lake by the current rules is not even hashed.
                let shapes = deps
                    .manifest
                    .ingested_shapes(&rec.url, &rec.sha256)
                    .await
                    .ok()
                    .flatten();
                let Some((kit, remote)) = recorded_remote(&deps.kits, &rec) else {
                    continue;
                };
                let current = deps
                    .kits
                    .kit(kit)
                    .zip(shapes.as_deref())
                    .is_some_and(|(k, s)| k.catalog.plans_current(&transforms(s)));
                if current {
                    continue;
                }
                {
                    out.push(Listed {
                        kit,
                        picks: None,
                        remote,
                        path: Some(std::path::PathBuf::from(&rec.cache_path)),
                        known: Some(rec),
                        archived: None,
                    });
                }
            }
            out
        }
    };
    summary.listed = listed.len();
    emit(
        &events,
        Event::Listed {
            files: listed.len(),
        },
    );
    let remotes: Vec<RemoteFile> = listed.iter().map(|l| l.remote.clone()).collect();
    if plan.options.dry_run {
        return finish(deps, run_id, report, summary, remotes, 0, started, &events).await;
    }

    // 2. Fetch, bounded.
    let fetcher = deps.fetcher.clone();
    let req = request.clone();
    let fetched = futures::stream::iter(listed)
        .map(
            |Listed {
                 kit,
                 remote,
                 path,
                 known,
                 archived,
                 picks,
             }| {
            let fetcher = fetcher.clone();
            let req = req.clone();
            async move {
                if let Some(l) = archived {
                    return vec![Fetched::Ok(kit, l)];
                }
                if let Some((source, query)) = picks {
                    let pick = |entries: &[phasor_kit::Entry]| source.pick(&remote, entries, &query);
                    return match fetcher.fetch_picks(&remote, &pick).await {
                        Ok(picked) => picked
                            .files()
                            .into_iter()
                            .map(|l| Fetched::Ok(kit, l))
                            .collect(),
                        Err(e) => vec![Fetched::Failed(
                            remote.clone(),
                            Box::new(e.diagnostic().with_request(req)),
                        )],
                    };
                }
                vec![match path {
                    Some(p) => match (hash_local(remote.clone(), p.clone()).await, known) {
                        // Keep what the server said of a recorded source.
                        (Ok(l), Some(known)) => Fetched::Ok(kit, LocalFile {
                            etag: known.etag,
                            last_modified: known.last_modified,
                            last_modified_at: known
                                .last_modified_at
                                .as_deref()
                                .and_then(phasor_core::timetext::parse_rfc3339),
                            ..l
                        }),
                        (Ok(l), None) => Fetched::Ok(kit, l),
                        (Err(e), Some(_)) => Fetched::Failed(
                            remote.clone(),
                            Box::new(
                                Diagnostic::error(
                                    "cache.source_missing",
                                    Category::CACHE,
                                    format!(
                                        "{} was ingested from {}, which can no longer be read, so it cannot be read again from disk. Fetch or ingest it again.",
                                        remote.url,
                                        p.display()
                                    ),
                                )
                                .with_file(p.display().to_string())
                                .with_request(req)
                                .caused_by(&e),
                            ),
                        ),
                        (Err(e), None) => Fetched::Failed(
                            remote.clone(),
                            Box::new(
                                Diagnostic::error(
                                    "csv.read_failed",
                                    Category::CSV,
                                    format!("The file {} could not be read.", p.display()),
                                )
                                .with_file(p.display().to_string())
                                .with_request(req)
                                .caused_by(&e),
                            ),
                        ),
                    },
                    None => match fetcher.fetch(&remote).await {
                        Ok(l) => Fetched::Ok(kit, l),
                        Err(e) => Fetched::Failed(
                            remote.clone(),
                            Box::new(e.diagnostic().with_request(req)),
                        ),
                    },
                }]
            }
        })
        .buffer_unordered(plan.options.downloads.max(1))
        .flat_map(futures::stream::iter)
        .take_until(cancel.cancelled());

    // 3. Ingest on blocking workers, bounded.
    let kits = deps.kits.clone();
    let home = deps.home.clone();
    let options = plan.options.clone();
    let manifest = deps.manifest.clone();
    let ev = events.clone();
    let req2 = request.clone();
    let ingested = fetched
        .map(|f| {
            let kits = kits.clone();
            let home = home.clone();
            let options = options.clone();
            let manifest = manifest.clone();
            let ev = ev.clone();
            let req = req2.clone();
            async move {
                match f {
                    Fetched::Failed(remote, d) => Step::FetchFailed(remote, d),
                    Fetched::Ok(kit, local) => {
                        emit(
                            &ev,
                            Event::DownloadDone {
                                url: local.remote.url.to_string(),
                                cached: local.cached,
                                bytes: local.size,
                            },
                        );
                        if !options.ingest {
                            return Step::Downloaded(Box::new(local));
                        }
                        // An entry whose zip was already read whole is in the lake.
                        let url = local.remote.url.to_string();
                        if let Some((zip, entry)) = url.split_once('!')
                            && matches!(manifest.is_ingested_from(zip, entry).await, Ok(true))
                        {
                            return Step::Skipped(Box::new(local));
                        }
                        // Entries of this zip fetched alone before are left alone.
                        let skip = manifest
                            .entries_ingested_under(&url)
                            .await
                            .unwrap_or_default();
                        let mut rules_changed = false;
                        match manifest
                            .ingested_shapes(local.remote.url.as_str(), &local.sha256)
                            .await
                        {
                            Ok(Some(shapes))
                                if kits.kit(kit).is_some_and(|k| {
                                    k.catalog.plans_current(&transforms(&shapes))
                                }) =>
                            {
                                return Step::Skipped(Box::new(local));
                            }
                            Ok(Some(_)) => rules_changed = true,
                            Ok(None) => {}
                            Err(e) => {
                                return Step::FetchFailed(
                                    local.remote.clone(),
                                    Box::new(e.diagnostic().with_request(req)),
                                );
                            }
                        }
                        emit(
                            &ev,
                            Event::IngestStarted {
                                source: local.remote.url.to_string(),
                            },
                        );
                        let input = IngestInput {
                            local,
                            kits,
                            kit,
                            lake_root: home.lake(),
                            quarantine_root: home.quarantine(),
                            tmp_dir: home.tmp(),
                            options,
                            request: req.clone(),
                            events: Some(ev.clone()),
                            rules_changed,
                            skip,
                        };
                        let ingest = move || {
                            let rep = ingest_file(input);
                            // Merge the worker's zones for zonetrace::take_all.
                            #[cfg(feature = "zones")]
                            zonetrace::flush();
                            rep
                        };
                        match tokio::task::spawn_blocking(ingest).await {
                            Ok(rep) => Step::Ingested(Box::new(rep)),
                            Err(e) => Step::Panicked(req, e.to_string()),
                        }
                    }
                }
            }
        })
        .buffer_unordered(plan.options.jobs.max(1));
    let mut ingested = Box::pin(ingested);
    // Whether a failure stopped the run, which cancels it as Ctrl-C does.
    let mut stopped = false;

    // 4. Commit each as it finishes.
    while let Some(step) = ingested.next().await {
        match step {
            Step::FetchFailed(remote, d) => {
                let d = (*d).with_url(remote.url.to_string());
                emit(&events, Event::Diagnostic(Box::new(d.clone())));
                if let Some(id) = run_id {
                    let _ = deps.manifest.record_diagnostic(id, None, d.clone()).await;
                }
                summary.failed += 1;
                summary.errors += 1;
                let outcome = SourceOutcome {
                    source: remote.url.to_string(),
                    status: SourceStatus::Failed,
                    failed_with: Some(d.code.clone()),
                    errors: 1,
                    warnings: 0,
                    rows: 0,
                    bad_rows: 0,
                };
                emit(&events, Event::SourceDone(Box::new(outcome.clone())));
                report.finish_source(outcome);
                if report.emit(d) == Flow::Stop {
                    stopped = true;
                    cancel.cancel();
                }
            }
            Step::Skipped(local) => {
                summary.skipped += 1;
                if local.cached {
                    summary.cached += 1;
                }
                let outcome = SourceOutcome {
                    source: local.remote.url.to_string(),
                    status: SourceStatus::Skipped,
                    failed_with: None,
                    errors: 0,
                    warnings: 0,
                    rows: 0,
                    bad_rows: 0,
                };
                emit(&events, Event::SourceDone(Box::new(outcome.clone())));
                report.finish_source(outcome);
            }
            Step::Downloaded(local) => {
                if local.cached {
                    summary.cached += 1;
                } else {
                    summary.downloaded += 1;
                }
                let outcome = SourceOutcome {
                    source: local.remote.url.to_string(),
                    status: SourceStatus::Ok,
                    failed_with: None,
                    errors: 0,
                    warnings: 0,
                    rows: 0,
                    bad_rows: 0,
                };
                emit(&events, Event::SourceDone(Box::new(outcome.clone())));
                report.finish_source(outcome);
            }
            Step::Panicked(req, msg) => {
                let d = Diagnostic::error(
                    "internal.panic",
                    Category::INTERNAL,
                    format!("A parse worker panicked, which is a bug in phasor: {msg}"),
                )
                .with_request(req);
                emit(&events, Event::Diagnostic(Box::new(d.clone())));
                summary.failed += 1;
                summary.errors += 1;
                report.finish_source(SourceOutcome {
                    source: "worker".into(),
                    status: SourceStatus::Failed,
                    failed_with: Some(d.code.clone()),
                    errors: 1,
                    warnings: 0,
                    rows: 0,
                    bad_rows: 0,
                });
                if report.emit(d) == Flow::Stop {
                    stopped = true;
                    cancel.cancel();
                }
            }
            Step::Ingested(rep) => {
                let stop = commit(&deps, run_id, *rep, &mut report, &mut summary, &events).await;
                if stop {
                    stopped = true;
                    cancel.cancel();
                }
            }
        }
    }
    // Cancelled, and not by a failure that stops the run: the user's Ctrl-C.
    let interrupted = cancel.is_cancelled() && !stopped;
    summary.interrupted = interrupted;
    let code = if interrupted {
        EXIT_INTERRUPTED
    } else {
        exit_code(&report)
    };
    finish(
        deps, run_id, report, summary, remotes, code, started, &events,
    )
    .await
}

enum Step {
    FetchFailed(RemoteFile, Box<Diagnostic>),
    Downloaded(Box<LocalFile>),
    Skipped(Box<LocalFile>),
    Ingested(Box<FileReport>),
    Panicked(String, String),
}

/// Records one file's results; returns whether the run should stop.
async fn commit(
    deps: &Deps,
    run_id: Option<i64>,
    rep: FileReport,
    report: &mut Report,
    summary: &mut Summary,
    events: &mpsc::Sender<Event>,
) -> bool {
    let local = &rep.local;
    if local.cached {
        summary.cached += 1;
    } else {
        summary.downloaded += 1;
    }
    summary.bytes_in += local.size;
    let mut stop = false;
    // The refusal is the entry's own error, not the discard it caused.
    let refused = rep
        .entries
        .iter()
        .filter_map(|e| e.error.as_ref())
        .min_by_key(|d| d.code.as_ref() == "store.file_discarded")
        .map(|d| d.code.to_string());
    let committed = rep.file_error.is_none() && refused.is_none();
    let previous = deps
        .manifest
        .file_by_url(local.remote.url.as_str())
        .await
        .ok()
        .flatten();
    let file_id = match deps
        .manifest
        .record_file(FileRecord {
            source: local.remote.source.to_owned(),
            url: local.remote.url.to_string(),
            cache_path: local.path.display().to_string(),
            // A failed read keeps the earlier hash, so a later success sees the change.
            sha256: match (&previous, committed) {
                (Some((_, prev)), false) => prev.sha256.clone(),
                _ => local.sha256.clone(),
            },
            size: local.size,
            etag: local.etag.clone(),
            last_modified: local.last_modified.clone(),
            last_modified_at: local.last_modified_at.map(|s| s.rfc3339()),
            fetched_at: local.fetched_at.map(|s| s.rfc3339()),
        })
        .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            let d = e.diagnostic();
            emit(events, Event::Diagnostic(Box::new(d.clone())));
            stop |= report.emit(d) == Flow::Stop;
            None
        }
    };
    // A changed source or rules: a committed read replaces the earlier read's files.
    let source_changed = previous
        .as_ref()
        .is_some_and(|(_, prev)| prev.sha256 != local.sha256);
    if let (Some(fid), Some(run), true) = (
        file_id,
        run_id,
        committed && previous.is_some() && (source_changed || rep.rules_changed),
    ) {
        // Files written again under the same names are already the new ones.
        let kept: Vec<String> = rep
            .results
            .iter()
            .flat_map(|r| {
                r.outputs
                    .iter()
                    .map(|o| o.path.clone())
                    .chain(r.quarantine_path.clone())
            })
            .collect();
        let replaced = deps.manifest.supersede(fid, kept).await.unwrap_or_default();
        let d = if source_changed {
            Diagnostic::warning(
                "manifest.superseded",
                Category::MANIFEST,
                format!(
                    "{} changed since it was last ingested; {} earlier Parquet file(s) were replaced.",
                    local.remote.url,
                    replaced
                ),
            )
        } else {
            Diagnostic::warning(
                "manifest.rules_changed",
                Category::MANIFEST,
                format!(
                    "The rules {} was ingested by have changed, so it was read again from {}; {} earlier Parquet file(s) were replaced.",
                    local.remote.url,
                    local.path.display(),
                    replaced
                ),
            )
        }
        .with_url(local.remote.url.to_string());
        let _ = deps.manifest.record_diagnostic(run, None, d.clone()).await;
        emit(events, Event::Diagnostic(Box::new(d.clone())));
        summary.warnings += 1;
        report.emit(d);
    }
    if !rep.left.is_empty() {
        let d = Diagnostic::warning(
            "manifest.already_ingested",
            Category::MANIFEST,
            format!(
                "{} entries of {} were fetched alone and ingested before; the read of the whole file left them as they are: {}",
                rep.left.len(),
                local.remote.url,
                rep.left.join(", ")
            ),
        )
        .with_url(local.remote.url.to_string());
        if let Some(run) = run_id {
            let _ = deps.manifest.record_diagnostic(run, None, d.clone()).await;
        }
        emit(events, Event::Diagnostic(Box::new(d.clone())));
        summary.warnings += 1;
        report.emit(d);
    }
    if let Some(d) = &rep.file_error {
        if let Some(run) = run_id {
            let _ = deps.manifest.record_diagnostic(run, None, d.clone()).await;
        }
        emit(events, Event::Diagnostic(Box::new(d.clone())));
        summary.failed += 1;
        summary.errors += 1;
        let outcome = SourceOutcome {
            source: local.remote.url.to_string(),
            status: SourceStatus::Failed,
            failed_with: Some(d.code.clone()),
            errors: 1,
            warnings: 0,
            rows: 0,
            bad_rows: 0,
        };
        emit(events, Event::SourceDone(Box::new(outcome.clone())));
        report.finish_source(outcome);
        stop |= report.emit(d.clone()) == Flow::Stop;
        return stop;
    }
    // One ingest per shape; failed, without files, when an entry failed.
    let mut first_ingest_id = None;
    // A failed re-read leaves the earlier read's ingests on record.
    let record = committed || previous.is_none();
    // Each entry's rows over its shapes, before the results are handed on.
    let totals: Vec<(u64, u64)> = rep
        .entries
        .iter()
        .map(|entry| {
            rep.results
                .iter()
                .flat_map(|r| &r.entries)
                .filter(|s| s.entry == entry.entry)
                .fold((0, 0), |(r, b), s| (r + s.rows, b + s.bad_rows))
        })
        .collect();
    for r in rep.results {
        for o in &r.outputs {
            summary.bytes_out += o.bytes;
        }
        if let (Some(fid), Some(run), true) = (file_id, run_id, record) {
            let start = IngestStart {
                run_id: run,
                file_id: fid,
                table: r.table,
                schema_version: r.version,
                fingerprint: r.fingerprint.to_string(),
                transform: r.transform,
            };
            match deps.manifest.begin_ingest(start).await {
                Ok(iid) => {
                    first_ingest_id.get_or_insert(iid);
                    let end = IngestEnd {
                        status: if refused.is_some() {
                            IngestStatus::Failed
                        } else {
                            IngestStatus::Ok
                        },
                        rows: r.rows,
                        bad_rows: r.bad_rows,
                        quarantine_path: r.quarantine_path,
                        outputs: r.outputs,
                        entries: r.entries,
                        error_code: refused.clone(),
                    };
                    if let Err(e) = deps.manifest.finish_ingest(iid, end).await {
                        report.emit(e.diagnostic());
                    }
                }
                Err(e) => {
                    report.emit(e.diagnostic());
                }
            }
        }
    }
    for (entry, (rows, bad_rows)) in rep.entries.into_iter().zip(totals) {
        let source_name = format!("{}!{}", local.remote.url, entry.entry);
        let mut errors = 0u32;
        // Row errors first, so the fault is on record before its refusal.
        for w in entry.row_errors.iter().chain(&entry.warnings) {
            if let Some(run) = run_id {
                let _ = deps
                    .manifest
                    .record_diagnostic(run, first_ingest_id, w.clone())
                    .await;
            }
            emit(events, Event::Diagnostic(Box::new(w.clone())));
            if w.is_error() {
                summary.errors += 1;
            } else {
                summary.warnings += 1;
            }
            // A row error never stops a run by itself.
            let _ = report.emit(w.clone());
        }
        summary.rows += rows;
        summary.bad_rows += bad_rows;
        let failed_with = entry.error.as_ref().map(|d| d.code.clone());
        if let Some(d) = &entry.error {
            errors += 1;
            if let Some(run) = run_id {
                let _ = deps
                    .manifest
                    .record_diagnostic(run, first_ingest_id, d.clone())
                    .await;
            }
            emit(events, Event::Diagnostic(Box::new(d.clone())));
            summary.failed += 1;
            summary.errors += 1;
            stop |= report.emit(d.clone()) == Flow::Stop;
        } else {
            summary.ingested += 1;
        }
        let outcome = SourceOutcome {
            source: source_name,
            status: if entry.error.is_some() {
                SourceStatus::Failed
            } else {
                SourceStatus::Ok
            },
            failed_with,
            errors,
            warnings: entry.warnings.len() as u32,
            rows,
            bad_rows,
        };
        emit(events, Event::SourceDone(Box::new(outcome.clone())));
        report.finish_source(outcome);
    }
    stop
}

#[allow(clippy::too_many_arguments)]
async fn finish(
    deps: Deps,
    run_id: Option<i64>,
    report: Report,
    mut summary: Summary,
    listed: Vec<RemoteFile>,
    exit_code: i32,
    started: Instant,
    events: &mpsc::Sender<Event>,
) -> RunReport {
    summary.elapsed_ms = started.elapsed().as_millis() as u64;
    if let Some(id) = run_id {
        let _ = deps
            .manifest
            .finish_run(
                id,
                exit_code,
                &serde_json::to_string(&summary).unwrap_or_default(),
            )
            .await;
    }
    emit(events, Event::Summary(Box::new(summary.clone())));
    RunReport {
        run_id,
        summary,
        report,
        listed,
        exit_code,
    }
}
