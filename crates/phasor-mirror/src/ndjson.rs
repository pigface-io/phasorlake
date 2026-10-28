//! The mirror's records of one JSON value a line: read whole, appended to.

use std::fs::File;
use std::io::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::error::MirrorError;

/// The text of the record at `path`; none when there is none.
pub(crate) fn read(path: &Path) -> Result<Option<String>, MirrorError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(MirrorError::io(path)(e)),
    }
}

/// Appends a line for each of `values` to the record at `path`, made when
/// missing, then `sync`s it.
pub(crate) fn append<T: Serialize>(
    path: &Path,
    values: impl IntoIterator<Item = T>,
    sync: impl FnOnce(&File) -> std::io::Result<()>,
) -> Result<(), MirrorError> {
    let mut text = Vec::new();
    for v in values {
        serde_json::to_writer(&mut text, &v).expect("a record line always serialises");
        text.push(b'\n');
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(&text).map(|()| f))
        .map_err(MirrorError::io(path))?;
    sync(&f).map_err(MirrorError::io(path))
}
