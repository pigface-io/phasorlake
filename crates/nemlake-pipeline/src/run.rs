//! The run: list, fetch, ingest, commit.

use std::time::Instant;

use futures::StreamExt;
use nemlake_core::Diagnose;
use nemlake_core::event::{Event, Summary};
use nemlake_core::report::{DiagSink, Flow, Report, SourceOutcome, SourceStatus};
use nemlake_core::{Category, Diagnostic};
use nemlake_fetch::cache::sha256_hex;
use nemlake_fetch::{FileKind, LocalFile, RemoteFile};
use nemlake_schema::{Mode, Registry, ResolveOptions};
use nemlake_store::manifest::{
    FileRecord, IngestEnd, IngestShape, IngestStart, IngestStatus, SchemaSeen,
};
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
        member: None,
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

/// A source the manifest has recorded, as the file to read it from again.
/// `None` when its URL does not parse or its path names no file.
pub fn recorded_remote(rec: &FileRecord) -> Option<RemoteFile> {
    let mut remote = local_remote(std::path::Path::new(&rec.cache_path))?;
    remote.url = Url::parse(&rec.url).ok()?;
    remote.source = match rec.source.as_str() {
        nemlake_fetch::mmsdm::SOURCE_ID => nemlake_fetch::mmsdm::SOURCE_ID,
        nemlake_fetch::wem::SOURCE_ID => nemlake_fetch::wem::SOURCE_ID,
        _ => "local",
    };
    Some(remote)
}

/// Whether every shape that was read from a source would be read by the
/// same rules today. The answer comes from the recorded shapes alone; the
/// source is not opened.
///
/// Strictness is not part of the question: a shape the registry would
/// refuse in strict mode is current when the permissive rules that wrote it
/// have not changed.
pub fn rules_current(registry: &Registry, shapes: &[IngestShape]) -> bool {
    shapes.iter().all(|s| {
        let t = &s.transform;
        if t.rules.is_empty() {
            return false;
        }
        let columns: Vec<&str> = t.source_columns.iter().map(String::as_str).collect();
        let irow = nemlake_core::IRow {
            report_type: &t.report_type,
            report_subtype: &t.report_subtype,
            version: t.source_version,
            columns: &columns,
        };
        registry
            .resolve(
                &irow,
                ResolveOptions {
                    mode: Mode::Permissive,
                },
            )
            .is_ok_and(|r| r.rules().to_string() == t.rules)
    })
}

/// A file to fetch or read: where it is, and for a file already on disk
/// its path and what the manifest knows of it.
struct Listed {
    remote: RemoteFile,
    path: Option<std::path::PathBuf>,
    known: Option<FileRecord>,
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
    let listed: Vec<Listed> = match &plan.input {
        Input::Remote { source, query } => match source.list(query, &deps.fetcher).await {
            Ok(files) => files
                .into_iter()
                .map(|remote| Listed {
                    remote,
                    path: None,
                    known: None,
                })
                .collect(),
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
                    Some(remote) => out.push(Listed {
                        remote,
                        path: Some(p.clone()),
                        known: None,
                    }),
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
                // Decided from the manifest alone: a source that is wholly
                // in the lake by the current rules is not even hashed.
                let shapes = deps
                    .manifest
                    .ingested_shapes(&rec.url, &rec.sha256)
                    .await
                    .ok()
                    .flatten();
                if shapes.is_some_and(|s| rules_current(&deps.registry, &s)) {
                    continue;
                }
                if let Some(remote) = recorded_remote(&rec) {
                    out.push(Listed {
                        remote,
                        path: Some(std::path::PathBuf::from(&rec.cache_path)),
                        known: Some(rec),
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
        .map(|Listed { remote, path, known }| {
            let fetcher = fetcher.clone();
            let req = req.clone();
            async move {
                match path {
                    Some(p) => match (local_file(remote.clone(), &p), known) {
                        // What the server said of a recorded source stays
                        // with it when it is read again.
                        (Ok(l), Some(known)) => Fetched::Ok(LocalFile {
                            etag: known.etag,
                            last_modified: known.last_modified,
                            ..l
                        }),
                        (Ok(l), None) => Fetched::Ok(l),
                        (Err(e), Some(_)) => Fetched::Failed(
                            remote.clone(),
                            Box::new(
                                Diagnostic::error(
                                    "cache.source_missing",
                                    Category::Cache,
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
                        // In the lake already, by rules that still hold:
                        // nothing to do. By rules that have changed: read
                        // it again.
                        let mut rules_changed = false;
                        match manifest
                            .ingested_shapes(local.remote.url.as_str(), &local.sha256)
                            .await
                        {
                            Ok(Some(shapes)) if rules_current(&registry, &shapes) => {
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
                            registry,
                            lake_root: home.lake(),
                            quarantine_root: home.quarantine(),
                            tmp_dir: home.tmp(),
                            options,
                            request: req.clone(),
                            events: Some(ev.clone()),
                            rules_changed,
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

/// Removes a Parquet file an earlier read wrote, and each directory above
/// it that is left empty, up to the lake's root.
fn remove_output(path: &std::path::Path, lake: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let mut dir = path.parent();
    while let Some(d) = dir {
        // `remove_dir` refuses a directory that still holds something.
        if d == lake || !d.starts_with(lake) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
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
    // A changed source, or changed rules: what the earlier read wrote is
    // replaced by what this one wrote. A file this read wrote again under
    // the same name is already the new one and stays.
    let source_changed = previous
        .as_ref()
        .is_some_and(|(_, prev)| prev.sha256 != local.sha256);
    if let (Some(fid), Some(run), true) = (
        file_id,
        run_id,
        previous.is_some() && (source_changed || rep.rules_changed),
    ) {
        let written: Vec<&str> = rep
            .entries
            .iter()
            .flat_map(|e| &e.results)
            .flat_map(|r| &r.outputs)
            .map(|o| o.path.as_str())
            .collect();
        let old = deps.manifest.outputs_of_file(fid).await.unwrap_or_default();
        let lake = deps.home.lake();
        for p in old.iter().filter(|p| !written.contains(&p.as_str())) {
            remove_output(std::path::Path::new(p), &lake);
        }
        let _ = deps.manifest.supersede_file(fid).await;
        let d = if source_changed {
            Diagnostic::warning(
                "manifest.superseded",
                Category::Manifest,
                format!(
                    "{} changed since it was last ingested; {} earlier Parquet file(s) were replaced.",
                    local.remote.url,
                    old.len()
                ),
            )
        } else {
            Diagnostic::warning(
                "manifest.rules_changed",
                Category::Manifest,
                format!(
                    "The rules {} was ingested by have changed, so it was read again from {}; {} earlier Parquet file(s) were replaced.",
                    local.remote.url,
                    local.path.display(),
                    old.len()
                ),
            )
        }
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
                    transform: r.transform.clone(),
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
                        report_type: r.transform.report_type.clone(),
                        report_subtype: r.transform.report_subtype.clone(),
                        version: r.transform.source_version,
                        columns: r.transform.source_columns.clone(),
                        status: r.transform.resolution.clone(),
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
