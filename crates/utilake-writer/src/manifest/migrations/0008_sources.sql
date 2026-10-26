-- An ingest is one shape of one source file, and each lake file it writes
-- holds rows of many of the file's entries, every row naming its own
-- (`_ul_source`). What each entry gave is kept beside the ingest, in
-- `ingest_entries`: the one place the manifest names an entry, with when
-- it went public (microseconds since 1970, UTC), which its rows carry as
-- `_ul_server_last_modified`. Entries merged from earlier runs have none: their
-- source files are read again, as lake format 6 asks.
--
-- The ingests of earlier runs were one per entry. Those of one source
-- file, table, version and shape are merged into the first of them: rows
-- summed, times widened, and the merged status `ok` only when every one
-- was. Outputs and diagnostics move to the merged ingest; each old ingest
-- becomes the share of its entry. Nothing is dropped.

CREATE TABLE ingest_merge AS
SELECT id,
       MIN(id) OVER (PARTITION BY file_id, table_name, schema_version, fingerprint) AS into_id
FROM ingests;

CREATE TABLE ingests_new (
    id              INTEGER PRIMARY KEY,
    run_id          INTEGER NOT NULL REFERENCES runs(id),
    file_id         INTEGER NOT NULL REFERENCES files(id),
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
    plan_hash       TEXT    NOT NULL DEFAULT '',
    schema_source   TEXT    NOT NULL DEFAULT '',
    resolution      TEXT    NOT NULL DEFAULT '',
    source_period   TEXT,
    shape_id        TEXT,
    schema_id       INTEGER,
    UNIQUE (file_id, table_name, schema_version, fingerprint)
);

INSERT INTO ingests_new
SELECT first.id, first.run_id, first.file_id, first.table_name, first.schema_version, first.fingerprint,
       CASE WHEN agg.statuses = 1 THEN first.status ELSE 'failed' END,
       agg.rows, agg.bad_rows, first.quarantine_path, agg.min_time, agg.max_time,
       first.started_at, agg.finished_at, agg.error_code,
       first.report_type, first.report_subtype, first.source_version, first.source_columns,
       first.plan_hash, first.schema_source, first.resolution, first.source_period,
       first.shape_id, first.schema_id
FROM ingests first
JOIN (
    SELECT m.into_id,
           COUNT(DISTINCT i.status) AS statuses,
           SUM(i.rows) AS rows,
           SUM(i.bad_rows) AS bad_rows,
           MIN(i.min_time) AS min_time,
           MAX(i.max_time) AS max_time,
           MAX(i.finished_at) AS finished_at,
           MAX(i.error_code) AS error_code
    FROM ingests i JOIN ingest_merge m ON m.id = i.id
    GROUP BY m.into_id
) agg ON agg.into_id = first.id;

CREATE TABLE ingest_entries (
    ingest_id INTEGER NOT NULL REFERENCES ingests(id) ON DELETE CASCADE,
    entry     TEXT    NOT NULL,
    rows      INTEGER NOT NULL DEFAULT 0,
    bad_rows  INTEGER NOT NULL DEFAULT 0,
    min_time  INTEGER,
    max_time  INTEGER,
    published INTEGER,
    UNIQUE (ingest_id, entry)
);

INSERT INTO ingest_entries (ingest_id, entry, rows, bad_rows, min_time, max_time)
SELECT m.into_id, i.entry, SUM(i.rows), SUM(i.bad_rows), MIN(i.min_time), MAX(i.max_time)
FROM ingests i JOIN ingest_merge m ON m.id = i.id
GROUP BY m.into_id, i.entry;

UPDATE outputs SET ingest_id = (SELECT into_id FROM ingest_merge WHERE id = outputs.ingest_id);
UPDATE diagnostics SET ingest_id = (SELECT into_id FROM ingest_merge WHERE id = diagnostics.ingest_id)
WHERE ingest_id IS NOT NULL;

DROP TABLE ingests;
ALTER TABLE ingests_new RENAME TO ingests;
DROP TABLE ingest_merge;

CREATE INDEX ingests_table ON ingests (table_name, status);
CREATE INDEX ingests_file ON ingests (file_id);
CREATE INDEX ingest_entries_entry ON ingest_entries (entry);
