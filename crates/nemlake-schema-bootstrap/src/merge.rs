//! Combining the three sources into one [`TableDef`].
//!
//! Precedence, per fact:
//! - column list and order per version: the sampled I row (the only source
//!   that knows what AEMO actually publishes);
//! - type class: the CTL loader type (reflects the current file), with the
//!   data model's precision, scale and length when they agree on class;
//! - primary key, package, visibility, description: the data model;
//! - snapshot-or-append: the curated list;
//! - ids and anything marked `curated`: the existing file.
//!
//! Disagreements are written into `provenance.note` so a reviewer sees them.

use std::collections::{BTreeMap, BTreeSet};

use nemlake_core::{Category, Diagnose, Diagnostic, Fingerprint, YearMonth};
use nemlake_schema_model::{
    ColumnDef, ColumnId, ColumnType, Dedupe, LoadMode, Market, Provenance, SchemaVersion,
    SourceKind, TableDef, VariantDef, VersionDef, Visibility,
};

use crate::ctl::{CtlFile, LoaderType};
use crate::datamodel::{DmTable, parse_oracle_type};
use crate::sample::SampledBlock;

/// What the merger is given for one table.
#[derive(Debug, Default)]
pub struct Inputs<'a> {
    /// Data-model name of the table.
    pub name: String,
    /// The control files that load this table, newest first. A column's
    /// loader type comes from the first one that lists it, so a column a
    /// later month dropped is still typed by the month that had it.
    pub ctls: Vec<&'a CtlFile>,
    /// The data-model entry, when the report has one.
    pub dm: Option<&'a DmTable>,
    /// Every sampled block that belongs to this table, any version.
    pub samples: Vec<(Option<YearMonth>, &'a SampledBlock)>,
    /// The committed file, to keep ids and curated fields.
    pub existing: Option<&'a TableDef>,
    /// Whether the table is a standing-data snapshot.
    pub snapshot: bool,
    /// Archive file stems that feed this table (`BIDPEROFFER` for `BIDPEROFFER1`).
    pub archive_files: Vec<String>,
    /// CTL file name, for provenance.
    pub ctl_path: Option<String>,
}

/// Why a table could not be merged.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MergeError {
    /// No source knows the columns.
    #[error(
        "Table {table} has no sampled I row, no CTL and no data-model entry, so its columns are unknown."
    )]
    NoColumns {
        /// The table.
        table: String,
    },
    /// No version could be established.
    #[error(
        "Table {table} has columns but no sampled I row, so no version number can be assigned; sample a real file."
    )]
    NoVersion {
        /// The table.
        table: String,
    },
}

impl Diagnose for MergeError {
    fn diagnostic(&self) -> Diagnostic {
        let table = match self {
            Self::NoColumns { table } | Self::NoVersion { table } => table.clone(),
        };
        let code = match self {
            Self::NoColumns { .. } => "schema.merge_no_columns",
            Self::NoVersion { .. } => "schema.merge_no_version",
        };
        Diagnostic::error(code, Category::Schema, self.to_string()).with_table(table)
    }
}

/// Maps a loader type to a logical type.
pub fn loader_to_type(l: &LoaderType) -> Option<ColumnType> {
    Some(match l {
        LoaderType::Filler => return None,
        LoaderType::Char(n) => ColumnType::Utf8 {
            max_len: Some(*n),
            dict: false,
        },
        LoaderType::Date(f) | LoaderType::Timestamp(Some(f)) => {
            if f.to_ascii_lowercase().contains("hh") {
                ColumnType::Timestamp
            } else {
                ColumnType::Date
            }
        }
        LoaderType::Timestamp(None) => ColumnType::Timestamp,
        LoaderType::FloatExternal | LoaderType::DecimalExternal => ColumnType::Number {
            precision: None,
            scale: None,
        },
        LoaderType::IntegerExternal => ColumnType::Integer,
    })
}

fn same_class(a: ColumnType, b: ColumnType) -> bool {
    use ColumnType::{Boolean, Date, Integer, Number, Timestamp, Utf8};
    matches!(
        (a, b),
        (Utf8 { .. }, Utf8 { .. })
            | (Number { .. } | Integer, Number { .. } | Integer)
            | (Timestamp | Date, Timestamp | Date)
            | (Boolean, Boolean)
    )
}

/// Low-cardinality hint for the writer: identifiers, types, statuses, flags.
fn is_dict_like(name: &str, ty: ColumnType) -> bool {
    matches!(ty, ColumnType::Utf8 { max_len: Some(n), .. } if n <= 30)
        && (name.ends_with("ID")
            || name.ends_with("TYPE")
            || name.ends_with("STATUS")
            || name.ends_with("FLAG")
            || name == "DUID")
}

const TIME_CANDIDATES: [&str; 8] = [
    "SETTLEMENTDATE",
    "INTERVAL_DATETIME",
    "DATETIME",
    "RUN_DATETIME",
    "TRADINGDATE",
    "EFFECTIVEDATE",
    "DAY",
    "LASTCHANGED",
];

/// Data-model text as it goes into a schema file: no trailing whitespace on
/// any line and no blank lines at either end, so the file passes whitespace
/// hooks unchanged and regenerates byte-identically.
fn tidy_text(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_matches('\n')
        .to_owned()
}

fn min_month(a: Option<YearMonth>, b: Option<YearMonth>) -> Option<YearMonth> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn max_month(a: Option<YearMonth>, b: Option<YearMonth>) -> Option<YearMonth> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Whether every version and variant carries `column`, under its own name
/// or one of its aliases.
fn in_every_shape(
    versions: &[VersionDef],
    column: &str,
    renames: &BTreeMap<String, Vec<String>>,
) -> bool {
    let is = |c: &String| c == column || renames.get(column).is_some_and(|a| a.contains(c));
    versions
        .iter()
        .all(|v| v.columns.iter().any(is) && v.variants.iter().all(|x| x.columns.iter().any(is)))
}

/// The list that stands for a version among every list sampled under its
/// number: the longest, then the one seen latest, then by fingerprint so
/// the choice never depends on input order.
fn main_shape<'a>(blocks: &[(Option<YearMonth>, &'a SampledBlock)]) -> &'a SampledBlock {
    blocks
        .iter()
        .max_by_key(|(m, b)| (b.columns.len(), *m, b.fingerprint.0))
        .map(|(_, b)| *b)
        .expect("a version has at least one block")
}

/// A fixed order so regeneration is deterministic: shortest first, then by name.
fn sort_variants(variants: &mut [VariantDef]) {
    variants.sort_by(|a, b| {
        a.columns
            .len()
            .cmp(&b.columns.len())
            .then_with(|| a.columns.cmp(&b.columns))
    });
}

/// Merges the inputs into a table definition.
pub fn merge(inputs: &Inputs<'_>) -> Result<TableDef, MergeError> {
    let name = inputs.name.clone();
    let mut notes: Vec<String> = Vec::new();

    // 1. Versions and the union column order.
    let mut by_version: BTreeMap<u16, Vec<(Option<YearMonth>, &SampledBlock)>> = BTreeMap::new();
    for (m, b) in &inputs.samples {
        by_version.entry(b.version).or_default().push((*m, b));
    }
    let mut column_order: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut versions: Vec<VersionDef> = Vec::new();
    // Renames AEMO made without bumping the version: canonical name -> other names seen.
    let mut renames: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut sampled_months: BTreeSet<YearMonth> = BTreeSet::new();
    let mut max_len: BTreeMap<String, u32> = BTreeMap::new();
    // (numeric, timestamp, any non-null) evidence per column across every sample.
    let mut evidence: BTreeMap<String, (bool, bool, bool)> = BTreeMap::new();
    // The decimal shape of each column's sampled values.
    let mut digits: BTreeMap<String, crate::sample::Digits> = BTreeMap::new();
    // Pass 1: renames. AEMO sometimes renames a column in place without
    // bumping the version: two lists of one version, same length, differing
    // only in names at a few positions. The name the data model knows (or
    // the one in the longer-lived list) stays canonical and the other
    // becomes an alias, so files with either name feed the same lake column.
    // A name that is a column in its own right somewhere, beside the other
    // name, is not an alias: then both are columns and the lists are simply
    // two shapes.
    let coexist = |x: &str, y: &str| {
        inputs
            .samples
            .iter()
            .any(|(_, b)| b.columns.iter().any(|c| c == x) && b.columns.iter().any(|c| c == y))
    };
    let dm_knows = |n: &str| {
        inputs
            .dm
            .is_some_and(|d| d.columns.iter().any(|c| c.name == n))
    };
    for (v, blocks) in &by_version {
        let main = main_shape(blocks);
        for (_, b) in blocks {
            if b.fingerprint == main.fingerprint || b.columns.len() != main.columns.len() {
                continue;
            }
            let differing: Vec<(&String, &String)> = main
                .columns
                .iter()
                .zip(&b.columns)
                .filter(|(x, y)| x != y)
                .collect();
            let is_rename = !differing.is_empty()
                && differing.iter().all(|(x, y)| !coexist(x, y))
                // Not a reorder: every differing name is new to the other list.
                && differing
                    .iter()
                    .all(|(x, y)| !b.columns.contains(x) && !main.columns.contains(y));
            if !is_rename {
                continue;
            }
            for (x, y) in differing {
                let (canon, alias) = if dm_knows(y) && !dm_knows(x) {
                    (y, x)
                } else {
                    (x, y)
                };
                // One canonical name per alias, and no chains.
                if renames.contains_key(alias)
                    || renames.values().any(|a| a.contains(canon))
                    || renames.iter().any(|(k, a)| k != canon && a.contains(alias))
                {
                    continue;
                }
                let e = renames.entry(canon.clone()).or_default();
                if !e.contains(alias) {
                    notes.push(format!("version {v} names column {canon} as {alias} in some files; recorded as an alias"));
                    e.push(alias.clone());
                }
            }
        }
    }
    let canonical = |c: &String| -> String {
        renames
            .iter()
            .find(|(_, aliases)| aliases.contains(c))
            .map_or_else(|| c.clone(), |(k, _)| k.clone())
    };

    // Pass 2: one version per number. Its columns are the longest list seen
    // (AEMO appends columns without bumping the version), latest month on a
    // tie; every other list seen is a variant, so each resolves exactly.
    for (v, blocks) in &by_version {
        let main = main_shape(blocks);
        let first_columns: Vec<String> = main.columns.iter().map(&canonical).collect();
        for c in &first_columns {
            if seen.insert(c.clone()) {
                column_order.push(c.clone());
            }
        }
        for (_, b) in blocks {
            for (i, (c, l)) in b.columns.iter().zip(&b.max_len).enumerate() {
                let c = canonical(c);
                let e = max_len.entry(c.clone()).or_default();
                *e = (*e).max(*l);
                let ev = evidence.entry(c.clone()).or_insert((true, true, false));
                ev.0 &= b.all_numeric[i];
                ev.1 &= b.all_timestamp[i];
                ev.2 |= !b.all_null[i];
                if let Some(seen) = b.digits.get(i) {
                    let d = digits.entry(c).or_default();
                    d.integer = d.integer.max(seen.integer);
                    d.fraction = d.fraction.max(seen.fraction);
                    d.exponent |= seen.exponent;
                }
            }
        }
        let months: Vec<YearMonth> = blocks.iter().filter_map(|(m, _)| *m).collect();
        sampled_months.extend(months.iter().copied());
        // Every other observed shape of this version, exactly as published.
        let main_fp = Fingerprint::of_names(first_columns.iter().map(String::as_str));
        let mut variants: Vec<VariantDef> = Vec::new();
        for (m, b) in blocks {
            if b.fingerprint == main_fp {
                continue;
            }
            match variants.iter_mut().find(|x| x.fingerprint == b.fingerprint) {
                Some(x) => {
                    x.first_seen = min_month(x.first_seen, *m);
                    x.last_seen = max_month(x.last_seen, *m);
                }
                None => {
                    if b.fingerprint != main.fingerprint {
                        let how = if main.columns.starts_with(&b.columns) {
                            "the longer list is the version and the shorter a variant".to_owned()
                        } else if b.columns.iter().map(&canonical).collect::<Vec<_>>()
                            == first_columns
                        {
                            "the same columns under other names; a variant".to_owned()
                        } else {
                            "the lists differ by more than appended columns; the shorter is a variant".to_owned()
                        };
                        notes.push(format!(
                            "version {v} was published with {} columns and with {}; {how}",
                            b.columns.len(),
                            main.columns.len()
                        ));
                    }
                    // A variant may carry columns the version's own list lacks.
                    for c in &b.columns {
                        let c = canonical(c);
                        if seen.insert(c.clone()) {
                            column_order.push(c);
                        }
                    }
                    variants.push(VariantDef {
                        columns: b.columns.clone(),
                        fingerprint: b.fingerprint,
                        first_seen: *m,
                        last_seen: *m,
                    });
                }
            }
        }
        sort_variants(&mut variants);
        versions.push(VersionDef {
            n: SchemaVersion(*v),
            fingerprint: main_fp,
            columns: first_columns,
            first_seen: months.iter().min().copied(),
            last_seen: months.iter().max().copied(),
            src: Some("irow".into()),
            variants,
        });
    }
    // Keep versions the existing file knows but this run did not sample.
    if let Some(ex) = inputs.existing {
        for ev in &ex.versions {
            if !by_version.contains_key(&ev.n.0) {
                versions.push(ev.clone());
                for c in ev
                    .columns
                    .iter()
                    .chain(ev.variants.iter().flat_map(|x| &x.columns))
                {
                    // A kept variant may name a column by an alias the
                    // existing file declares.
                    let c = ex
                        .column(c)
                        .map_or_else(|| canonical(c), |d| d.name.clone());
                    if seen.insert(c.clone()) {
                        column_order.push(c);
                    }
                }
            } else if let Some(v) = versions.iter_mut().find(|v| v.n == ev.n) {
                v.first_seen = min_month(v.first_seen, ev.first_seen);
                v.last_seen = max_month(v.last_seen, ev.last_seen);
                // A variant is a fact: keep the ones this run did not sample.
                for ex_variant in &ev.variants {
                    if ex_variant.fingerprint == v.fingerprint {
                        continue;
                    }
                    match v
                        .variants
                        .iter_mut()
                        .find(|x| x.fingerprint == ex_variant.fingerprint)
                    {
                        Some(x) => {
                            x.first_seen = min_month(x.first_seen, ex_variant.first_seen);
                            x.last_seen = max_month(x.last_seen, ex_variant.last_seen);
                        }
                        None => {
                            for c in &ex_variant.columns {
                                let c = ex
                                    .column(c)
                                    .map_or_else(|| canonical(c), |d| d.name.clone());
                                if seen.insert(c.clone()) {
                                    column_order.push(c);
                                }
                            }
                            v.variants.push(ex_variant.clone());
                        }
                    }
                }
                sort_variants(&mut v.variants);
            }
        }
        versions.sort_by_key(|v| v.n);
    }
    let mut ctl_cols: BTreeMap<String, &LoaderType> = BTreeMap::new();
    for ctl in &inputs.ctls {
        for c in ctl.data_columns() {
            ctl_cols.entry(c.name.to_ascii_uppercase()).or_insert(&c.ty);
        }
    }
    let dm_cols: BTreeMap<String, &crate::datamodel::DmColumn> = inputs
        .dm
        .map(|d| d.columns.iter().map(|c| (c.name.clone(), c)).collect())
        .unwrap_or_default();
    if column_order.is_empty() {
        if let Some(c) = inputs.ctls.first() {
            column_order = c
                .data_columns()
                .map(|c| c.name.to_ascii_uppercase())
                .collect();
        } else if let Some(d) = inputs.dm {
            column_order = d.columns.iter().map(|c| c.name.clone()).collect();
        }
    }
    if column_order.is_empty() {
        return Err(MergeError::NoColumns { table: name });
    }
    if versions.is_empty() {
        return Err(MergeError::NoVersion { table: name });
    }

    // 2. Primary key from the data model, in column order.
    let dm_pk: BTreeSet<String> = inputs
        .dm
        .map(|d| d.primary_key.iter().cloned().collect())
        .unwrap_or_default();
    let mut primary_key: Vec<String> = column_order
        .iter()
        .filter(|c| dm_pk.contains(*c))
        .cloned()
        .collect();
    for k in &dm_pk {
        if !column_order.contains(k) {
            notes.push(format!("data model lists primary key column {k}, which no sampled file carries; dropped from primary_key"));
        }
    }
    if let Some(ex) = inputs.existing
        && !ex.primary_key.is_empty()
        && ex.primary_key.iter().all(|k| column_order.contains(k))
        && inputs.dm.is_none()
    {
        primary_key = ex.primary_key.clone();
    }

    // 3. Columns: ids from the existing file, types by precedence.
    let mut next_id = inputs.existing.map_or(1, |e| e.next_column_id);
    let mut columns: Vec<ColumnDef> = Vec::with_capacity(column_order.len());
    for cname in &column_order {
        let existing = inputs.existing.and_then(|e| e.column(cname));
        if let Some(ec) = existing
            && ec.curated
        {
            columns.push(ec.clone());
            continue;
        }
        let id = existing.map_or_else(
            || {
                let id = ColumnId(next_id);
                next_id += 1;
                id
            },
            |e| e.id,
        );
        let mut src = Vec::new();
        let ctl_ty = ctl_cols.get(cname).and_then(|l| loader_to_type(l));
        let dm = dm_cols.get(cname);
        let dm_ty = dm.and_then(|d| parse_oracle_type(&d.oracle));
        if ctl_cols.contains_key(cname) {
            src.push("ctl".to_owned());
        }
        if dm.is_some() {
            src.push("dm".to_owned());
        }
        if inputs.samples.iter().any(|(_, b)| {
            b.columns
                .iter()
                .any(|c| c == cname || renames.get(cname).is_some_and(|a| a.contains(c)))
        }) {
            src.push("irow".to_owned());
        }
        let mut ty = match (ctl_ty, dm_ty) {
            (Some(c), Some(d)) if same_class(c, d) => match (c, d) {
                // The data model knows precision and length; the CTL only the class.
                (
                    ColumnType::Number { .. } | ColumnType::Integer,
                    d @ (ColumnType::Number { .. } | ColumnType::Integer),
                ) => d,
                (ColumnType::Utf8 { .. }, d @ ColumnType::Utf8 { .. }) => d,
                (c, _) => c,
            },
            (Some(c), Some(d)) => {
                notes.push(format!(
                    "column {cname}: CTL says {} but the data model says {} ({}); the CTL wins",
                    c.kind_name(),
                    d.kind_name(),
                    dm.map_or("", |d| d.oracle.as_str())
                ));
                c
            }
            (Some(c), None) => c,
            (None, Some(d)) => d,
            (None, None) => match evidence.get(cname) {
                // Typed from the data itself; weaker than a CTL or the data model
                // but real evidence, so it may be committed.
                Some((_, true, true)) => {
                    src.push("sampled".to_owned());
                    ColumnType::Timestamp
                }
                Some((true, _, true)) => {
                    src.push("sampled".to_owned());
                    ColumnType::Number {
                        precision: None,
                        scale: None,
                    }
                }
                Some((_, _, true)) => {
                    src.push("sampled".to_owned());
                    ColumnType::Utf8 {
                        max_len: None,
                        dict: false,
                    }
                }
                _ => {
                    src.push("inferred".to_owned());
                    ColumnType::Utf8 {
                        max_len: max_len.get(cname).copied().filter(|&n| n > 0),
                        dict: false,
                    }
                }
            },
        };
        if let ColumnType::Utf8 {
            max_len: Some(declared),
            dict,
        } = ty
            && let Some(&observed) = max_len.get(cname)
            && observed > declared
        {
            notes.push(format!("column {cname}: declared length {declared} but a sampled value has {observed} bytes; max_len dropped"));
            ty = ColumnType::Utf8 {
                max_len: None,
                dict,
            };
        }
        // A declared NUMBER(p,s) that real values do not fit cannot be read
        // as an exact decimal; without precision the column is a float in
        // every mode. Whether it is numeric at all is the CTL's call.
        if let ColumnType::Number {
            precision: Some(p),
            scale: Some(sc),
        } = ty
            && let Some(seen) = digits.get(cname)
            && !seen.fits(p, sc)
        {
            notes.push(if seen.exponent {
                format!("column {cname}: declared NUMBER({p},{sc}) but a sampled value is in exponent form; precision dropped")
            } else {
                format!(
                    "column {cname}: declared NUMBER({p},{sc}) but a sampled value has {} integer and {} fractional digits; precision dropped",
                    seen.integer, seen.fraction
                )
            });
            ty = ColumnType::Number {
                precision: None,
                scale: None,
            };
        }
        // An integer column whose values carry fractional digits is not one.
        if ty == ColumnType::Integer
            && let Some(seen) = digits.get(cname)
            && (seen.fraction > 0 || seen.exponent || seen.integer > 18)
        {
            notes.push(format!(
                "column {cname}: declared an integer but a sampled value has {} integer and {} fractional digits; read as a number",
                seen.integer, seen.fraction
            ));
            ty = ColumnType::Number {
                precision: None,
                scale: None,
            };
        }
        if let ColumnType::Utf8 { max_len, .. } = ty {
            ty = ColumnType::Utf8 {
                max_len,
                dict: is_dict_like(cname, ty),
            };
        }
        let added_in = versions
            .iter()
            .filter(|v| v.columns.contains(cname))
            .map(|v| v.n)
            .min()
            .unwrap_or(versions[0].n);
        let last_with = versions
            .iter()
            .filter(|v| v.columns.contains(cname))
            .map(|v| v.n)
            .max();
        let removed_in = match last_with {
            Some(last) => versions.iter().map(|v| v.n).find(|&n| n > last),
            None => None,
        };
        columns.push(ColumnDef {
            id,
            name: cname.clone(),
            aliases: {
                let mut a = existing.map(|e| e.aliases.clone()).unwrap_or_default();
                for r in renames.get(cname).into_iter().flatten() {
                    if !a.contains(r) {
                        a.push(r.clone());
                    }
                }
                a
            },
            ty,
            // A key column refuses nulls, unless some published shape of the
            // table lacks it: then old files have no value to give.
            nullable: !(primary_key.contains(cname) && in_every_shape(&versions, cname, &renames)),
            oracle: dm.map(|d| d.oracle.clone()),
            loader: ctl_cols.get(cname).map(|l| l.render()),
            added_in,
            removed_in,
            description: dm.map(|d| tidy_text(&d.comment)).filter(|c| !c.is_empty()),
            src,
            curated: false,
        });
    }

    // 4. Table-level facts.
    let has_lastchanged = column_order.iter().any(|c| c == "LASTCHANGED");
    let load_mode = if inputs.snapshot {
        LoadMode::Snapshot
    } else {
        LoadMode::Append
    };
    let dedupe = match (primary_key.is_empty(), has_lastchanged) {
        (true, _) => Dedupe::None,
        (false, true) => Dedupe::PkLastchanged,
        (false, false) => Dedupe::Pk,
    };
    let time_column = if inputs.snapshot {
        None
    } else {
        TIME_CANDIDATES
            .iter()
            .find(|c| {
                columns.iter().any(|col| {
                    col.name == **c && matches!(col.ty, ColumnType::Timestamp | ColumnType::Date)
                })
            })
            .map(|c| (*c).to_owned())
            .or_else(|| {
                columns
                    .iter()
                    .find(|c| {
                        matches!(c.ty, ColumnType::Timestamp | ColumnType::Date)
                            && primary_key.contains(&c.name)
                    })
                    .map(|c| c.name.clone())
            })
    };
    if time_column.is_none() && !inputs.snapshot {
        notes.push("no timestamp column found for partitioning; set time_column by hand or mark the table a snapshot".into());
    }
    let visibility = match inputs.dm.and_then(|d| d.visibility.as_deref()) {
        Some(v) if v.to_ascii_lowercase().contains("public") => Visibility::Public,
        Some(_) => Visibility::Private,
        None => Visibility::Public,
    };
    let mut report_pairs: Vec<(String, String)> = inputs
        .samples
        .iter()
        .map(|(_, b)| (b.report_type.clone(), b.report_subtype.clone()))
        .collect();
    report_pairs.sort();
    report_pairs.dedup();
    if let Some(ex) = inputs.existing {
        for p in &ex.report_pairs {
            if !report_pairs.contains(p) {
                report_pairs.push(p.clone());
            }
        }
    }
    let mut provenance = Provenance {
        ctl: inputs.ctl_path.clone(),
        data_model: inputs.dm.map(|d| format!("{}@v5.0.0.1", d.page)),
        sampled_months: sampled_months.into_iter().collect(),
        generated: None,
        note: None,
    };
    if let Some(ex) = inputs.existing
        && let Some(n) = &ex.provenance.note
        && let Some(human) = n.split(" | ").find(|part| part.starts_with("human:"))
    {
        // Regenerated notes are derived from the inputs and rewritten every run;
        // only a note a person marked `human:` survives.
        notes.insert(0, human.to_owned());
    }
    if !notes.is_empty() {
        provenance.note = Some(notes.join(" | "));
    }
    Ok(TableDef {
        market: Market::Nem,
        source: SourceKind::Mms,
        name,
        package: inputs
            .dm
            .map_or_else(|| "UNKNOWN".to_owned(), |d| d.package.clone()),
        visibility,
        description: inputs
            .dm
            .and_then(|d| d.description.as_deref())
            .map(tidy_text)
            .filter(|d| !d.is_empty()),
        report_pairs,
        datasets: vec![],
        archive_files: inputs.archive_files.clone(),
        feeds: inputs.existing.map(|e| e.feeds.clone()).unwrap_or_default(),
        load_mode,
        dedupe,
        primary_key,
        time_column,
        next_column_id: next_id,
        deprecated: inputs.existing.is_some_and(|e| e.deprecated),
        columns,
        versions,
        provenance,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use nemlake_schema_model::validate::validate_table;

    use super::*;
    use crate::datamodel::DataModel;
    use crate::sample::sample_path;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
    }

    fn scada_inputs<'a>(
        ctl: &'a CtlFile,
        dm: &'a DataModel,
        blocks: &'a [SampledBlock],
    ) -> Inputs<'a> {
        Inputs {
            name: "DISPATCH_UNIT_SCADA".into(),
            ctls: vec![ctl],
            dm: dm.table("DISPATCH_UNIT_SCADA"),
            samples: blocks
                .iter()
                .map(|b| (Some("2024-01".parse().unwrap()), b))
                .collect(),
            existing: None,
            snapshot: false,
            archive_files: vec!["DISPATCH_UNIT_SCADA".into()],
            ctl_path: Some("MMSDM_2024_01/CTL/PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401.ctl".into()),
        }
    }

    fn load_scada() -> (CtlFile, DataModel, Vec<SampledBlock>) {
        let ctl_text = std::fs::read_to_string(
            fixtures()
                .join("bootstrap/MMSDM_2024_01/CTL/PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401.ctl"),
        )
        .unwrap();
        let ctl = crate::ctl::parse("x.ctl", &ctl_text).unwrap();
        let dm =
            DataModel::load(&fixtures().join("bootstrap/datamodel/mms_data_model_v5.0.0.1.json"))
                .unwrap();
        let blocks = sample_path(&fixtures().join("csv/DISPATCH_UNIT_SCADA_v1_head.csv"), 100)
            .unwrap()
            .blocks;
        (ctl, dm, blocks)
    }

    #[test]
    fn merges_unit_scada_from_all_three_sources() {
        let (ctl, dm, blocks) = load_scada();
        let t = merge(&scada_inputs(&ctl, &dm, &blocks)).unwrap();
        assert_eq!(validate_table(&t), vec![], "{t:#?}");
        assert_eq!(t.package, "DISPATCH");
        assert_eq!(t.visibility, Visibility::Public);
        assert_eq!(
            t.report_pairs,
            vec![("DISPATCH".to_owned(), "UNIT_SCADA".to_owned())]
        );
        assert_eq!(t.primary_key, ["SETTLEMENTDATE", "DUID"]);
        assert_eq!(t.time_column.as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(t.load_mode, LoadMode::Append);
        assert_eq!(t.dedupe, Dedupe::Pk);
        assert_eq!(t.next_column_id, 4);
        let scada = t.column("SCADAVALUE").unwrap();
        assert_eq!(
            scada.ty,
            ColumnType::Number {
                precision: Some(16),
                scale: Some(6)
            }
        );
        assert_eq!(scada.loader.as_deref(), Some("FLOAT EXTERNAL"));
        assert_eq!(scada.oracle.as_deref(), Some("Number(16,6)"));
        assert_eq!(scada.src, ["ctl", "dm", "irow"]);
        assert!(scada.nullable);
        let duid = t.column("DUID").unwrap();
        assert_eq!(
            duid.ty,
            ColumnType::Utf8 {
                max_len: Some(20),
                dict: true
            }
        );
        assert!(!duid.nullable);
        assert_eq!(t.versions.len(), 1);
        assert_eq!(
            t.versions[0].first_seen.map(|m| m.to_string()).as_deref(),
            Some("2024-01")
        );
        assert!(t.provenance.note.is_none(), "{:?}", t.provenance.note);
    }

    #[test]
    fn existing_file_keeps_ids_curated_columns_and_old_versions() {
        let (ctl, dm, blocks) = load_scada();
        let mut existing = merge(&scada_inputs(&ctl, &dm, &blocks)).unwrap();
        // Pretend the old file numbered DUID first and curated its description.
        existing.columns[0].id = ColumnId(7);
        existing.columns[1].id = ColumnId(1);
        existing.next_column_id = 8;
        existing.columns[1].description = Some("Hand-written.".into());
        existing.columns[1].curated = true;
        existing.columns[2].aliases = vec!["OLD_SCADA".into()];
        existing.versions.push(VersionDef {
            n: SchemaVersion(2),
            columns: vec![
                "SETTLEMENTDATE".into(),
                "DUID".into(),
                "SCADAVALUE".into(),
                "QUALITY".into(),
            ],
            fingerprint: Fingerprint::of_names(["SETTLEMENTDATE", "DUID", "SCADAVALUE", "QUALITY"]),
            first_seen: Some("2026-01".parse().unwrap()),
            last_seen: None,
            src: None,
            variants: Vec::new(),
        });
        existing.versions[0].first_seen = Some("2009-07".parse().unwrap());
        existing.feeds = vec!["Dispatch_SCADA".into()];
        existing.provenance.note = Some("human: keep me | version 1 was published with 3 columns and later with 4; the longer list is recorded and shorter files read as drift".into());
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs.existing = Some(&existing);
        let t = merge(&inputs).unwrap();
        assert_eq!(t.column("SETTLEMENTDATE").unwrap().id, ColumnId(7));
        assert_eq!(t.column("DUID").unwrap().id, ColumnId(1));
        assert_eq!(
            t.column("DUID").unwrap().description.as_deref(),
            Some("Hand-written.")
        );
        assert!(t.column("DUID").unwrap().curated);
        assert_eq!(t.column("SCADAVALUE").unwrap().aliases, ["OLD_SCADA"]);
        assert_eq!(t.column("QUALITY").unwrap().id, ColumnId(8));
        assert_eq!(t.next_column_id, 9);
        assert_eq!(t.versions.len(), 2);
        assert_eq!(
            t.versions[0].first_seen.map(|m| m.to_string()).as_deref(),
            Some("2009-07")
        );
        assert_eq!(
            t.versions[0].last_seen.map(|m| m.to_string()).as_deref(),
            Some("2024-01")
        );
        assert_eq!(t.column("QUALITY").unwrap().added_in, SchemaVersion(2));
        assert_eq!(
            t.column("QUALITY").unwrap().src,
            ["inferred"],
            "no source knows QUALITY's type"
        );
        assert_eq!(t.feeds, ["Dispatch_SCADA"]);
        assert!(
            t.provenance
                .note
                .as_deref()
                .unwrap()
                .starts_with("human: keep me")
        );
        let issues = validate_table(&t);
        assert!(
            issues.iter().any(|i| i.message.contains("inferred")),
            "{issues:?}"
        );
    }

    #[test]
    fn snapshot_table_has_no_time_column_and_keeps_pk() {
        let dm =
            DataModel::load(&fixtures().join("bootstrap/datamodel/mms_data_model_v5.0.0.1.json"))
                .unwrap();
        let ctl_text = std::fs::read_to_string(
            fixtures().join("bootstrap/MMSDM_2024_01/CTL/PUBLIC_DVD_REGION_202401.ctl"),
        )
        .unwrap();
        let ctl = crate::ctl::parse("x.ctl", &ctl_text).unwrap();
        let blocks = sample_path(&fixtures().join("csv/REGION_v1_head.csv"), 100)
            .unwrap()
            .blocks;
        let t = merge(&Inputs {
            name: "REGION".into(),
            ctls: vec![&ctl],
            dm: dm.table("REGION"),
            samples: blocks.iter().map(|b| (None, b)).collect(),
            existing: None,
            snapshot: true,
            archive_files: vec!["REGION".into()],
            ctl_path: None,
        })
        .unwrap();
        assert_eq!(validate_table(&t), vec![]);
        assert_eq!(t.load_mode, LoadMode::Snapshot);
        assert_eq!(t.time_column, None);
        assert_eq!(t.primary_key, ["REGIONID"]);
        assert_eq!(t.dedupe, Dedupe::PkLastchanged);
        assert!(t.versions[0].first_seen.is_none());
        let desc = t.column("DESCRIPTION").unwrap();
        assert!(matches!(desc.ty, ColumnType::Utf8 { dict: false, .. }));
    }

    #[test]
    fn conflicts_are_noted_and_ctl_wins() {
        let (ctl, dm, blocks) = load_scada();
        let mut dm_t = dm.table("DISPATCH_UNIT_SCADA").unwrap().clone();
        dm_t.columns[2].oracle = "VARCHAR2(5)".into(); // disagrees with FLOAT EXTERNAL
        dm_t.columns[1].oracle = "VARCHAR2(3)".into(); // shorter than observed
        dm_t.primary_key.push("GHOST".into());
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs.dm = Some(&dm_t);
        let t = merge(&inputs).unwrap();
        assert_eq!(
            t.column("SCADAVALUE").unwrap().ty,
            ColumnType::Number {
                precision: None,
                scale: None
            }
        );
        assert_eq!(
            t.column("DUID").unwrap().ty,
            ColumnType::Utf8 {
                max_len: None,
                dict: false
            }
        );
        let note = t.provenance.note.unwrap();
        assert!(note.contains("the CTL wins"), "{note}");
        assert!(note.contains("max_len dropped"), "{note}");
        assert!(note.contains("GHOST"), "{note}");
    }

    #[test]
    fn errors() {
        let (ctl, dm, blocks) = load_scada();
        let e = merge(&Inputs {
            name: "X".into(),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(e, MergeError::NoColumns { table: "X".into() });
        assert_eq!(e.diagnostic().code, "schema.merge_no_columns");
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs.samples.clear();
        let e = merge(&inputs).unwrap_err();
        assert!(matches!(e, MergeError::NoVersion { .. }));
        assert_eq!(e.diagnostic().code, "schema.merge_no_version");
        assert_eq!(e.diagnostic().table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
    }

    #[test]
    fn other_lists_of_a_version_become_variants_not_errors() {
        let (ctl, dm, blocks) = load_scada();
        // Same length with two names swapped: a reorder, not a rename.
        let mut swapped = blocks[0].clone();
        swapped.columns[1] = "SCADAVALUE".into();
        swapped.columns[2] = "DUID".into();
        swapped.fingerprint = Fingerprint::of_names(swapped.columns.iter().map(String::as_str));
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs.samples.push((None, &swapped));
        let t = merge(&inputs).unwrap();
        assert_eq!(validate_table(&t), vec![]);
        assert_eq!(t.versions.len(), 1);
        assert_eq!(t.versions[0].variants.len(), 1);
        assert!(
            t.columns.iter().all(|c| c.aliases.is_empty()),
            "no alias from a reorder"
        );
        assert_eq!(t.columns.len(), 3);

        // A longer list that renames one column and appends another: the new
        // name sits beside nothing it could alias, so both are columns. The
        // longer list is the version; the shorter one is a variant that
        // lacks OTHER and MORE, and carries DUID which the version lacks.
        let mut other = blocks[0].clone();
        other.columns[1] = "OTHER".into();
        other.columns.push("MORE".into());
        other.max_len.push(1);
        other.all_null.push(false);
        other.all_numeric.push(true);
        other.all_timestamp.push(false);
        other.fingerprint = Fingerprint::of_names(other.columns.iter().map(String::as_str));
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs
            .samples
            .push((Some("2026-09".parse().unwrap()), &other));
        let t = merge(&inputs).unwrap();
        assert_eq!(
            t.versions[0].columns,
            ["SETTLEMENTDATE", "OTHER", "SCADAVALUE", "MORE"]
        );
        assert_eq!(t.versions[0].variants.len(), 1);
        assert_eq!(t.versions[0].variants[0].columns, blocks[0].columns);
        let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["SETTLEMENTDATE", "OTHER", "SCADAVALUE", "MORE", "DUID"]
        );
        // DUID is a key the newer list does not carry, so it must admit null.
        assert!(t.primary_key.contains(&"DUID".to_owned()));
        assert!(t.column("DUID").unwrap().nullable);
        assert!(!t.column("SETTLEMENTDATE").unwrap().nullable);
        assert!(
            t.provenance
                .note
                .as_deref()
                .unwrap()
                .contains("differ by more than appended columns"),
            "{:?}",
            t.provenance.note
        );
        assert_eq!(validate_table(&t), vec![]);
    }

    #[test]
    fn a_column_dropped_later_is_typed_by_an_older_control_file() {
        let (ctl, dm, blocks) = load_scada();
        let old_ctl = crate::ctl::parse(
            "old.ctl",
            "LOAD DATA\nINFILE 'PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.CSV'\nAPPEND INTO TABLE DISPATCH_UNIT_SCADA\nWHEN (1:1) = 'D'\nFIELDS TERMINATED BY ','\n(row_type FILLER, report_type FILLER, report_subtype FILLER, report_version FILLER,\nSETTLEMENTDATE DATE \"yyyy/mm/dd hh24:mi:ss\", DUID CHAR(20), SCADAVALUE CHAR(5), OLDFLAG FLOAT EXTERNAL)\n",
        )
        .unwrap();
        let mut older = blocks[0].clone();
        older.version = 0;
        older.columns.push("OLDFLAG".into());
        older.max_len.push(0);
        older.all_null.push(true);
        older.all_numeric.push(true);
        older.all_timestamp.push(true);
        older.fingerprint = Fingerprint::of_names(older.columns.iter().map(String::as_str));
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs.ctls.push(&old_ctl);
        inputs
            .samples
            .push((Some("2012-01".parse().unwrap()), &older));
        let t = merge(&inputs).unwrap();
        assert_eq!(validate_table(&t), vec![]);
        // The newest control file types the columns it knows...
        assert!(matches!(
            t.column("SCADAVALUE").unwrap().ty,
            ColumnType::Number { .. }
        ));
        // ...and the older one the column only it has, though no value was sampled.
        let old = t.column("OLDFLAG").unwrap();
        assert!(matches!(old.ty, ColumnType::Number { .. }));
        assert_eq!(old.src, ["ctl", "irow"]);
        assert_eq!(old.removed_in, Some(SchemaVersion(1)));
    }

    #[test]
    fn same_version_with_appended_columns_takes_the_superset_and_types_from_evidence() {
        let (ctl, dm, blocks) = load_scada();
        let mut longer = blocks[0].clone();
        for (name, numeric, ts) in [
            ("EXTRA_MW", true, false),
            ("EXTRA_TIME", false, true),
            ("EXTRA_TEXT", false, false),
            ("EXTRA_EMPTY", true, true),
        ] {
            longer.columns.push(name.into());
            longer
                .max_len
                .push(if name == "EXTRA_EMPTY" { 0 } else { 5 });
            longer.all_null.push(name == "EXTRA_EMPTY");
            longer.all_numeric.push(numeric);
            longer.all_timestamp.push(ts);
        }
        longer.fingerprint = Fingerprint::of_names(longer.columns.iter().map(String::as_str));
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs
            .samples
            .push((Some("2026-09".parse().unwrap()), &longer));
        let t = merge(&inputs).unwrap();
        assert_eq!(t.versions.len(), 1);
        assert_eq!(t.versions[0].columns.len(), 7);
        assert_eq!(t.versions[0].fingerprint, longer.fingerprint);
        assert!(
            t.provenance
                .note
                .as_deref()
                .unwrap()
                .contains("the shorter a variant")
        );
        // The shorter published list is a variant with its own sighting.
        let variants = &t.versions[0].variants;
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].columns, blocks[0].columns);
        assert_eq!(variants[0].fingerprint, blocks[0].fingerprint);
        assert_eq!(variants[0].first_seen, inputs.samples[0].0);

        // Regenerating over the result with only the long sample keeps the variant.
        let mut again = scada_inputs(&ctl, &dm, &[]);
        again.samples = vec![(Some("2026-10".parse().unwrap()), &longer)];
        again.existing = Some(&t);
        let t2 = merge(&again).unwrap();
        assert_eq!(t2.versions[0].variants, t.versions[0].variants);
        assert_eq!(t2.versions[0].last_seen, Some("2026-10".parse().unwrap()));
        assert_eq!(
            t.column("EXTRA_MW").unwrap().ty,
            ColumnType::Number {
                precision: None,
                scale: None
            }
        );
        assert_eq!(t.column("EXTRA_MW").unwrap().src, ["irow", "sampled"]);
        assert_eq!(t.column("EXTRA_TIME").unwrap().ty, ColumnType::Timestamp);
        assert_eq!(
            t.column("EXTRA_TEXT").unwrap().ty,
            ColumnType::Utf8 {
                max_len: None,
                dict: false
            }
        );
        assert_eq!(t.column("EXTRA_EMPTY").unwrap().src, ["irow", "inferred"]);
        assert!(
            validate_table(&t)
                .iter()
                .any(|i| i.message.contains("EXTRA_EMPTY"))
        );
    }

    #[test]
    fn same_version_rename_becomes_alias() {
        let (ctl, dm, blocks) = load_scada();
        let mut renamed = blocks[0].clone();
        renamed.columns[1] = "CONFIDENTIAL_TO".into();
        renamed.fingerprint = Fingerprint::of_names(renamed.columns.iter().map(String::as_str));
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs
            .samples
            .push((Some("2026-09".parse().unwrap()), &renamed));
        let t = merge(&inputs).unwrap();
        assert_eq!(validate_table(&t), vec![]);
        let duid = t.column("DUID").unwrap();
        assert_eq!(duid.aliases, ["CONFIDENTIAL_TO"]);
        assert_eq!(t.versions[0].columns[1], "DUID");
        assert!(t.column("CONFIDENTIAL_TO").is_some(), "alias lookup works");
        // The renamed list is a variant, written with the name as published.
        assert_eq!(t.versions[0].variants.len(), 1);
        assert_eq!(t.versions[0].variants[0].columns[1], "CONFIDENTIAL_TO");
        assert_eq!(t.versions[0].variants[0].fingerprint, renamed.fingerprint);
        assert!(
            t.provenance
                .note
                .as_deref()
                .unwrap()
                .contains("recorded as an alias")
        );
        // The data-model name wins even when the renamed sample is the longer/first one.
        let mut inputs = scada_inputs(&ctl, &dm, &blocks);
        inputs.samples.insert(0, (None, &renamed));
        let t = merge(&inputs).unwrap();
        assert_eq!(t.versions[0].columns[1], "DUID");
    }

    #[test]
    fn tidy_text_strips_line_ends() {
        assert_eq!(tidy_text("a: \n b\t\n\n"), "a:\n b");
        assert_eq!(tidy_text("\n\nplain"), "plain");
        assert_eq!(tidy_text("  \n"), "");
    }

    #[test]
    fn loader_mapping() {
        assert_eq!(loader_to_type(&LoaderType::Filler), None);
        assert_eq!(
            loader_to_type(&LoaderType::Char(3)),
            Some(ColumnType::Utf8 {
                max_len: Some(3),
                dict: false
            })
        );
        assert_eq!(
            loader_to_type(&LoaderType::Date("yyyy/mm/dd".into())),
            Some(ColumnType::Date)
        );
        assert_eq!(
            loader_to_type(&LoaderType::Date("yyyy/mm/dd hh24:mi:ss".into())),
            Some(ColumnType::Timestamp)
        );
        assert_eq!(
            loader_to_type(&LoaderType::Timestamp(None)),
            Some(ColumnType::Timestamp)
        );
        assert_eq!(
            loader_to_type(&LoaderType::Timestamp(Some("yyyy/mm/dd".into()))),
            Some(ColumnType::Date)
        );
        assert_eq!(
            loader_to_type(&LoaderType::DecimalExternal),
            Some(ColumnType::Number {
                precision: None,
                scale: None
            })
        );
        assert_eq!(
            loader_to_type(&LoaderType::IntegerExternal),
            Some(ColumnType::Integer)
        );
        assert!(is_dict_like(
            "REGIONID",
            ColumnType::Utf8 {
                max_len: Some(10),
                dict: false
            }
        ));
        assert!(!is_dict_like(
            "DESCRIPTION",
            ColumnType::Utf8 {
                max_len: Some(10),
                dict: false
            }
        ));
        assert!(!is_dict_like(
            "REGIONID",
            ColumnType::Utf8 {
                max_len: Some(200),
                dict: false
            }
        ));
        assert!(!is_dict_like("REGIONID", ColumnType::Integer));
    }
}
