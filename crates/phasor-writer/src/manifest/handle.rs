//! The manifest as an actor: one thread owns the connection, callers await
//! replies over channels.

use std::path::Path;
use std::thread::JoinHandle;

use phasor_core::Diagnostic;
use tokio::sync::{mpsc, oneshot};

use super::{
    DiagFilter, FileRecord, IngestEnd, IngestRecord, IngestShape, IngestStart, Manifest, RunRecord,
};
use crate::error::ManifestError;

type Reply<T> = oneshot::Sender<Result<T, ManifestError>>;

enum Cmd {
    StartRun(String, Reply<i64>),
    FinishRun(i64, i32, String, Reply<()>),
    RecordFile(FileRecord, Reply<i64>),
    IngestedShapes(String, String, Reply<Option<Vec<IngestShape>>>),
    EntriesIngestedUnder(String, Reply<Vec<String>>),
    IsIngestedFrom(String, String, Reply<bool>),
    ListFiles(Reply<Vec<FileRecord>>),
    FileByUrl(String, Reply<Option<(i64, FileRecord)>>),
    LiveQuarantine(Reply<Vec<String>>),
    LiveOutputs(Option<String>, Reply<Vec<super::LiveOutput>>),
    Supersede(i64, Vec<String>, Reply<usize>),
    BeginIngest(IngestStart, Reply<i64>),
    FinishIngest(i64, IngestEnd, Reply<()>),
    RecordDiagnostic(i64, Option<i64>, Box<Diagnostic>, Reply<()>),
    ListIngests(Option<String>, bool, usize, Reply<Vec<IngestRecord>>),
    ListDiagnostics(DiagFilter, Reply<Vec<(i64, Option<i64>, Diagnostic)>>),
    LatestRun(Reply<Option<RunRecord>>),
    TablesOfRun(i64, Reply<Vec<String>>),
}

/// An async handle to the manifest thread. Cloning shares the thread.
#[derive(Clone)]
pub struct ManifestHandle {
    tx: mpsc::Sender<Cmd>,
}

/// Owns the thread. The thread stops when the last [`ManifestHandle`] is
/// dropped; dropping this detaches it, [`ManifestThread::join`] waits for it.
pub struct ManifestThread {
    join: Option<JoinHandle<()>>,
}

impl ManifestThread {
    /// Waits for the thread to finish. Every handle must be dropped first or
    /// this never returns.
    pub fn join(mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl ManifestHandle {
    /// Opens the manifest on its own thread.
    pub fn spawn(path: &Path) -> Result<(Self, ManifestThread), ManifestError> {
        let manifest = Manifest::open(path)?;
        Ok(Self::spawn_with(manifest))
    }

    /// Runs an already-open manifest on its own thread.
    pub fn spawn_with(mut manifest: Manifest) -> (Self, ManifestThread) {
        let (tx, mut rx) = mpsc::channel::<Cmd>(256);
        let join = std::thread::Builder::new()
            .name("phasor-manifest".into())
            .spawn(move || {
                while let Some(cmd) = rx.blocking_recv() {
                    match cmd {
                        Cmd::StartRun(argv, r) => {
                            let _ = r.send(manifest.start_run(&argv));
                        }
                        Cmd::FinishRun(id, code, summary, r) => {
                            let _ = r.send(manifest.finish_run(id, code, &summary));
                        }
                        Cmd::RecordFile(f, r) => {
                            let _ = r.send(manifest.record_file(&f));
                        }
                        Cmd::IngestedShapes(url, sha, r) => {
                            let _ = r.send(manifest.ingested_shapes(&url, &sha));
                        }
                        Cmd::EntriesIngestedUnder(url, r) => {
                            let _ = r.send(manifest.entries_ingested_under(&url));
                        }
                        Cmd::IsIngestedFrom(url, entry, r) => {
                            let _ = r.send(manifest.is_ingested_from(&url, &entry));
                        }
                        Cmd::ListFiles(r) => {
                            let _ = r.send(manifest.list_files());
                        }
                        Cmd::FileByUrl(url, r) => {
                            let _ = r.send(manifest.file_by_url(&url));
                        }
                        Cmd::LiveQuarantine(r) => {
                            let _ = r.send(manifest.live_quarantine());
                        }
                        Cmd::LiveOutputs(t, r) => {
                            let _ = r.send(manifest.live_outputs(t.as_deref()));
                        }
                        Cmd::Supersede(id, kept, r) => {
                            let _ = r.send(manifest.supersede(id, &kept));
                        }
                        Cmd::BeginIngest(s, r) => {
                            let _ = r.send(manifest.begin_ingest(&s));
                        }
                        Cmd::FinishIngest(id, e, r) => {
                            let _ = r.send(manifest.finish_ingest(id, &e));
                        }
                        Cmd::RecordDiagnostic(run, ingest, d, r) => {
                            let _ = r.send(manifest.record_diagnostic(run, ingest, &d));
                        }
                        Cmd::ListIngests(table, failed, limit, r) => {
                            let _ = r.send(manifest.list_ingests(table.as_deref(), failed, limit));
                        }
                        Cmd::ListDiagnostics(f, r) => {
                            let _ = r.send(manifest.list_diagnostics(&f));
                        }
                        Cmd::LatestRun(r) => {
                            let _ = r.send(manifest.latest_run());
                        }
                        Cmd::TablesOfRun(run, r) => {
                            let _ = r.send(manifest.tables_of_run(run));
                        }
                    }
                }
            })
            .expect("spawning the manifest thread");
        (Self { tx }, ManifestThread { join: Some(join) })
    }

    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, ManifestError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(make(tx))
            .await
            .map_err(|_| ManifestError::Closed)?;
        rx.await.map_err(|_| ManifestError::Closed)?
    }

    /// See [`Manifest::start_run`].
    pub async fn start_run(&self, argv: &str) -> Result<i64, ManifestError> {
        self.call(|r| Cmd::StartRun(argv.to_owned(), r)).await
    }
    /// See [`Manifest::finish_run`].
    pub async fn finish_run(
        &self,
        run_id: i64,
        exit_code: i32,
        summary_json: &str,
    ) -> Result<(), ManifestError> {
        self.call(|r| Cmd::FinishRun(run_id, exit_code, summary_json.to_owned(), r))
            .await
    }
    /// See [`Manifest::record_file`].
    pub async fn record_file(&self, f: FileRecord) -> Result<i64, ManifestError> {
        self.call(|r| Cmd::RecordFile(f, r)).await
    }
    /// See [`Manifest::ingested_shapes`].
    pub async fn ingested_shapes(
        &self,
        url: &str,
        sha256: &str,
    ) -> Result<Option<Vec<IngestShape>>, ManifestError> {
        self.call(|r| Cmd::IngestedShapes(url.to_owned(), sha256.to_owned(), r))
            .await
    }
    /// See [`Manifest::list_files`].
    pub async fn list_files(&self) -> Result<Vec<FileRecord>, ManifestError> {
        self.call(Cmd::ListFiles).await
    }
    /// See [`Manifest::entries_ingested_under`].
    pub async fn entries_ingested_under(&self, url: &str) -> Result<Vec<String>, ManifestError> {
        self.call(|r| Cmd::EntriesIngestedUnder(url.to_owned(), r))
            .await
    }
    /// See [`Manifest::is_ingested_from`].
    pub async fn is_ingested_from(&self, url: &str, entry: &str) -> Result<bool, ManifestError> {
        self.call(|r| Cmd::IsIngestedFrom(url.to_owned(), entry.to_owned(), r))
            .await
    }
    /// See [`Manifest::file_by_url`].
    pub async fn file_by_url(&self, url: &str) -> Result<Option<(i64, FileRecord)>, ManifestError> {
        self.call(|r| Cmd::FileByUrl(url.to_owned(), r)).await
    }
    /// See [`Manifest::live_quarantine`].
    pub async fn live_quarantine(&self) -> Result<Vec<String>, ManifestError> {
        self.call(Cmd::LiveQuarantine).await
    }
    /// See [`Manifest::live_outputs`].
    pub async fn live_outputs(
        &self,
        table: Option<&str>,
    ) -> Result<Vec<super::LiveOutput>, ManifestError> {
        self.call(|r| Cmd::LiveOutputs(table.map(str::to_owned), r))
            .await
    }
    /// See [`Manifest::supersede`].
    pub async fn supersede(&self, file_id: i64, kept: Vec<String>) -> Result<usize, ManifestError> {
        self.call(|r| Cmd::Supersede(file_id, kept, r)).await
    }
    /// See [`Manifest::begin_ingest`].
    pub async fn begin_ingest(&self, s: IngestStart) -> Result<i64, ManifestError> {
        self.call(|r| Cmd::BeginIngest(s, r)).await
    }
    /// See [`Manifest::finish_ingest`].
    pub async fn finish_ingest(&self, ingest_id: i64, e: IngestEnd) -> Result<(), ManifestError> {
        self.call(|r| Cmd::FinishIngest(ingest_id, e, r)).await
    }
    /// See [`Manifest::record_diagnostic`].
    pub async fn record_diagnostic(
        &self,
        run_id: i64,
        ingest_id: Option<i64>,
        d: Diagnostic,
    ) -> Result<(), ManifestError> {
        self.call(|r| Cmd::RecordDiagnostic(run_id, ingest_id, Box::new(d), r))
            .await
    }
    /// See [`Manifest::list_ingests`].
    pub async fn list_ingests(
        &self,
        table: Option<String>,
        failed_only: bool,
        limit: usize,
    ) -> Result<Vec<IngestRecord>, ManifestError> {
        self.call(|r| Cmd::ListIngests(table, failed_only, limit, r))
            .await
    }
    /// See [`Manifest::list_diagnostics`].
    pub async fn list_diagnostics(
        &self,
        f: DiagFilter,
    ) -> Result<Vec<(i64, Option<i64>, Diagnostic)>, ManifestError> {
        self.call(|r| Cmd::ListDiagnostics(f, r)).await
    }
    /// See [`Manifest::latest_run`].
    pub async fn latest_run(&self) -> Result<Option<RunRecord>, ManifestError> {
        self.call(Cmd::LatestRun).await
    }
    /// See [`Manifest::tables_of_run`].
    pub async fn tables_of_run(&self, run_id: i64) -> Result<Vec<String>, ManifestError> {
        self.call(|r| Cmd::TablesOfRun(run_id, r)).await
    }
}

#[cfg(test)]
mod tests {
    fn share(entry: impl Into<String>) -> crate::manifest::EntryShare {
        crate::manifest::EntryShare {
            entry: entry.into(),
            ..Default::default()
        }
    }
    use phasor_core::Category;

    use super::*;
    use crate::manifest::IngestStatus;

    #[tokio::test]
    async fn handle_round_trips_through_the_thread() {
        let (h, thread) = ManifestHandle::spawn_with(Manifest::in_memory().unwrap());
        let run = h.start_run("x").await.unwrap();
        let fid = h
            .record_file(FileRecord {
                source: "s".into(),
                url: "u".into(),
                cache_path: "c".into(),
                sha256: "sha".into(),
                size: 1,
                etag: None,
                last_modified: None,
                last_modified_at: None,
                fetched_at: None,
            })
            .await
            .unwrap();
        assert_eq!(h.file_by_url("u").await.unwrap().unwrap().0, fid);
        assert!(h.ingested_shapes("u", "sha").await.unwrap().is_none());
        assert_eq!(h.list_files().await.unwrap().len(), 1);
        let iid = h
            .begin_ingest(IngestStart {
                run_id: run,
                file_id: fid,
                table: "T".into(),
                schema_version: 1,
                fingerprint: "f".into(),
                transform: crate::writer::Transform {
                    plan_hash: "plan:1".into(),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        h.finish_ingest(
            iid,
            IngestEnd {
                entries: vec![share("e")],
                status: IngestStatus::Ok,
                rows: 2,
                bad_rows: 0,
                quarantine_path: None,
                outputs: vec![],
                error_code: None,
            },
        )
        .await
        .unwrap();
        let shapes = h.ingested_shapes("u", "sha").await.unwrap().unwrap();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].table, "T");
        assert_eq!(shapes[0].transform.plan_hash, "plan:1");
        h.record_diagnostic(
            run,
            Some(iid),
            Diagnostic::warning("schema.x", Category::SCHEMA, "w"),
        )
        .await
        .unwrap();
        assert_eq!(
            h.list_diagnostics(DiagFilter {
                limit: 5,
                ..Default::default()
            })
            .await
            .unwrap()
            .len(),
            1
        );
        assert_eq!(h.list_ingests(None, false, 5).await.unwrap().len(), 1);
        assert!(h.live_quarantine().await.unwrap().is_empty());
        assert!(h.live_outputs(None).await.unwrap().is_empty());
        assert_eq!(h.supersede(fid, Vec::new()).await.unwrap(), 0);
        h.finish_run(run, 0, "{}").await.unwrap();
        assert_eq!(h.latest_run().await.unwrap().unwrap().exit_code, Some(0));
        drop(h);
        thread.join();
    }

    #[tokio::test]
    async fn closed_thread_reports_closed() {
        let (h, thread) = ManifestHandle::spawn_with(Manifest::in_memory().unwrap());
        // Make the thread exit by closing the receiver side: only possible by
        // dropping every sender, so clone a handle, drop the original, then
        // drop the clone and observe via a fresh send failing.
        let h2 = h.clone();
        drop(h);
        drop(h2);
        drop(thread);
        let (tx, _rx) = mpsc::channel::<Cmd>(1);
        drop(tx);
        // A handle whose channel is closed reports Closed.
        let (tx, rx) = mpsc::channel::<Cmd>(1);
        drop(rx);
        let dead = ManifestHandle { tx };
        assert!(matches!(
            dead.latest_run().await,
            Err(ManifestError::Closed)
        ));
        let d =
            ManifestHandle::spawn(&tempfile::tempdir().unwrap().path().join("m.sqlite")).unwrap();
        assert!(d.0.latest_run().await.unwrap().is_none());
    }
}
