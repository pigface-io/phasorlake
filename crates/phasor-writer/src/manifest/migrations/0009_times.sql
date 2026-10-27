-- Every lake row carries each time its source was stamped with, as found
-- and named for where it was found; so does each entry's share. The
-- server's Last-Modified (was `published`) keeps its values under its
-- explicit name; the zip entry's modified time and the timestamp in the
-- file's name start empty, as the files of lake format 2 hold neither and
-- are read again (lake format 3).
ALTER TABLE ingest_entries RENAME COLUMN published TO server_last_modified;
ALTER TABLE ingest_entries ADD COLUMN zip_entry_modified INTEGER;
ALTER TABLE ingest_entries ADD COLUMN filename_timestamp INTEGER;
