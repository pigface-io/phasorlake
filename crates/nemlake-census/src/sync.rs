//! `nemlake-census sync`: our schemas follow the control files AEMO
//! publishes, one archived month after another.
//!
//! For each month, the control file of each table is compared with the
//! increment in force that month. A control file that lays the table out
//! as that increment's does changes nothing. One that lays it out anew
//! appends an increment from that month, numbered with the next free `n`,
//! with the control file kept byte for byte beside `table.toml`; a column
//! new to the table gets a fresh id and the default type of its loader
//! type. A table the schemas lack starts from its control file, the data
//! model (key, description), the README's list of standing-data tables and
//! the census of the month (its report pairs). The I-row versions the
//! census recorded join the increment in force.
//!
//! Nothing that is there is changed: no increment's month, columns or
//! control file, no column's id, type or nullability.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use nemlake_core::{Category, Diagnostic, YearMonth};
use nemlake_registry::file_model::blob::SchemaSet;
use nemlake_registry::file_model::ctl::{CtlFile, parse, sha256};
use nemlake_registry::file_model::file::{CTL_DIR, TABLE_FILE, ctl_file_name, write_table};
use nemlake_registry::file_model::shapes::ShapeEntry;
use nemlake_registry::file_model::{
    Column, ColumnId, Dedupe, Increment, LakeType, LoadMode, Market, Table,
};

use crate::census::{FileEntry, table_keys};
use crate::datamodel::DataModel;

/// Columns that partition a table by month, in the order they are chosen.
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

/// A control file of a month: AEMO's name for it, its bytes, parsed.
#[derive(Debug, Clone)]
pub struct MonthCtl {
    /// AEMO's file name.
    pub name: String,
    /// The file as published.
    pub bytes: Vec<u8>,
    /// Parsed.
    pub ctl: CtlFile,
}

/// The control files of a month by the table each loads, upper-case. Of
/// several for one table (`FILE01`, `FILE02`, splits) the first by name
/// stands. One that does not parse is a problem, and the rest go on.
pub fn by_table(
    files: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> (BTreeMap<String, MonthCtl>, Vec<Diagnostic>) {
    let mut files: Vec<(String, Vec<u8>)> = files.into_iter().collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = BTreeMap::new();
    let mut problems = Vec::new();
    for (name, bytes) in files {
        match parse(&name, &String::from_utf8_lossy(&bytes)) {
            Ok(ctl) => {
                out.entry(ctl.table.to_ascii_uppercase())
                    .or_insert(MonthCtl { name, bytes, ctl });
            }
            Err(e) => problems.push(nemlake_core::Diagnose::diagnostic(&e)),
        }
    }
    (out, problems)
}

/// The control files among a month's archived files, loose or inside a
/// whole-month zip, as `(name, bytes)`. `tmp` takes nested zips.
pub fn read_ctls(files: &[PathBuf], tmp: &Path) -> (Vec<(String, Vec<u8>)>, Vec<Diagnostic>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let read = |name: String, r: &mut dyn std::io::Read, out: &mut Vec<(String, Vec<u8>)>| {
        let mut bytes = Vec::new();
        if r.read_to_end(&mut bytes).is_ok() {
            out.push((name, bytes));
        }
    };
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".ctl") {
            if let Ok(mut f) = std::fs::File::open(path) {
                read(name, &mut f, &mut out);
            }
        } else if lower.ends_with(".zip") {
            let walked = nemlake_decode::archive::walk_csv_entries::<Box<Diagnostic>>(
                path,
                tmp,
                &mut |_| Ok(()),
                &mut |e| {
                    let base = e.name.rsplit(['/', '!']).next().unwrap_or(&e.name);
                    if base.to_ascii_lowercase().ends_with(".ctl") {
                        read(base.to_owned(), e.reader, &mut out);
                    }
                },
            );
            match walked {
                Ok(_)
                | Err(nemlake_decode::archive::WalkError::Archive(
                    nemlake_decode::archive::ArchiveError::NoCsv { .. },
                )) => {}
                Err(nemlake_decode::archive::WalkError::Archive(a)) => {
                    problems.push(nemlake_core::Diagnose::diagnostic(&a));
                }
                Err(nemlake_decode::archive::WalkError::Callback(d)) => problems.push(*d),
            }
        }
    }
    (out, problems)
}

/// What the census of a month saw: per archive file table (the table part
/// of a file's name), the report pairs and I-row versions of its blocks.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    blocks: BTreeMap<String, BTreeSet<(String, String, u16)>>,
}

impl Evidence {
    /// The census of a month (file name to shape ids) with every shape
    /// the census recorded.
    pub fn new(files: &BTreeMap<String, FileEntry>, shapes: &[ShapeEntry]) -> Self {
        let by_id: BTreeMap<_, _> = shapes.iter().map(|e| (e.id, e)).collect();
        let mut blocks: BTreeMap<String, BTreeSet<(String, String, u16)>> = BTreeMap::new();
        for (file, entry) in files {
            let Some(table) = nemlake_core::archive_name::parse(file).map(|n| n.table) else {
                continue;
            };
            for id in entry.ids() {
                if let Some(e) = by_id.get(&id) {
                    blocks
                        .entry(table.to_ascii_uppercase())
                        .or_default()
                        .insert((e.report_type.clone(), e.report_subtype.clone(), e.version));
                }
            }
        }
        Self { blocks }
    }

    /// The blocks of the files a control file loads: those named for the
    /// table its `INFILE` names, or a split of it.
    fn of_ctl(&self, ctl: &CtlFile) -> BTreeSet<(String, String, u16)> {
        let Some(infile) = nemlake_core::archive_name::parse(&ctl.infile).map(|n| n.table) else {
            return BTreeSet::new();
        };
        let wanted = table_keys(&infile);
        self.blocks
            .iter()
            .filter(|(t, _)| table_keys(t).iter().any(|k| wanted.contains(k)))
            .flat_map(|(_, b)| b.iter().cloned())
            .collect()
    }

    /// The versions of the blocks of these report pairs.
    fn versions(&self, pairs: &[(String, String)]) -> BTreeSet<u16> {
        self.blocks
            .values()
            .flatten()
            .filter(|(rt, st, _)| pairs.iter().any(|(a, b)| a == rt && b == st))
            .map(|(_, _, v)| *v)
            .collect()
    }
}

/// What a new table takes from outside its control file.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// AEMO's data model: keys and descriptions.
    pub datamodel: Option<DataModel>,
    /// Tables of standing data, loaded as snapshots.
    pub snapshots: BTreeSet<String>,
}

/// A table being synced: its definition, and the control files to write
/// beside it.
#[derive(Debug, Clone)]
struct Entry {
    table: Table,
    new_ctls: Vec<(String, Vec<u8>)>,
    changed: bool,
    /// Under `--rebuild`, the increments the table had: they come back if
    /// no month synced has a control file of the table.
    before: Option<Vec<Increment>>,
}

/// The schemas of a market, as `sync` changes them.
#[derive(Debug)]
pub struct Sync {
    dir: PathBuf,
    tables: BTreeMap<String, Entry>,
}

impl Sync {
    /// The tables under `schemas/nem/`.
    pub fn load(schemas: &Path) -> Result<Self, Box<Diagnostic>> {
        let set = SchemaSet::read_dir(schemas)
            .map_err(|e| Box::new(nemlake_core::Diagnose::diagnostic(&e)))?;
        Ok(Self {
            dir: schemas.join(Market::Nem.as_str()),
            tables: set
                .tables
                .into_iter()
                .filter(|t| t.market == Market::Nem)
                .map(|table| {
                    (
                        table.name.clone(),
                        Entry {
                            table,
                            new_ctls: Vec::new(),
                            changed: false,
                            before: None,
                        },
                    )
                })
                .collect(),
        })
    }

    /// The table, as it stands now.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.get(name).map(|e| &e.table)
    }

    /// The control file kept for an increment, parsed.
    fn kept_ctl(&self, e: &Entry, inc: &Increment) -> Option<CtlFile> {
        let name = inc.ctl.as_ref()?;
        let bytes = e
            .new_ctls
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.clone())
            .or_else(|| {
                std::fs::read(self.dir.join(&e.table.name).join(CTL_DIR).join(name)).ok()
            })?;
        parse(name, &String::from_utf8_lossy(&bytes)).ok()
    }

    /// Takes in month `month`: its control files, by table, and what its
    /// census saw. Says what changed, one line each.
    pub fn month(
        &mut self,
        month: YearMonth,
        ctls: &BTreeMap<String, MonthCtl>,
        evidence: &Evidence,
        facts: &Facts,
    ) -> Vec<String> {
        let mut said = Vec::new();
        for (name, c) in ctls {
            if self.tables.contains_key(name) {
                said.extend(self.increment(name, month, c, evidence));
            } else {
                said.push(
                    self.new_table(name, month, c, evidence, facts)
                        .unwrap_or_else(|why| format!("skipped {name}: {why}")),
                );
            }
        }
        // The versions the census saw join the increment in force.
        for e in self.tables.values_mut() {
            let seen = evidence.versions(&e.table.report_pairs);
            let Some(inc) = e
                .table
                .increments
                .iter_mut()
                .rev()
                .find(|i| i.from <= month)
            else {
                continue;
            };
            let before = inc.aemo_versions.len();
            inc.aemo_versions.extend(seen);
            inc.aemo_versions.sort_unstable();
            inc.aemo_versions.dedup();
            if inc.aemo_versions.len() != before {
                said.push(format!(
                    "{}@{}: AEMO version(s) {:?} seen in {month}",
                    e.table.name, inc.n, inc.aemo_versions
                ));
                e.changed = true;
            }
        }
        said
    }

    /// Appends an increment to `name` when `c` lays it out anew in `month`.
    fn increment(
        &mut self,
        name: &str,
        month: YearMonth,
        c: &MonthCtl,
        evidence: &Evidence,
    ) -> Option<String> {
        let e = &self.tables[name];
        let t = &e.table;
        // A table being rebuilt has no increment until its first month.
        let in_force = t
            .increments
            .iter()
            .rev()
            .find(|i| i.from <= month)
            .or_else(|| t.increments.first());
        if let Some(in_force) = in_force {
            let same = match self.kept_ctl(e, in_force) {
                Some(kept) => kept.same_layout(&c.ctl),
                None => in_force.columns == c.ctl.column_names(),
            };
            if same || t.increments.iter().any(|i| i.from == month) {
                return None;
            }
        }
        let n = t.next_increment();
        let versions = evidence.versions(&t.report_pairs);
        let e = self.tables.get_mut(name).expect("the table is there");
        let added = add_columns(&mut e.table, &c.ctl);
        let file = ctl_file_name(n);
        let inc = Increment {
            n,
            from: month,
            ctl: Some(file.clone()),
            ctl_sha256: Some(sha256(&c.bytes)),
            aemo_ctl: Some(c.name.clone()),
            aemo_versions: versions.into_iter().collect(),
            columns: c.ctl.column_names(),
            note: None,
        };
        let at = e
            .table
            .increments
            .iter()
            .position(|i| i.from > month)
            .unwrap_or(e.table.increments.len());
        e.table.increments.insert(at, inc);
        e.new_ctls.push((file, c.bytes.clone()));
        e.changed = true;
        // A key column this layout lacks takes nulls in its files.
        let relaxed = relax_missing(&mut e.table);
        let mut line = format!("{name}@{n}: new increment from {month} ({})", c.name);
        if !added.is_empty() {
            let _ = write!(line, ", new column(s) {}", added.join(", "));
        }
        if !relaxed.is_empty() {
            let _ = write!(line, ", made nullable {}", relaxed.join(", "));
        }
        Some(line)
    }

    /// A table the schemas lack, from its control file of `month`.
    fn new_table(
        &mut self,
        name: &str,
        month: YearMonth,
        c: &MonthCtl,
        evidence: &Evidence,
        facts: &Facts,
    ) -> Result<String, String> {
        let claimed: BTreeSet<(&str, &str)> = self
            .tables
            .values()
            .flat_map(|e| e.table.report_pairs.iter())
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let blocks = evidence.of_ctl(&c.ctl);
        let mut report_pairs: Vec<(String, String)> = blocks
            .iter()
            .map(|(rt, st, _)| (rt.clone(), st.clone()))
            .filter(|(rt, st)| !claimed.contains(&(rt.as_str(), st.as_str())))
            .collect();
        report_pairs.dedup();
        if report_pairs.is_empty() {
            return Err(format!(
                "the census of {month} has no block of the files its control file {} loads; take the census of {month} first",
                c.name
            ));
        }
        let columns = c.ctl.column_names();
        let dm = facts.datamodel.as_ref().and_then(|d| d.table(name));
        // The data model lists a key alphabetically; it is kept in file order.
        let primary_key: Vec<String> = dm
            .map(|d| {
                columns
                    .iter()
                    .filter(|c| d.primary_key.iter().any(|k| k.eq_ignore_ascii_case(c)))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let snapshot = facts.snapshots.contains(name);
        let mut t = Table {
            market: Market::Nem,
            name: name.to_owned(),
            description: dm.and_then(|d| d.description.clone()),
            report_pairs,
            load_mode: if snapshot {
                LoadMode::Snapshot
            } else {
                LoadMode::Append
            },
            dedupe: match (
                primary_key.is_empty(),
                columns.iter().any(|c| c == "LASTCHANGED"),
            ) {
                (true, _) => Dedupe::None,
                (false, true) => Dedupe::PkLastchanged,
                (false, false) => Dedupe::Pk,
            },
            primary_key,
            time_column: None,
            next_column_id: 1,
            columns: Vec::new(),
            increments: Vec::new(),
        };
        add_columns(&mut t, &c.ctl);
        for col in &mut t.columns {
            col.nullable = !t.primary_key.contains(&col.name);
        }
        if !snapshot {
            let timed = |c: &&Column| matches!(c.ty, LakeType::TimestampNs | LakeType::Date);
            t.time_column = TIME_CANDIDATES
                .iter()
                .find_map(|n| t.columns.iter().filter(timed).find(|c| c.name == *n))
                .or_else(|| {
                    t.columns
                        .iter()
                        .filter(timed)
                        .find(|c| t.primary_key.contains(&c.name))
                })
                .or_else(|| t.columns.iter().find(timed))
                .map(|c| c.name.clone());
        }
        let file = ctl_file_name(1);
        t.increments.push(Increment {
            n: 1,
            from: month,
            ctl: Some(file.clone()),
            ctl_sha256: Some(sha256(&c.bytes)),
            aemo_ctl: Some(c.name.clone()),
            aemo_versions: blocks
                .iter()
                .map(|(_, _, v)| *v)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            columns,
            note: None,
        });
        let line = format!(
            "{name}: new table from {month} ({}), report pair(s) {:?}",
            c.name, t.report_pairs
        );
        self.tables.insert(
            name.to_owned(),
            Entry {
                table: t,
                new_ctls: vec![(file, c.bytes.clone())],
                changed: true,
                before: None,
            },
        );
        Ok(line)
    }

    /// Starts every table's increments again: each comes back, in month
    /// order and numbered from 1, from the control files of the months
    /// synced next. Columns, ids, types and notes stay; so does what an
    /// increment did not come from (the notes of a hand fix are lost).
    pub fn rebuild(&mut self) {
        for e in self.tables.values_mut() {
            e.before = Some(std::mem::take(&mut e.table.increments));
            e.new_ctls.clear();
            e.changed = true;
        }
    }

    /// Ends a rebuild: a table no synced month had a control file of keeps
    /// the increments it had. Says which, one line each.
    pub fn finish(&mut self) -> Vec<String> {
        let mut said = Vec::new();
        for e in self.tables.values_mut() {
            let Some(before) = e.before.take() else {
                continue;
            };
            if e.table.increments.is_empty() {
                e.table.increments = before;
                e.changed = false;
                said.push(format!(
                    "{}: no control file in the months synced; its increments are kept",
                    e.table.name
                ));
            } else if e.table.increments == before {
                e.changed = false;
            } else {
                e.before = Some(before);
            }
        }
        said
    }

    /// The tables that changed.
    pub fn changed(&self) -> usize {
        self.tables.values().filter(|e| e.changed).count()
    }

    /// Writes every table that changed and its new control files.
    pub fn write(&self) -> Result<usize, Box<Diagnostic>> {
        let fail = |path: &Path, e: &std::io::Error| {
            Box::new(
                Diagnostic::error(
                    "schema.sync_write",
                    Category::Schema,
                    format!("The schema file {} could not be written.", path.display()),
                )
                .with_file(path.display().to_string())
                .caused_by(e),
            )
        };
        for e in self.tables.values().filter(|e| e.changed) {
            let dir = self.dir.join(&e.table.name);
            let ctl_dir = dir.join(CTL_DIR);
            // A rebuilt table's control files are all new.
            if e.before.is_some() && ctl_dir.is_dir() {
                std::fs::remove_dir_all(&ctl_dir).map_err(|err| fail(&ctl_dir, &err))?;
            }
            std::fs::create_dir_all(&ctl_dir).map_err(|err| fail(&ctl_dir, &err))?;
            for (name, bytes) in &e.new_ctls {
                let p = ctl_dir.join(name);
                std::fs::write(&p, bytes).map_err(|err| fail(&p, &err))?;
            }
            let p = dir.join(TABLE_FILE);
            std::fs::write(&p, write_table(&e.table)).map_err(|err| fail(&p, &err))?;
        }
        Ok(self.changed())
    }
}

/// Makes nullable each non-nullable column an increment lacks; the names.
fn relax_missing(t: &mut Table) -> Vec<String> {
    let mut out = Vec::new();
    for c in &mut t.columns {
        if !c.nullable && t.increments.iter().any(|i| !i.columns.contains(&c.name)) {
            c.nullable = true;
            out.push(c.name.clone());
        }
    }
    out
}

/// Adds each column of `ctl` the table lacks, with a fresh id and the
/// default type of its loader type; the names added.
fn add_columns(t: &mut Table, ctl: &CtlFile) -> Vec<String> {
    let mut added = Vec::new();
    for c in ctl.data_columns() {
        let name = c.name.to_ascii_uppercase();
        if t.column(&name).is_some() {
            continue;
        }
        t.columns.push(Column {
            id: ColumnId(t.next_column_id),
            name: name.clone(),
            ty: c.ty.default_type().expect("a data column has a type"),
            nullable: true,
            note: None,
        });
        t.next_column_id += 1;
        added.push(name);
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// A bootstrap month's control files, by table.
    fn fixture_ctls(month: &str) -> BTreeMap<String, MonthCtl> {
        let dir = root()
            .join("testdata/bootstrap")
            .join(format!("MMSDM_{}", month.replace('-', "_")))
            .join("CTL");
        let files = std::fs::read_dir(dir).unwrap().map(|e| {
            let p = e.unwrap().path();
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        });
        let (ctls, problems) = by_table(files);
        assert!(problems.is_empty(), "{problems:?}");
        ctls
    }

    /// The census of 2026-08 as it is committed.
    fn evidence_2026_08() -> Evidence {
        let schemas = root().join("schemas");
        let month = crate::census::read_month(&schemas, "2026-08".parse().unwrap())
            .unwrap()
            .unwrap();
        let shapes: Vec<ShapeEntry> = SchemaSet::read_dir(&schemas)
            .unwrap()
            .shapes
            .into_values()
            .flatten()
            .collect();
        Evidence::new(&month, &shapes)
    }

    /// The committed schemas are what syncing the bootstrap months gives:
    /// nothing changes.
    #[test]
    fn the_bootstrap_months_change_nothing() {
        let mut s = Sync::load(&root().join("schemas")).unwrap();
        let facts = Facts::default();
        let said = s.month(
            "2024-01".parse().unwrap(),
            &fixture_ctls("2024-01"),
            &Evidence::default(),
            &facts,
        );
        assert!(said.is_empty(), "{said:?}");
        let said = s.month(
            "2026-08".parse().unwrap(),
            &fixture_ctls("2026-08"),
            &evidence_2026_08(),
            &facts,
        );
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(s.changed(), 0);
    }

    /// A month whose control file adds a column gets an increment with it,
    /// the column a fresh id, and every other id kept.
    #[test]
    fn a_control_file_with_a_new_column_appends_an_increment() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("schemas");
        let nem = schemas.join("nem");
        let table_dir = nem.join("DISPATCH_UNIT_SCADA");
        std::fs::create_dir_all(table_dir.join(CTL_DIR)).unwrap();
        let committed = root().join("schemas/nem/DISPATCH_UNIT_SCADA");
        for f in ["table.toml", "ctl/001.ctl", "ctl/002.ctl"] {
            std::fs::copy(committed.join(f), table_dir.join(f)).unwrap();
        }
        std::fs::copy(
            root().join("schemas/nem/market.toml"),
            nem.join("market.toml"),
        )
        .unwrap();
        let mut s = Sync::load(&schemas).unwrap();
        let before = s.table("DISPATCH_UNIT_SCADA").unwrap().clone();
        let c = &fixture_ctls("2026-08")["DISPATCH_UNIT_SCADA"];
        let text = String::from_utf8(c.bytes.clone()).unwrap().replace(
            "LASTCHANGED DATE \"YYYY/MM/DD HH24:MI:SS\")",
            "LASTCHANGED DATE \"YYYY/MM/DD HH24:MI:SS\",\nQUALITY CHAR(10))",
        );
        let wider = MonthCtl {
            name: c.name.replace("202608", "202611"),
            ctl: parse("x.ctl", &text).unwrap(),
            bytes: text.into_bytes(),
        };
        let month: YearMonth = "2026-11".parse().unwrap();
        let ctls: BTreeMap<String, MonthCtl> =
            [("DISPATCH_UNIT_SCADA".to_owned(), wider.clone())].into();
        let said = s.month(month, &ctls, &Evidence::default(), &Facts::default());
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains("@3: new increment from 2026-11"),
            "{said:?}"
        );
        assert!(said[0].contains("QUALITY"), "{said:?}");
        let t = s.table("DISPATCH_UNIT_SCADA").unwrap().clone();
        for c in &before.columns {
            assert_eq!(t.column(&c.name).unwrap().id, c.id, "{}", c.name);
        }
        let q = t.column("QUALITY").unwrap();
        assert_eq!(
            (q.id, q.ty, q.nullable),
            (ColumnId(5), LakeType::String, true)
        );
        assert_eq!(t.next_column_id, 6);
        assert_eq!(t.increments[..2], before.increments[..]);
        let inc = &t.increments[2];
        assert_eq!((inc.n, inc.from), (3, month));
        assert_eq!(inc.columns.last().map(String::as_str), Some("QUALITY"));
        assert_eq!(
            inc.ctl_sha256.as_deref(),
            Some(sha256(&wider.bytes).as_str())
        );
        // The same layout the month after changes nothing.
        let said = s.month(month.next(), &ctls, &Evidence::default(), &Facts::default());
        assert!(said.is_empty(), "{said:?}");
        // Written, it is a valid schema, the control file beside it.
        assert_eq!(s.write().unwrap(), 1);
        let set = SchemaSet::read_dir(&schemas).unwrap();
        assert!(set.validate().is_empty(), "{:?}", set.validate());
        assert!(table_dir.join("ctl/003.ctl").is_file());
        // An older month synced later: the next free n, in month order.
        let mut s = Sync::load(&schemas).unwrap();
        let said = s.month(
            "2025-06".parse().unwrap(),
            &ctls,
            &Evidence::default(),
            &Facts::default(),
        );
        assert!(
            said[0].contains("@4: new increment from 2025-06"),
            "{said:?}"
        );
        let order: Vec<u16> = s
            .table("DISPATCH_UNIT_SCADA")
            .unwrap()
            .increments
            .iter()
            .map(|i| i.n)
            .collect();
        assert_eq!(order, [1, 4, 2, 3]);
    }

    /// A table the schemas lack starts from its control file, the data
    /// model and the census; without the census it is skipped, with why.
    #[test]
    fn a_new_table_starts_from_its_control_file() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("schemas");
        std::fs::create_dir_all(schemas.join("nem")).unwrap();
        std::fs::copy(
            root().join("schemas/nem/market.toml"),
            schemas.join("nem/market.toml"),
        )
        .unwrap();
        let mut s = Sync::load(&schemas).unwrap();
        let all = fixture_ctls("2026-08");
        let ctls: BTreeMap<String, MonthCtl> = [(
            "DISPATCH_UNIT_SCADA".to_owned(),
            all["DISPATCH_UNIT_SCADA"].clone(),
        )]
        .into();
        let month: YearMonth = "2026-08".parse().unwrap();
        let said = s.month(month, &ctls, &Evidence::default(), &Facts::default());
        assert!(
            said[0].contains("take the census of 2026-08 first"),
            "{said:?}"
        );
        let facts = Facts {
            datamodel: Some(
                DataModel::load(
                    &root().join("testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json"),
                )
                .unwrap(),
            ),
            snapshots: BTreeSet::new(),
        };
        let said = s.month(month, &ctls, &evidence_2026_08(), &facts);
        assert!(said[0].contains("new table"), "{said:?}");
        let t = s.table("DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(t.report_pairs, [("DISPATCH".into(), "UNIT_SCADA".into())]);
        assert_eq!(t.primary_key, ["SETTLEMENTDATE", "DUID"]);
        assert_eq!(t.dedupe, Dedupe::PkLastchanged);
        assert_eq!(t.time_column.as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(t.increments[0].aemo_versions, [1]);
        assert!(!t.column("DUID").unwrap().nullable);
        assert_eq!(t.column("SCADAVALUE").unwrap().ty, LakeType::Double);
        assert!(t.description.is_some());
        s.write().unwrap();
        let set = SchemaSet::read_dir(&schemas).unwrap();
        assert!(set.validate().is_empty(), "{:?}", set.validate());
        // As standing data it is a snapshot with no time column.
        let empty = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(empty.path().join("nem")).unwrap();
        let mut s = Sync::load(empty.path()).unwrap();
        let facts = Facts {
            snapshots: ["DISPATCH_UNIT_SCADA".to_owned()].into(),
            ..facts
        };
        s.month(month, &ctls, &evidence_2026_08(), &facts);
        let t = s.table("DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(
            (t.load_mode, t.time_column.as_deref()),
            (LoadMode::Snapshot, None)
        );
    }

    /// Rebuilt from the two bootstrap months, every table's increments
    /// come back as the conversion gave them, but for the hand fixes
    /// (columns the files carry beyond their control file), which a
    /// rebuild drops; ids and types stay.
    #[test]
    fn a_rebuild_of_the_bootstrap_months_gives_the_converted_increments() {
        let mut s = Sync::load(&root().join("schemas")).unwrap();
        let before: BTreeMap<String, Table> = s
            .tables
            .iter()
            .map(|(n, e)| (n.clone(), e.table.clone()))
            .collect();
        s.rebuild();
        let facts = Facts::default();
        for (m, ev) in [
            ("2024-01", Evidence::default()),
            ("2026-08", evidence_2026_08()),
        ] {
            s.month(m.parse().unwrap(), &fixture_ctls(m), &ev, &facts);
        }
        assert!(s.finish().is_empty());
        let fixed = [
            "DISPATCHCONSTRAINT",
            "MTPASA_REGIONRESULT",
            "P5MIN_CONSTRAINTSOLUTION",
            "PREDISPATCHCONSTRAINT",
            "PREDISPATCHPRICE",
        ];
        let key = |t: &Table| -> Vec<_> {
            t.increments
                .iter()
                .map(|i| {
                    (
                        i.n,
                        i.from,
                        i.ctl.clone(),
                        i.ctl_sha256.clone(),
                        i.columns.clone(),
                    )
                })
                .collect()
        };
        for (name, old) in &before {
            let new = s.table(name).unwrap();
            assert_eq!(new.columns, old.columns, "{name}");
            if fixed.contains(&name.as_str()) {
                assert_ne!(key(new), key(old), "{name}");
                assert!(new.increments.iter().all(|i| i.note.is_none()), "{name}");
            } else {
                assert_eq!(key(new), key(old), "{name}");
            }
        }
        // PREDISPATCHPRICE's two months lay it out alike: one increment.
        assert_eq!(s.table("PREDISPATCHPRICE").unwrap().increments.len(), 1);
        // A table no synced month has keeps what it had.
        let mut s = Sync::load(&root().join("schemas")).unwrap();
        s.rebuild();
        let said = s.finish();
        assert_eq!(said.len(), before.len());
        assert_eq!(s.changed(), 0);
    }

    #[test]
    fn a_layout_without_a_key_column_makes_it_nullable() {
        let mut t = Sync::load(&root().join("schemas"))
            .unwrap()
            .table("DISPATCH_UNIT_SCADA")
            .unwrap()
            .clone();
        t.increments[0].columns.retain(|c| c != "DUID");
        assert_eq!(relax_missing(&mut t), ["DUID"]);
        assert!(t.column("DUID").unwrap().nullable);
        assert!(relax_missing(&mut t).is_empty());
    }

    #[test]
    fn unreadable_control_files_are_problems() {
        let (ctls, problems) = by_table([("bad.ctl".to_owned(), b"nope".to_vec())]);
        assert!(ctls.is_empty());
        assert_eq!(problems[0].code, "schema.ctl_parse");
        let dir = tempfile::tempdir().unwrap();
        let loose = dir.path().join("A.ctl");
        std::fs::write(&loose, b"x").unwrap();
        let bad_zip = dir.path().join("B.zip");
        std::fs::write(&bad_zip, b"not a zip").unwrap();
        let (files, problems) = read_ctls(&[loose, bad_zip], dir.path());
        assert_eq!(files, [("A.ctl".to_owned(), b"x".to_vec())]);
        assert_eq!(problems.len(), 1);
    }
}
