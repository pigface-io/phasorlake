-- An ingest records the hash of the conformance plan its shape was read
-- with, which covers only what decides that shape's output, in place of
-- the hash of the whole table's rules: declaring a column puts behind only
-- the sources whose shapes carry it. The values already recorded are rules
-- hashes, which never equal a plan hash, so every source ingested before
-- this migration is read again by the next run that covers it.
--
-- Beside it, the ids the census gives the shape (comma-separated, sorted),
-- when the census has seen it.
ALTER TABLE ingests RENAME COLUMN rules TO plan_hash;
ALTER TABLE ingests ADD COLUMN shape_id TEXT;
