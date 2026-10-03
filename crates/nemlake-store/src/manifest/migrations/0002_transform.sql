-- Every ingest records its transform: the shape that was read (report pair,
-- version as published, column list) and the rules it was read with (their
-- hash, where the table's definition came from, how the shape matched it).
-- With both, an ingest whose rules have changed since can be found without
-- opening the source, and done again from the archive.
--
-- Two column lists of one version in one entry are two ingests, so the
-- shape's fingerprint joins the key. SQLite cannot alter a constraint: the
-- table is rebuilt. The migration runs with foreign keys off, so outputs
-- and diagnostics keep pointing at the same ingest ids.
--
-- Rows from before this migration have an empty rules hash, which never
-- equals a current one: their sources are read again on the next run that
-- covers them.

CREATE TABLE ingests_new (
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
    report_type     TEXT    NOT NULL DEFAULT '',
    report_subtype  TEXT    NOT NULL DEFAULT '',
    source_version  INTEGER NOT NULL DEFAULT 0,
    source_columns  TEXT    NOT NULL DEFAULT '',
    rules           TEXT    NOT NULL DEFAULT '',
    schema_source   TEXT    NOT NULL DEFAULT '',
    resolution      TEXT    NOT NULL DEFAULT '',
    UNIQUE (file_id, entry, table_name, schema_version, fingerprint)
);

INSERT INTO ingests_new (id, run_id, file_id, entry, table_name, schema_version, fingerprint, status, rows, bad_rows,
                         quarantine_path, min_time, max_time, started_at, finished_at, error_code)
SELECT id, run_id, file_id, entry, table_name, schema_version, fingerprint, status, rows, bad_rows,
       quarantine_path, min_time, max_time, started_at, finished_at, error_code
FROM ingests;

DROP TABLE ingests;
ALTER TABLE ingests_new RENAME TO ingests;

CREATE INDEX ingests_table ON ingests (table_name, status);
CREATE INDEX ingests_file ON ingests (file_id);
