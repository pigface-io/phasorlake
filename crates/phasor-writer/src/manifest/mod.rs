//! The SQLite manifest.
//!
//! [`Manifest`] is the synchronous API; [`ManifestHandle`] runs one on its own
//! thread so SQLite has exactly one writer and async code never blocks on it.

mod handle;

use std::path::{Path, PathBuf};

use phasor_core::{Diagnostic, Severity};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::ManifestError;
use crate::partition::{max_time, min_time};
use crate::writer::{Transform, WrittenFile};

pub use handle::{ManifestHandle, ManifestThread};

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("migrations/0001_init.sql")),
    (
        "0002_transform",
        include_str!("migrations/0002_transform.sql"),
    ),
    (
        "0003_source_period",
        include_str!("migrations/0003_source_period.sql"),
    ),
    (
        "0004_plan_hash",
        include_str!("migrations/0004_plan_hash.sql"),
    ),
    (
        "0005_drop_schemas_seen",
        include_str!("migrations/0005_drop_schemas_seen.sql"),
    ),
    (
        "0006_last_modified_at",
        include_str!("migrations/0006_last_modified_at.sql"),
    ),
    (
        "0007_schema_id",
        include_str!("migrations/0007_schema_id.sql"),
    ),
    ("0008_sources", include_str!("migrations/0008_sources.sql")),
    ("0009_times", include_str!("migrations/0009_times.sql")),
];

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
    /// Last-Modified from the server, as sent (an HTTP date).
    pub last_modified: Option<String>,
    /// `last_modified` as an instant, RFC 3339 UTC.
    #[serde(default)]
    pub last_modified_at: Option<String>,
    /// When the bytes were fetched, RFC 3339; none records the time of recording.
    #[serde(default)]
    pub fetched_at: Option<String>,
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
    /// Table.
    pub table: String,
    /// Version.
    pub schema_version: u16,
    /// Fingerprint text.
    pub fingerprint: String,
    /// The shape that was read and the rules it was read with.
    pub transform: Transform,
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
    /// What each entry of the source file gave, in the order they were read.
    pub entries: Vec<EntryShare>,
    /// Code of the error that failed it.
    pub error_code: Option<String>,
}

/// What one entry of a source file gave an ingest.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EntryShare {
    /// The entry (`a.zip!b.CSV`); empty when the source is the file itself.
    pub entry: String,
    /// Rows written.
    pub rows: u64,
    /// Rows quarantined.
    pub bad_rows: u64,
    /// Its least time value, nanoseconds since 1970.
    pub min_time: Option<i64>,
    /// Its greatest.
    pub max_time: Option<i64>,
    /// The server's `Last-Modified`, microseconds since 1970 UTC: `_phasor_server_last_modified`.
    #[serde(default)]
    pub server_last_modified: Option<i64>,
    /// The zip central directory's modified time, packer's clock: `_phasor_zip_entry_modified`.
    #[serde(default)]
    pub zip_entry_modified: Option<i64>,
    /// The stamp in the entry's file name, host's clock: `_phasor_filename_timestamp`.
    #[serde(default)]
    pub filename_timestamp: Option<i64>,
}

impl EntryShare {
    /// Counts `rows` more, whose time column spans `bounds`.
    pub fn add(&mut self, rows: u64, bounds: Option<(Option<i64>, Option<i64>)>) {
        self.rows += rows;
        if let Some((min, max)) = bounds {
            self.min_time = min_time(self.min_time, min);
            self.max_time = max_time(self.max_time, max);
        }
    }
}

/// An output of a live ingest, with what the lake's Iceberg tables need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveOutput {
    /// The table.
    pub table: String,
    /// The Parquet file.
    pub path: String,
    /// Its rows.
    pub rows: u64,
    /// Its size.
    pub bytes: u64,
    /// The Iceberg schema its block is written under, when known.
    pub schema_id: Option<u32>,
    /// AEMO's version of the block.
    pub source_version: u16,
    /// The month of the source, `YYYY-MM`.
    pub source_period: Option<String>,
    /// The source file.
    pub url: String,
    /// The entries of the source file its ingest read.
    pub entries: Vec<EntryShare>,
    /// Its least time value, nanoseconds since 1970.
    pub min_time: Option<i64>,
    /// Its greatest.
    pub max_time: Option<i64>,
    /// The partition it is in, as its directory names it.
    #[serde(default)]
    pub partition: String,
    /// When the server last changed the source file, RFC 3339 UTC.
    #[serde(default)]
    pub last_modified_at: Option<String>,
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
    /// The entries it read.
    pub entries: Vec<EntryShare>,
    /// Table.
    pub table: String,
    /// Version.
    pub schema_version: u16,
    /// Fingerprint of the shape that was read.
    #[serde(default)]
    pub fingerprint: String,
    /// The shape that was read and the rules it was read with.
    #[serde(default)]
    pub transform: Transform,
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
///
/// Paths under its home (the manifest's directory) are stored relative to it,
/// so a home moved whole still names its own files.
pub struct Manifest {
    conn: Connection,
    path: String,
    home: Option<PathBuf>,
    /// The home's run lock, held from the first run started until close.
    lock: Option<std::fs::File>,
}

/// Now, RFC 3339 UTC to the second.
fn now() -> String {
    phasor_core::clock::now_utc_rfc3339()
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
        let home = path.parent().map(Path::to_path_buf);
        let mut m = Self {
            conn,
            path: p,
            home,
            lock: None,
        };
        m.exec_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        m.migrate()?;
        Ok(m)
    }

    /// An in-memory manifest, for tests.
    #[cfg(test)]
    pub(crate) fn in_memory() -> Result<Self, ManifestError> {
        let conn = Connection::open_in_memory().map_err(|e| ManifestError::Sqlite {
            path: ":memory:".into(),
            message: e.to_string(),
        })?;
        let mut m = Self {
            conn,
            path: ":memory:".into(),
            home: None,
            lock: None,
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
        if at as usize >= MIGRATIONS.len() {
            return Ok(());
        }
        // A table rebuild would cascade-delete rows that point at it, and the
        // setting cannot change inside a transaction, so it is off for the pass.
        self.exec_batch("PRAGMA foreign_keys = OFF;")?;
        let migrated = self.apply_migrations(&mut at);
        self.exec_batch("PRAGMA foreign_keys = ON;")?;
        migrated
    }

    fn apply_migrations(&mut self, at: &mut u32) -> Result<(), ManifestError> {
        for (i, (name, sql)) in MIGRATIONS.iter().enumerate() {
            let n = i as u32 + 1;
            if n <= *at {
                continue;
            }
            let script = format!(
                "BEGIN; {sql}; INSERT OR REPLACE INTO meta(key, value) VALUES ('migration', '{n}'); COMMIT;"
            );
            if let Err(e) = self.conn.execute_batch(&script) {
                let _ = self.conn.execute_batch("ROLLBACK;");
                return Err(ManifestError::Migration {
                    path: self.path.clone(),
                    at: *at,
                    failed: (*name).to_owned(),
                    message: e.to_string(),
                });
            }
            *at = n;
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

    /// Starts a run, aborting what an unfinished run left and removing its half-written files.
    ///
    /// The home is locked first, so a second concurrent run is refused; the
    /// lock dies with the process.
    pub fn start_run(&mut self, argv: &str) -> Result<i64, ManifestError> {
        self.lock_home()?;
        self.abort_running()?;
        if let Some(h) = &self.home {
            let _ = crate::parquet_hive::sweep_tmp(&h.join("lake"));
        }
        self.begin_run(argv)
    }

    /// Takes the home's run lock, once per manifest.
    fn lock_home(&mut self) -> Result<(), ManifestError> {
        let (Some(home), None) = (&self.home, &self.lock) else {
            return Ok(());
        };
        let path = home.join("lake.lock");
        let busy = |cause: String| ManifestError::Busy {
            path: path.display().to_string(),
            cause,
        };
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| busy(e.to_string()))?;
        file.try_lock().map_err(|e| busy(e.to_string()))?;
        self.lock = Some(file);
        Ok(())
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

    /// The tables a run began an ingest of, by name.
    pub fn tables_of_run(&self, run_id: i64) -> Result<Vec<String>, ManifestError> {
        let r = (|| {
            let mut st = self.conn.prepare_cached(
                "SELECT DISTINCT table_name FROM ingests WHERE run_id = ?1 ORDER BY table_name",
            )?;
            let rows = st.query_map([run_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()
        })();
        self.sql(r)
    }

    /// `p` as the manifest stores it: relative to the home when under it.
    fn stored(&self, p: &str) -> String {
        match &self.home {
            Some(h) => Path::new(p)
                .strip_prefix(h)
                .map_or_else(|_| p.to_owned(), |rel| rel.display().to_string()),
            None => p.to_owned(),
        }
    }

    /// A stored path as callers use it: under the home, wherever it is now.
    fn resolved(&self, p: String) -> String {
        match &self.home {
            Some(h) if Path::new(&p).is_relative() => h.join(p).display().to_string(),
            _ => p,
        }
    }

    /// Records (or updates) a fetched file; returns its id.
    pub fn record_file(&mut self, f: &FileRecord) -> Result<i64, ManifestError> {
        let r = self.conn.query_row(
            "INSERT INTO files (source, url, cache_path, sha256, size, etag, last_modified, last_modified_at, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(url) DO UPDATE SET cache_path = excluded.cache_path, sha256 = excluded.sha256, size = excluded.size,
                 etag = excluded.etag, last_modified = excluded.last_modified, last_modified_at = excluded.last_modified_at, fetched_at = excluded.fetched_at
             RETURNING id",
            params![f.source, f.url, self.stored(&f.cache_path), f.sha256, f.size as i64, f.etag, f.last_modified, f.last_modified_at, f.fetched_at.clone().unwrap_or_else(now)],
            |r| r.get::<_, i64>(0),
        );
        self.sql(r)
    }

    /// The recorded file for a URL.
    pub fn file_by_url(&self, url: &str) -> Result<Option<(i64, FileRecord)>, ManifestError> {
        let r = self
            .conn
            .query_row("SELECT id, source, url, cache_path, sha256, size, etag, last_modified, last_modified_at, fetched_at FROM files WHERE url = ?1", [url], |r| {
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
                        last_modified_at: r.get(8)?,
                        fetched_at: r.get(9)?,
                    },
                ))
            })
            .optional();
        Ok(self.sql(r)?.map(|(id, mut f)| {
            f.cache_path = self.resolved(f.cache_path);
            (id, f)
        }))
    }

    /// Every file the manifest has recorded, in first-seen order.
    pub fn list_files(&self) -> Result<Vec<FileRecord>, ManifestError> {
        let mut stmt = self.sql(self.conn.prepare_cached(
            "SELECT source, url, cache_path, sha256, size, etag, last_modified, last_modified_at, fetched_at FROM files ORDER BY id",
        ))?;
        self.sql(
            stmt.query_map([], |r| {
                Ok(FileRecord {
                    source: r.get(0)?,
                    url: r.get(1)?,
                    cache_path: r.get(2)?,
                    sha256: r.get(3)?,
                    size: r.get::<_, i64>(4)? as u64,
                    etag: r.get(5)?,
                    last_modified: r.get(6)?,
                    last_modified_at: r.get(7)?,
                    fetched_at: r.get(8)?,
                })
            })
            .and_then(|rows| rows.collect::<rusqlite::Result<Vec<FileRecord>>>()),
        )
        .map(|files: Vec<FileRecord>| {
            files
                .into_iter()
                .map(|mut f| {
                    f.cache_path = self.resolved(f.cache_path);
                    f
                })
                .collect()
        })
    }

    /// What was read from a source with this URL and sha256, when it is wholly in the lake.
    ///
    /// `None` unless every ingest is `ok` and every output is still on disk.
    /// The caller decides whether the transforms still match the current rules.
    pub fn ingested_shapes(
        &self,
        url: &str,
        sha256: &str,
    ) -> Result<Option<Vec<IngestShape>>, ManifestError> {
        let Some((file_id, rec)) = self.file_by_url(url)? else {
            return Ok(None);
        };
        if rec.sha256 != sha256 {
            return Ok(None);
        }
        let mut stmt = self.sql(self.conn.prepare_cached(
            "SELECT status, table_name, report_type, report_subtype, source_version, source_columns, plan_hash, schema_source, resolution, source_period, shape_id, schema_id
             FROM ingests WHERE file_id = ?1 AND status != 'superseded' ORDER BY id",
        ))?;
        let rows: Vec<(String, IngestShape)> = self.sql(
            stmt.query_map([file_id], |r| {
                Ok((
                    r.get(0)?,
                    IngestShape {
                        table: r.get(1)?,
                        transform: transform_from_row(r, 2)?,
                    },
                ))
            })
            .and_then(Iterator::collect),
        )?;
        if rows.is_empty() || rows.iter().any(|(s, _)| s != "ok") {
            return Ok(None);
        }
        if !self
            .stored_outputs(file_id)?
            .into_iter()
            .all(|p| Path::new(&self.resolved(p)).is_file())
        {
            return Ok(None);
        }
        Ok(Some(rows.into_iter().map(|(_, s)| s).collect()))
    }

    /// Every entry fetched alone out of the zip at `url`, as its path within the zip.
    pub fn entries_ingested_under(&self, url: &str) -> Result<Vec<String>, ManifestError> {
        let prefix = format!("{url}!");
        let pattern = format!("{}%", like_escaped(&prefix));
        let mut stmt = self.sql(self.conn.prepare_cached(
            "SELECT DISTINCT f.url, e.entry FROM ingest_entries e JOIN ingests i ON e.ingest_id = i.id JOIN files f ON i.file_id = f.id
             WHERE i.status = 'ok' AND f.url LIKE ?1 ESCAPE '\\' ORDER BY f.url, e.entry",
        ))?;
        let rows: Vec<(String, String)> = self.sql(
            stmt.query_map([pattern], |r| Ok((r.get(0)?, r.get(1)?)))
                .and_then(Iterator::collect),
        )?;
        Ok(rows
            .into_iter()
            .map(|(file_url, entry)| format!("{}!{entry}", &file_url[prefix.len()..]))
            .collect())
    }

    /// Whether a read of the whole zip at `url` put `entry`, or anything nested in it, in the lake.
    pub fn is_ingested_from(&self, url: &str, entry: &str) -> Result<bool, ManifestError> {
        let nested = format!("{}!%", like_escaped(entry));
        let r = self.conn.query_row(
            "SELECT COUNT(*) FROM ingest_entries e JOIN ingests i ON e.ingest_id = i.id JOIN files f ON i.file_id = f.id
             WHERE i.status = 'ok' AND f.url = ?1 AND (e.entry = ?2 OR e.entry LIKE ?3 ESCAPE '\\')",
            params![url, entry, nested],
            |r| r.get::<_, i64>(0),
        );
        Ok(self.sql(r)? > 0)
    }

    /// Paths of every output a file's ingests wrote, for superseding.
    pub fn outputs_of_file(&self, file_id: i64) -> Result<Vec<String>, ManifestError> {
        let paths = self.stored_outputs(file_id)?;
        Ok(paths.into_iter().map(|p| self.resolved(p)).collect())
    }

    /// Paths of every output a file's ingests wrote, as stored.
    fn stored_outputs(&self, file_id: i64) -> Result<Vec<String>, ManifestError> {
        let mut stmt = self.sql(self.conn.prepare_cached("SELECT o.parquet_path FROM outputs o JOIN ingests i ON o.ingest_id = i.id WHERE i.file_id = ?1"))?;
        self.sql(
            stmt.query_map([file_id], |r| r.get(0))
                .and_then(Iterator::collect),
        )
    }

    /// Paths of the quarantine files a file's current ingests wrote.
    pub fn quarantine_of_file(&self, file_id: i64) -> Result<Vec<String>, ManifestError> {
        let mut stmt = self.sql(self.conn.prepare_cached("SELECT DISTINCT quarantine_path FROM ingests WHERE file_id = ?1 AND quarantine_path IS NOT NULL AND status != 'superseded' ORDER BY quarantine_path"))?;
        let paths: Vec<String> = self.sql(
            stmt.query_map([file_id], |r| r.get(0))
                .and_then(Iterator::collect),
        )?;
        Ok(paths.into_iter().map(|p| self.resolved(p)).collect())
    }

    /// Paths of every quarantine file a current ingest recorded.
    pub fn live_quarantine(&self) -> Result<Vec<String>, ManifestError> {
        let mut stmt = self.sql(self.conn.prepare_cached("SELECT DISTINCT quarantine_path FROM ingests WHERE quarantine_path IS NOT NULL AND status != 'superseded' ORDER BY quarantine_path"))?;
        let paths: Vec<String> =
            self.sql(stmt.query_map([], |r| r.get(0)).and_then(Iterator::collect))?;
        Ok(paths.into_iter().map(|p| self.resolved(p)).collect())
    }

    fn entries_by_ingest(
        &self,
        table: Option<&str>,
    ) -> Result<std::collections::HashMap<i64, Vec<EntryShare>>, ManifestError> {
        let mut stmt = self.sql(self.conn.prepare_cached(
            "SELECT e.ingest_id, e.entry, e.rows, e.bad_rows, e.min_time, e.max_time, e.server_last_modified, e.zip_entry_modified, e.filename_timestamp
             FROM ingest_entries e JOIN ingests i ON e.ingest_id = i.id
             WHERE ?1 IS NULL OR i.table_name = ?1 ORDER BY e.ingest_id, e.entry",
        ))?;
        let rows: Vec<(i64, EntryShare)> = self.sql(
            stmt.query_map([table], |r| {
                Ok((
                    r.get(0)?,
                    EntryShare {
                        entry: r.get(1)?,
                        rows: r.get::<_, i64>(2)? as u64,
                        bad_rows: r.get::<_, i64>(3)? as u64,
                        min_time: r.get(4)?,
                        max_time: r.get(5)?,
                        server_last_modified: r.get(6)?,
                        zip_entry_modified: r.get(7)?,
                        filename_timestamp: r.get(8)?,
                    },
                ))
            })
            .and_then(Iterator::collect),
        )?;
        let mut by: std::collections::HashMap<i64, Vec<EntryShare>> =
            std::collections::HashMap::new();
        for (id, entry) in rows {
            by.entry(id).or_default().push(entry);
        }
        Ok(by)
    }

    /// Every output of an `ok` ingest, of one table or all, in Iceberg commit order.
    pub fn live_outputs(&self, table: Option<&str>) -> Result<Vec<LiveOutput>, ManifestError> {
        let entries = self.entries_by_ingest(table)?;
        let mut stmt = self.sql(self.conn.prepare_cached(
            "SELECT i.table_name, o.parquet_path, o.rows, o.bytes, i.schema_id, i.source_version, i.source_period, f.url, i.id, o.min_time, o.max_time, o.partition, f.last_modified_at
             FROM outputs o JOIN ingests i ON o.ingest_id = i.id JOIN files f ON i.file_id = f.id
             WHERE i.status = 'ok' AND (?1 IS NULL OR i.table_name = ?1)
             ORDER BY i.table_name, i.source_period, f.url, o.id",
        ))?;
        let rows: Vec<(LiveOutput, i64)> = self.sql(
            stmt.query_map([table], |r| {
                Ok((
                    LiveOutput {
                        table: r.get(0)?,
                        path: r.get(1)?,
                        rows: r.get::<_, i64>(2)? as u64,
                        bytes: r.get::<_, i64>(3)? as u64,
                        schema_id: r.get(4)?,
                        source_version: r.get::<_, i64>(5)? as u16,
                        source_period: r.get(6)?,
                        url: r.get(7)?,
                        entries: Vec::new(),
                        min_time: r.get(9)?,
                        max_time: r.get(10)?,
                        partition: r.get(11)?,
                        last_modified_at: r.get(12)?,
                    },
                    r.get(8)?,
                ))
            })
            .and_then(Iterator::collect),
        )?;
        Ok(rows
            .into_iter()
            .map(|(mut o, ingest)| {
                o.entries = entries.get(&ingest).cloned().unwrap_or_default();
                o.path = self.resolved(o.path);
                o
            })
            .collect())
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

    /// Replaces what a file's earlier read wrote, keeping `kept`; returns its Parquet file count.
    ///
    /// Removed files take emptied directories with them, up to the lake root.
    pub fn supersede(&mut self, file_id: i64, kept: &[String]) -> Result<usize, ManifestError> {
        let old = self.outputs_of_file(file_id)?;
        let lake = self.home.as_ref().map(|h| h.join("lake"));
        for p in old.iter().filter(|p| !kept.contains(p)) {
            remove_output(Path::new(p), lake.as_deref());
        }
        for p in self
            .quarantine_of_file(file_id)?
            .iter()
            .filter(|p| !kept.contains(p))
        {
            let _ = std::fs::remove_file(p);
        }
        self.supersede_file(file_id)?;
        Ok(old.len())
    }

    /// Starts an ingest; a previous row for the same key is replaced.
    pub fn begin_ingest(&mut self, s: &IngestStart) -> Result<i64, ManifestError> {
        let path = self.path.clone();
        let err = |er: rusqlite::Error| ManifestError::Sqlite {
            path: path.clone(),
            message: er.to_string(),
        };
        let tx = self.conn.transaction().map_err(err)?;
        tx.execute(
            "DELETE FROM ingests WHERE file_id = ?1 AND table_name = ?2 AND schema_version = ?3 AND fingerprint = ?4",
            params![s.file_id, s.table, i64::from(s.schema_version), s.fingerprint],
        )
        .map_err(err)?;
        let t = &s.transform;
        tx.execute(
            "INSERT INTO ingests (run_id, file_id, table_name, schema_version, fingerprint, status, started_at,
                                  report_type, report_subtype, source_version, source_columns, plan_hash, schema_source, resolution, source_period, shape_id, schema_id)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                s.run_id,
                s.file_id,
                s.table,
                i64::from(s.schema_version),
                s.fingerprint,
                now(),
                t.block_family,
                t.block_name,
                i64::from(t.source_version),
                t.source_columns.join(","),
                t.plan_hash,
                t.schema_source,
                t.resolution,
                t.source_period,
                t.shape_id,
                t.schema_id
            ],
        )
        .map_err(err)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(err)?;
        Ok(id)
    }

    /// Finishes an ingest and records its outputs in one transaction.
    pub fn finish_ingest(&mut self, ingest_id: i64, e: &IngestEnd) -> Result<(), ManifestError> {
        let (min, max) = e
            .outputs
            .iter()
            .fold((None::<i64>, None::<i64>), |(mn, mx), o| {
                (min_time(mn, o.min_time), max_time(mx, o.max_time))
            });
        let path = self.path.clone();
        let quarantine = e.quarantine_path.as_deref().map(|q| self.stored(q));
        let outputs: Vec<String> = e.outputs.iter().map(|o| self.stored(&o.path)).collect();
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
                params![ingest_id, e.status.as_str(), e.rows as i64, e.bad_rows as i64, quarantine, min, max, now(), e.error_code],
            )
            .map_err(|er| ManifestError::Sqlite { path: path.clone(), message: er.to_string() })?;
        if n == 0 {
            return Err(ManifestError::NotFound {
                kind: "ingest",
                id: ingest_id,
            });
        }
        for x in &e.entries {
            tx.execute(
                "INSERT OR REPLACE INTO ingest_entries (ingest_id, entry, rows, bad_rows, min_time, max_time, server_last_modified, zip_entry_modified, filename_timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![ingest_id, x.entry, x.rows as i64, x.bad_rows as i64, x.min_time, x.max_time, x.server_last_modified, x.zip_entry_modified, x.filename_timestamp],
            )
            .map_err(|er| ManifestError::Sqlite { path: path.clone(), message: er.to_string() })?;
        }
        for (o, stored) in e.outputs.iter().zip(&outputs) {
            tx.execute(
                "INSERT OR REPLACE INTO outputs (ingest_id, parquet_path, partition, rows, bytes, min_time, max_time) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![ingest_id, stored, o.partition.path_segment(), o.rows as i64, o.bytes as i64, o.min_time, o.max_time],
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

    /// Ingests, newest first, optionally for one table or only failed ones.
    pub fn list_ingests(
        &self,
        table: Option<&str>,
        failed_only: bool,
        limit: usize,
    ) -> Result<Vec<IngestRecord>, ManifestError> {
        let sql = format!(
            "SELECT i.id, i.run_id, f.url, i.table_name, i.schema_version, i.status, i.rows, i.bad_rows, i.started_at, i.finished_at, i.error_code,
                    i.fingerprint, i.report_type, i.report_subtype, i.source_version, i.source_columns, i.plan_hash, i.schema_source, i.resolution, i.source_period, i.shape_id, i.schema_id
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
                    entries: Vec::new(),
                    table: r.get(3)?,
                    schema_version: r.get::<_, i64>(4)? as u16,
                    fingerprint: r.get(11)?,
                    transform: transform_from_row(r, 12)?,
                    status: IngestStatus::parse(&r.get::<_, String>(5)?),
                    rows: r.get::<_, i64>(6)? as u64,
                    bad_rows: r.get::<_, i64>(7)? as u64,
                    started_at: r.get(8)?,
                    finished_at: r.get(9)?,
                    error_code: r.get(10)?,
                    outputs: vec![],
                })
            })
            .and_then(Iterator::collect),
        )?;
        let mut stmt = self.sql(self.conn.prepare_cached("SELECT parquet_path, partition, rows, bytes, min_time, max_time FROM outputs WHERE ingest_id = ?1 ORDER BY id"))?;
        let mut entries = self.entries_by_ingest(table)?;
        for rec in &mut recs {
            rec.entries = entries.remove(&rec.id).unwrap_or_default();
        }
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
                .and_then(|rows| rows.collect::<rusqlite::Result<Vec<WrittenFile>>>()),
            )?;
            for o in &mut rec.outputs {
                o.path = self.resolved(std::mem::take(&mut o.path));
            }
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
                        phasor_core::Category::INTERNAL,
                        "A stored diagnostic could not be decoded.",
                    )
                });
                Ok((r.get(0)?, r.get(1)?, d))
            })
            .and_then(Iterator::collect),
        )
    }
}

/// One ingest of a source: the table it went to and its transform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestShape {
    /// Table.
    pub table: String,
    /// The shape that was read and the rules it was read with.
    pub transform: Transform,
}

/// Reads the transform columns of an `ingests` row, starting at `at`.
/// `s` as a LIKE pattern matches it literally, with `\\` as the escape.
fn like_escaped(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn transform_from_row(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Transform> {
    let columns: String = r.get(at + 3)?;
    Ok(Transform {
        block_family: r.get(at)?,
        block_name: r.get(at + 1)?,
        source_version: r.get::<_, i64>(at + 2)? as u16,
        source_columns: if columns.is_empty() {
            vec![]
        } else {
            columns.split(',').map(str::to_owned).collect()
        },
        plan_hash: r.get(at + 4)?,
        schema_source: r.get(at + 5)?,
        resolution: r.get(at + 6)?,
        source_period: r.get(at + 7)?,
        shape_id: r.get(at + 8)?,
        schema_id: r.get(at + 9)?,
    })
}

fn parse_partition(seg: &str) -> crate::writer::Partition {
    use crate::writer::Partition;
    if let Some(rest) = seg.strip_prefix("snapshot=")
        && rest.len() == 6
        && let (Ok(y), Ok(m)) = (rest[..4].parse(), rest[4..].parse())
    {
        return Partition::Snapshot { year: y, month: m };
    }
    if let Some(rest) = seg.strip_prefix("snapshot=")
        && rest.len() == 8
        && let (Ok(y), Ok(m), Ok(d)) = (rest[..4].parse(), rest[4..6].parse(), rest[6..].parse())
    {
        return Partition::SnapshotDay {
            year: y,
            month: m,
            day: d,
        };
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

/// Removes a file and each directory above it left empty, up to `lake`.
fn remove_output(path: &Path, lake: Option<&Path>) {
    let _ = std::fs::remove_file(path);
    let Some(lake) = lake else { return };
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == lake || !d.starts_with(lake) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[cfg(test)]
mod tests {
    /// One entry's share, its counts left to the ingest's.
    fn share(entry: impl Into<String>) -> EntryShare {
        EntryShare {
            entry: entry.into(),
            ..EntryShare::default()
        }
    }
    use phasor_core::Category;

    use super::*;
    use crate::writer::Partition;

    fn transform() -> Transform {
        Transform {
            block_family: "A".into(),
            block_name: "B".into(),
            source_version: 3,
            source_columns: vec!["X".into(), "Y".into()],
            source_period: Some("2024-01".into()),
            plan_hash: "plan:1".into(),
            schema_source: "registry".into(),
            resolution: "exact".into(),
            shape_id: Some("0123456789abcdef".into()),
            schema_id: Some(3),
        }
    }

    fn file(url: &str, sha: &str) -> FileRecord {
        FileRecord {
            source: "nem-mmsdm".into(),
            url: url.into(),
            cache_path: "/cache/x.zip".into(),
            sha256: sha.into(),
            size: 10,
            etag: Some("e".into()),
            last_modified: None,
            last_modified_at: Some("2024-02-01T00:00:00Z".into()),
            fetched_at: Some("2024-02-01T01:00:00Z".into()),
        }
    }

    #[test]
    fn a_superseded_read_leaves_only_what_was_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let mut m = Manifest::open(&home.join("manifest.sqlite")).unwrap();
        // A run that ended without finishing left an ingest running and a
        // file half written.
        let crashed = m.start_run("ingest").unwrap();
        let fid = m.record_file(&file("https://h/a.zip", "aaa")).unwrap();
        let start = IngestStart {
            run_id: crashed,
            file_id: fid,
            table: "T".into(),
            schema_version: 1,
            fingerprint: "xxh3:1".into(),
            transform: transform(),
        };
        m.begin_ingest(&start).unwrap();
        let half = home.join("lake/nem/t/data/year=2024/month=02/.tmp-1.parquet");
        std::fs::create_dir_all(half.parent().unwrap()).unwrap();
        std::fs::write(&half, b"x").unwrap();
        let run = m.start_run("ingest").unwrap();
        assert!(!half.exists());
        assert_eq!(
            m.list_ingests(None, false, 5).unwrap()[0].status,
            IngestStatus::Aborted
        );
        let iid = m
            .begin_ingest(&IngestStart {
                run_id: run,
                ..start
            })
            .unwrap();
        let file_at = |p: &str| {
            let p = home.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
            p.display().to_string()
        };
        let (gone, again) = (
            file_at("lake/nem/t/data/year=2024/month=01/x.parquet"),
            file_at("lake/nem/t/data/year=2024/month=03/y.parquet"),
        );
        let quarantined = file_at("quarantine/t/a.jsonl");
        let output = |path: &str| WrittenFile {
            path: path.to_owned(),
            partition: Partition::Unknown,
            rows: 1,
            bytes: 1,
            min_time: None,
            max_time: None,
            sorted: false,
        };
        m.finish_ingest(
            iid,
            &IngestEnd {
                entries: vec![share("A.CSV")],
                status: IngestStatus::Ok,
                rows: 2,
                bad_rows: 1,
                quarantine_path: Some(quarantined.clone()),
                outputs: vec![output(&gone), output(&again)],
                error_code: None,
            },
        )
        .unwrap();
        assert_eq!(m.supersede(fid, std::slice::from_ref(&again)).unwrap(), 2);
        // The file not written again is gone, and its emptied partition
        // with it; the one written again, and the table above, stay.
        assert!(!Path::new(&gone).exists());
        assert!(!home.join("lake/nem/t/data/year=2024/month=01").exists());
        assert!(Path::new(&again).exists());
        assert!(!Path::new(&quarantined).exists());
        assert!(m.outputs_of_file(fid).unwrap().is_empty());
        // In memory, with no home: a file is removed, and nothing above it.
        let mut mem = Manifest::in_memory().unwrap();
        mem.start_run("x").unwrap();
        let lone = file_at("elsewhere/z.parquet");
        remove_output(Path::new(&lone), None);
        assert!(!Path::new(&lone).exists() && home.join("elsewhere").exists());
    }

    #[test]
    fn paths_under_the_home_are_kept_relative_so_a_moved_home_names_its_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let mut m = Manifest::open(&a.join("manifest.sqlite")).unwrap();
        let run = m.begin_run("ingest").unwrap();
        let under = |home: &Path, p: &str| home.join(p).display().to_string();
        let fid = m
            .record_file(&FileRecord {
                cache_path: under(&a, "cache/x.zip"),
                ..file("https://h/a.zip", "aaa")
            })
            .unwrap();
        let iid = m
            .begin_ingest(&IngestStart {
                run_id: run,
                file_id: fid,
                table: "T".into(),
                schema_version: 1,
                fingerprint: "xxh3:1".into(),
                transform: transform(),
            })
            .unwrap();
        let output = |path: String| WrittenFile {
            path,
            partition: Partition::Month {
                year: 2024,
                month: 1,
            },
            rows: 1,
            bytes: 1,
            min_time: None,
            max_time: None,
            sorted: false,
        };
        m.finish_ingest(
            iid,
            &IngestEnd {
                entries: vec![share("A.CSV")],
                status: IngestStatus::Ok,
                rows: 2,
                bad_rows: 0,
                quarantine_path: Some(under(&a, "quarantine/q.jsonl")),
                outputs: vec![
                    output(under(&a, "lake/nem/t/data/year=2024/month=01/x.parquet")),
                    output("/elsewhere/y.parquet".into()),
                ],
                error_code: None,
            },
        )
        .unwrap();
        // Stored relative to the home; a path outside it as it is.
        let stored: Vec<String> = {
            let mut st = m
                .conn
                .prepare("SELECT parquet_path FROM outputs ORDER BY id")
                .unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(
            stored,
            [
                "lake/nem/t/data/year=2024/month=01/x.parquet",
                "/elsewhere/y.parquet"
            ]
        );
        drop(m);
        // The home moved whole: every path is where it is now.
        std::fs::rename(&a, &b).unwrap();
        let m = Manifest::open(&b.join("manifest.sqlite")).unwrap();
        assert_eq!(
            m.outputs_of_file(fid).unwrap(),
            [
                under(&b, "lake/nem/t/data/year=2024/month=01/x.parquet"),
                "/elsewhere/y.parquet".to_owned()
            ]
        );
        assert_eq!(
            m.live_outputs(None).unwrap()[0].path,
            m.outputs_of_file(fid).unwrap()[0]
        );
        assert_eq!(
            m.quarantine_of_file(fid).unwrap(),
            [under(&b, "quarantine/q.jsonl")]
        );
        assert_eq!(
            m.live_quarantine().unwrap(),
            [under(&b, "quarantine/q.jsonl")]
        );
        assert_eq!(
            m.file_by_url("https://h/a.zip")
                .unwrap()
                .unwrap()
                .1
                .cache_path,
            under(&b, "cache/x.zip")
        );
        assert_eq!(
            m.list_files().unwrap()[0].cache_path,
            under(&b, "cache/x.zip")
        );
        let ingests = m.list_ingests(None, false, 10).unwrap();
        assert_eq!(
            ingests[0].outputs[0].path,
            under(&b, "lake/nem/t/data/year=2024/month=01/x.parquet")
        );
    }

    /// One run on a home at a time: a second is refused while the first holds it.
    #[test]
    fn a_second_run_on_a_home_is_refused_while_the_first_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.sqlite");
        let mut first = Manifest::open(&path).unwrap();
        first.start_run("one").unwrap();
        // The same manifest starts its next run under the lock it holds.
        first.start_run("one again").unwrap();
        let mut second = Manifest::open(&path).unwrap();
        let e = second.start_run("two").unwrap_err();
        assert!(matches!(e, ManifestError::Busy { .. }), "{e}");
        assert_eq!(phasor_core::Diagnose::diagnostic(&e).code, "manifest.busy");
        drop(first);
        second.start_run("two").unwrap();
        // A home that cannot hold a lock file: refused, saying why.
        let gone = dir.path().join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        let mut third = Manifest::open(&gone.join("manifest.sqlite")).unwrap();
        std::fs::create_dir_all(gone.join("lake.lock")).unwrap();
        assert!(matches!(
            third.start_run("three"),
            Err(ManifestError::Busy { .. })
        ));
    }

    #[test]
    fn migrates_and_round_trips_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("manifest.sqlite");
        let m = Manifest::open(&p).unwrap();
        assert_eq!(m.version().unwrap(), MIGRATIONS.len() as u32);
        // Reopening does not re-run migrations.
        drop(m);
        let mut m = Manifest::open(&p).unwrap();
        assert_eq!(m.version().unwrap(), MIGRATIONS.len() as u32);
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
            m.ingested_shapes("https://h/a.zip", "bbb")
                .unwrap()
                .is_none(),
            "no ingests yet"
        );
        assert!(
            m.ingested_shapes("https://h/none", "bbb")
                .unwrap()
                .is_none()
        );
        assert_eq!(m.list_files().unwrap(), [file("https://h/a.zip", "bbb")]);
        let out_path = dir.path().join("out.parquet");
        std::fs::write(&out_path, b"x").unwrap();
        let start = IngestStart {
            run_id: run,
            file_id: fid,
            table: "T".into(),
            schema_version: 1,
            fingerprint: "xxh3:1".into(),
            transform: transform(),
        };
        let iid = m.begin_ingest(&start).unwrap();
        assert!(
            m.ingested_shapes("https://h/a.zip", "bbb")
                .unwrap()
                .is_none(),
            "running"
        );
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
                entries: vec![share("A.CSV")],
                status: IngestStatus::Ok,
                rows: 5,
                bad_rows: 1,
                quarantine_path: Some("q".into()),
                outputs: outputs.clone(),
                error_code: None,
            },
        )
        .unwrap();
        assert_eq!(
            m.ingested_shapes("https://h/a.zip", "bbb").unwrap(),
            Some(vec![IngestShape {
                table: "T".into(),
                transform: transform()
            }])
        );
        assert!(
            m.ingested_shapes("https://h/a.zip", "ccc")
                .unwrap()
                .is_none(),
            "sha changed"
        );
        // A relative path is the home's.
        let q = dir.path().join("sub").join("q").display().to_string();
        assert_eq!(m.quarantine_of_file(fid).unwrap(), std::slice::from_ref(&q));
        assert_eq!(m.live_quarantine().unwrap(), [q]);
        // The live outputs, as the Iceberg tables take them.
        let live = m.live_outputs(Some("T")).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(
            (live[0].rows, live[0].schema_id, live[0].source_version),
            (5, Some(3), 3)
        );
        assert_eq!(live[0].source_period.as_deref(), Some("2024-01"));
        assert_eq!(
            (live[0].url.as_str(), live[0].entries[0].entry.as_str()),
            ("https://h/a.zip", "A.CSV")
        );
        assert_eq!((live[0].min_time, live[0].max_time), (Some(10), Some(20)));
        assert_eq!(live[0].partition, outputs[0].partition.to_string());
        assert_eq!(
            live[0].last_modified_at.as_deref(),
            Some("2024-02-01T00:00:00Z")
        );
        assert_eq!(m.live_outputs(None).unwrap(), live);
        assert!(m.live_outputs(Some("NONE")).unwrap().is_empty());
        let recs = m.list_ingests(None, false, 10).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].fingerprint, "xxh3:1");
        assert_eq!(recs[0].transform, transform());
        // Another column list of the same version is an ingest of its own.
        let other = m
            .begin_ingest(&IngestStart {
                fingerprint: "xxh3:2".into(),
                ..start.clone()
            })
            .unwrap();
        assert_eq!(m.list_ingests(None, false, 10).unwrap().len(), 2);
        assert!(
            m.ingested_shapes("https://h/a.zip", "bbb")
                .unwrap()
                .is_none(),
            "one of its ingests is still running"
        );
        let r = m.conn.execute("DELETE FROM ingests WHERE id = ?1", [other]);
        assert_eq!(r.unwrap(), 1);
        // A Parquet file that is gone makes the source one to read again.
        std::fs::remove_file(&out_path).unwrap();
        assert!(
            m.ingested_shapes("https://h/a.zip", "bbb")
                .unwrap()
                .is_none()
        );
        std::fs::write(&out_path, b"x").unwrap();
        assert_eq!(recs[0].status, IngestStatus::Ok);
        assert_eq!(m.tables_of_run(run).unwrap(), [recs[0].table.clone()]);
        assert!(m.tables_of_run(run + 1).unwrap().is_empty());
        assert_eq!(recs[0].outputs, outputs);
        assert_eq!(recs[0].bad_rows, 1);
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
                entries: vec![share("A.CSV")],
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
        assert!(
            m.ingested_shapes("https://h/a.zip", "bbb")
                .unwrap()
                .is_none()
        );
        // Diagnostics.
        let d = Diagnostic::error("zip.bad_entry", Category::ZIP, "Bad.")
            .with_table("T")
            .with_url("https://h/a.zip");
        m.record_diagnostic(run, Some(iid2), &d).unwrap();
        m.record_diagnostic(
            run,
            None,
            &Diagnostic::warning("schema.compatible_drift", Category::SCHEMA, "Drift.")
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
        // Supersede and abort_running.
        let iid3 = m
            .begin_ingest(&IngestStart {
                table: "U".into(),
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
                entries: vec![share("B.CSV")],
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
        assert!(m.quarantine_of_file(fid).unwrap().is_empty());
        assert!(m.live_quarantine().unwrap().is_empty(), "superseded");
        assert_eq!(
            m.list_ingests(None, false, 10).unwrap()[0].status,
            IngestStatus::Superseded
        );
        // What a superseded source left behind does not hold back the
        // source that replaced it.
        let fresh = m.begin_ingest(&IngestStart { ..start.clone() }).unwrap();
        m.finish_ingest(
            fresh,
            &IngestEnd {
                entries: vec![share("C.CSV")],
                status: IngestStatus::Ok,
                rows: 1,
                bad_rows: 0,
                quarantine_path: None,
                outputs: vec![],
                error_code: None,
            },
        )
        .unwrap();
        assert_eq!(
            m.ingested_shapes("https://h/a.zip", "bbb")
                .unwrap()
                .map(|s| s.len()),
            Some(1)
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
                    entries: vec![share("C.CSV")],
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
        assert_eq!(m.version().unwrap(), MIGRATIONS.len() as u32);
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
        assert_eq!(
            parse_partition("snapshot=20261007"),
            Partition::SnapshotDay {
                year: 2026,
                month: 10,
                day: 7
            }
        );
        assert_eq!(parse_partition("snapshot=2026100"), Partition::Unknown);
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
                table: String::new(),
                schema_version: 0,
                fingerprint: String::new(),
                transform: Transform::default(),
            })
            .is_err()
        );
    }

    /// A manifest without transforms keeps every row through the `ingests` rebuild.
    #[test]
    fn an_older_manifest_is_migrated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("manifest.sqlite");
        let out = dir.path().join("out.parquet");
        std::fs::write(&out, b"x").unwrap();
        {
            let conn = Connection::open(&p).unwrap();
            conn.execute_batch(&format!(
                "PRAGMA foreign_keys = ON; BEGIN; {}; INSERT INTO meta(key, value) VALUES ('migration', '1');
                 INSERT INTO runs (id, argv, started_at, version) VALUES (1, 'ingest x', 't', '0');
                 INSERT INTO files (id, source, url, cache_path, sha256, size, fetched_at) VALUES (1, 'local', 'file:///x.csv', '/x.csv', 'sha', 1, 't');
                 INSERT INTO ingests (id, run_id, file_id, entry, table_name, schema_version, fingerprint, status, rows, started_at)
                     VALUES (7, 1, 1, 'x.csv', 'T', 2, 'xxh3:9', 'ok', 5, 't');
                 INSERT INTO outputs (ingest_id, parquet_path, partition, rows, bytes) VALUES (7, '{}', 'year=2024/month=01', 5, 1);
                 INSERT INTO diagnostics (run_id, ingest_id, severity, code, category, message, json, created_at)
                     VALUES (1, 7, 'warning', 'schema.x', 'schema', 'w', '{{}}', 't');
                 COMMIT;",
                MIGRATIONS[0].1,
                out.display()
            ))
            .unwrap();
        }
        let m = Manifest::open(&p).unwrap();
        assert_eq!(m.version().unwrap(), MIGRATIONS.len() as u32);
        let recs = m.list_ingests(None, false, 10).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(
            (recs[0].id, recs[0].rows, recs[0].schema_version),
            (7, 5, 2)
        );
        assert_eq!(recs[0].fingerprint, "xxh3:9");
        assert_eq!(recs[0].outputs.len(), 1, "outputs survive the rebuild");
        assert_eq!(recs[0].transform, Transform::default());
        let kept: i64 = m
            .conn
            .query_row(
                "SELECT count(*) FROM diagnostics WHERE ingest_id = 7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "diagnostics still point at their ingest");
        // The source is wholly in the lake, by rules nobody recorded.
        let shapes = m.ingested_shapes("file:///x.csv", "sha").unwrap().unwrap();
        assert_eq!(shapes[0].transform.plan_hash, "");
        assert!(shapes[0].transform.source_columns.is_empty());
        // Foreign keys are back on: an ingest that goes takes its outputs.
        m.conn
            .execute("DELETE FROM ingests WHERE id = 7", [])
            .unwrap();
        assert!(m.outputs_of_file(1).unwrap().is_empty());
    }

    /// Rules hashes from migration 3 survive as hashes that match no plan.
    #[test]
    fn a_manifest_of_rules_hashes_moves_to_plan_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("manifest.sqlite");
        {
            let conn = Connection::open(&p).unwrap();
            let mut batch = String::from("PRAGMA foreign_keys = OFF; BEGIN;");
            for (_, sql) in &MIGRATIONS[..3] {
                batch.push_str(sql);
            }
            batch.push_str(
                "INSERT INTO meta(key, value) VALUES ('migration', '3');
                 INSERT INTO runs (id, argv, started_at, version) VALUES (1, 'ingest x', 't', '0');
                 INSERT INTO files (id, source, url, cache_path, sha256, size, fetched_at) VALUES (1, 'local', 'file:///x.csv', '/x.csv', 'sha', 1, 't');
                 INSERT INTO ingests (id, run_id, file_id, entry, table_name, schema_version, fingerprint, status, rows, started_at,
                                      report_type, report_subtype, source_version, source_columns, rules, schema_source, resolution, source_period)
                     VALUES (7, 1, 1, 'x.csv', 'T', 1, 'xxh3:9', 'ok', 5, 't', 'A', 'B', 1, 'X,Y', 'rules:abc', 'registry', 'exact', '2024-01');
                 COMMIT;",
            );
            conn.execute_batch(&batch).unwrap();
        }
        let mut m = Manifest::open(&p).unwrap();
        assert_eq!(m.version().unwrap(), MIGRATIONS.len() as u32);
        let shapes = m.ingested_shapes("file:///x.csv", "sha").unwrap().unwrap();
        let t = &shapes[0].transform;
        assert_eq!(t.plan_hash, "rules:abc");
        assert_eq!((t.shape_id.clone(), t.schema_id), (None, None));
        assert_eq!(t.source_columns, ["X", "Y"]);
        assert_eq!(t.source_period.as_deref(), Some("2024-01"));
        // A new ingest records both.
        let iid = m
            .begin_ingest(&IngestStart {
                run_id: 1,
                file_id: 1,
                table: "T".into(),
                schema_version: 1,
                fingerprint: "xxh3:9".into(),
                transform: transform(),
            })
            .unwrap();
        let rec = m
            .list_ingests(None, false, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.id == iid)
            .unwrap();
        assert_eq!(rec.transform, transform());
    }

    /// Per-entry ingests from migration 7 merge into one per source file and shape.
    #[test]
    fn ingests_per_entry_merge_into_one_per_source_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("manifest.sqlite");
        {
            let conn = Connection::open(&p).unwrap();
            let mut batch = String::from("PRAGMA foreign_keys = OFF; BEGIN;");
            for (_, sql) in &MIGRATIONS[..7] {
                batch.push_str(sql);
                batch.push(';');
            }
            batch.push_str(
                "INSERT INTO meta(key, value) VALUES ('migration', '7');
                 INSERT INTO runs (id, argv, started_at, version) VALUES (1, 'ingest z', 't', '0');
                 INSERT INTO files (id, source, url, cache_path, sha256, size, fetched_at) VALUES (1, 'local', 'https://h/z.zip', '/z.zip', 'sha', 1, 't');
                 INSERT INTO ingests (id, run_id, file_id, entry, table_name, schema_version, fingerprint, status, rows, bad_rows, min_time, max_time, started_at, finished_at)
                     VALUES (3, 1, 1, 'A.CSV', 'T', 1, 'xxh3:1', 'ok', 5, 1, 10, 20, 't', 't1'),
                            (4, 1, 1, 'B.CSV', 'T', 1, 'xxh3:1', 'ok', 7, 0, 5, 30, 't', 't2'),
                            (5, 1, 1, 'A.CSV', 'U', 1, 'xxh3:2', 'ok', 2, 0, NULL, NULL, 't', 't'),
                            (6, 1, 1, 'B.CSV', 'U', 1, 'xxh3:2', 'failed', 0, 0, NULL, NULL, 't', 't');
                 INSERT INTO outputs (ingest_id, parquet_path, partition, rows, bytes) VALUES
                     (3, '/l/a.parquet', 'year=2024/month=01', 5, 1),
                     (4, '/l/b.parquet', 'year=2024/month=01', 7, 1);
                 INSERT INTO diagnostics (run_id, ingest_id, severity, code, category, message, json, created_at)
                     VALUES (1, 4, 'warning', 'schema.x', 'schema', 'w', '{}', 't');
                 COMMIT;",
            );
            conn.execute_batch(&batch).unwrap();
        }
        let m = Manifest::open(&p).unwrap();
        assert_eq!(m.version().unwrap(), MIGRATIONS.len() as u32);
        let mut recs = m.list_ingests(None, false, 10).unwrap();
        recs.sort_by_key(|r| r.id);
        assert_eq!(recs.len(), 2, "one per shape of the file");
        let t = &recs[0];
        assert_eq!(
            (t.id, t.table.as_str(), t.rows, t.bad_rows),
            (3, "T", 12, 1)
        );
        assert_eq!(t.status, IngestStatus::Ok);
        assert_eq!(t.outputs.len(), 2, "both entries' outputs");
        let entries: Vec<(&str, u64)> = t
            .entries
            .iter()
            .map(|e| (e.entry.as_str(), e.rows))
            .collect();
        assert_eq!(entries, [("A.CSV", 5), ("B.CSV", 7)]);
        assert!(t.entries.iter().all(|e| {
            e.server_last_modified.is_none()
                && e.zip_entry_modified.is_none()
                && e.filename_timestamp.is_none()
        }));
        let u = &recs[1];
        assert_eq!(
            (u.id, u.status),
            (5, IngestStatus::Failed),
            "an entry failed"
        );
        let moved: i64 = m
            .conn
            .query_row(
                "SELECT count(*) FROM diagnostics WHERE ingest_id = 3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(moved, 1, "the diagnostic moved with its entry");
        // The skip list reads the shares.
        assert!(m.is_ingested_from("https://h/z.zip", "B.CSV").unwrap());
        let live = m.live_outputs(Some("T")).unwrap();
        assert_eq!(live.len(), 2);
        assert_eq!(live[0].entries.len(), 2);
    }

    /// A failed migration leaves the manifest as it was and names itself.
    #[test]
    fn a_failed_migration_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("manifest.sqlite");
        {
            // Claims to be at migration 1 but has no `ingests` to rebuild.
            let conn = Connection::open(&p).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta(key, value) VALUES ('migration', '1');",
            )
            .unwrap();
        }
        match Manifest::open(&p) {
            Err(ManifestError::Migration { at, failed, .. }) => {
                assert_eq!(at, 1);
                assert_eq!(failed, "0002_transform");
            }
            other => panic!("{:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn entries_fetched_alone_and_whole_zips_know_of_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Manifest::open(&dir.path().join("m.sqlite")).unwrap();
        let run = m.begin_run("get").unwrap();
        let ok = |m: &mut Manifest, url: &str, entries: &[&str]| {
            let fid = m.record_file(&file(url, "s")).unwrap();
            let iid = m
                .begin_ingest(&IngestStart {
                    run_id: run,
                    file_id: fid,
                    table: "T".into(),
                    schema_version: 1,
                    fingerprint: "xxh3:1".into(),
                    transform: transform(),
                })
                .unwrap();
            m.finish_ingest(
                iid,
                &IngestEnd {
                    entries: entries.iter().map(|e| share(*e)).collect(),
                    status: IngestStatus::Ok,
                    rows: 1,
                    bad_rows: 0,
                    quarantine_path: None,
                    outputs: vec![],
                    error_code: None,
                },
            )
            .unwrap();
        };
        let zip = "https://h/M_2014_01.zip";
        // Two entries fetched alone: one a nested zip with a CSV, one a CSV.
        ok(
            &mut m,
            &format!("{zip}!PUBLIC_DVD_A_201401010000.zip"),
            &["A.CSV"],
        );
        ok(&mut m, &format!("{zip}!d/notes.csv"), &["d/notes.csv"]);
        // A zip of a near name, and a failed ingest, do not count.
        ok(&mut m, "https://h/M_2014_01.zip.bak!X.zip", &["X.CSV"]);
        let fid = m
            .record_file(&file(&format!("{zip}!PUBLIC_DVD_B_201401010000.zip"), "s"))
            .unwrap();
        let iid = m
            .begin_ingest(&IngestStart {
                run_id: run,
                file_id: fid,
                table: "T".into(),
                schema_version: 1,
                fingerprint: "xxh3:1".into(),
                transform: transform(),
            })
            .unwrap();
        m.finish_ingest(
            iid,
            &IngestEnd {
                entries: vec![share("B.CSV")],
                status: IngestStatus::Failed,
                rows: 0,
                bad_rows: 0,
                quarantine_path: None,
                outputs: vec![],
                error_code: Some("x".into()),
            },
        )
        .unwrap();
        assert_eq!(
            m.entries_ingested_under(zip).unwrap(),
            [
                "PUBLIC_DVD_A_201401010000.zip!A.CSV",
                "d/notes.csv!d/notes.csv"
            ]
        );
        assert!(
            m.entries_ingested_under("https://h/none.zip")
                .unwrap()
                .is_empty()
        );
        // The whole zip read: its entries, nested paths joined with `!`.
        ok(
            &mut m,
            zip,
            &["PUBLIC_DVD_C_201401010000.zip!C.CSV", "d/plain.csv"],
        );
        assert!(
            m.is_ingested_from(zip, "PUBLIC_DVD_C_201401010000.zip")
                .unwrap()
        );
        assert!(m.is_ingested_from(zip, "d/plain.csv").unwrap());
        assert!(
            !m.is_ingested_from(zip, "PUBLIC_DVD_A_201401010000.zip")
                .unwrap()
        );
        assert!(
            !m.is_ingested_from(zip, "PUBLIC_DVD_C_2014").unwrap(),
            "a prefix is not an entry"
        );
        assert!(
            !m.is_ingested_from("https://h/other.zip", "d/plain.csv")
                .unwrap()
        );
    }
}
