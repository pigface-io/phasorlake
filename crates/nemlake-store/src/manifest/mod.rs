//! The SQLite manifest.
//!
//! [`Manifest`] is the synchronous API over one connection; the pipeline
//! uses [`ManifestHandle`], which runs a `Manifest` on its own thread and
//! answers over channels, so SQLite always has exactly one writer and async
//! code never blocks on it.

mod handle;

use std::path::Path;

pub use handle::SchemaSeen;

use nemlake_core::{Diagnostic, Severity};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::ManifestError;
use crate::writer::WrittenFile;

pub use handle::{ManifestHandle, ManifestThread};

const MIGRATIONS: &[(&str, &str)] = &[("0001_init", include_str!("migrations/0001_init.sql"))];

/// A row of `files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Source id, e.g. `nem-mmsdm`.
    pub source: String,
    /// URL (or `file://` path).
    pub url: String,
    /// Where the bytes are on disk.
    pub cache_path: String,
    /// sha256 hex.
    pub sha256: String,
    /// Bytes.
    pub size: u64,
    /// ETag from the server.
    pub etag: Option<String>,
    /// Last-Modified from the server.
    pub last_modified: Option<String>,
}

/// How an ingest ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStatus {
    /// In progress.
    Running,
    /// Done.
    Ok,
    /// Failed.
    Failed,
    /// Interrupted.
    Aborted,
    /// Replaced by a later ingest of a changed source.
    Superseded,
}

impl IngestStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Superseded => "superseded",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "ok" => Self::Ok,
            "failed" => Self::Failed,
            "aborted" => Self::Aborted,
            _ => Self::Superseded,
        }
    }
}

/// The start of an ingest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestStart {
    /// Run.
    pub run_id: i64,
    /// File.
    pub file_id: i64,
    /// Zip entry (`a.zip!b.CSV`).
    pub entry: String,
    /// Table.
    pub table: String,
    /// Version.
    pub schema_version: u16,
    /// Fingerprint text.
    pub fingerprint: String,
}

/// The end of an ingest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestEnd {
    /// Status.
    pub status: IngestStatus,
    /// Rows written.
    pub rows: u64,
    /// Rows quarantined.
    pub bad_rows: u64,
    /// Quarantine file.
    pub quarantine_path: Option<String>,
    /// Files written.
    pub outputs: Vec<WrittenFile>,
    /// Code of the error that failed it.
    pub error_code: Option<String>,
}

/// A row of `ingests` with its outputs, for listings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestRecord {
    /// Id.
    pub id: i64,
    /// Run.
    pub run_id: i64,
    /// Source URL.
    pub url: String,
    /// Entry.
    pub entry: String,
    /// Table.
    pub table: String,
    /// Version.
    pub schema_version: u16,
    /// Status.
    pub status: IngestStatus,
    /// Rows.
    pub rows: u64,
    /// Bad rows.
    pub bad_rows: u64,
    /// Started.
    pub started_at: String,
    /// Finished.
    pub finished_at: Option<String>,
    /// Error code.
    pub error_code: Option<String>,
    /// Outputs.
    pub outputs: Vec<WrittenFile>,
}

/// A row of `runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Id.
    pub id: i64,
    /// Command line.
    pub argv: String,
    /// Started.
    pub started_at: String,
    /// Finished.
    pub finished_at: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Summary JSON.
    pub summary: Option<String>,
}

/// Filters for listing diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagFilter {
    /// Only this run.
    pub run_id: Option<i64>,
    /// Only this table.
    pub table: Option<String>,
    /// Only this code.
    pub code: Option<String>,
    /// Only this severity.
    pub severity: Option<Severity>,
    /// At most this many, newest first.
    pub limit: usize,
}

/// The manifest over one connection.
pub struct Manifest {
    conn: Connection,
    path: String,
}

fn now() -> String {
    // RFC 3339 UTC to the second, without pulling a date crate into the hot
    // path crates: the manifest is the only place wall-clock time matters.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    nemlake_core::nemtime::format_local_seconds(secs, 0)
        .replace('/', "-")
        .replacen(' ', "T", 1)
        + "Z"
}

impl Manifest {
    /// Opens (creating and migrating) the manifest at `path`.
    pub fn open(path: &Path) -> Result<Self, ManifestError> {
        let p = path.display().to_string();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ManifestError::Sqlite {
                path: p.clone(),
                message: e.to_string(),
            })?;
        }
        let conn = Connection::open(path).map_err(|e| ManifestError::Sqlite {
            path: p.clone(),
            message: e.to_string(),
        })?;
        let mut m = Self { conn, path: p };
        m.exec_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        m.migrate()?;
        Ok(m)
    }

    /// An in-memory manifest, for tests.
    pub fn in_memory() -> Result<Self, ManifestError> {
        let conn = Connection::open_in_memory().map_err(|e| ManifestError::Sqlite {
            path: ":memory:".into(),
            message: e.to_string(),
        })?;
        let mut m = Self {
            conn,
            path: ":memory:".into(),
        };
        m.exec_batch("PRAGMA foreign_keys = ON;")?;
        m.migrate()?;
        Ok(m)
    }

    fn sql<T>(&self, r: Result<T, rusqlite::Error>) -> Result<T, ManifestError> {
        r.map_err(|e| ManifestError::Sqlite {
            path: self.path.clone(),
            message: e.to_string(),
        })
    }

    fn exec_batch(&mut self, sql: &str) -> Result<(), ManifestError> {
        let r = self.conn.execute_batch(sql);
        self.sql(r)
    }

    /// The migration version applied.
    pub fn version(&self) -> Result<u32, ManifestError> {
        let r = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'migration'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional();
        Ok(self.sql(r)?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    fn migrate(&mut self) -> Result<(), ManifestError> {
        let has_meta: bool = self.sql(self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |r| r.get::<_, i64>(0),
        ))? > 0;
        let mut at = if has_meta { self.version()? } else { 0 };
        for (i, (name, sql)) in MIGRATIONS.iter().enumerate() {
            let n = i as u32 + 1;
            if n <= at {
                continue;
            }
            let script = format!(
                "BEGIN; {sql}; INSERT OR REPLACE INTO meta(key, value) VALUES ('migration', '{n}'); COMMIT;"
            );
            if let Err(e) = self.conn.execute_batch(&script) {
                let _ = self.conn.execute_batch("ROLLBACK;");
                return Err(ManifestError::Migration {
                    path: self.path.clone(),
                    at,
                    failed: (*name).to_owned(),
                    message: e.to_string(),
                });
            }
            at = n;
        }
        Ok(())
    }

    /// Starts a run.
    pub fn begin_run(&mut self, argv: &str) -> Result<i64, ManifestError> {
        let r = self.conn.execute(
            "INSERT INTO runs (argv, started_at, version) VALUES (?1, ?2, ?3)",
            params![argv, now(), env!("CARGO_PKG_VERSION")],
        );
        self.sql(r)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Finishes a run.
    pub fn finish_run(
        &mut self,
        run_id: i64,
        exit_code: i32,
        summary_json: &str,
    ) -> Result<(), ManifestError> {
        let r = self.conn.execute(
            "UPDATE runs SET finished_at = ?2, exit_code = ?3, summary = ?4 WHERE id = ?1",
            params![run_id, now(), exit_code, summary_json],
        );
        match self.sql(r)? {
            0 => Err(ManifestError::NotFound {
                kind: "run",
                id: run_id,
            }),
            _ => Ok(()),
        }
    }

    /// The latest run.
    pub fn latest_run(&self) -> Result<Option<RunRecord>, ManifestError> {
        let r = self
            .conn
            .query_row("SELECT id, argv, started_at, finished_at, exit_code, summary FROM runs ORDER BY id DESC LIMIT 1", [], |r| {
                Ok(RunRecord {
                    id: r.get(0)?,
                    argv: r.get(1)?,
                    started_at: r.get(2)?,
                    finished_at: r.get(3)?,
                    exit_code: r.get(4)?,
                    summary: r.get(5)?,
                })
            })
            .optional();
        self.sql(r)
    }

    /// Records (or updates) a fetched file; returns its id.
    pub fn record_file(&mut self, f: &FileRecord) -> Result<i64, ManifestError> {
        let r = self.conn.execute(
            "INSERT INTO files (source, url, cache_path, sha256, size, etag, last_modified, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(url) DO UPDATE SET cache_path = excluded.cache_path, sha256 = excluded.sha256, size = excluded.size,
                 etag = excluded.etag, last_modified = excluded.last_modified, fetched_at = excluded.fetched_at",
            params![f.source, f.url, f.cache_path, f.sha256, f.size as i64, f.etag, f.last_modified, now()],
        );
        self.sql(r)?;
        let r = self
            .conn
            .query_row("SELECT id FROM files WHERE url = ?1", [&f.url], |r| {
                r.get::<_, i64>(0)
            });
        self.sql(r)
    }

    /// The recorded file for a URL.
    pub fn file_by_url(&self, url: &str) -> Result<Option<(i64, FileRecord)>, ManifestError> {
        let r = self
            .conn
            .query_row("SELECT id, source, url, cache_path, sha256, size, etag, last_modified FROM files WHERE url = ?1", [url], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    FileRecord {
                        source: r.get(1)?,
                        url: r.get(2)?,
                        cache_path: r.get(3)?,
                        sha256: r.get(4)?,
                        size: r.get::<_, i64>(5)? as u64,
                        etag: r.get(6)?,
                        last_modified: r.get(7)?,
                    },
                ))
            })
            .optional();
        self.sql(r)
    }

    /// Whether a source with this URL and sha256 has every ingest `ok` and
    /// every output still on disk, so it can be skipped.
    pub fn is_ingested(&self, url: &str, sha256: &str) -> Result<bool, ManifestError> {
        let Some((file_id, rec)) = self.file_by_url(url)? else {
            return Ok(false);
        };
        if rec.sha256 != sha256 {
            return Ok(false);
        }
        let mut stmt = self.sql(
            self.conn
                .prepare("SELECT id, status FROM ingests WHERE file_id = ?1"),
        )?;
        let rows: Vec<(i64, String)> = self.sql(
            stmt.query_map([file_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .and_then(Iterator::collect),
        )?;
        if rows.is_empty() || rows.iter().any(|(_, s)| s != "ok") {
            return Ok(false);
        }
        let mut stmt = self.sql(self.conn.prepare("SELECT o.parquet_path FROM outputs o JOIN ingests i ON o.ingest_id = i.id WHERE i.file_id = ?1"))?;
        let paths: Vec<String> = self.sql(
            stmt.query_map([file_id], |r| r.get(0))
                .and_then(Iterator::collect),
        )?;
        Ok(paths.iter().all(|p| Path::new(p).is_file()))
    }

    /// Paths of every output a file's ingests wrote, for superseding.
    pub fn outputs_of_file(&self, file_id: i64) -> Result<Vec<String>, ManifestError> {
        let mut stmt = self.sql(self.conn.prepare("SELECT o.parquet_path FROM outputs o JOIN ingests i ON o.ingest_id = i.id WHERE i.file_id = ?1"))?;
        self.sql(
            stmt.query_map([file_id], |r| r.get(0))
                .and_then(Iterator::collect),
        )
    }

    /// Marks every ingest of a file superseded and forgets its outputs.
    pub fn supersede_file(&mut self, file_id: i64) -> Result<usize, ManifestError> {
        let r = self.conn.execute("UPDATE ingests SET status = 'superseded' WHERE file_id = ?1 AND status IN ('ok', 'failed', 'aborted')", [file_id]);
        let n = self.sql(r)?;
        let r = self.conn.execute(
            "DELETE FROM outputs WHERE ingest_id IN (SELECT id FROM ingests WHERE file_id = ?1)",
            [file_id],
        );
        self.sql(r)?;
        Ok(n)
    }

    /// Starts an ingest; a previous row for the same key is replaced.
    pub fn begin_ingest(&mut self, s: &IngestStart) -> Result<i64, ManifestError> {
        let r = self.conn.execute(
            "DELETE FROM ingests WHERE file_id = ?1 AND entry = ?2 AND table_name = ?3 AND schema_version = ?4",
            params![s.file_id, s.entry, s.table, i64::from(s.schema_version)],
        );
        self.sql(r)?;
        let r = self.conn.execute(
            "INSERT INTO ingests (run_id, file_id, entry, table_name, schema_version, fingerprint, status, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7)",
            params![s.run_id, s.file_id, s.entry, s.table, i64::from(s.schema_version), s.fingerprint, now()],
        );
        self.sql(r)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Finishes an ingest and records its outputs in one transaction.
    pub fn finish_ingest(&mut self, ingest_id: i64, e: &IngestEnd) -> Result<(), ManifestError> {
        let (min, max) = e
            .outputs
            .iter()
            .fold((None::<i64>, None::<i64>), |(mn, mx), o| {
                (
                    match (mn, o.min_time) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    },
                    match (mx, o.max_time) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    },
                )
            });
        let path = self.path.clone();
        let tx = self
            .conn
            .transaction()
            .map_err(|er| ManifestError::Sqlite {
                path: path.clone(),
                message: er.to_string(),
            })?;
        let n = tx
            .execute(
                "UPDATE ingests SET status = ?2, rows = ?3, bad_rows = ?4, quarantine_path = ?5, min_time = ?6, max_time = ?7, finished_at = ?8, error_code = ?9 WHERE id = ?1",
                params![ingest_id, e.status.as_str(), e.rows as i64, e.bad_rows as i64, e.quarantine_path, min, max, now(), e.error_code],
            )
            .map_err(|er| ManifestError::Sqlite { path: path.clone(), message: er.to_string() })?;
        if n == 0 {
            return Err(ManifestError::NotFound {
                kind: "ingest",
                id: ingest_id,
            });
        }
        for o in &e.outputs {
            tx.execute(
                "INSERT OR REPLACE INTO outputs (ingest_id, parquet_path, partition, rows, bytes, min_time, max_time) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![ingest_id, o.path, o.partition.path_segment(), o.rows as i64, o.bytes as i64, o.min_time, o.max_time],
            )
            .map_err(|er| ManifestError::Sqlite { path: path.clone(), message: er.to_string() })?;
        }
        tx.commit().map_err(|er| ManifestError::Sqlite {
            path,
            message: er.to_string(),
        })
    }

    /// Marks every `running` ingest aborted (startup after a crash).
    pub fn abort_running(&mut self) -> Result<usize, ManifestError> {
        let r = self.conn.execute(
            "UPDATE ingests SET status = 'aborted', finished_at = ?1 WHERE status = 'running'",
            [now()],
        );
        self.sql(r)
    }

    /// Records a diagnostic.
    pub fn record_diagnostic(
        &mut self,
        run_id: i64,
        ingest_id: Option<i64>,
        d: &Diagnostic,
    ) -> Result<(), ManifestError> {
        let json = serde_json::to_string(d).unwrap_or_default();
        let r = self.conn.execute(
            "INSERT INTO diagnostics (run_id, ingest_id, severity, code, category, message, json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![run_id, ingest_id, d.severity.to_string(), d.code.as_ref(), d.category.as_str(), d.message, json, now()],
        );
        self.sql(r).map(|_| ())
    }

    /// Records a shape seen in the wild; the first sighting wins.
    pub fn record_schema_seen(&mut self, s: &SchemaSeen) -> Result<(), ManifestError> {
        let r = self.conn.execute(
            "INSERT OR IGNORE INTO schemas_seen (fingerprint, table_name, report_type, report_subtype, schema_version, columns, status, first_seen_file, first_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![s.fingerprint, s.table, s.report_type, s.report_subtype, i64::from(s.version), s.columns.join(","), s.status, s.file_id, now()],
        );
        self.sql(r).map(|_| ())
    }

    /// Ingests, newest first, optionally for one table or only failed ones.
    pub fn list_ingests(
        &self,
        table: Option<&str>,
        failed_only: bool,
        limit: usize,
    ) -> Result<Vec<IngestRecord>, ManifestError> {
        let sql = format!(
            "SELECT i.id, i.run_id, f.url, i.entry, i.table_name, i.schema_version, i.status, i.rows, i.bad_rows, i.started_at, i.finished_at, i.error_code
             FROM ingests i JOIN files f ON i.file_id = f.id
             WHERE (?1 IS NULL OR i.table_name = ?1) AND (?2 = 0 OR i.status = 'failed')
             ORDER BY i.id DESC LIMIT {}",
            limit.max(1)
        );
        let mut stmt = self.sql(self.conn.prepare(&sql))?;
        let mut recs: Vec<IngestRecord> = self.sql(
            stmt.query_map(params![table, i64::from(failed_only)], |r| {
                Ok(IngestRecord {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    url: r.get(2)?,
                    entry: r.get(3)?,
                    table: r.get(4)?,
                    schema_version: r.get::<_, i64>(5)? as u16,
                    status: IngestStatus::parse(&r.get::<_, String>(6)?),
                    rows: r.get::<_, i64>(7)? as u64,
                    bad_rows: r.get::<_, i64>(8)? as u64,
                    started_at: r.get(9)?,
                    finished_at: r.get(10)?,
                    error_code: r.get(11)?,
                    outputs: vec![],
                })
            })
            .and_then(Iterator::collect),
        )?;
        let mut stmt = self.sql(self.conn.prepare("SELECT parquet_path, partition, rows, bytes, min_time, max_time FROM outputs WHERE ingest_id = ?1 ORDER BY id"))?;
        for rec in &mut recs {
            rec.outputs = self.sql(
                stmt.query_map([rec.id], |r| {
                    let seg: String = r.get(1)?;
                    Ok(WrittenFile {
                        path: r.get(0)?,
                        partition: parse_partition(&seg),
                        rows: r.get::<_, i64>(2)? as u64,
                        bytes: r.get::<_, i64>(3)? as u64,
                        min_time: r.get(4)?,
                        max_time: r.get(5)?,
                        sorted: false,
                    })
                })
                .and_then(Iterator::collect),
            )?;
        }
        Ok(recs)
    }

    /// Diagnostics matching a filter, newest first.
    pub fn list_diagnostics(
        &self,
        f: &DiagFilter,
    ) -> Result<Vec<(i64, Option<i64>, Diagnostic)>, ManifestError> {
        let sql = format!(
            "SELECT d.run_id, d.ingest_id, d.json FROM diagnostics d LEFT JOIN ingests i ON d.ingest_id = i.id
             WHERE (?1 IS NULL OR d.run_id = ?1) AND (?2 IS NULL OR i.table_name = ?2 OR d.json LIKE '%' || ?2 || '%')
               AND (?3 IS NULL OR d.code = ?3) AND (?4 IS NULL OR d.severity = ?4)
             ORDER BY d.id DESC LIMIT {}",
            f.limit.max(1)
        );
        let mut stmt = self.sql(self.conn.prepare(&sql))?;
        let sev = f.severity.map(|s| s.to_string());
        self.sql(
            stmt.query_map(params![f.run_id, f.table, f.code, sev], |r| {
                let json: String = r.get(2)?;
                let d: Diagnostic = serde_json::from_str(&json).unwrap_or_else(|_| {
                    Diagnostic::error(
                        "internal.diagnostic_json",
                        nemlake_core::Category::Internal,
                        "A stored diagnostic could not be decoded.",
                    )
                });
                Ok((r.get(0)?, r.get(1)?, d))
            })
            .and_then(Iterator::collect),
        )
    }

    /// Row counts for a table across `ok` ingests, for `verify`.
    pub fn table_totals(&self, table: &str) -> Result<(u64, u64), ManifestError> {
        let r = self.conn.query_row(
            "SELECT COALESCE(SUM(i.rows), 0), COUNT(DISTINCT o.id) FROM ingests i LEFT JOIN outputs o ON o.ingest_id = i.id WHERE i.table_name = ?1 AND i.status = 'ok'",
            [table],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        );
        self.sql(r)
    }
}

fn parse_partition(seg: &str) -> crate::writer::Partition {
    use crate::writer::Partition;
    if let Some(rest) = seg.strip_prefix("snapshot=")
        && rest.len() == 6
        && let (Ok(y), Ok(m)) = (rest[..4].parse(), rest[4..].parse())
    {
        return Partition::Snapshot { year: y, month: m };
    }
    if let Some((y, m)) = seg
        .strip_prefix("year=")
        .and_then(|s| s.split_once("/month="))
        && let (Ok(y), Ok(m)) = (y.parse(), m.parse())
    {
        return Partition::Month { year: y, month: m };
    }
    Partition::Unknown
}

#[cfg(test)]
mod tests {
    use nemlake_core::Category;

    use super::*;
    use crate::writer::Partition;

    fn file(url: &str, sha: &str) -> FileRecord {
        FileRecord {
            source: "nem-mmsdm".into(),
            url: url.into(),
            cache_path: "/cache/x.zip".into(),
            sha256: sha.into(),
            size: 10,
            etag: Some("e".into()),
            last_modified: None,
        }
    }

    #[test]
    fn migrates_and_round_trips_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("manifest.sqlite");
        let m = Manifest::open(&p).unwrap();
        assert_eq!(m.version().unwrap(), 1);
        // Reopening does not re-run migrations.
        drop(m);
        let mut m = Manifest::open(&p).unwrap();
        assert_eq!(m.version().unwrap(), 1);
        let run = m.begin_run("fetch nem --table X").unwrap();
        assert_eq!(m.latest_run().unwrap().unwrap().id, run);
        let fid = m.record_file(&file("https://h/a.zip", "aaa")).unwrap();
        let fid2 = m.record_file(&file("https://h/a.zip", "bbb")).unwrap();
        assert_eq!(fid, fid2, "same URL updates in place");
        assert_eq!(
            m.file_by_url("https://h/a.zip").unwrap().unwrap().1.sha256,
            "bbb"
        );
        assert!(m.file_by_url("https://h/none").unwrap().is_none());
        assert!(
            !m.is_ingested("https://h/a.zip", "bbb").unwrap(),
            "no ingests yet"
        );
        let out_path = dir.path().join("out.parquet");
        std::fs::write(&out_path, b"x").unwrap();
        let start = IngestStart {
            run_id: run,
            file_id: fid,
            entry: "A.CSV".into(),
            table: "T".into(),
            schema_version: 1,
            fingerprint: "xxh3:1".into(),
        };
        let iid = m.begin_ingest(&start).unwrap();
        assert!(!m.is_ingested("https://h/a.zip", "bbb").unwrap(), "running");
        let outputs = vec![WrittenFile {
            path: out_path.display().to_string(),
            partition: Partition::Month {
                year: 2024,
                month: 1,
            },
            rows: 5,
            bytes: 1,
            min_time: Some(10),
            max_time: Some(20),
            sorted: false,
        }];
        m.finish_ingest(
            iid,
            &IngestEnd {
                status: IngestStatus::Ok,
                rows: 5,
                bad_rows: 1,
                quarantine_path: Some("q".into()),
                outputs: outputs.clone(),
                error_code: None,
            },
        )
        .unwrap();
        assert!(m.is_ingested("https://h/a.zip", "bbb").unwrap());
        assert!(
            !m.is_ingested("https://h/a.zip", "ccc").unwrap(),
            "sha changed"
        );
        let recs = m.list_ingests(None, false, 10).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].status, IngestStatus::Ok);
        assert_eq!(recs[0].outputs, outputs);
        assert_eq!(recs[0].bad_rows, 1);
        assert_eq!(m.table_totals("T").unwrap(), (5, 1));
        assert_eq!(m.list_ingests(Some("OTHER"), false, 10).unwrap().len(), 0);
        assert_eq!(m.list_ingests(None, true, 10).unwrap().len(), 0);
        // Re-begin replaces the row; a failure is listed with its code.
        let iid2 = m.begin_ingest(&start).unwrap();
        assert_eq!(
            m.list_ingests(None, false, 10).unwrap().len(),
            1,
            "the earlier row for the same key is replaced"
        );
        m.finish_ingest(
            iid2,
            &IngestEnd {
                status: IngestStatus::Failed,
                rows: 0,
                bad_rows: 0,
                quarantine_path: None,
                outputs: vec![],
                error_code: Some("zip.bad_entry".into()),
            },
        )
        .unwrap();
        let failed = m.list_ingests(None, true, 10).unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].error_code.as_deref(), Some("zip.bad_entry"));
        assert!(!m.is_ingested("https://h/a.zip", "bbb").unwrap());
        // Diagnostics.
        let d = Diagnostic::error("zip.bad_entry", Category::Zip, "Bad.")
            .with_table("T")
            .with_url("https://h/a.zip");
        m.record_diagnostic(run, Some(iid2), &d).unwrap();
        m.record_diagnostic(
            run,
            None,
            &Diagnostic::warning("schema.compatible_drift", Category::Schema, "Drift.")
                .with_table("T"),
        )
        .unwrap();
        let all = m
            .list_diagnostics(&DiagFilter {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].2.code, "zip.bad_entry");
        assert_eq!(all[1].1, Some(iid2));
        let errs = m
            .list_diagnostics(&DiagFilter {
                severity: Some(Severity::Error),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(errs.len(), 1);
        let by_code = m
            .list_diagnostics(&DiagFilter {
                code: Some("schema.compatible_drift".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_code.len(), 1);
        let by_table = m
            .list_diagnostics(&DiagFilter {
                table: Some("T".into()),
                run_id: Some(run),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_table.len(), 2);
        assert!(
            m.list_diagnostics(&DiagFilter {
                run_id: Some(999),
                limit: 10,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
        );
        // Schema seen.
        let seen = SchemaSeen {
            fingerprint: "xxh3:1".into(),
            table: Some("T".into()),
            report_type: "A".into(),
            report_subtype: "B".into(),
            version: 1,
            columns: vec!["X".into(), "Y".into()],
            status: "exact".into(),
            file_id: Some(fid),
        };
        m.record_schema_seen(&seen).unwrap();
        m.record_schema_seen(&seen).unwrap();
        // Supersede and abort_running.
        let iid3 = m
            .begin_ingest(&IngestStart {
                entry: "B.CSV".into(),
                ..start.clone()
            })
            .unwrap();
        assert_eq!(m.abort_running().unwrap(), 1);
        assert_eq!(
            m.list_ingests(None, false, 10).unwrap()[0].status,
            IngestStatus::Aborted
        );
        assert_eq!(
            m.outputs_of_file(fid).unwrap().len(),
            0,
            "the ok ingest was replaced, its outputs went with it"
        );
        m.finish_ingest(
            iid3,
            &IngestEnd {
                status: IngestStatus::Ok,
                rows: 1,
                bad_rows: 0,
                quarantine_path: None,
                outputs: outputs.clone(),
                error_code: None,
            },
        )
        .unwrap();
        assert_eq!(m.outputs_of_file(fid).unwrap().len(), 1);
        assert_eq!(m.supersede_file(fid).unwrap(), 2);
        assert_eq!(m.outputs_of_file(fid).unwrap().len(), 0);
        assert_eq!(
            m.list_ingests(None, false, 10).unwrap()[0].status,
            IngestStatus::Superseded
        );
        // Run end.
        m.finish_run(run, 3, "{}").unwrap();
        let r = m.latest_run().unwrap().unwrap();
        assert_eq!(r.exit_code, Some(3));
        assert!(r.finished_at.is_some());
        assert!(matches!(
            m.finish_run(999, 0, ""),
            Err(ManifestError::NotFound { kind: "run", .. })
        ));
        assert!(matches!(
            m.finish_ingest(
                999,
                &IngestEnd {
                    status: IngestStatus::Ok,
                    rows: 0,
                    bad_rows: 0,
                    quarantine_path: None,
                    outputs: vec![],
                    error_code: None
                }
            ),
            Err(ManifestError::NotFound { .. })
        ));
    }

    #[test]
    fn in_memory_and_partition_parse_and_errors() {
        let mut m = Manifest::in_memory().unwrap();
        assert_eq!(m.version().unwrap(), 1);
        assert!(m.latest_run().unwrap().is_none());
        assert_eq!(m.abort_running().unwrap(), 0);
        assert_eq!(
            parse_partition("year=2024/month=03"),
            Partition::Month {
                year: 2024,
                month: 3
            }
        );
        assert_eq!(
            parse_partition("snapshot=202412"),
            Partition::Snapshot {
                year: 2024,
                month: 12
            }
        );
        assert_eq!(parse_partition("junk"), Partition::Unknown);
        assert_eq!(parse_partition("year=x/month=1"), Partition::Unknown);
        assert_eq!(IngestStatus::parse("ok"), IngestStatus::Ok);
        assert_eq!(IngestStatus::parse("whatever"), IngestStatus::Superseded);
        assert_eq!(IngestStatus::Running.as_str(), "running");
        // A directory where the file should be is an open error.
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Manifest::open(dir.path()),
            Err(ManifestError::Sqlite { .. })
        ));
        let n = now();
        assert!(n.ends_with('Z') && n.contains('T') && n.len() == 20, "{n}");
        // Foreign keys hold.
        assert!(
            m.begin_ingest(&IngestStart {
                run_id: 99,
                file_id: 99,
                entry: String::new(),
                table: String::new(),
                schema_version: 0,
                fingerprint: String::new()
            })
            .is_err()
        );
    }
}
