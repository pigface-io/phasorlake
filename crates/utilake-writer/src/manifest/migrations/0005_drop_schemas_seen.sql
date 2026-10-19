-- Every ingest already records the shape it read (fingerprint, report
-- pair, source version and columns, resolution) in its own row, so the
-- first-sighting table nothing read is dropped.
DROP TABLE schemas_seen;
