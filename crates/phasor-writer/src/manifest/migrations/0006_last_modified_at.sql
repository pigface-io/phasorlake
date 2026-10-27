-- files.last_modified holds the server's Last-Modified header as sent (an
-- HTTP date) and keeps that meaning. The same time as an instant, RFC 3339
-- UTC, goes beside it. Rows from before this migration have it empty; a
-- later fetch of the URL fills it.

ALTER TABLE files ADD COLUMN last_modified_at TEXT;
