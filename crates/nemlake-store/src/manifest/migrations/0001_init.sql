-- The manifest: what was fetched, what was written where, and what went wrong.
-- Timestamps are RFC 3339 text in UTC. Sizes are bytes.

CREATE TABLE runs (
    id          INTEGER PRIMARY KEY,
    argv        TEXT    NOT NULL,
    started_at  TEXT    NOT NULL,
    finished_at TEXT,
    exit_code   INTEGER,
    summary     TEXT,
    version     TEXT    NOT NULL
);

CREATE TABLE files (
    id            INTEGER PRIMARY KEY,
    source        TEXT NOT NULL,
    url           TEXT NOT NULL UNIQUE,
    cache_path    TEXT NOT NULL,
    sha256        TEXT NOT NULL,
    size          INTEGER NOT NULL,
    etag          TEXT,
    last_modified TEXT,
    fetched_at    TEXT NOT NULL
);

CREATE TABLE ingests (
    id              INTEGER PRIMARY KEY,
    run_id          INTEGER NOT NULL REFERENCES runs(id),
    file_id         INTEGER NOT NULL REFERENCES files(id),
    entry           TEXT    NOT NULL,
    table_name      TEXT    NOT NULL,
    schema_version  INTEGER NOT NULL,
    fingerprint     TEXT    NOT NULL,
    status          TEXT    NOT NULL CHECK (status IN ('running', 'ok', 'failed', 'aborted', 'superseded')),
    rows            INTEGER NOT NULL DEFAULT 0,
    bad_rows        INTEGER NOT NULL DEFAULT 0,
    quarantine_path TEXT,
    min_time        INTEGER,
    max_time        INTEGER,
    started_at      TEXT    NOT NULL,
    finished_at     TEXT,
    error_code      TEXT,
    UNIQUE (file_id, entry, table_name, schema_version)
);

CREATE INDEX ingests_table ON ingests (table_name, status);
CREATE INDEX ingests_file ON ingests (file_id);

CREATE TABLE outputs (
    id           INTEGER PRIMARY KEY,
    ingest_id    INTEGER NOT NULL REFERENCES ingests(id) ON DELETE CASCADE,
    parquet_path TEXT    NOT NULL UNIQUE,
    partition    TEXT    NOT NULL,
    rows         INTEGER NOT NULL,
    bytes        INTEGER NOT NULL,
    min_time     INTEGER,
    max_time     INTEGER
);

CREATE TABLE schemas_seen (
    fingerprint       TEXT PRIMARY KEY,
    table_name        TEXT,
    report_type       TEXT NOT NULL,
    report_subtype    TEXT NOT NULL,
    schema_version    INTEGER NOT NULL,
    columns           TEXT NOT NULL,
    status            TEXT NOT NULL,
    first_seen_file   INTEGER REFERENCES files(id),
    first_seen_at     TEXT NOT NULL
);

CREATE TABLE diagnostics (
    id         INTEGER PRIMARY KEY,
    run_id     INTEGER NOT NULL REFERENCES runs(id),
    ingest_id  INTEGER REFERENCES ingests(id) ON DELETE SET NULL,
    severity   TEXT NOT NULL,
    code       TEXT NOT NULL,
    category   TEXT NOT NULL,
    message    TEXT NOT NULL,
    json       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX diagnostics_run ON diagnostics (run_id, severity);
CREATE INDEX diagnostics_code ON diagnostics (code);

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
