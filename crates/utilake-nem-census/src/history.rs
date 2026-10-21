//! Each table's history as an Iceberg table, folded from every census
//! record: its schemas, and the runs of AEMO files written under them.
//!
//! Every block the census recorded is one observation. A table's
//! observations are taken in the order AEMO released them: the month of
//! the release (the MMSDM archive month a file belongs to, a report CSV's
//! month), then where the block's data ends (its last time value, as an
//! instant), then the file's name, then the block's place in it. This is
//! the order an Iceberg table commits them in. It is also the only order
//! that holds the history still when files restate the past: each month's
//! `FPP_USAGE` file holds every interval since 2024-12-09, and the
//! `FPP_CONTRIBUTION_FACTOR` files of 2026-05 on restate 2025 in a new
//! layout; by data alone those interleave with the files before them. A
//! file's recorded publication time cannot order them either: it is when
//! the server last touched the file, and AEMO touched most of the archive
//! again in 2026-04. Each run keeps the exact span of its data. Each observation is given the
//! schema its columns need:
//!
//! - **One field per lineage.** A column's lineage is its observations
//!   until its values turn from a number or a date to text (or between the
//!   two). The lineage is one field: its id is never minted twice, also
//!   across files that leave the column out, so a table that alternates
//!   between two column lists keeps two schemas, not a new one per file. A
//!   new lineage is a new field, recorded as breaking with its file and
//!   time.
//! - **One type per field.** A field's type is [`choose`] over everything
//!   its lineage holds and the widest that the control files and data-model
//!   releases of its months declare. Choosing it from the values seen so far
//!   instead would re-type a column whose first months were all null, and
//!   would turn more digits after the point into a new field (Iceberg
//!   cannot widen a scale); nulls fit any type, and a column's values are
//!   all known when the census is folded.
//! - **A key column is required** until a file holds a row without it, and
//!   optional from then on.
//! - **Schemas are interned:** one with the same columns, order and key as
//!   an earlier one is that one, with its id.
//!
//! Consecutive observations with the same schema, report version and
//! report pair fold into a run, with the kinds of file it came in (the
//! same data published as archive and DVD files is one run, not one per
//! file). A run says what changed at its start (`schema`, `version`,
//! `report`), the Iceberg updates from the run before it, and whether its
//! data overlaps the run before. Nothing in a run is merged away:
//! its first and last file, files and rows are kept, so every change has
//! the file and the time it happened at.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use utilake_core::YearMonth;
use utilake_iceberg::{Declared, Evidence, Field, IcebergType, Kind, Schema, choose};
use utilake_mms_registry::shape::TypeClass;

use crate::census::{BlockRecord, FileRecord};
use crate::fold::market_time;

/// What a data-model release declares of a table's numeric columns:
/// precision and scale by column name.
pub type Numeric = BTreeMap<String, (u8, u8)>;

/// What AEMO declares beyond the control files: each table's key, and the
/// numeric columns of every data-model release by the month it appeared.
#[derive(Debug, Clone, Default)]
pub struct Declarations {
    /// Key columns by table.
    pub keys: BTreeMap<String, Vec<String>>,
    /// Releases in the order they appeared: their first month and their
    /// tables' numeric columns.
    pub releases: Vec<(YearMonth, BTreeMap<String, Numeric>)>,
}

impl Declarations {
    /// The numeric columns of `table` in the newest release out by `month`
    /// that declares it, else the first release that does.
    fn numeric(&self, table: &str, month: YearMonth) -> Option<&Numeric> {
        let declaring = || {
            self.releases
                .iter()
                .filter_map(|(m, t)| Some((*m, t.get(table)?)))
        };
        declaring()
            .rfind(|(m, _)| *m <= month)
            .or_else(|| declaring().next())
            .map(|(_, n)| n)
    }
}

/// Where in the archive an observation starts or ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// The file.
    pub file: String,
    /// Its sha256, when recorded.
    pub sha256: Option<String>,
    /// When AEMO published it, RFC 3339.
    pub published: Option<String>,
    /// The data's time there, RFC 3339 in market time.
    pub data_time: Option<String>,
}

/// One observation: a block, or a run of report files of one shape.
#[derive(Debug, Clone)]
struct Obs {
    /// Release month, data end in seconds, file, block.
    key: (YearMonth, i64, String, u32),
    month: YearMonth,
    report: (String, String),
    version: u16,
    kind: String,
    from: Mark,
    to: Mark,
    files: u64,
    rows: u64,
    columns: Vec<String>,
    ctl: Vec<TypeClass>,
    evidence: Vec<Evidence>,
    nulls: Vec<u64>,
}

/// Every observation of every table, gathering.
#[derive(Debug, Default)]
pub struct History {
    tables: BTreeMap<String, Vec<Obs>>,
}

/// A change at the start of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// The table's first run.
    First,
    /// Another schema.
    Schema,
    /// Another AEMO version of the report.
    Version,
    /// Another report pair.
    Report,
}

impl Change {
    /// The name in the JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Schema => "schema",
            Self::Version => "version",
            Self::Report => "report",
        }
    }
}

/// One Iceberg update between consecutive schemas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A field no schema had before.
    Add(u32),
    /// A field an earlier schema had, back again.
    Restore(u32),
    /// A field left out.
    Delete(u32),
    /// A field whose place among the others changed.
    Move(u32),
    /// A field that now takes nulls.
    MakeOptional(u32),
    /// The key's fields.
    SetIdentifier(Vec<u32>),
}

/// A column whose new values no promotion of its type holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breaking {
    /// The column.
    pub column: String,
    /// The field it had.
    pub old_id: u32,
    /// Its type.
    pub old_type: IcebergType,
    /// The field it has from here.
    pub new_id: u32,
    /// Its type.
    pub new_type: IcebergType,
    /// Where it happened.
    pub at: Mark,
}

/// Observations in a row with one schema and the same properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Its schema's id.
    pub schema: u32,
    /// The report pair.
    pub report: (String, String),
    /// AEMO's version of the report.
    pub version: u16,
    /// The kinds of file it came in, sorted.
    pub kinds: Vec<String>,
    /// Its first observation.
    pub from: Mark,
    /// Its last.
    pub to: Mark,
    /// Files it holds.
    pub files: u64,
    /// Rows it holds.
    pub rows: u64,
    /// What changed at its start.
    pub changes: Vec<Change>,
    /// The updates from the run before.
    pub steps: Vec<Step>,
    /// Whether its data starts before the run before it ends.
    pub overlaps: bool,
}

/// One table's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableHistory {
    /// Schemas by id.
    pub schemas: Vec<Schema>,
    /// Runs in data order.
    pub runs: Vec<Run>,
    /// Columns that needed a new field.
    pub breaking: Vec<Breaking>,
}

impl History {
    /// Takes in block `index` of `file` of the census of `month`.
    pub fn block(
        &mut self,
        table: &str,
        month: YearMonth,
        (file, index): (&str, u32),
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        self.take(table, month, (file, file, 1, index), f, b);
    }

    /// Takes in a run of `files` report files of one shape, from the file
    /// named `first` to the one named `last`.
    pub fn run(
        &mut self,
        table: &str,
        month: YearMonth,
        (first, last, files): (&str, &str, u64),
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        self.take(table, month, (first, last, files, 0), f, b);
    }

    fn take(
        &mut self,
        table: &str,
        month: YearMonth,
        (first, last, files, index): (&str, &str, u64, u32),
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        let start = b.first.as_deref().and_then(market_time);
        let end = b.last.as_deref().and_then(market_time);
        let mark = |file: &str, data_time: Option<String>| Mark {
            file: file.to_owned(),
            sha256: f.sha256.clone(),
            published: f.published.clone(),
            data_time,
        };
        // Blocks without a time sort at the start of their month.
        let ends_at = instant(end.as_deref())
            .or_else(|| instant(Some(&format!("{month}-01T00:00:00+10:00"))))
            .unwrap_or_default();
        let n = b.columns.len();
        let profiles: Vec<_> = (0..n).map(|j| b.profile_of(j)).collect();
        self.tables.entry(table.to_owned()).or_default().push(Obs {
            key: (month, ends_at, first.to_owned(), index),
            month,
            report: b.report.clone(),
            version: b.version,
            kind: f.kind.clone(),
            from: mark(first, start),
            to: mark(last, end),
            files,
            rows: b.rows,
            columns: b.columns.clone(),
            ctl: (0..n).map(|j| b.ctl_of(j)).collect(),
            evidence: profiles
                .iter()
                .map(crate::profile::ColumnProfile::evidence)
                .collect(),
            nulls: profiles.iter().map(|p| p.nulls).collect(),
        });
    }

    /// Every table's history.
    pub fn finish(self, d: &Declarations) -> BTreeMap<String, TableHistory> {
        self.tables
            .into_iter()
            .map(|(table, mut obs)| {
                obs.sort_by(|a, b| a.key.cmp(&b.key));
                let key = d.keys.get(&table).cloned().unwrap_or_default();
                let h = fold_table(&table, &key, d, &obs);
                (table, h)
            })
            .collect()
    }
}

/// An RFC 3339 time as seconds since 1970.
fn instant(text: Option<&str>) -> Option<i64> {
    text?
        .parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_second)
}

fn declared(ctl: TypeClass, numeric: Option<(u8, u8)>, key: bool) -> Declared {
    let (precision, scale) = numeric.map_or((None, None), |(p, s)| (Some(p), Some(s)));
    Declared {
        whole: scale == Some(0) || ctl == TypeClass::Integer,
        fractional: scale.is_some_and(|s| s > 0) || ctl == TypeClass::Number,
        precision,
        scale,
        fallback: fallback(precision, scale),
        key,
    }
}

fn fallback(precision: Option<u8>, scale: Option<u8>) -> Option<IcebergType> {
    match (precision, scale) {
        (Some(_), Some(0)) => Some(IcebergType::Long),
        (Some(p), Some(s)) if p <= 18 => IcebergType::decimal(p, s),
        _ => None,
    }
}

/// The widest of two declarations: whole only if never declared with a
/// fraction, the larger precision and scale.
fn widest(a: Declared, b: Declared) -> Declared {
    let precision = a.precision.max(b.precision);
    let scale = a.scale.max(b.scale);
    let fractional = a.fractional || b.fractional;
    Declared {
        whole: (a.whole || b.whole) && !fractional,
        fractional,
        precision,
        scale,
        fallback: fallback(precision, scale).or(b.fallback).or(a.fallback),
        key: a.key || b.key,
    }
}

/// Whether values of kind `new` end a lineage of kind `old`: a number or a
/// date (or the other) that turns to text.
fn ends(old: Kind, new: Kind) -> bool {
    old != Kind::Empty && new != Kind::Empty && old != Kind::Text && old.join(new) == Kind::Text
}

/// A column's observations from one field.
#[derive(Debug, Clone, Copy)]
struct Lineage {
    /// The first observation it holds.
    start: usize,
    evidence: Evidence,
    declared: Declared,
}

fn fold_table(table: &str, key: &[String], d: &Declarations, obs: &[Obs]) -> TableHistory {
    // Each column's lineages, over every observation.
    let mut lineages: BTreeMap<&str, Vec<Lineage>> = BTreeMap::new();
    for (i, o) in obs.iter().enumerate() {
        let numeric = d.numeric(table, o.month);
        for (j, name) in o.columns.iter().enumerate() {
            let decl = declared(
                o.ctl[j],
                numeric.and_then(|n| n.get(name)).copied(),
                key.contains(name),
            );
            let seen = o.evidence[j];
            let v = lineages.entry(name).or_default();
            match v.last_mut() {
                Some(l) if !ends(l.evidence.kind, seen.kind) => {
                    l.evidence = l.evidence.join(seen);
                    l.declared = widest(l.declared, decl);
                }
                _ => v.push(Lineage {
                    start: i,
                    evidence: seen,
                    declared: decl,
                }),
            }
        }
    }
    let mut ids: BTreeMap<(&str, usize), (u32, IcebergType)> = BTreeMap::new();
    let mut required: BTreeMap<u32, bool> = BTreeMap::new();
    let mut last_id = 0u32;
    let mut out = TableHistory {
        schemas: Vec::new(),
        runs: Vec::new(),
        breaking: Vec::new(),
    };
    for (i, o) in obs.iter().enumerate() {
        let mut fields = Vec::with_capacity(o.columns.len());
        for (j, name) in o.columns.iter().enumerate() {
            let v = &lineages[name.as_str()];
            let n = v.partition_point(|l| l.start <= i) - 1;
            let (id, ty) = match ids.get(&(name.as_str(), n)) {
                Some(f) => *f,
                None => {
                    last_id += 1;
                    let ty = choose(v[n].evidence, v[n].declared).ty;
                    if let Some(&(old_id, old_type)) =
                        n.checked_sub(1).and_then(|p| ids.get(&(name.as_str(), p)))
                    {
                        out.breaking.push(Breaking {
                            column: name.clone(),
                            old_id,
                            old_type,
                            new_id: last_id,
                            new_type: ty,
                            at: o.from.clone(),
                        });
                    }
                    ids.insert((name.as_str(), n), (last_id, ty));
                    (last_id, ty)
                }
            };
            let req = required.entry(id).or_insert_with(|| key.contains(name));
            *req &= o.nulls[j] == 0;
            fields.push(Field {
                id,
                name: name.clone(),
                ty,
                required: *req,
                doc: None,
            });
        }
        // Iceberg's identifier fields are required; a key column that took
        // a null leaves the key until the schema says otherwise.
        let identifier_field_ids = key
            .iter()
            .filter_map(|c| fields.iter().find(|f| &f.name == c && f.required))
            .map(|f| f.id)
            .collect();
        let schema = Schema {
            schema_id: 0,
            fields,
            identifier_field_ids,
            last_column_id: last_id,
        };
        let id = match out.schemas.iter().find(|s| s.same_as(&schema)) {
            Some(s) => s.schema_id,
            None => {
                let id = u32::try_from(out.schemas.len()).unwrap_or(u32::MAX);
                out.schemas.push(Schema {
                    schema_id: id,
                    ..schema
                });
                id
            }
        };
        extend(&mut out, id, o);
    }
    out
}

/// Adds `o`, under schema `id`, to the last run or as a new one.
fn extend(out: &mut TableHistory, id: u32, o: &Obs) {
    if let Some(r) = out.runs.last_mut()
        && r.schema == id
        && r.version == o.version
        && r.report == o.report
    {
        if let Err(at) = r.kinds.binary_search(&o.kind) {
            r.kinds.insert(at, o.kind.clone());
        }
        // The run ends where its latest data ends (no time sorts first).
        if o.to.data_time >= r.to.data_time {
            r.to = o.to.clone();
        }
        r.files += o.files;
        r.rows += o.rows;
        return;
    }
    let (changes, steps, overlaps) = match out.runs.last() {
        None => (vec![Change::First], Vec::new(), false),
        Some(r) => {
            let mut changes = Vec::new();
            if r.schema != id {
                changes.push(Change::Schema);
            }
            if r.version != o.version {
                changes.push(Change::Version);
            }
            if r.report != o.report {
                changes.push(Change::Report);
            }
            let before = &out.schemas[r.schema as usize];
            let earlier: Vec<&Schema> = out
                .runs
                .iter()
                .map(|r| &out.schemas[r.schema as usize])
                .collect();
            let steps = steps(before, &out.schemas[id as usize], &earlier);
            let overlaps = match (&r.to.data_time, &o.from.data_time) {
                (Some(end), Some(start)) => start < end,
                _ => false,
            };
            (changes, steps, overlaps)
        }
    };
    out.runs.push(Run {
        schema: id,
        report: o.report.clone(),
        version: o.version,
        kinds: vec![o.kind.clone()],
        from: o.from.clone(),
        to: o.to.clone(),
        files: o.files,
        rows: o.rows,
        changes,
        steps,
        overlaps,
    });
}

/// The positions, in `seq`, outside one longest increasing subsequence:
/// the fewest elements that moved.
fn outside_lis(seq: &[usize]) -> Vec<bool> {
    let mut tails: Vec<usize> = Vec::new();
    let mut back = vec![usize::MAX; seq.len()];
    for (i, v) in seq.iter().enumerate() {
        let k = tails.partition_point(|&t| seq[t] < *v);
        if k > 0 {
            back[i] = tails[k - 1];
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut outside = vec![true; seq.len()];
    let mut i = tails.last().copied().unwrap_or(usize::MAX);
    while i != usize::MAX {
        outside[i] = false;
        i = back[i];
    }
    outside
}

/// The updates that take `a` to `b`; `earlier` holds the schemas of every
/// run so far, so a field back from one of them is a restore, not an add.
pub fn steps(a: &Schema, b: &Schema, earlier: &[&Schema]) -> Vec<Step> {
    let mut out = Vec::new();
    for f in &a.fields {
        if b.field(f.id).is_none() {
            out.push(Step::Delete(f.id));
        }
    }
    for f in &b.fields {
        match a.field(f.id) {
            None if earlier.iter().any(|s| s.field(f.id).is_some()) => {
                out.push(Step::Restore(f.id));
            }
            None => out.push(Step::Add(f.id)),
            Some(old) => {
                if old.required && !f.required {
                    out.push(Step::MakeOptional(f.id));
                }
            }
        }
    }
    let shared: Vec<u32> = b
        .fields
        .iter()
        .filter(|f| a.field(f.id).is_some())
        .map(|f| f.id)
        .collect();
    let positions: Vec<usize> = shared
        .iter()
        .filter_map(|id| a.fields.iter().position(|f| f.id == *id))
        .collect();
    for (id, moved) in shared.iter().zip(outside_lis(&positions)) {
        if moved {
            out.push(Step::Move(*id));
        }
    }
    let (mut ka, mut kb) = (
        a.identifier_field_ids.clone(),
        b.identifier_field_ids.clone(),
    );
    ka.sort_unstable();
    kb.sort_unstable();
    if ka != kb {
        out.push(Step::SetIdentifier(b.identifier_field_ids.clone()));
    }
    out
}

fn mark_json(m: &Mark) -> Value {
    json!({
        "file": m.file,
        "sha256": m.sha256,
        "published": m.published,
        "data-time": m.data_time,
    })
}

fn step_json(s: &Step) -> Value {
    match s {
        Step::Add(id) => json!({"action": "add", "field-id": id}),
        Step::Restore(id) => json!({"action": "restore", "field-id": id}),
        Step::Delete(id) => json!({"action": "delete", "field-id": id}),
        Step::Move(id) => json!({"action": "move", "field-id": id}),
        Step::MakeOptional(id) => json!({"action": "make-optional", "field-id": id}),
        Step::SetIdentifier(ids) => json!({"action": "set-identifier-fields", "field-ids": ids}),
    }
}

impl TableHistory {
    /// The history as `iceberg.json` holds it.
    pub fn to_json(&self, table: &str) -> Value {
        json!({
            "table": table,
            "format-version": 3,
            "schemas": self.schemas.iter().map(Schema::to_json).collect::<Vec<_>>(),
            "runs": self.runs.iter().map(|r| json!({
                "schema-id": r.schema,
                "report": [r.report.0, r.report.1],
                "version": r.version,
                "kinds": r.kinds,
                "from": mark_json(&r.from),
                "to": mark_json(&r.to),
                "files": r.files,
                "rows": r.rows,
                "changes": r.changes.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                "updates": r.steps.iter().map(step_json).collect::<Vec<_>>(),
                "overlaps": r.overlaps,
            })).collect::<Vec<_>>(),
            "breaking": self.breaking.iter().map(|b| json!({
                "column": b.column,
                "old-field-id": b.old_id,
                "old-type": b.old_type.name(),
                "new-field-id": b.new_id,
                "new-type": b.new_type.name(),
                "at": mark_json(&b.at),
            })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use utilake_mms_registry::shape::ShapeId;

    use super::*;

    /// A column: name, observed kind, digits before and after the point,
    /// nulls.
    type Col<'a> = (&'a str, Kind, u8, u8, u64);

    fn record(kind: &str, published: Option<&str>) -> FileRecord {
        FileRecord {
            source: "src".into(),
            sha256: Some("ab".into()),
            published: published.map(str::to_owned),
            kind: kind.into(),
            block: Vec::new(),
        }
    }

    fn block(version: u16, span: Option<(&str, &str)>, cols: &[Col<'_>]) -> BlockRecord {
        BlockRecord {
            shape: ShapeId(1),
            report: ("DISPATCH".into(), "PRICE".into()),
            version,
            rows: 10,
            time: span.map(|_| "SETTLEMENTDATE".into()),
            first: span.map(|s| s.0.into()),
            last: span.map(|s| s.1.into()),
            columns: cols.iter().map(|c| c.0.into()).collect(),
            ctl: cols.iter().map(|_| TypeClass::Unknown.letter()).collect(),
            kinds: cols.iter().map(|c| c.1.letter()).collect(),
            nulls: cols.iter().map(|c| c.4).collect(),
            int_digits: cols.iter().map(|c| c.2).collect(),
            scale: cols.iter().map(|c| c.3).collect(),
            sig_digits: cols.iter().map(|c| c.2 + c.3).collect(),
            max_len: cols.iter().map(|_| 8).collect(),
            min: cols.iter().map(|_| String::new()).collect(),
            max: cols.iter().map(|_| String::new()).collect(),
        }
    }

    const T: Col<'static> = ("SETTLEMENTDATE", Kind::Timestamp, 0, 0, 0);
    const R: Col<'static> = ("REGIONID", Kind::Text, 0, 0, 0);
    const P: Col<'static> = ("RRP", Kind::Decimal, 3, 2, 0);
    const X: Col<'static> = ("RAISE1SECRRP", Kind::Decimal, 2, 2, 3);

    fn decl() -> Declarations {
        Declarations {
            keys: BTreeMap::from([(
                "DISPATCHPRICE".into(),
                vec!["SETTLEMENTDATE".into(), "REGIONID".into()],
            )]),
            releases: Vec::new(),
        }
    }

    fn month(m: &str) -> YearMonth {
        m.parse().unwrap()
    }

    fn span_of(m: &str) -> (String, String) {
        (
            format!("{}/{}/01 00:05:00", &m[..4], &m[5..]),
            format!("{}/{}/28 00:00:00", &m[..4], &m[5..]),
        )
    }

    /// One block per month, in the order given, each spanning that month.
    fn months(blocks: &[(&str, u16, &str, &[Col<'_>])]) -> TableHistory {
        let mut h = History::default();
        for (i, (m, version, kind, cols)) in blocks.iter().enumerate() {
            let (start, end) = span_of(m);
            h.block(
                "DISPATCHPRICE",
                month(m),
                (&format!("F{i}.zip"), 0),
                &record(kind, None),
                &block(*version, Some((&start, &end)), cols),
            );
        }
        h.finish(&decl()).remove("DISPATCHPRICE").unwrap()
    }

    fn schemas_of(h: &TableHistory) -> Vec<u32> {
        h.runs.iter().map(|r| r.schema).collect()
    }

    #[test]
    fn a_column_list_that_comes_back_keeps_its_schema_and_ids() {
        let h = months(&[
            ("2024-06", 1, "dvd", &[T, R, P]),
            ("2024-07", 1, "dvd", &[T, R, X, P]),
            ("2024-08", 1, "dvd", &[T, R, P]),
            ("2024-09", 1, "dvd", &[T, R, X, P]),
        ]);
        assert_eq!(schemas_of(&h), [0, 1, 0, 1]);
        assert_eq!(h.schemas.len(), 2);
        assert_eq!(h.schemas[1].by_name("RAISE1SECRRP").map(|f| f.id), Some(4));
        assert_eq!(
            h.runs[1].steps,
            [Step::Add(4)],
            "an insertion moves nothing"
        );
        assert_eq!(h.runs[2].steps, [Step::Delete(4)]);
        assert_eq!(h.runs[3].steps, [Step::Restore(4)]);
        assert_eq!(h.runs[0].changes, [Change::First]);
        assert_eq!(h.runs[1].changes, [Change::Schema]);
        assert!(h.breaking.is_empty());
        // The key: both columns, both required; RAISE1SECRRP held nulls.
        assert_eq!(h.schemas[0].identifier_field_ids, [1, 2]);
        assert!(!h.schemas[1].by_name("RAISE1SECRRP").unwrap().required);
    }

    #[test]
    fn a_column_keeps_one_type_until_its_values_turn_to_text() {
        let wide = ("RRP", Kind::Decimal, 9, 3, 0);
        let text = ("RRP", Kind::Text, 0, 0, 0);
        let h = months(&[
            ("2024-01", 1, "dvd", &[T, R, P]),
            ("2024-02", 1, "dvd", &[T, R, wide]),
            ("2024-03", 1, "dvd", &[T, R, P]),
            ("2024-04", 1, "dvd", &[T, R, text]),
            ("2024-05", 1, "dvd", &[T, R, P]),
        ]);
        // The widest the lineage holds, from its first file: no change for
        // more digits, and none Iceberg could not make for a wider scale.
        let rrp = |s: usize| h.schemas[s].by_name("RRP").map(|f| (f.id, f.ty));
        let wide_ty = IcebergType::decimal(12, 3).unwrap();
        assert_eq!(rrp(0), Some((3, wide_ty)));
        assert_eq!(h.runs[0].files, 3);
        // Text ends the lineage: a new field, and text holds what follows.
        assert_eq!(rrp(1), Some((4, IcebergType::String)));
        assert_eq!(schemas_of(&h), [0, 1]);
        let b = &h.breaking[0];
        assert_eq!((b.old_id, b.old_type), (3, wide_ty));
        assert_eq!((b.new_id, b.new_type), (4, IcebergType::String));
        assert_eq!(b.at.file, "F3.zip");
        assert_eq!(h.runs[1].steps, [Step::Delete(3), Step::Add(4)]);
    }

    #[test]
    fn a_column_null_at_first_takes_the_type_its_values_have_later() {
        let empty = ("LASTCHANGED", Kind::Empty, 0, 0, 5);
        let times = ("LASTCHANGED", Kind::Timestamp, 0, 0, 0);
        let h = months(&[
            ("2024-10", 1, "dvd", &[T, R, empty]),
            ("2024-11", 1, "dvd", &[T, R, times]),
        ]);
        assert!(h.breaking.is_empty());
        assert_eq!(schemas_of(&h), [0]);
        let f = h.schemas[0].by_name("LASTCHANGED").unwrap();
        assert_eq!((f.id, f.ty), (3, IcebergType::Timestamptz));
    }

    #[test]
    fn a_key_column_that_takes_a_null_leaves_the_key() {
        let null_region = ("REGIONID", Kind::Text, 0, 0, 2);
        let h = months(&[
            ("2024-01", 1, "dvd", &[T, R, P]),
            ("2024-02", 1, "dvd", &[T, null_region, P]),
            ("2024-03", 1, "dvd", &[T, R, P]),
        ]);
        assert_eq!(
            h.runs[1].steps,
            [Step::MakeOptional(2), Step::SetIdentifier(vec![1])]
        );
        assert_eq!(schemas_of(&h), [0, 1], "once optional, always optional");
    }

    #[test]
    fn a_moved_column_is_one_move() {
        let h = months(&[
            ("2024-01", 1, "dvd", &[T, R, P, X]),
            ("2024-02", 1, "dvd", &[X, T, R, P]),
        ]);
        assert_eq!(h.runs[1].steps, [Step::Move(4)]);
    }

    #[test]
    fn versions_and_reports_change_runs_not_schemas() {
        let h = months(&[
            ("2024-01", 1, "dvd", &[T, R, P]),
            ("2024-02", 2, "dvd", &[T, R, P]),
            ("2024-03", 2, "archive", &[T, R, P]),
        ]);
        assert_eq!(schemas_of(&h), [0, 0]);
        assert_eq!(h.runs[1].changes, [Change::Version]);
        assert!(h.runs[1].steps.is_empty());
        // The same data as DVD and archive files is one run of both kinds.
        assert_eq!(h.runs[1].kinds, ["archive", "dvd"]);
        assert_eq!(h.runs[1].files, 2);

        let mut hist = History::default();
        let mut other = block(1, None, &[T, R, P]);
        other.report = ("DISPATCH".into(), "PRICE2".into());
        let rec = record("dvd", None);
        let jan = month("2024-01");
        hist.block(
            "DISPATCHPRICE",
            jan,
            ("A.zip", 0),
            &rec,
            &block(1, None, &[T, R, P]),
        );
        hist.block("DISPATCHPRICE", jan, ("A.zip", 1), &rec, &other);
        let h = hist.finish(&decl()).remove("DISPATCHPRICE").unwrap();
        assert_eq!(h.runs[1].changes, [Change::Report]);
        assert_eq!(h.runs[0].from.data_time, None);
    }

    #[test]
    fn observations_go_in_data_order_and_overlaps_are_marked() {
        let mut h = History::default();
        let rec = record("dvd", Some("2024-02-03T00:00:00Z"));
        let feb = month("2024-02");
        // Taken out of order: the later data first.
        let b = block(
            1,
            Some(("2024/02/01 00:05:00", "2024/02/29 00:00:00")),
            &[T, R, X, P],
        );
        h.block("DISPATCHPRICE", feb, ("B.zip", 0), &rec, &b);
        let a = block(
            1,
            Some(("2024/01/01 00:05:00", "2024/02/10 00:00:00")),
            &[T, R, P],
        );
        h.block("DISPATCHPRICE", month("2024-01"), ("A.zip", 0), &rec, &a);
        // A report run of B's shape, of another kind.
        let r = block(
            1,
            Some(("2024/02/05 00:05:00", "2024/03/01 00:00:00")),
            &[T, R, X, P],
        );
        let reports = record("reports/current", None);
        h.run(
            "DISPATCHPRICE",
            feb,
            ("PUBLIC_1.CSV", "PUBLIC_9.CSV", 9),
            &reports,
            &r,
        );
        let t = h.finish(&decl()).remove("DISPATCHPRICE").unwrap();
        assert_eq!(t.runs[0].from.file, "A.zip");
        assert_eq!(t.runs[1].from.file, "B.zip");
        assert!(t.runs[1].overlaps, "B starts before A's data ends");
        // The report run has B's schema: one run of 10 files of two kinds.
        assert_eq!(t.runs.len(), 2);
        assert_eq!(t.runs[1].files, 10);
        assert_eq!(t.runs[1].kinds, ["dvd", "reports/current"]);
        assert_eq!(
            t.runs[1].to.file, "PUBLIC_9.CSV",
            "the report run ends last"
        );
    }

    #[test]
    fn a_run_ends_where_its_latest_data_ends() {
        let mut h = History::default();
        let rec = record("dvd", None);
        let jan = month("2024-01");
        let long = block(
            1,
            Some(("2024/01/01 00:05:00", "2024/03/01 00:00:00")),
            &[T, R, P],
        );
        let short = block(
            1,
            Some(("2024/01/02 00:05:00", "2024/01/03 00:00:00")),
            &[T, R, P],
        );
        h.block("DISPATCHPRICE", jan, ("A.zip", 0), &rec, &long);
        h.block("DISPATCHPRICE", jan, ("B.zip", 0), &rec, &short);
        let t = h.finish(&decl()).remove("DISPATCHPRICE").unwrap();
        assert_eq!(t.runs.len(), 1);
        assert_eq!(t.runs[0].from.file, "B.zip", "B ends first, so it leads");
        assert_eq!(t.runs[0].to.file, "A.zip");
        assert_eq!(t.runs[0].rows, 20);
    }

    #[test]
    fn the_widest_declaration_of_a_lineage_sets_its_type() {
        let mut d = decl();
        let release = |p, s| {
            BTreeMap::from([(
                "DISPATCHPRICE".to_owned(),
                Numeric::from([("RRP".to_owned(), (p, s))]),
            )])
        };
        d.releases = vec![
            (month("2024-02"), release(15, 5)),
            (month("2024-04"), release(18, 5)),
            (month("2024-06"), release(15, 5)),
        ];
        let mut h = History::default();
        for m in ["2024-01", "2024-03", "2024-05", "2024-07"] {
            let (start, end) = span_of(m);
            let b = block(1, Some((&start, &end)), &[T, R, P]);
            h.block("DISPATCHPRICE", month(m), (m, 0), &record("dvd", None), &b);
        }
        let t = h.finish(&d).remove("DISPATCHPRICE").unwrap();
        let rrp = t.schemas[0].by_name("RRP").unwrap().ty;
        assert_eq!(rrp, IcebergType::decimal(18, 5).unwrap());
        assert_eq!((t.runs.len(), t.runs[0].files), (1, 4));
        // Before the first release, the first that declares the table.
        let first = d.numeric("DISPATCHPRICE", month("2024-01")).unwrap();
        assert_eq!(first["RRP"], (15, 5));
        assert_eq!(d.numeric("NONE", month("2024-01")), None);
        // A whole number the model declares whole, never seen: a long.
        let whole = declared(TypeClass::Unknown, Some((10, 0)), false);
        assert_eq!(choose(Evidence::default(), whole).ty, IcebergType::Long);
        assert_eq!(declared(TypeClass::Integer, None, false).fallback, None);
        let huge = declared(TypeClass::Number, Some((40, 2)), false);
        assert!(huge.fallback.is_none());
        // Declared whole, then with a fraction: no longer whole.
        let w = widest(
            declared(TypeClass::Integer, None, true),
            declared(TypeClass::Number, None, false),
        );
        assert!(!w.whole && w.fractional && w.key);
        assert!(ends(Kind::Integer, Kind::Text) && ends(Kind::Date, Kind::Decimal));
        assert!(!ends(Kind::Text, Kind::Integer) && !ends(Kind::Empty, Kind::Text));
        assert!(!ends(Kind::Integer, Kind::Empty) && !ends(Kind::Integer, Kind::Decimal));
    }

    #[test]
    fn files_go_in_the_order_they_were_released_even_when_they_restate_the_past() {
        let lc = ("LASTCHANGED", Kind::Timestamp, 0, 0, 0);
        let mut h = History::default();
        // Monthly releases, each holding everything since January, then a
        // June release in a new layout that restates January only. The
        // server touched every file on one day, which orders nothing.
        let touched = record("archive", Some("2026-04-17T23:21:00Z"));
        for (file, release, end, cols) in [
            ("JAN.zip", "2024-01", "2024/01/31 00:00:00", &[T, R, P][..]),
            (
                "BACKFILL.zip",
                "2024-06",
                "2024/01/31 00:00:00",
                &[T, R, P, lc][..],
            ),
            ("MAR.zip", "2024-03", "2024/03/31 00:00:00", &[T, R, P][..]),
            ("FEB.zip", "2024-02", "2024/02/28 00:00:00", &[T, R, P][..]),
        ] {
            let b = block(1, Some(("2024/01/01 00:05:00", end)), cols);
            h.block("DISPATCHPRICE", month(release), (file, 0), &touched, &b);
        }
        let t = h.finish(&decl()).remove("DISPATCHPRICE").unwrap();
        let files: Vec<&str> = t.runs.iter().map(|r| r.from.file.as_str()).collect();
        assert_eq!(files, ["JAN.zip", "BACKFILL.zip"]);
        assert_eq!(
            (t.runs[0].files, t.runs[0].to.file.as_str()),
            (3, "MAR.zip")
        );
        let backfill = &t.runs[1];
        assert_eq!(
            backfill.from.data_time.as_deref(),
            Some("2024-01-01T00:05:00+10:00")
        );
        assert!(backfill.overlaps, "it restates data the run before holds");
        assert_eq!(
            instant(Some("2024-01-01T10:00:00+10:00")),
            instant(Some("2024-01-01T00:00:00Z"))
        );
        assert_eq!(instant(Some("not a time")), None);
        assert_eq!(instant(None), None);
    }

    #[test]
    fn writes_the_history_as_json() {
        let text = ("RRP", Kind::Text, 0, 0, 0);
        let null_region = ("REGIONID", Kind::Text, 0, 0, 2);
        let h = months(&[
            ("2024-01", 1, "dvd", &[T, R, P, X]),
            ("2024-02", 1, "dvd", &[X, T, null_region, P]),
            ("2024-03", 1, "dvd", &[T, R, X]),
            ("2024-04", 1, "dvd", &[T, R, X, text]),
            ("2024-05", 1, "dvd", &[T, R, text]),
            ("2024-06", 1, "dvd", &[T, R, X, text]),
        ]);
        let v = h.to_json("DISPATCHPRICE");
        assert_eq!(v["table"], "DISPATCHPRICE");
        assert_eq!(v["format-version"], 3);
        assert_eq!(v["runs"][0]["changes"], json!(["first"]));
        assert_eq!(v["runs"][0]["kinds"], json!(["dvd"]));
        let actions: Vec<String> = v["runs"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["updates"].as_array().unwrap().iter())
            .map(|u| u["action"].as_str().unwrap().to_owned())
            .collect();
        for a in [
            "move",
            "make-optional",
            "set-identifier-fields",
            "delete",
            "add",
            "restore",
        ] {
            assert!(actions.contains(&a.to_owned()), "{a} in {actions:?}");
        }
        assert_eq!(v["breaking"][0]["new-type"], "string");
        assert_eq!(v["runs"][1]["from"]["file"], "F1.zip");
        let names = [
            Change::First,
            Change::Schema,
            Change::Version,
            Change::Report,
        ]
        .map(Change::as_str);
        assert_eq!(names, ["first", "schema", "version", "report"]);
    }
}
