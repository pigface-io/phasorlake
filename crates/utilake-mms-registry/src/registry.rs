//! The loaded registry: tables, indexes and conformance plans.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::file_model::blob::SchemaSet;
use crate::file_model::history::History;
use crate::file_model::shapes::ShapeEntry;
use crate::file_model::{
    Column, ColumnId, Increment, LakeType, LoadMode, Market, MarketDef, SchemaFileError, Table,
};
use crate::shape::ShapeId;
use arrow_schema::SchemaRef;
use hashbrown::Equivalent;
use utilake_core::{Fingerprint, YearMonth};
use utilake_mms_decode::IRow;

use crate::arrow::lake_schema;
use crate::plan::ConformancePlan;

/// A dense handle to a table within one registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableId(pub u32);

/// An owned `(report_type, report_subtype)` key that can be looked up with
/// borrowed strings and no allocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PairKey(pub String, pub String);

/// The borrowed form of [`PairKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PairRef<'a>(pub &'a str, pub &'a str);

impl Equivalent<PairKey> for PairRef<'_> {
    fn equivalent(&self, key: &PairKey) -> bool {
        key.0 == self.0 && key.1 == self.1
    }
}

/// Where the definition of a table came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaSource {
    /// The registry utilake was built with, or a schemas directory loaded
    /// in its place.
    Registry,
    /// A user's own table file, laid over the registry.
    User,
    /// The file being read: no table claimed its report pair.
    File,
}

impl SchemaSource {
    /// One word for manifests and Parquet footers.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::User => "user",
            Self::File => "file",
        }
    }
}

impl std::fmt::Display for SchemaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Every table utilake knows, indexed for the hot path.
#[derive(Debug)]
pub struct Registry {
    markets: Vec<MarketDef>,
    tables: Vec<Table>,
    /// Whether each table came from a user's overlay.
    from_user: Vec<bool>,
    by_name: HashMap<String, TableId>,
    by_pair: hashbrown::HashMap<PairKey, TableId>,
    /// The lake schema and the plans of each table's increments, built the
    /// first time the table is used. A run reads a handful of tables;
    /// building them for every table the registry knows would cost each
    /// run the memory and the time of all of them.
    shapes: Vec<OnceLock<TableShapes>>,
    /// Shapes that deviate from their increment, or of a derived table,
    /// worked out when first seen.
    deviating: Mutex<HashMap<DeviationKey, Arc<Deviation>>>,
    /// Tables derived at run time from the header of a file whose report
    /// pair no table claims. Slots are written once and never moved, so a
    /// reference to one lives as long as the registry. Each is boxed: an
    /// empty slot costs a pointer, not a table.
    derived: Vec<OnceLock<Box<Derived>>>,
    /// The report pair of each derived table, in slot order.
    derived_pairs: Mutex<Vec<(String, String)>>,
    /// The census's shapes of each table, by table name; for the embedded
    /// registry decoded from their blob on first use.
    census: OnceLock<BTreeMap<String, Vec<ShapeEntry>>>,
    /// The blob the census's shapes are decoded from, when not given.
    census_blob: &'static [u8],
    /// Census shapes by report pair and fingerprint of their names, built
    /// on first use.
    census_index: OnceLock<HashMap<(String, String, Fingerprint), Vec<CensusShape>>>,
    /// Each table's history as an Iceberg table, by table name; for the
    /// embedded registry decoded from its blob on first use.
    histories: OnceLock<BTreeMap<String, History>>,
    /// The blob the histories are decoded from, when not given.
    history_blob: &'static [u8],
}

/// What resolution needs of a census shape: its id and when it was seen.
#[derive(Debug, Clone, Copy)]
struct CensusShape {
    id: ShapeId,
    first_seen: YearMonth,
    last_seen: YearMonth,
}

/// What the files of one table are written with.
#[derive(Debug)]
pub(crate) struct TableShapes {
    /// The lake schema.
    pub(crate) lake: SchemaRef,
    /// Per increment, in the table's order: the fingerprint of its columns
    /// and the plan of a file that conforms to it.
    pub(crate) increments: Vec<(Fingerprint, Arc<ConformancePlan>)>,
}

/// A shape that does not conform: its table, the increment it was read
/// against, the fingerprint of its columns and its I-row version.
pub(crate) type DeviationKey = (TableId, u16, Fingerprint, u16);

/// What resolving a shape that does not conform gives, every time.
#[derive(Debug)]
pub(crate) struct Deviation {
    /// The plan it is read with.
    pub(crate) plan: Arc<ConformancePlan>,
    /// How it matched.
    pub(crate) status: crate::resolve::Status,
    /// What resolving it says.
    pub(crate) warnings: Vec<utilake_core::Diagnostic>,
}

/// A table that is not in the registry: its definition, built from an I
/// row, and the schema its files are written with.
#[derive(Debug)]
struct Derived {
    table: Table,
    lake: SchemaRef,
}

/// How many tables one run may derive. The whole archive has a few hundred
/// tables; a file that asks for more than this is not an MMS file.
const DERIVED_SLOTS: usize = 4096;

/// The month the NEM began, which a derived table's one increment applies
/// from.
const NEM_START: (i32, u8) = (1998, 12);

/// The definition of a market no file defines: fixed market time, AEMO's
/// timestamp format.
fn default_market(market: Market) -> MarketDef {
    MarketDef {
        market,
        utc_offset_secs: if market == Market::Wem {
            28_800
        } else {
            36_000
        },
        timestamp_format: crate::file_model::TimestampFormat::AemoSlash,
    }
}

static EMBEDDED: OnceLock<Arc<Registry>> = OnceLock::new();

/// The blob `build.rs` packed from `schemas/`.
const EMBEDDED_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/registry.bin"));

/// The census's shapes `build.rs` packed from `schemas/`.
const EMBEDDED_SHAPES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shapes.bin"));

/// Each table's Iceberg history `build.rs` packed from `schemas/`.
const EMBEDDED_HISTORIES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/iceberg.bin"));

/// The embedded blob, for benchmarks and tools that decode it themselves.
pub fn embedded_blob() -> &'static [u8] {
    EMBEDDED_BLOB
}

impl Registry {
    /// The registry compiled into the binary, built once per process.
    pub fn load_embedded() -> Arc<Registry> {
        EMBEDDED
            .get_or_init(|| {
                let set = SchemaSet::decode(EMBEDDED_BLOB)
                    .expect("build.rs validated and packed the embedded registry");
                let mut registry = Self::from_set(set);
                registry.census_blob = EMBEDDED_SHAPES;
                registry.history_blob = EMBEDDED_HISTORIES;
                Arc::new(registry)
            })
            .clone()
    }

    /// A registry from a `schemas/` directory, validated first.
    pub fn load_dir(dir: &Path) -> Result<Registry, SchemaFileError> {
        let set = SchemaSet::read_dir(dir)?;
        let issues = set.validate();
        if let Some(first) = issues.first() {
            return Err(SchemaFileError::Invalid {
                path: dir.display().to_string(),
                table: first.table.clone(),
                message: format!("{} issue(s); first: {}", issues.len(), first.message),
            });
        }
        Ok(Self::from_set(set))
    }

    /// Lays `overlay` (a user's own tables, laid out like `schemas/`) over
    /// this registry: a table of the same name replaces the embedded one,
    /// other tables are added. The result is validated as a set.
    pub fn with_overlay(&self, overlay: &Path) -> Result<Registry, SchemaFileError> {
        let over = SchemaSet::read_dir(overlay)?;
        let mut tables: Vec<Table> = self
            .tables
            .iter()
            .filter(|t| !over.tables.iter().any(|o| o.name == t.name))
            .cloned()
            .collect();
        let theirs: Vec<String> = over.tables.iter().map(|t| t.name.clone()).collect();
        tables.extend(over.tables);
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        let mut markets = self.markets.clone();
        for m in over.markets {
            markets.retain(|x| x.market != m.market);
            markets.push(m);
        }
        let mut shapes = self.census().clone();
        for (table, theirs) in over.shapes {
            shapes.insert(table, theirs);
        }
        let mut histories = self.histories().clone();
        histories.extend(over.histories);
        let set = SchemaSet {
            markets,
            tables,
            shapes,
            histories,
            file_issues: over.file_issues,
        };
        let issues = set.validate();
        if let Some(first) = issues.first() {
            return Err(SchemaFileError::Invalid {
                path: overlay.display().to_string(),
                table: first.table.clone(),
                message: format!(
                    "{} issue(s) after overlay; first: {}",
                    issues.len(),
                    first.message
                ),
            });
        }
        let mut registry = Self::from_set(set);
        registry.from_user = registry
            .tables
            .iter()
            .map(|t| theirs.contains(&t.name))
            .collect();
        Ok(registry)
    }

    /// Builds the indexes. The set must already be valid.
    pub fn from_set(set: SchemaSet) -> Self {
        let mut markets = set.markets;
        let tables = set.tables;
        let mut by_name = HashMap::with_capacity(tables.len());
        let mut by_pair = hashbrown::HashMap::with_capacity(tables.len());
        for (i, t) in tables.iter().enumerate() {
            let id = TableId(i as u32);
            by_name.insert(t.name.clone(), id);
            for (a, b) in &t.report_pairs {
                by_pair.insert(PairKey(a.clone(), b.clone()), id);
            }
        }
        // A market no file defines gets the definition its name implies.
        for m in [Market::Nem, Market::Wem] {
            if !markets.iter().any(|d| d.market == m) {
                markets.push(default_market(m));
            }
        }
        Self {
            markets,
            from_user: vec![false; tables.len()],
            shapes: std::iter::repeat_with(OnceLock::new)
                .take(tables.len())
                .collect(),
            tables,
            by_name,
            by_pair,
            deviating: Mutex::new(HashMap::new()),
            derived: std::iter::repeat_with(OnceLock::new)
                .take(DERIVED_SLOTS)
                .collect(),
            derived_pairs: Mutex::new(Vec::new()),
            census: if set.shapes.is_empty() {
                OnceLock::new()
            } else {
                OnceLock::from(set.shapes)
            },
            census_blob: &[],
            census_index: OnceLock::new(),
            histories: if set.histories.is_empty() {
                OnceLock::new()
            } else {
                OnceLock::from(set.histories)
            },
            history_blob: &[],
        }
    }

    /// Each table's history as an Iceberg table, by table name.
    pub fn histories(&self) -> &BTreeMap<String, History> {
        self.histories.get_or_init(|| {
            if self.history_blob.is_empty() {
                BTreeMap::new()
            } else {
                SchemaSet::decode_histories(self.history_blob)
                    .expect("build.rs validated and packed the histories")
            }
        })
    }

    /// The Iceberg schema a block of table `id` is written under: the one
    /// of the table's history whose columns are the block's, in order.
    pub fn schema_id(&self, id: TableId, irow: &IRow<'_>) -> Option<u32> {
        self.histories()
            .get(&self.table(id).name)?
            .schema_of(irow.columns)
    }

    /// The census's shapes of each table, by table name.
    pub fn census(&self) -> &BTreeMap<String, Vec<ShapeEntry>> {
        self.census.get_or_init(|| {
            if self.census_blob.is_empty() {
                BTreeMap::new()
            } else {
                SchemaSet::decode_shapes(self.census_blob)
                    .expect("build.rs validated and packed the census's shapes")
            }
        })
    }

    /// The ids the census gives a block of this report pair and column
    /// list in a source of month `period`: the census shapes with these
    /// names, seen in that month when it is known (a shape's id also covers
    /// its control-file types, which an I row does not carry). Empty when
    /// the census has not seen the block.
    pub fn shape_ids(&self, irow: &IRow<'_>, period: Option<YearMonth>) -> Vec<ShapeId> {
        let index = self.census_index.get_or_init(|| {
            let mut index: HashMap<(String, String, Fingerprint), Vec<CensusShape>> =
                HashMap::new();
            for e in self.census().values().flatten() {
                index
                    .entry((
                        e.report_type.clone(),
                        e.report_subtype.clone(),
                        e.fingerprint(),
                    ))
                    .or_default()
                    .push(CensusShape {
                        id: e.id,
                        first_seen: e.first_seen,
                        last_seen: e.last_seen,
                    });
            }
            index
        });
        let key = (
            irow.report_type.to_owned(),
            irow.report_subtype.to_owned(),
            irow.fingerprint(),
        );
        let mut ids: Vec<ShapeId> = index
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|s| period.is_none_or(|p| s.first_seen <= p && p <= s.last_seen))
            .map(|s| s.id)
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// The lake schema and increment plans of a table of the registry,
    /// built on first use.
    pub(crate) fn shapes(&self, index: usize) -> &TableShapes {
        self.shapes[index].get_or_init(|| {
            let t = &self.tables[index];
            let market = self
                .market(t.market)
                .expect("every table's market has a definition");
            let lake = lake_schema(t, market);
            let dir = t.dir_name();
            let increments = t
                .increments
                .iter()
                .map(|i| {
                    let names: Vec<&str> = i.columns.iter().map(String::as_str).collect();
                    (
                        Fingerprint::of_names(names.iter().copied()),
                        Arc::new(ConformancePlan::new(t, market, &lake, i.n, &names, &dir)),
                    )
                })
                .collect();
            TableShapes { lake, increments }
        })
    }

    /// The table behind a handle: one of the registry's, or one derived
    /// from a file during this run.
    pub fn table(&self, id: TableId) -> &Table {
        let i = id.0 as usize;
        match self.tables.get(i) {
            Some(t) => t,
            None => &self.derived_slot(i).table,
        }
    }

    fn derived_slot(&self, index: usize) -> &Derived {
        self.derived[index - self.tables.len()]
            .get()
            .expect("a handle beyond the registry's tables names a derived table")
    }

    /// Whether the table was derived from a file rather than declared in
    /// the registry.
    pub fn is_derived(&self, id: TableId) -> bool {
        id.0 as usize >= self.tables.len()
    }

    /// Where the definition of a table came from.
    pub fn schema_source(&self, id: TableId) -> SchemaSource {
        match self.from_user.get(id.0 as usize) {
            Some(true) => SchemaSource::User,
            Some(false) => SchemaSource::Registry,
            None => SchemaSource::File,
        }
    }

    /// The directory, under a market's lake, that a table's files go to. A
    /// derived table is kept apart under `_unregistered/` and named by its
    /// report pair, so its text columns never mix with the typed files of a
    /// table the registry declares, now or later.
    pub fn table_dir(&self, id: TableId) -> String {
        let t = self.table(id);
        if self.is_derived(id) {
            let (rt, st) = &t.report_pairs[0];
            let pair = if st.is_empty() {
                rt.to_ascii_lowercase()
            } else {
                format!("{}.{}", rt.to_ascii_lowercase(), st.to_ascii_lowercase())
            };
            format!("_unregistered/{pair}")
        } else {
            t.dir_name()
        }
    }

    /// The table for a report pair no table claims, derived from the I row
    /// that first showed it: every column of the row, as nullable text,
    /// which is exactly what the file says and nothing it does not. The
    /// same pair gives the same table for the rest of the run. `None` when
    /// more tables were derived than a run has room for.
    pub fn derive(&self, market: Market, irow: &IRow<'_>) -> Option<TableId> {
        let mut pairs = self
            .derived_pairs
            .lock()
            .expect("the derived-table list is never poisoned");
        if let Some(i) = pairs
            .iter()
            .position(|(rt, st)| rt == irow.report_type && st == irow.report_subtype)
        {
            return Some(TableId((self.tables.len() + i) as u32));
        }
        let slot = pairs.len();
        if slot >= self.derived.len() {
            return None;
        }
        let table = derived_table(market, irow);
        let lake = lake_schema(
            &table,
            self.market(market).expect("every market has a definition"),
        );
        let _ = self.derived[slot].set(Box::new(Derived { table, lake }));
        pairs.push((irow.report_type.to_owned(), irow.report_subtype.to_owned()));
        Some(TableId((self.tables.len() + slot) as u32))
    }

    /// The handle for a table name.
    pub fn table_by_name(&self, name: &str) -> Option<TableId> {
        self.by_name
            .get(name)
            .copied()
            .or_else(|| self.by_name.get(&name.to_ascii_uppercase()).copied())
    }

    /// The handle for a report pair, without allocating: a table of the
    /// registry, or failing that one derived earlier in this run.
    pub fn table_by_pair(&self, report_type: &str, report_subtype: &str) -> Option<TableId> {
        if let Some(id) = self.by_pair.get(&PairRef(report_type, report_subtype)) {
            return Some(*id);
        }
        self.derived_pairs
            .lock()
            .expect("the derived-table list is never poisoned")
            .iter()
            .position(|(rt, st)| rt == report_type && st == report_subtype)
            .map(|i| TableId((self.tables.len() + i) as u32))
    }

    /// The handle for a report pair the registry itself declares.
    pub fn declared_by_pair(&self, report_type: &str, report_subtype: &str) -> Option<TableId> {
        self.by_pair
            .get(&PairRef(report_type, report_subtype))
            .copied()
    }

    /// Every table with its handle.
    pub fn tables(&self) -> impl Iterator<Item = (TableId, &Table)> {
        self.tables
            .iter()
            .enumerate()
            .map(|(i, t)| (TableId(i as u32), t))
    }

    /// The market definition for a market.
    pub fn market(&self, m: Market) -> Option<&MarketDef> {
        self.markets.iter().find(|d| d.market == m)
    }

    /// The market definition of a table.
    pub fn market_of(&self, id: TableId) -> &MarketDef {
        let m = self.table(id).market;
        self.market(m)
            .expect("every table's market has a definition")
    }

    /// The lake schema of a table.
    pub fn lake_schema(&self, id: TableId) -> &SchemaRef {
        let i = id.0 as usize;
        if i < self.tables.len() {
            &self.shapes(i).lake
        } else {
            &self.derived_slot(i).lake
        }
    }

    /// A table's increments, by ascending month.
    pub fn increments(&self, table: &str) -> Option<&[Increment]> {
        let id = self.table_by_name(table)?;
        Some(&self.table(id).increments)
    }

    /// What a shape that does not conform resolves to, cached.
    pub(crate) fn deviation(
        &self,
        key: DeviationKey,
        build: impl FnOnce() -> Deviation,
    ) -> Arc<Deviation> {
        let mut cache = self
            .deviating
            .lock()
            .expect("the plan cache is never poisoned");
        cache
            .entry(key)
            .or_insert_with(|| Arc::new(build()))
            .clone()
    }

    /// Changes between this registry and `newer` that break the lake.
    pub fn check_compat(&self, newer: &Registry) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for t in &self.tables {
            match newer.table_by_name(&t.name) {
                None => out.push((
                    t.name.clone(),
                    "table was deleted; its files stay in the lake".into(),
                )),
                Some(id) => {
                    for m in crate::file_model::compat::breaking_changes(t, newer.table(id)) {
                        out.push((t.name.clone(), m));
                    }
                }
            }
        }
        out
    }
}

/// The table a file's I row says, when no table claims its report pair:
/// every column nullable text, one increment of exactly these columns.
fn derived_table(market: Market, irow: &IRow<'_>) -> Table {
    // Some tables publish no subtype: the pair (GENCONDATA, "") is GENCONDATA.
    let name = if irow.report_subtype.is_empty() {
        irow.report_type.to_owned()
    } else {
        format!("{}_{}", irow.report_type, irow.report_subtype)
    };
    let columns: Vec<String> = irow.columns.iter().map(|c| (*c).to_owned()).collect();
    let mut t = Table {
        market,
        name,
        description: Some(
            "Derived from the file: no table in the registry claims this report pair.".into(),
        ),
        report_pairs: vec![(irow.report_type.to_owned(), irow.report_subtype.to_owned())],
        load_mode: LoadMode::Append,
        temporal: crate::file_model::Temporal {
            origin: crate::file_model::RoleOrigin::Derived,
            entity: Vec::new(),
            valid: crate::file_model::ValidAxis::Timeless,
            order: Vec::new(),
            cutoff: None,
        },
        primary_key: vec![],
        time_column: None,
        next_column_id: columns.len() as u32 + 1,
        columns: columns
            .iter()
            .enumerate()
            .map(|(i, c)| Column {
                id: ColumnId(i as u32 + 1),
                name: c.clone(),
                ty: LakeType::String,
                nullable: true,
                note: None,
            })
            .collect(),
        increments: vec![Increment {
            n: 1,
            from: YearMonth::new(NEM_START.0, NEM_START.1).expect("a month"),
            ctl: None,
            ctl_sha256: None,
            aemo_ctl: None,
            aemo_versions: vec![irow.version],
            columns,
            note: None,
        }],
    };
    t.temporal = crate::file_model::temporal::derive(&t.primary_key, &t.columns);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded census is decoded when a shape is first asked for, and
    /// knows the anchor month's shapes.
    #[test]
    fn the_embedded_census_names_the_anchor_months_shapes() {
        let r = Registry::load_embedded();
        let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"];
        let irow = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        let anchor = "2026-08".parse().ok();
        let ids = r.shape_ids(&irow, anchor);
        assert_eq!(ids.len(), 1);
        let recorded = &r.census()["DISPATCH_UNIT_SCADA"];
        assert!(recorded.iter().any(|e| e.id == ids[0]));
        assert!(r.shape_ids(&irow, "2009-07".parse().ok()).is_empty());
        assert!(r.census().len() > 200);
    }

    #[test]
    fn embedded_registry_loads_and_indexes() {
        let r = Registry::load_embedded();
        assert!(Arc::ptr_eq(&r, &Registry::load_embedded()));
        let id = r.table_by_name("dispatch_unit_scada").unwrap();
        assert_eq!(r.table_by_pair("DISPATCH", "UNIT_SCADA"), Some(id));
        assert_eq!(r.declared_by_pair("DISPATCH", "UNIT_SCADA"), Some(id));
        assert_eq!(r.table_by_pair("DISPATCH", "NOPE"), None);
        assert_eq!(r.table(id).name, "DISPATCH_UNIT_SCADA");
        assert_eq!(r.market_of(id).arrow_tz(), "+10:00");
        assert_eq!(r.lake_schema(id).fields().len(), 4);
        assert_eq!(r.schema_source(id), SchemaSource::Registry);
        assert_eq!(SchemaSource::User.to_string(), "user");
        let incs = r.increments("DISPATCH_UNIT_SCADA").unwrap();
        assert_eq!(incs.len(), 2);
        assert!(incs[0].from < incs[1].from);
        assert!(r.increments("NOPE").is_none());
        assert!(r.tables().count() >= 200);
        assert!(r.market(Market::Nem).is_some());
        assert!(r.market(Market::Wem).is_some());
        assert!(r.check_compat(&r).is_empty());
        assert!(!embedded_blob().is_empty());
    }

    /// A user's own tables, laid out like `schemas/`.
    fn write(dir: &Path, t: &Table) {
        let d = dir.join("nem").join(&t.name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join(crate::file_model::file::TABLE_FILE),
            crate::file_model::file::write_table(t),
        )
        .unwrap();
    }

    #[test]
    fn load_dir_and_overlay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        let r = Registry::load_dir(&root).unwrap();
        assert!(r.table_by_name("REGION").is_some());
        let dir = tempfile::tempdir().unwrap();
        // Overlay replaces REGION with a one-column table and adds a new one.
        let mut t = r.table(r.table_by_name("REGION").unwrap()).clone();
        t.columns.truncate(1);
        t.primary_key = vec![t.columns[0].name.clone()];
        t.columns[0].nullable = false;
        t.time_column = None;
        for i in &mut t.increments {
            i.columns = vec![t.columns[0].name.clone()];
            i.ctl = None;
            i.ctl_sha256 = None;
        }
        t.temporal = crate::file_model::temporal::derive(&t.primary_key, &t.columns);
        write(dir.path(), &t);
        let mut n = t.clone();
        n.name = "NEWTABLE".into();
        n.report_pairs = vec![("X".into(), "Y".into())];
        write(dir.path(), &n);
        let o = r.with_overlay(dir.path()).unwrap();
        let region = o.table_by_name("REGION").unwrap();
        assert_eq!(o.table(region).columns.len(), 1);
        assert_eq!(o.schema_source(region), SchemaSource::User);
        assert!(o.table_by_pair("X", "Y").is_some());
        assert_eq!(o.tables().count(), r.tables().count() + 1);
        // Removing a column is a breaking change; so is removing a table.
        let breaks = r.check_compat(&o);
        assert!(
            breaks
                .iter()
                .any(|(t, m)| t == "REGION" && m.contains("removed")),
            "{breaks:?}"
        );
        assert!(o.check_compat(&r).iter().any(|(t, _)| t == "NEWTABLE"));
        // An invalid overlay is refused.
        let mut bad = n.clone();
        bad.next_column_id = 0;
        write(dir.path(), &bad);
        assert!(r.with_overlay(dir.path()).is_err());
        assert!(Registry::load_dir(dir.path()).is_err());
        assert!(Registry::load_dir(&dir.path().join("none")).is_err());
        assert!(r.with_overlay(&dir.path().join("none")).is_err());
    }

    #[test]
    fn derived_tables_fill_their_slots() {
        let r = Registry::from_set(SchemaSet::default());
        let cols = ["A"];
        let irow = |rt: &'static str| IRow {
            report_type: rt,
            report_subtype: "S",
            version: 2,
            columns: &cols,
        };
        let a = r.derive(Market::Nem, &irow("A")).unwrap();
        assert_eq!(r.derive(Market::Nem, &irow("A")), Some(a));
        let t = r.table(a);
        assert_eq!(t.increments[0].aemo_versions, [2]);
        assert_eq!(t.columns[0].ty, LakeType::String);
        assert_eq!(r.schema_source(a), SchemaSource::File);
        assert_eq!(r.lake_schema(a).fields().len(), 1);
        assert_eq!(r.market_of(a).arrow_tz(), "+10:00");
    }
}
