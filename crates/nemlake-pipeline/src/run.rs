//! The run: list, fetch, ingest, commit.

use std::time::Instant;

use futures::StreamExt;
use nemlake_core::Diagnose;
use nemlake_core::event::{Event, Summary};
use nemlake_core::report::{DiagSink, Flow, Report, SourceOutcome, SourceStatus};
use nemlake_core::{Category, Diagnostic};
use nemlake_fetch::cache::sha256_hex;
use nemlake_fetch::{FileKind, LocalFile, RemoteFile};
use nemlake_store::manifest::{FileRecord, IngestEnd, IngestStart, IngestStatus, SchemaSeen};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::ingest_file::{FileReport, IngestInput, ingest_file};
use crate::plan::{Deps, Input, Plan};
use crate::report::{EXIT_FAILURE, EXIT_OFFLINE, RunReport, exit_code};

fn emit(tx: &mpsc::Sender<Event>, e: Event) {
    let _ = tx.try_send(e);
}

/// A local path as a `RemoteFile` with a `file://` URL.
pub fn local_remote(path: &std::path::Path) -> Option<RemoteFile> {
    let abs = std::path::absolute(path).ok()?;
    let url = Url::from_file_path(&abs).ok()?;
    let kind = if abs
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
    {
        FileKind::PlainCsv
    } else {
        FileKind::MmsZip
    };
    let stem = abs.file_stem()?.to_string_lossy().to_ascii_uppercase();
    let period = crate::ingest_file::period_from_stem(&stem);
    let table = nemlake_fetch::MmsdmArchive::parse_name(&abs.file_name()?.to_string_lossy())
        .map(|(t, _)| t);
    Some(RemoteFile {
        url,
        source: "local",
        table,
        period,
        kind,
        size: std::fs::metadata(&abs).ok().map(|m| m.len()),
        modified: None,
    })
}

/// Hashes a local file into a `LocalFile`.
pub fn local_file(remote: RemoteFile, path: &std::path::Path) -> std::io::Result<LocalFile> {
    let bytes = std::fs::read(path)?;
    Ok(LocalFile {
        remote,
        path: path.to_path_buf(),
        sha256: sha256_hex(&bytes),
        size: bytes.len() as u64,
        cached: true,
        etag: None,
        last_modified: None,
    })
}

enum Fetched {
    Ok(LocalFile),
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

    let run_id = match deps.manifest.begin_run(&request).await {
        Ok(id) => Some(id),
        Err(e) => {
            let d = e.diagnostic().with_request(request.clone());
            emit(&events, Event::Diagnostic(Box::new(d.clone())));
            report.emit(d);
            None
        }
    };
    if let Err(e) = deps.manifest.abort_running().await {
        report.emit(e.diagnostic());
    }
    let _ = nemlake_store::parquet_hive::sweep_tmp(&deps.home.lake());

    // 1. List.
    let listed: Vec<(RemoteFile, Option<std::path::PathBuf>)> = match &plan.input {
        Input::Remote { source, query } => match source.list(query, &deps.fetcher).await {
            Ok(files) => files.into_iter().map(|f| (f, None)).collect(),
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
        Input::Local(paths) => {
            let mut out = Vec::new();
            for p in paths {
                match local_remote(p) {
                    Some(r) => out.push((r, Some(p.clone()))),
                    None => {
                        let d = Diagnostic::error(
                            "config.bad_path",
                            Category::Config,
                            format!("The path {} is not a file nemlake can ingest.", p.display()),
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
    };
    summary.listed = listed.len();
    emit(
        &events,
        Event::Listed {
            files: listed.len(),
        },
    );
    let remotes: Vec<RemoteFile> = listed.iter().map(|(r, _)| r.clone()).collect();
    if plan.options.dry_run {
        return finish(deps, run_id, report, summary, remotes, 0, started, &events).await;
    }

    // 2. Fetch, bounded.
    let fetcher = deps.fetcher.clone();
    let req = request.clone();
    let fetched = futures::stream::iter(listed)
        .map(|(remote, local_path)| {
            let fetcher = fetcher.clone();
            let req = req.clone();
            async move {
                match local_path {
                    Some(p) => match local_file(remote.clone(), &p) {
                        Ok(l) => Fetched::Ok(l),
                        Err(e) => Fetched::Failed(
                            remote.clone(),
                            Box::new(
                                Diagnostic::error(
                                    "csv.read_failed",
                                    Category::Csv,
                                    format!("The file {} could not be read.", p.display()),
                                )
                                .with_file(p.display().to_string())
                                .with_request(req)
                                .caused_by(&e),
                            ),
                        ),
                    },
                    None => match fetcher.fetch(&remote).await {
                        Ok(l) => Fetched::Ok(l),
                        Err(e) => Fetched::Failed(
                            remote.clone(),
                            Box::new(e.diagnostic().with_request(req)),
                        ),
                    },
                }
            }
        })
        .buffer_unordered(plan.options.downloads.max(1))
        .take_until(cancel.cancelled());

    // 3. Ingest on blocking workers, bounded.
    let registry = deps.registry.clone();
    let home = deps.home.clone();
    let options = plan.options.clone();
    let manifest = deps.manifest.clone();
    let ev = events.clone();
    let req2 = request.clone();
    let ingested = fetched
        .map(|f| {
            let registry = registry.clone();
            let home = home.clone();
            let options = options.clone();
            let manifest = manifest.clone();
            let ev = ev.clone();
            let req = req2.clone();
            async move {
                match f {
                    Fetched::Failed(remote, d) => Step::FetchFailed(remote, d),
                    Fetched::Ok(local) => {
                        emit(
                            &ev,
                            Event::DownloadDone {
                                url: local.remote.url.to_string(),
                                cached: local.cached,
                                bytes: local.size,
                            },
                        );
                        match manifest
                            .is_ingested(local.remote.url.as_str(), &local.sha256)
                            .await
                        {
                            Ok(true) => return Step::Skipped(Box::new(local)),
                            Ok(false) => {}
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
                            registry,
                            lake_root: home.lake(),
                            quarantine_root: home.quarantine(),
                            tmp_dir: home.tmp(),
                            options,
                            request: req.clone(),
                            events: Some(ev.clone()),
                        };
                        match tokio::task::spawn_blocking(move || ingest_file(input)).await {
                            Ok(rep) => Step::Ingested(Box::new(rep)),
                            Err(e) => Step::Panicked(req, e.to_string()),
                        }
                    }
                }
            }
        })
        .buffer_unordered(plan.options.jobs.max(1));
    let mut ingested = Box::pin(ingested);

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
            Step::Panicked(req, msg) => {
                let d = Diagnostic::error(
                    "internal.panic",
                    Category::Internal,
                    format!("A parse worker panicked, which is a bug in nemlake: {msg}"),
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
                    cancel.cancel();
                }
            }
            Step::Ingested(rep) => {
                let stop = commit(&deps, run_id, *rep, &mut report, &mut summary, &events).await;
                if stop {
                    cancel.cancel();
                }
            }
        }
    }
    let code = exit_code(&report);
    finish(
        deps, run_id, report, summary, remotes, code, started, &events,
    )
    .await
}

enum Step {
    FetchFailed(RemoteFile, Box<Diagnostic>),
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
    // What the manifest knew before this file is recorded, to detect a change.
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
            sha256: local.sha256.clone(),
            size: local.size,
            etag: local.etag.clone(),
            last_modified: local.last_modified.clone(),
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
    // A changed source: forget the previous outputs.
    if let (Some(fid), Some(run), Some((_, prev))) = (file_id, run_id, previous)
        && prev.sha256 != local.sha256
    {
        let old = deps.manifest.outputs_of_file(fid).await.unwrap_or_default();
        for p in &old {
            let _ = std::fs::remove_file(p);
        }
        let _ = deps.manifest.supersede_file(fid).await;
        let d = Diagnostic::warning(
            "manifest.superseded",
            Category::Manifest,
            format!(
                "{} changed since it was last ingested; {} earlier Parquet file(s) were replaced.",
                local.remote.url,
                old.len()
            ),
        )
        .with_url(local.remote.url.to_string());
        let _ = deps.manifest.record_diagnostic(run, None, d.clone()).await;
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
    for entry in rep.entries {
        let source_name = format!("{}!{}", local.remote.url, entry.entry);
        let mut errors = 0u32;
        let mut rows = 0u64;
        let mut bad_rows = 0u64;
        let status = if entry.error.is_some() {
            IngestStatus::Failed
        } else {
            IngestStatus::Ok
        };
        let mut first_ingest_id = None;
        for r in &entry.results {
            rows += r.rows;
            bad_rows += r.bad_rows;
            if let (Some(fid), Some(run)) = (file_id, run_id) {
                let start = IngestStart {
                    run_id: run,
                    file_id: fid,
                    entry: entry.entry.clone(),
                    table: r.table.clone(),
                    schema_version: r.version,
                    fingerprint: r.fingerprint.to_string(),
                };
                match deps.manifest.begin_ingest(start).await {
                    Ok(iid) => {
                        first_ingest_id.get_or_insert(iid);
                        let end = IngestEnd {
                            status,
                            rows: r.rows,
                            bad_rows: r.bad_rows,
                            quarantine_path: r.quarantine_path.clone(),
                            outputs: r.outputs.clone(),
                            error_code: entry.error.as_ref().map(|d| d.code.to_string()),
                        };
                        if let Err(e) = deps.manifest.finish_ingest(iid, end).await {
                            report.emit(e.diagnostic());
                        }
                    }
                    Err(e) => {
                        report.emit(e.diagnostic());
                    }
                }
                let _ = deps
                    .manifest
                    .record_schema_seen(SchemaSeen {
                        fingerprint: r.fingerprint.to_string(),
                        table: Some(r.table.clone()),
                        report_type: r.report_type.clone(),
                        report_subtype: r.report_subtype.clone(),
                        version: r.observed_version,
                        columns: r.columns.clone(),
                        status: r.status.to_owned(),
                        file_id: Some(fid),
                    })
                    .await;
            }
            for o in &r.outputs {
                summary.bytes_out += o.bytes;
            }
        }
        for w in &entry.warnings {
            if let Some(run) = run_id {
                let _ = deps
                    .manifest
                    .record_diagnostic(run, first_ingest_id, w.clone())
                    .await;
            }
            emit(events, Event::Diagnostic(Box::new(w.clone())));
            report.emit(w.clone());
        }
        summary.warnings += entry.warnings.len() as u64;
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
