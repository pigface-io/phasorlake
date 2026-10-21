-- An ingest records the Iceberg schema its shape is written under: the
-- schema of the table's history whose columns are the block's, when the
-- census has folded that column list. The lake's Iceberg tables are built
-- from it, one snapshot per run of ingests of one schema.
ALTER TABLE ingests ADD COLUMN schema_id INTEGER;
