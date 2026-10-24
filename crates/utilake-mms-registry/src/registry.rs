//! The loaded registry: tables, indexes, histories and conformance plans.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::file_model::blob::SchemaSet;
use crate::file_model::{
    Column, ColumnId, Folder, LakeType, LoadMode, Market, MarketDef, SchemaFileError, Table,
    TableSchema,
};
use arrow_schema::SchemaRef;
use hashbrown::Equivalent;
use std::collections::HashMap;
use utilake_core::{Fingerprint, YearMonth};
use utilake_mms_decode::IRow;

use crate::arrow::lake_schema;
use crate::plan::ConformancePlan;

/// A dense handle to a table within one registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableId(pub u32);

/// An owned `(market, report_type, report_subtype)` key that can be looked
/// up with borrowed strings and no allocation. A pair names a table within
/// its market: two markets may publish the same pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PairKey(pub Market, pub String, pub String);

/// The borrowed form of [`PairKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PairRef<'a>(pub Market, pub &'a str, pub &'a str);

impl Equivalent<PairKey> for PairRef<'_> {
    fn equivalent(&self, key: &PairKey) -> bool {
        key.0 == self.0 && key.1 == self.1 && key.2 == self.2
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
    /// Which report folder of nemweb carries each block, as pulled.
    folders: Vec<Folder>,
    /// Whether each table came from a user's overlay.
    from_user: Vec<bool>,
    by_name: HashMap<String, TableId>,
    /// Each report pair's table, and the pair's place in its list.
    by_pair: hashbrown::HashMap<PairKey, (TableId, u16)>,
    /// The lake schema, the history and the plans of each table's schemas,
    /// built the first time the table is used. A run reads a handful of
    /// tables; building them for every table the registry knows would cost
    /// each run the memory and the time of all of them.
    shapes: Vec<OnceLock<TableShapes>>,
    /// What each block shape resolved to, worked out when first seen: the
    /// table, the fingerprint of its columns, its I-row version and the
    /// month of its source decide it.
    resolutions: RwLock<hashbrown::HashMap<ResolutionKey, Arc<Resolution>>>,
    /// Tables derived at run time from the header of a file whose report
    /// pair no table claims. Slots are written once and never moved, so a
    /// reference to one lives as long as the registry. Each is boxed: an
    /// empty slot costs a pointer, not a table.
    derived: Vec<OnceLock<Box<Derived>>>,
    /// The market and report pair of each derived table, in slot order.
    derived_pairs: Mutex<Vec<(Market, String, String)>>,
}

/// What the files of one table are written with.
#[derive(Debug)]
pub(crate) struct TableShapes {
    /// The lake schema.
    pub(crate) lake: SchemaRef,
    /// The table's history as an Iceberg table, which resolution reads.
    pub(crate) history: utilake_iceberg::History,
    /// Per schema of the history: its id and the plan of a block of its
    /// columns.
    pub(crate) plans: Vec<(u16, Arc<ConformancePlan>)>,
    /// What `History::resolve` names, worked out ahead, for each column
    /// list (by report pair's place and fingerprint) that only one schema
    /// has: whatever the month and version, the block reads with it.
    pub(crate) exact: ExactAnswers,
    /// The AEMO versions of each report pair's runs, by the pair's place.
    pub(crate) versions: Vec<(u16, u16)>,
}

impl TableShapes {
    /// The plan of a block of schema `id`'s columns.
    pub(crate) fn plan(&self, id: u16) -> Option<&Arc<ConformancePlan>> {
        self.plans.iter().find(|(s, _)| *s == id).map(|(_, p)| p)
    }
}

/// A block shape: its table, the fingerprint of its columns, its I-row
/// version and the month of its source.
pub(crate) type ResolutionKey = (TableId, Fingerprint, u16, Option<YearMonth>);

/// What resolving a block shape gives, every time.
#[derive(Debug)]
pub(crate) struct Resolution {
    /// The plan it is read with.
    pub(crate) plan: Arc<ConformancePlan>,
    /// How it matched.
    pub(crate) status: crate::resolve::Status,
    /// The table's schema it was read against.
    pub(crate) version: u16,
    /// The schema of the history it resolved to; none when it deviates.
    pub(crate) schema: Option<u32>,
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

/// The definition of a market no file defines: fixed market time, AEMO's
/// timestamp format, the trading day's start.
fn default_market(market: Market) -> MarketDef {
    let wem = market == Market::Wem;
    MarketDef {
        market,
        utc_offset_secs: market.offset_secs(),
        timestamp_format: crate::file_model::TimestampFormat::AemoSlash,
        trading_day_start_secs: if wem { 28_800 } else { 14_400 },
    }
}

static EMBEDDED: OnceLock<Arc<Registry>> = OnceLock::new();

/// The blob `build.rs` packed from `schemas/`.
const EMBEDDED_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/registry.bin"));

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
                Arc::new(Self::from_set(set))
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
        let set = SchemaSet {
            markets,
            tables,
            folders: self.folders.clone(),
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
        let folders = set.folders;
        let mut by_name = HashMap::with_capacity(tables.len());
        let mut by_pair = hashbrown::HashMap::with_capacity(tables.len());
        for (i, t) in tables.iter().enumerate() {
            let id = TableId(i as u32);
            by_name.insert(t.name.clone(), id);
            for (k, (a, b)) in t.report_pairs.iter().enumerate() {
                by_pair.insert(PairKey(t.market, a.clone(), b.clone()), (id, k as u16));
            }
        }
        // A market no file defines gets the definition its name implies.
        for m in Market::ALL {
            if !markets.iter().any(|d| d.market == m) {
                markets.push(default_market(m));
            }
        }
        Self {
            markets,
            folders,
            from_user: vec![false; tables.len()],
            shapes: std::iter::repeat_with(OnceLock::new)
                .take(tables.len())
                .collect(),
            tables,
            by_name,
            by_pair,
            resolutions: RwLock::new(hashbrown::HashMap::new()),
            derived: std::iter::repeat_with(OnceLock::new)
                .take(DERIVED_SLOTS)
                .collect(),
            derived_pairs: Mutex::new(Vec::new()),
        }
    }

    /// Every report folder the pulled census recorded.
    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// The report folders that carried blocks of this table, by its report
    /// pairs; empty for a table only the monthly archive publishes, or
    /// when no folders were pulled.
    pub fn folders_of(&self, id: TableId) -> Vec<&Folder> {
        let t = self.table(id);
        self.folders
            .iter()
            .filter(|f| t.report_pairs.iter().any(|(a, b)| f.carries(a, b)))
            .collect()
    }

    /// The history of a table of the registry as an Iceberg table; none
    /// for a table derived from a file.
    pub fn history(&self, id: TableId) -> Option<&utilake_iceberg::History> {
        let i = id.0 as usize;
        (i < self.tables.len()).then(|| &self.shapes(i).history)
    }

    /// The lake schema, history and schema plans of a table of the
    /// registry, built on first use.
    pub(crate) fn shapes(&self, index: usize) -> &TableShapes {
        self.shapes[index].get_or_init(|| {
            let t = &self.tables[index];
            let market = self
                .market(t.market)
                .expect("every table's market has a definition");
            let lake = lake_schema(t, market);
            let dir = t.dir_name();
            let plans = t
                .schemas
                .iter()
                .map(|s| {
                    let names = t.schema_columns(s);
                    (
                        s.id,
                        Arc::new(ConformancePlan::new(t, market, &lake, s.id, &names, &dir)),
                    )
                })
                .collect();
            let history = t.history();
            let (exact, versions) = exact_answers(t, &history);
            TableShapes {
                lake,
                history,
                plans,
                exact,
                versions,
            }
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
        if let Some(i) = pairs.iter().position(|(m, rt, st)| {
            *m == market && rt == irow.report_type && st == irow.report_subtype
        }) {
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
        pairs.push((
            market,
            irow.report_type.to_owned(),
            irow.report_subtype.to_owned(),
        ));
        Some(TableId((self.tables.len() + slot) as u32))
    }

    /// The handle for a table name.
    pub fn table_by_name(&self, name: &str) -> Option<TableId> {
        self.by_name
            .get(name)
            .copied()
            .or_else(|| self.by_name.get(&name.to_ascii_uppercase()).copied())
    }

    /// The handle for a report pair of `market`, without allocating: a
    /// table of the registry, or failing that one derived earlier in this
    /// run.
    pub fn table_by_pair(
        &self,
        market: Market,
        report_type: &str,
        report_subtype: &str,
    ) -> Option<TableId> {
        if let Some((id, _)) = self
            .by_pair
            .get(&PairRef(market, report_type, report_subtype))
        {
            return Some(*id);
        }
        self.derived_pairs
            .lock()
            .expect("the derived-table list is never poisoned")
            .iter()
            .position(|(m, rt, st)| *m == market && rt == report_type && st == report_subtype)
            .map(|i| TableId((self.tables.len() + i) as u32))
    }

    /// The handle for a report pair of `market` the registry itself
    /// declares.
    pub fn declared_by_pair(
        &self,
        market: Market,
        report_type: &str,
        report_subtype: &str,
    ) -> Option<TableId> {
        self.pair(market, report_type, report_subtype)
            .map(|(id, _)| id)
    }

    /// The table the registry declares for a report pair of `market`, and
    /// the pair's place in the table's list.
    pub(crate) fn pair(
        &self,
        market: Market,
        report_type: &str,
        report_subtype: &str,
    ) -> Option<(TableId, u16)> {
        self.by_pair
            .get(&PairRef(market, report_type, report_subtype))
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

    /// What a block shape resolves to, worked out once.
    pub(crate) fn resolution(
        &self,
        key: ResolutionKey,
        build: impl FnOnce() -> Resolution,
    ) -> Arc<Resolution> {
        if let Some(r) = self
            .resolutions
            .read()
            .expect("the resolution cache is never poisoned")
            .get(&key)
        {
            return r.clone();
        }
        let built = Arc::new(build());
        self.resolutions
            .write()
            .expect("the resolution cache is never poisoned")
            .entry(key)
            .or_insert(built)
            .clone()
    }
}

/// The schema of each column list one schema alone has, by the report
/// pair's place and the list's fingerprint.
pub(crate) type ExactAnswers = hashbrown::HashMap<(u16, Fingerprint), u16>;

/// The schema `History::resolve` names for each column list of a report
/// pair that one schema alone has, which it names whatever the month and
/// version; and the versions each pair's runs came under.
fn exact_answers(t: &Table, history: &utilake_iceberg::History) -> (ExactAnswers, Vec<(u16, u16)>) {
    let mut seen: hashbrown::HashMap<(u16, Fingerprint), Vec<u16>> = hashbrown::HashMap::new();
    let mut versions = Vec::new();
    for r in &t.runs {
        let Some(k) = t.report_pairs.iter().position(|p| *p == r.report) else {
            continue;
        };
        let k = k as u16;
        if !versions.contains(&(k, r.version)) {
            versions.push((k, r.version));
        }
        let Some(s) = t.schema(r.schema) else {
            continue;
        };
        let fp = Fingerprint::of_names(t.schema_columns(s).iter().copied());
        let ids = seen.entry((k, fp)).or_default();
        if !ids.contains(&s.id) {
            ids.push(s.id);
        }
    }
    let exact = seen
        .into_iter()
        .filter_map(|((k, fp), ids)| {
            let [only] = ids.as_slice() else {
                return None;
            };
            let (a, b) = &t.report_pairs[usize::from(k)];
            let cols = t.schema_columns(t.schema(*only)?);
            let named =
                history.resolve([a.as_str(), b.as_str()], 0, &cols, crate::resolve::LATEST)?;
            debug_assert_eq!(named, u32::from(*only), "one schema has these columns");
            Some(((k, fp), u16::try_from(named).ok()?))
        })
        .collect();
    (exact, versions)
}

/// The table a file's I row says, when no table claims its report pair:
/// every column nullable text, one schema of exactly these columns.
fn derived_table(market: Market, irow: &IRow<'_>) -> Table {
    // Some tables publish no subtype: the pair (GENCONDATA, "") is GENCONDATA.
    let name = if irow.report_subtype.is_empty() {
        irow.report_type.to_owned()
    } else {
        format!("{}_{}", irow.report_type, irow.report_subtype)
    };
    let columns: Vec<&str> = irow.columns.to_vec();
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
        current_schema: 0,
        columns: columns
            .iter()
            .enumerate()
            .map(|(i, c)| Column {
                id: ColumnId(i as u32 + 1),
                name: (*c).to_owned(),
                ty: LakeType::String,
                nullable: true,
                note: None,
            })
            .collect(),
        schemas: vec![TableSchema {
            id: 0,
            fields: (1..=columns.len() as u32)
                .map(|i| (ColumnId(i), false))
                .collect(),
            key: Vec::new(),
        }],
        runs: Vec::new(),
    };
    t.temporal = crate::file_model::temporal::derive(&t.primary_key, &t.columns);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_registry_loads_and_indexes() {
        let r = Registry::load_embedded();
        assert!(Arc::ptr_eq(&r, &Registry::load_embedded()));
        let id = r.table_by_name("dispatch_unit_scada").unwrap();
        assert_eq!(
            r.table_by_pair(Market::Nem, "DISPATCH", "UNIT_SCADA"),
            Some(id)
        );
        assert_eq!(
            r.declared_by_pair(Market::Nem, "DISPATCH", "UNIT_SCADA"),
            Some(id)
        );
        assert_eq!(r.table_by_pair(Market::Nem, "DISPATCH", "NOPE"), None);
        // A pair names a table within its market only.
        assert_eq!(r.table_by_pair(Market::Gas, "DISPATCH", "UNIT_SCADA"), None);
        assert_eq!(r.market(Market::Gas).unwrap().utc_offset_secs, 36_000);
        assert_eq!(r.table(id).name, "DISPATCH_UNIT_SCADA");
        assert_eq!(r.market_of(id).arrow_tz(), "+10:00");
        assert_eq!(r.lake_schema(id).fields().len(), 4);
        assert_eq!(r.schema_source(id), SchemaSource::Registry);
        assert_eq!(SchemaSource::User.to_string(), "user");
        let h = r.history(id).unwrap();
        assert!(h.schemas.len() >= 2 && !h.runs.is_empty());
        assert_eq!(h.table, "DISPATCH_UNIT_SCADA");
        // Every table the data model declares, and nothing it does not.
        assert_eq!(r.tables().count(), 241);
        assert!(r.table_by_name("SPDRC_").is_none());
        assert!(r.market(Market::Nem).is_some());
        assert!(r.market(Market::Wem).is_some());
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
        t.schemas.truncate(1);
        t.schemas[0].fields = vec![(t.columns[0].id, true)];
        t.schemas[0].key = vec![t.columns[0].id];
        t.runs.retain(|r| r.schema == t.schemas[0].id);
        t.temporal = crate::file_model::temporal::derive(&t.primary_key, &t.columns);
        write(dir.path(), &t);
        let mut n = t.clone();
        n.name = "NEWTABLE".into();
        n.report_pairs = vec![("X".into(), "Y".into())];
        for r in &mut n.runs {
            r.report = ("X".into(), "Y".into());
        }
        write(dir.path(), &n);
        // The overlay's own market definition replaces the embedded one.
        std::fs::write(
            dir.path().join("nem").join("market.toml"),
            "schema_format = 1\nmarket = \"nem\"\nutc_offset_secs = 36000\ntimestamp_format = \"aemo_slash\"\n",
        )
        .unwrap();
        let o = r.with_overlay(dir.path()).unwrap();
        assert_eq!(
            o.markets.iter().filter(|m| m.market == Market::Nem).count(),
            1
        );
        let region = o.table_by_name("REGION").unwrap();
        assert_eq!(o.table(region).columns.len(), 1);
        assert_eq!(o.schema_source(region), SchemaSource::User);
        assert!(o.table_by_pair(Market::Nem, "X", "Y").is_some());
        assert_eq!(o.tables().count(), r.tables().count() + 1);
        // An invalid overlay is refused.
        let mut bad = n.clone();
        bad.columns[0].id = ColumnId(0);
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
        assert_eq!(t.schemas[0].fields, [(ColumnId(1), false)]);
        assert_eq!(t.columns[0].ty, LakeType::String);
        assert_eq!(r.schema_source(a), SchemaSource::File);
        assert_eq!(r.lake_schema(a).fields().len(), 1);
        assert_eq!(r.market_of(a).arrow_tz(), "+10:00");
        assert!(r.history(a).is_none());
    }
}
