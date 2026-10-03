//! The loaded registry: tables, indexes and precomputed projections.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use arrow_schema::SchemaRef;
use hashbrown::Equivalent;
use nemlake_core::Fingerprint;
use nemlake_schema_model::blob::SchemaSet;
use nemlake_schema_model::compat::{Change, diff_versions};
use nemlake_schema_model::{Market, MarketDef, SchemaFileError, SchemaVersion, TableDef};

use crate::arrow::lake_schema;
use crate::resolve::Projection;

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
    by_dataset: HashMap<String, TableId>,
    by_fingerprint: HashMap<Fingerprint, (TableId, SchemaVersion)>,
    lake_schemas: Vec<SchemaRef>,
    projections: HashMap<(TableId, SchemaVersion), Arc<Projection>>,
    /// Projections of the declared variants of a version, by their fingerprint.
    variants: HashMap<(TableId, SchemaVersion, Fingerprint), Arc<Projection>>,
    /// Projections built at run time for drifted or aliased shapes.
    dynamic: Mutex<HashMap<(TableId, Fingerprint), Arc<Projection>>>,
    /// Tables derived at run time from the header of a file whose report
    /// pair no table claims. Slots are written once and never moved, so a
    /// reference to one lives as long as the registry.
    derived: Vec<OnceLock<Derived>>,
    /// The report pair of each derived table, in slot order.
    derived_pairs: Mutex<Vec<(String, String)>>,
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

static EMBEDDED_F64: OnceLock<Arc<Registry>> = OnceLock::new();
static EMBEDDED_DECIMAL: OnceLock<Arc<Registry>> = OnceLock::new();

/// The blob `build.rs` packed from `schemas/`.
const EMBEDDED_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/registry.bin"));

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
            Arc::new(Self::from_set(set, opts))
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
        let set = SchemaSet { markets, tables };
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
        let markets = set.markets;
        let tables = set.tables;
        let mut by_name = HashMap::with_capacity(tables.len());
        let mut by_pair = hashbrown::HashMap::with_capacity(tables.len());
        let mut by_dataset = HashMap::new();
        let mut by_fingerprint = HashMap::new();
        let mut lake_schemas = Vec::with_capacity(tables.len());
        let mut projections = HashMap::new();
        let mut variants = HashMap::new();
        for (i, t) in tables.iter().enumerate() {
            let id = TableId(i as u32);
            by_name.insert(t.name.clone(), id);
            for (a, b) in &t.report_pairs {
                by_pair.insert(PairKey(a.clone(), b.clone()), id);
            }
            for d in &t.datasets {
                by_dataset.insert(d.clone(), id);
            }
            let market = markets
                .iter()
                .find(|m| m.market == t.market)
                .cloned()
                .unwrap_or(MarketDef {
                    market: t.market,
                    utc_offset_secs: if t.market == Market::Wem {
                        28_800
                    } else {
                        36_000
                    },
                    timestamp_format: nemlake_schema_model::TimestampFormat::AemoSlash,
                });
            let schema = lake_schema(t, &market, opts);
            for v in &t.versions {
                by_fingerprint.entry(v.fingerprint).or_insert((id, v.n));
                let names: Vec<&str> = v.columns.iter().map(String::as_str).collect();
                let proj = Projection::build(t, &market, opts, &schema, &names)
                    .expect("a validated version projects onto its table");
                projections.insert((id, v.n), Arc::new(proj));
                for x in &v.variants {
                    let names: Vec<&str> = x.columns.iter().map(String::as_str).collect();
                    let proj = Projection::build(t, &market, opts, &schema, &names)
                        .expect("a validated variant projects onto its table");
                    variants.insert((id, v.n, x.fingerprint), Arc::new(proj));
                }
            }
            lake_schemas.push(schema);
        }
        Self {
            opts,
            markets,
            from_user: vec![false; tables.len()],
            tables,
            by_name,
            by_pair,
            by_dataset,
            by_fingerprint,
            lake_schemas,
            projections,
            variants,
            dynamic: Mutex::new(HashMap::new()),
            derived: std::iter::repeat_with(OnceLock::new)
                .take(DERIVED_SLOTS)
                .collect(),
            derived_pairs: Mutex::new(Vec::new()),
        }
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
        let market_def = self.market(market).cloned().unwrap_or(MarketDef {
            market,
            utc_offset_secs: if market == Market::Wem {
                28_800
            } else {
                36_000
            },
            timestamp_format: nemlake_schema_model::TimestampFormat::AemoSlash,
        });
        let lake = lake_schema(&table, &market_def, self.opts);
        let _ = self.derived[slot].set(Derived { table, lake });
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

    /// The handle for a plain-CSV dataset.
    pub fn table_by_dataset(&self, dataset: &str) -> Option<TableId> {
        self.by_dataset.get(dataset).copied()
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
        match self.lake_schemas.get(i) {
            Some(s) => s,
            None => &self.derived_slot(i).lake,
        }
    }

    /// The precomputed projection of a declared version.
    pub fn projection(&self, id: TableId, version: SchemaVersion) -> Option<&Arc<Projection>> {
        self.projections.get(&(id, version))
    }

    /// The precomputed projection of a declared variant of a version.
    pub fn variant_projection(
        &self,
        id: TableId,
        version: SchemaVersion,
        fp: Fingerprint,
    ) -> Option<&Arc<Projection>> {
        self.variants.get(&(id, version, fp))
    }

    /// The table and version that published a column list with this fingerprint.
    pub fn by_fingerprint(&self, fp: Fingerprint) -> Option<(TableId, SchemaVersion)> {
        self.by_fingerprint.get(&fp).copied()
    }

    /// A run-time projection for a drifted or aliased shape, cached.
    pub(crate) fn dynamic_projection(
        &self,
        id: TableId,
        fp: Fingerprint,
        build: impl FnOnce() -> Arc<Projection>,
    ) -> Arc<Projection> {
        let mut cache = self
            .dynamic
            .lock()
            .expect("the projection cache is never poisoned");
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
                    for m in nemlake_schema_model::compat::breaking_changes(t, newer.table(id)) {
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
        assert_eq!(r.lake_schema(id).fields().len(), 3);
        let v = r.table(id).versions[0].n;
        assert!(r.projection(id, v).is_some());
        assert_eq!(
            r.by_fingerprint(r.table(id).versions[0].fingerprint),
            Some((id, v))
        );
        assert!(r.tables().count() >= 20);
        assert!(r.market(Market::Nem).is_some());
        assert!(r.market(Market::Wem).is_some());
        assert!(r.table_by_dataset("facility-scada").is_none());
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
        t.dedupe = nemlake_schema_model::Dedupe::Pk;
        std::fs::write(
            over.join("REGION.toml"),
            nemlake_schema_model::file::write_table(&t),
        )
        .unwrap();
        let mut n = t.clone();
        n.name = "NEWTABLE".into();
        n.report_pairs = vec![("X".into(), "Y".into())];
        std::fs::write(
            over.join("NEWTABLE.toml"),
            nemlake_schema_model::file::write_table(&n),
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
            nemlake_schema_model::file::write_table(&guessed),
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
            nemlake_schema_model::file::write_table(&n)
                .replace("next_column_id = ", "next_column_id = 0 #"),
        )
        .unwrap();
        assert!(r.with_overlay(dir.path()).is_err());
        assert!(Registry::load_dir(dir.path(), TypeOptions::default()).is_err());
        assert!(Registry::load_dir(&dir.path().join("none"), TypeOptions::default()).is_err());
    }
}
