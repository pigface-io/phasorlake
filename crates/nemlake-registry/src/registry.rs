//! The loaded registry: tables, indexes and precomputed conformance plans.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::file_model::blob::SchemaSet;
use crate::file_model::compat::{Change, diff_versions};
use crate::file_model::shapes::ShapeEntry;
use crate::file_model::{Market, MarketDef, SchemaFileError, SchemaVersion, TableDef};
use arrow_schema::SchemaRef;
use hashbrown::Equivalent;
use nemlake_core::{Fingerprint, IRow, ShapeId, YearMonth};

use crate::arrow::lake_schema;
use crate::plan::ConformancePlan;

/// A dense handle to a table within one registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableId(pub u32);

/// How logical types become Arrow types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TypeOptions {
    /// Emit `Decimal128(p, s)` for `NUMBER(p, s)` instead of `Float64`.
    pub decimal: bool,
}

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

/// One version in a table's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageEntry {
    /// The version.
    pub version: SchemaVersion,
    /// Earliest archive month seen.
    pub first_seen: Option<String>,
    /// Latest archive month seen.
    pub last_seen: Option<String>,
    /// Changes since the previous version.
    pub changes: Vec<Change>,
    /// How many other column lists were published under this version.
    pub variants: usize,
}

/// Where the definition of a table came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaSource {
    /// The registry nemlake was built with, or a schemas directory loaded
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

/// Every table nemlake knows, indexed for the hot path.
#[derive(Debug)]
pub struct Registry {
    opts: TypeOptions,
    markets: Vec<MarketDef>,
    tables: Vec<TableDef>,
    /// Whether each table came from a user's overlay.
    from_user: Vec<bool>,
    by_name: HashMap<String, TableId>,
    by_pair: hashbrown::HashMap<PairKey, TableId>,
    by_fingerprint: HashMap<Fingerprint, (TableId, SchemaVersion)>,
    /// The lake schema and the plans of each table, built the first
    /// time the table is used. A run reads a handful of tables; building
    /// them for every table the registry knows would cost each run the
    /// memory and the time of all of them.
    shapes: Vec<OnceLock<TableShapes>>,
    /// ConformancePlans built at run time for drifted or aliased shapes.
    dynamic: Mutex<HashMap<(TableId, Fingerprint), Arc<ConformancePlan>>>,
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
struct TableShapes {
    /// The unified lake schema.
    lake: SchemaRef,
    /// One entry per declared version, in the table's order.
    versions: Vec<VersionShapes>,
}

/// The plans of one declared version: of its own column list and of
/// each other list published under its number.
#[derive(Debug)]
struct VersionShapes {
    n: SchemaVersion,
    own: Arc<ConformancePlan>,
    variants: Vec<(Fingerprint, Arc<ConformancePlan>)>,
}

/// A table that is not in the registry: its definition, built from an I
/// row, and the schema its files are written with.
#[derive(Debug)]
struct Derived {
    table: TableDef,
    lake: SchemaRef,
}

/// How many tables one run may derive. The whole archive has a few hundred
/// tables; a file that asks for more than this is not an MMS file.
const DERIVED_SLOTS: usize = 4096;

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

static EMBEDDED_F64: OnceLock<Arc<Registry>> = OnceLock::new();
static EMBEDDED_DECIMAL: OnceLock<Arc<Registry>> = OnceLock::new();

/// The blob `build.rs` packed from `schemas/`.
const EMBEDDED_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/registry.bin"));

/// The census's shapes `build.rs` packed from `schemas/`.
const EMBEDDED_SHAPES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shapes.bin"));

/// The embedded blob, for benchmarks and tools that decode it themselves.
pub fn embedded_blob() -> &'static [u8] {
    EMBEDDED_BLOB
}

impl Registry {
    /// The registry compiled into the binary, built once per type mode.
    pub fn load_embedded(opts: TypeOptions) -> Arc<Registry> {
        let cell = if opts.decimal {
            &EMBEDDED_DECIMAL
        } else {
            &EMBEDDED_F64
        };
        cell.get_or_init(|| {
            let set = SchemaSet::decode(EMBEDDED_BLOB)
                .expect("build.rs validated and packed the embedded registry");
            let mut registry = Self::from_set(set, opts);
            registry.census_blob = EMBEDDED_SHAPES;
            Arc::new(registry)
        })
        .clone()
    }

    /// A registry from a `schemas/` directory, validated first.
    pub fn load_dir(dir: &Path, opts: TypeOptions) -> Result<Registry, SchemaFileError> {
        let set = SchemaSet::read_dir(dir)?;
        let issues = set.validate();
        if let Some(first) = issues.first() {
            return Err(SchemaFileError::Invalid {
                path: dir.display().to_string(),
                table: first.table.clone(),
                message: format!("{} issue(s); first: {}", issues.len(), first.message),
            });
        }
        Ok(Self::from_set(set, opts))
    }

    /// Lays `overlay` (a user's own tables) over this registry: a table of
    /// the same name replaces the embedded one, other tables are added.
    /// Files named `DRAFT_*` are not read. The result is validated as a set
    /// for structure; unlike the committed registry it may hold columns
    /// whose type was only inferred.
    pub fn with_overlay(&self, overlay: &Path) -> Result<Registry, SchemaFileError> {
        let over = SchemaSet::read_dir(overlay)?;
        let mut tables: Vec<TableDef> = self
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
        let set = SchemaSet {
            markets,
            tables,
            shapes,
        };
        // A user's own table may keep inferred columns; it must still be
        // well formed and fit the rest of the registry.
        let issues = set.validate_structure();
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
        let mut registry = Self::from_set(set, self.opts);
        registry.from_user = registry
            .tables
            .iter()
            .map(|t| theirs.contains(&t.name))
            .collect();
        Ok(registry)
    }

    /// Builds the indexes. The set must already be valid.
    pub fn from_set(set: SchemaSet, opts: TypeOptions) -> Self {
        let mut markets = set.markets;
        let tables = set.tables;
        let mut by_name = HashMap::with_capacity(tables.len());
        let mut by_pair = hashbrown::HashMap::with_capacity(tables.len());
        let mut by_fingerprint = HashMap::new();
        for (i, t) in tables.iter().enumerate() {
            let id = TableId(i as u32);
            by_name.insert(t.name.clone(), id);
            for (a, b) in &t.report_pairs {
                by_pair.insert(PairKey(a.clone(), b.clone()), id);
            }
            // A market no file defines gets the definition its name implies.
            if !markets.iter().any(|m| m.market == t.market) {
                markets.push(default_market(t.market));
            }
            for v in &t.versions {
                by_fingerprint.entry(v.fingerprint).or_insert((id, v.n));
                for x in &v.variants {
                    // A variant's list under an undeclared version number
                    // is found by its fingerprint like a version's own.
                    by_fingerprint.entry(x.fingerprint).or_insert((id, v.n));
                }
            }
        }
        Self {
            opts,
            markets,
            from_user: vec![false; tables.len()],
            shapes: std::iter::repeat_with(OnceLock::new)
                .take(tables.len())
                .collect(),
            tables,
            by_name,
            by_pair,
            by_fingerprint,
            dynamic: Mutex::new(HashMap::new()),
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
        }
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

    /// The lake schema and plans of a table of the registry, built
    /// on first use.
    fn shapes(&self, index: usize) -> &TableShapes {
        self.shapes[index].get_or_init(|| {
            let t = &self.tables[index];
            let market = self
                .market(t.market)
                .expect("every table's market has a definition");
            let lake = lake_schema(t, market, self.opts);
            let project = |columns: &[String]| {
                let names: Vec<&str> = columns.iter().map(String::as_str).collect();
                Arc::new(ConformancePlan::declared(
                    t, market, self.opts, &lake, &names,
                ))
            };
            let versions = t
                .versions
                .iter()
                .map(|v| VersionShapes {
                    n: v.n,
                    own: project(&v.columns),
                    variants: v
                        .variants
                        .iter()
                        .map(|x| (x.fingerprint, project(&x.columns)))
                        .collect(),
                })
                .collect();
            TableShapes { lake, versions }
        })
    }

    /// The type options this registry was built with.
    pub const fn options(&self) -> TypeOptions {
        self.opts
    }

    /// The table behind a handle: one of the registry's, or one derived
    /// from a file during this run.
    pub fn table(&self, id: TableId) -> &TableDef {
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
    pub fn derive(&self, market: Market, irow: &nemlake_core::IRow<'_>) -> Option<TableId> {
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
        let mut table = crate::infer::draft_table(market, irow, &crate::infer::Sampler::new(0));
        table.description =
            Some("Derived from the file: no table in the registry claims this report pair.".into());
        table.provenance.note = Some("derived at run time; every column is text".into());
        let market_def = self
            .market(market)
            .cloned()
            .unwrap_or_else(|| default_market(market));
        let lake = lake_schema(&table, &market_def, self.opts);
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
    pub fn tables(&self) -> impl Iterator<Item = (TableId, &TableDef)> {
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

    /// The unified lake schema of a table.
    pub fn lake_schema(&self, id: TableId) -> &SchemaRef {
        let i = id.0 as usize;
        if i < self.tables.len() {
            &self.shapes(i).lake
        } else {
            &self.derived_slot(i).lake
        }
    }

    fn version_shapes(&self, id: TableId, version: SchemaVersion) -> Option<&VersionShapes> {
        let i = id.0 as usize;
        if i >= self.tables.len() {
            return None;
        }
        self.shapes(i).versions.iter().find(|v| v.n == version)
    }

    /// The plan of a declared version's own column list.
    pub fn plan(&self, id: TableId, version: SchemaVersion) -> Option<&Arc<ConformancePlan>> {
        self.version_shapes(id, version).map(|v| &v.own)
    }

    /// The plan of a declared variant of a version.
    pub fn variant_plan(
        &self,
        id: TableId,
        version: SchemaVersion,
        fp: Fingerprint,
    ) -> Option<&Arc<ConformancePlan>> {
        self.version_shapes(id, version)?
            .variants
            .iter()
            .find(|(f, _)| *f == fp)
            .map(|(_, p)| p)
    }

    /// The table and version that published a column list with this fingerprint.
    pub fn by_fingerprint(&self, fp: Fingerprint) -> Option<(TableId, SchemaVersion)> {
        self.by_fingerprint.get(&fp).copied()
    }

    /// A run-time plan for a drifted or aliased shape, cached.
    pub(crate) fn dynamic_plan(
        &self,
        id: TableId,
        fp: Fingerprint,
        build: impl FnOnce() -> Arc<ConformancePlan>,
    ) -> Arc<ConformancePlan> {
        let mut cache = self
            .dynamic
            .lock()
            .expect("the plan cache is never poisoned");
        cache.entry((id, fp)).or_insert_with(build).clone()
    }

    /// The differences between two versions of a table.
    pub fn diff(&self, table: &str, from: SchemaVersion, to: SchemaVersion) -> Option<Vec<Change>> {
        let id = self.table_by_name(table)?;
        diff_versions(self.table(id), from, to)
    }

    /// Each version of a table with its first and last sighting and the
    /// changes since the previous version.
    pub fn lineage(&self, table: &str) -> Option<Vec<LineageEntry>> {
        let id = self.table_by_name(table)?;
        let t = self.table(id);
        let mut out = Vec::new();
        let mut prev: Option<SchemaVersion> = None;
        for v in &t.versions {
            let changes = prev
                .and_then(|p| diff_versions(t, p, v.n))
                .unwrap_or_default();
            out.push(LineageEntry {
                version: v.n,
                first_seen: v.first_seen.map(|m| m.to_string()),
                last_seen: v.last_seen.map(|m| m.to_string()),
                changes,
                variants: v.variants.len(),
            });
            prev = Some(v.n);
        }
        Some(out)
    }

    /// Changes between this registry and `newer` that break the lake.
    pub fn check_compat(&self, newer: &Registry) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for t in &self.tables {
            match newer.table_by_name(&t.name) {
                None => out.push((
                    t.name.clone(),
                    "table was deleted; mark it deprecated instead".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded census is decoded when a shape is first asked for, and
    /// knows the anchor month's shapes.
    #[test]
    fn the_embedded_census_names_the_anchor_months_shapes() {
        let r = Registry::load_embedded(TypeOptions::default());
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
        let r = Registry::load_embedded(TypeOptions::default());
        let r2 = Registry::load_embedded(TypeOptions::default());
        assert!(Arc::ptr_eq(&r, &r2));
        let id = r.table_by_name("dispatch_unit_scada").unwrap();
        assert_eq!(r.table_by_pair("DISPATCH", "UNIT_SCADA"), Some(id));
        assert_eq!(r.table_by_pair("DISPATCH", "NOPE"), None);
        assert_eq!(r.table(id).name, "DISPATCH_UNIT_SCADA");
        assert_eq!(r.market_of(id).arrow_tz(), "+10:00");
        assert_eq!(r.lake_schema(id).fields().len(), 4);
        let v = r.table(id).versions[0].n;
        assert!(r.plan(id, v).is_some());
        assert_eq!(
            r.by_fingerprint(r.table(id).versions[0].fingerprint),
            Some((id, v))
        );
        assert!(r.tables().count() >= 20);
        assert!(r.market(Market::Nem).is_some());
        assert!(r.market(Market::Wem).is_some());
        let dec = Registry::load_embedded(TypeOptions { decimal: true });
        assert!(dec.options().decimal);
        let f = &dec.lake_schema(id).fields()[2];
        assert!(
            matches!(f.data_type(), arrow_schema::DataType::Decimal128(16, 6)),
            "{f:?}"
        );
        assert!(r.diff("DISPATCH_UNIT_SCADA", v, v).unwrap().is_empty());
        assert!(r.diff("NOPE", v, v).is_none());
        let lin = r.lineage("DISPATCHPRICE").unwrap();
        assert!(!lin.is_empty());
        assert!(r.lineage("NOPE").is_none());
        assert!(r.check_compat(&r).is_empty());
    }

    #[test]
    fn load_dir_and_overlay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        let r = Registry::load_dir(&root, TypeOptions::default()).unwrap();
        assert!(r.table_by_name("REGION").is_some());
        let dir = tempfile::tempdir().unwrap();
        let over = dir.path().join("nem").join("mms");
        std::fs::create_dir_all(&over).unwrap();
        // Overlay replaces REGION with a one-column table and adds a new one.
        let mut t = r.table(r.table_by_name("REGION").unwrap()).clone();
        t.columns.truncate(1);
        t.primary_key = vec!["REGIONID".into()];
        t.versions.truncate(1);
        t.versions[0].columns = vec!["REGIONID".into()];
        t.versions[0].fingerprint = Fingerprint::of_names(["REGIONID"]);
        t.dedupe = crate::file_model::Dedupe::Pk;
        std::fs::write(
            over.join("REGION.toml"),
            crate::file_model::file::write_table(&t),
        )
        .unwrap();
        let mut n = t.clone();
        n.name = "NEWTABLE".into();
        n.report_pairs = vec![("X".into(), "Y".into())];
        std::fs::write(
            over.join("NEWTABLE.toml"),
            crate::file_model::file::write_table(&n),
        )
        .unwrap();
        let o = r.with_overlay(dir.path()).unwrap();
        assert_eq!(o.table(o.table_by_name("REGION").unwrap()).columns.len(), 1);
        assert!(o.table_by_pair("X", "Y").is_some());
        assert_eq!(o.tables().count(), r.tables().count() + 1);
        // Removing a column is a breaking change.
        let breaks = r.check_compat(&o);
        assert!(
            breaks
                .iter()
                .any(|(t, m)| t == "REGION" && m.contains("deleted")),
            "{breaks:?}"
        );
        // A user's table may keep an inferred column; a directory that is
        // to be committed as the registry may not.
        let mut guessed = n.clone();
        guessed.name = "GUESSED".into();
        guessed.report_pairs = vec![("X".into(), "Z".into())];
        guessed.columns[0].src = vec!["inferred".into()];
        guessed.columns[0].nullable = false;
        std::fs::write(
            over.join("GUESSED.toml"),
            crate::file_model::file::write_table(&guessed),
        )
        .unwrap();
        let o = r.with_overlay(dir.path()).unwrap();
        assert!(o.table_by_pair("X", "Z").is_some());
        std::fs::write(
            dir.path().join("nem").join("market.toml"),
            std::fs::read(root.join("nem").join("market.toml")).unwrap(),
        )
        .unwrap();
        let e = Registry::load_dir(dir.path(), TypeOptions::default()).unwrap_err();
        assert!(e.to_string().contains("src inferred"), "{e}");
        std::fs::remove_file(over.join("GUESSED.toml")).unwrap();
        // A draft is not read at all.
        std::fs::write(over.join("DRAFT_BROKEN.toml"), "not toml at all [").unwrap();
        assert!(r.with_overlay(dir.path()).is_ok());
        // An invalid overlay is refused.
        std::fs::write(
            over.join("NEWTABLE.toml"),
            crate::file_model::file::write_table(&n)
                .replace("next_column_id = ", "next_column_id = 0 #"),
        )
        .unwrap();
        assert!(r.with_overlay(dir.path()).is_err());
        assert!(Registry::load_dir(dir.path(), TypeOptions::default()).is_err());
        assert!(Registry::load_dir(&dir.path().join("none"), TypeOptions::default()).is_err());
    }
}
