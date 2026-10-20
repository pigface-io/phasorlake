//! The shapes of every table, folded from every census file.
//!
//! The census records each block of each file as it was read; this reads
//! all of them and writes `shapes/{TABLE}.toml` from scratch: one entry per
//! distinct shape, with the AEMO versions and kinds of file it came in, the
//! exact file and time it was first and last seen, and what each column
//! held over all of them. Nothing is widened in place, so a census taken
//! again, or removed, changes the shapes to match.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use utilake_core::YearMonth;
use utilake_mms_registry::Registry;
use utilake_mms_registry::file_model::shapes::{Mark, ShapeColumn, ShapeEntry, render_shapes};
use utilake_mms_registry::shape::{ShapeId, TypeClass};

use crate::census::{
    BlockRecord, CensusError, FileRecord, census_dir, read_month, shapes_dir, table_of, write_text,
};
use crate::profile::ColumnProfile;

/// What a fold wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Folded {
    /// Census files read.
    pub months: usize,
    /// Tables with a shapes file.
    pub tables: usize,
    /// Distinct shapes over every table.
    pub shapes: usize,
}

/// AEMO's `YYYY/MM/DD[ HH:MM:SS[.f]]` as RFC 3339 in market time.
pub fn market_time(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if !(b.len() >= 10
        && digits(0..4)
        && b[4] == b'/'
        && digits(5..7)
        && b[7] == b'/'
        && digits(8..10))
    {
        return None;
    }
    let date = format!("{}-{}-{}", &text[0..4], &text[5..7], &text[8..10]);
    let time = match text.get(10..) {
        Some("") => "00:00:00",
        Some(t) => t.strip_prefix(' ').filter(|t| t.len() >= 8)?,
        None => return None,
    };
    Some(format!("{date}T{time}+10:00"))
}

/// One column of a shape, gathering.
#[derive(Debug, Clone, Default)]
struct ColumnAcc {
    name: String,
    ctl: Option<TypeClass>,
    profile: ColumnProfile,
    nulls_since: Option<(Key, String)>,
}

/// Where in the archive an observation lies: month, data time, file.
type Key = (YearMonth, String, String);

/// One shape of a table, gathering.
#[derive(Debug, Clone)]
struct ShapeAcc {
    report: (String, String),
    versions: BTreeSet<u16>,
    kinds: BTreeSet<String>,
    from: (Key, Mark),
    to: (Key, Mark),
    files: u64,
    /// The last file counted, so a file of two blocks of one shape counts
    /// once.
    counted: (YearMonth, String),
    rows: u64,
    columns: Vec<ColumnAcc>,
}

/// Every shape of every table, gathering from census records.
#[derive(Debug, Default)]
pub struct Fold {
    tables: BTreeMap<String, BTreeMap<ShapeId, ShapeAcc>>,
}

impl Fold {
    /// Takes in one block of `file` of `month`.
    pub fn block(
        &mut self,
        table: &str,
        month: YearMonth,
        file: &str,
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        self.run(table, month, (file, file), 1, f, b);
    }

    /// Takes in a run of `files` files of `month`, from the file named
    /// `first` to the one named `last`, whose blocks of one shape are
    /// merged in `b`.
    pub fn run(
        &mut self,
        table: &str,
        month: YearMonth,
        (first_file, last_file): (&str, &str),
        files: u64,
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        let first = b.first.as_deref().and_then(market_time);
        let last = b.last.as_deref().and_then(market_time);
        let mark = |file: &str, data_time: Option<String>| Mark {
            month,
            file: file.to_owned(),
            sha256: f.sha256.clone(),
            published: f.published.clone(),
            data_time,
        };
        let from_key: Key = (
            month,
            first.clone().unwrap_or_default(),
            first_file.to_owned(),
        );
        let to_key: Key = (
            month,
            last.clone().unwrap_or_default(),
            last_file.to_owned(),
        );
        let acc = self
            .tables
            .entry(table.to_owned())
            .or_default()
            .entry(b.shape)
            .or_insert_with(|| ShapeAcc {
                report: b.report.clone(),
                versions: BTreeSet::new(),
                kinds: BTreeSet::new(),
                from: (from_key.clone(), mark(first_file, first.clone())),
                to: (to_key.clone(), mark(last_file, last.clone())),
                files: 0,
                counted: (month, String::new()),
                rows: 0,
                columns: b
                    .columns
                    .iter()
                    .map(|n| ColumnAcc {
                        name: n.clone(),
                        ..ColumnAcc::default()
                    })
                    .collect(),
            });
        acc.versions.insert(b.version);
        acc.kinds.insert(f.kind.clone());
        if files != 1 || acc.counted != (month, first_file.to_owned()) {
            acc.files += files;
            acc.counted = (month, first_file.to_owned());
        }
        acc.rows += b.rows;
        if from_key < acc.from.0 {
            acc.from = (from_key.clone(), mark(first_file, first));
        }
        if to_key > acc.to.0 {
            acc.to = (to_key, mark(last_file, last));
        }
        for (j, c) in acc.columns.iter_mut().enumerate() {
            let p = b.profile_of(j);
            if p.nulls > 0 && c.nulls_since.as_ref().is_none_or(|(k, _)| from_key < *k) {
                c.nulls_since = Some((from_key.clone(), first_file.to_owned()));
            }
            c.profile.merge(&p);
            let ctl = b.ctl_of(j);
            if ctl != TypeClass::Unknown {
                c.ctl = Some(ctl);
            }
        }
    }

    /// Takes in every block of a month's census.
    pub fn month(
        &mut self,
        registry: &Registry,
        month: YearMonth,
        files: &BTreeMap<String, FileRecord>,
    ) {
        for (file, f) in files {
            for b in &f.block {
                let table = table_of(registry, &b.report.0, &b.report.1);
                self.block(&table, month, file, f, b);
            }
        }
    }

    /// The shapes of each table, by table name, each table's shapes in the
    /// order first seen.
    pub fn shapes(self) -> BTreeMap<String, Vec<ShapeEntry>> {
        self.tables
            .into_iter()
            .map(|(table, shapes)| {
                let spans: Vec<(ShapeId, YearMonth, YearMonth)> = shapes
                    .iter()
                    .map(|(id, a)| (*id, a.from.1.month, a.to.1.month))
                    .collect();
                let mut entries: Vec<ShapeEntry> = shapes
                    .into_iter()
                    .map(|(id, a)| entry(id, a, &spans))
                    .collect();
                entries.sort_by(|a, b| {
                    (a.first_seen, &a.from.file, a.id).cmp(&(b.first_seen, &b.from.file, b.id))
                });
                (table, entries)
            })
            .collect()
    }
}

fn entry(id: ShapeId, a: ShapeAcc, spans: &[(ShapeId, YearMonth, YearMonth)]) -> ShapeEntry {
    let (first_seen, last_seen) = (a.from.1.month, a.to.1.month);
    let text = |v: Option<Vec<u8>>| v.map(|v| String::from_utf8_lossy(&v).into_owned());
    ShapeEntry {
        id,
        report_type: a.report.0,
        report_subtype: a.report.1,
        versions: a.versions.into_iter().collect(),
        file_kinds: a.kinds.into_iter().collect(),
        first_seen,
        last_seen,
        from: a.from.1,
        to: a.to.1,
        files: a.files,
        rows: a.rows,
        concurrent_with: spans
            .iter()
            .filter(|(other, f, l)| *other != id && *f <= last_seen && first_seen <= *l)
            .map(|(other, ..)| *other)
            .collect(),
        columns: a
            .columns
            .into_iter()
            .map(|c| ShapeColumn {
                name: c.name,
                ctl: c.ctl.unwrap_or(TypeClass::Unknown),
                kind: c.profile.kind,
                nulls: c.profile.nulls,
                nulls_since: c.nulls_since.map(|(_, f)| f),
                int_digits: c.profile.int_digits,
                scale: c.profile.scale,
                max_len: c.profile.max_len,
                min: text(c.profile.min),
                max: text(c.profile.max),
            })
            .collect(),
    }
}

/// The months a census was recorded for, from the census directory.
pub fn recorded_months(root: &Path) -> Result<Vec<YearMonth>, CensusError> {
    let dir = census_dir(root);
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(CensusError::Read {
                path: dir.display().to_string(),
                message: e.to_string(),
            });
        }
    };
    let mut months: Vec<YearMonth> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".toml"))
                .and_then(|m| m.parse().ok())
        })
        .collect();
    months.sort();
    Ok(months)
}

/// Folds every recorded census into the shapes files: each table's file is
/// written again, and a shapes file of a table no census holds any more is
/// removed.
pub fn fold(root: &Path, registry: &Registry) -> Result<Folded, CensusError> {
    let mut acc = Fold::default();
    let months = recorded_months(root)?;
    for &month in &months {
        if let Some(files) = read_month(root, month)? {
            acc.month(registry, month, &files);
        }
    }
    crate::reports::fold_into(root, registry, &mut acc)?;
    let shapes = acc.shapes();
    let dir = shapes_dir(root);
    let keep: BTreeSet<String> = shapes.keys().map(|t| format!("{t}.toml")).collect();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".toml") && !keep.contains(&name) {
                std::fs::remove_file(e.path()).map_err(|source| CensusError::Write {
                    path: e.path().display().to_string(),
                    source,
                })?;
            }
        }
    }
    let mut folded = Folded {
        months: months.len(),
        tables: shapes.len(),
        shapes: 0,
    };
    for (table, entries) in &shapes {
        folded.shapes += entries.len();
        write_text(&dir.join(format!("{table}.toml")), &render_shapes(entries))?;
    }
    Ok(folded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_times() {
        assert_eq!(
            market_time("2024/08/01 00:05:00").as_deref(),
            Some("2024-08-01T00:05:00+10:00")
        );
        assert_eq!(
            market_time("2024/08/01 00:05:00.5").as_deref(),
            Some("2024-08-01T00:05:00.5+10:00")
        );
        assert_eq!(
            market_time("2024/08/01").as_deref(),
            Some("2024-08-01T00:00:00+10:00")
        );
        assert_eq!(market_time("2024/08/01 00:05"), None);
        assert_eq!(market_time("2024-08-01"), None);
        assert_eq!(market_time("2024/08/01X"), None);
    }
}
