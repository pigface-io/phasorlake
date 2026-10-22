//! Walking a zip, including zips inside zips, and handing each entry a
//! decoder accepts to a callback as a streaming reader: [`utilake_zip`],
//! shared with the census of utilake-schemas, so both read AEMO's archives
//! the same way. Its errors are diagnosed by `utilake-core`.

pub use utilake_zip::{
    Accepts, ArchiveError, CSV, Entry, MAX_DEPTH, WalkError, Walked, has_extension,
    walk_csv_entries, walk_csv_entries_in, walk_entries, walk_entries_in,
};
