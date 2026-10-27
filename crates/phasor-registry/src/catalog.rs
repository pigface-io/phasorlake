//! The registry as a kit sees it: every market's tables as a
//! [`Catalog`], each as a block names it ([`table_ref`]), and the lookups a
//! feed's source makes of the registry ([`table_of`], [`serves_feed`]).

use std::path::Path;
use std::sync::Arc;

use phasor_core::{Diagnose, Diagnostic};
use phasor_kit::{Catalog, LoadMode, Serves, TableOf, TableRef, Transform};
use phasor_mms_decode::IRow;
use phasor_plain_decode::BlockName;

use crate::file_model::Market;
use crate::{Mode, Registry, ResolveOptions, TableId};

/// The kit's load mode of a table the registry declares.
pub fn load_mode(m: crate::LoadMode) -> LoadMode {
    match m {
        crate::LoadMode::Append => LoadMode::Append,
        crate::LoadMode::Snapshot => LoadMode::Snapshot,
    }
}

/// A table of the registry as a block names it.
pub fn table_ref(
    registry: &Registry,
    id: TableId,
    plan_mode: LoadMode,
    time_column: Option<usize>,
) -> TableRef {
    let t = registry.table(id);
    TableRef {
        market: t.market.id().to_owned(),
        name: t.name.clone(),
        dir: registry.table_dir(id),
        utc_offset_secs: registry.market_of(id).utc_offset_secs,
        load_mode: plan_mode,
        time_column,
    }
}

/// The table of `market` that claims a block, by its family and name, when
/// the registry declares one.
pub fn table_of(registry: Arc<Registry>, market: Market) -> TableOf {
    Arc::new(move |b: &BlockName| {
        registry
            .table_by_pair(market, &b.family, &b.name)
            .map(|id| registry.table(id).name.clone())
    })
}

/// Whether a table is one the census saw in the folders of `feed`
/// (folders.json): a table seen in none is served by no feed.
pub fn serves_feed(registry: Arc<Registry>, feed: &'static str) -> Serves {
    Arc::new(move |table: &str| {
        registry
            .table_by_name(&table.to_ascii_uppercase())
            .is_some_and(|id| registry.folders_of(id).iter().any(|f| f.feed == feed))
    })
}

/// The registry's tables of one market as a [`Catalog`].
pub struct RegistryCatalog {
    registry: Arc<Registry>,
    market: Market,
}

impl RegistryCatalog {
    /// A catalog over `registry`'s tables of `market`.
    pub fn new(registry: Arc<Registry>, market: Market) -> Self {
        Self { registry, market }
    }

    /// The registry behind the catalog, for the schema commands.
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    fn table_of(&self, id: TableId) -> Arc<TableRef> {
        let t = self.registry.table(id);
        let time_column = t
            .time_column
            .as_deref()
            .and_then(|c| self.registry.lake_schema(id).index_of(c).ok());
        Arc::new(table_ref(
            &self.registry,
            id,
            load_mode(t.load_mode),
            time_column,
        ))
    }
}

impl Catalog for RegistryCatalog {
    fn market(&self) -> &str {
        self.market.id()
    }

    fn tables(&self) -> Vec<Arc<TableRef>> {
        let mut out: Vec<Arc<TableRef>> = self
            .registry
            .tables()
            .filter(|(_, t)| t.market == self.market)
            .map(|(id, _)| self.table_of(id))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn table(&self, name: &str) -> Option<Arc<TableRef>> {
        self.registry
            .table_by_name(&name.to_ascii_uppercase())
            .map(|id| self.table_of(id))
    }

    /// Whether every shape that was read from a source would be read by
    /// the same rules today. The answer comes from the recorded transforms
    /// alone; the source is not opened.
    ///
    /// Strictness is not part of the question: a shape the registry would
    /// refuse in strict mode is current when the permissive rules that
    /// wrote it have not changed.
    fn plans_current(&self, transforms: &[Transform]) -> bool {
        transforms.iter().all(|t| {
            if t.plan_hash.is_empty() {
                return false;
            }
            let columns: Vec<&str> = t.source_columns.iter().map(String::as_str).collect();
            let irow = IRow {
                report_type: &t.block_family,
                report_subtype: &t.block_name,
                version: t.source_version,
                columns: &columns,
            };
            // The month the source was read as: it picks among the runs
            // of the table's history that fit the block.
            let period = t.source_period.as_deref().and_then(|p| p.parse().ok());
            self.registry
                .resolve_in(
                    &irow,
                    ResolveOptions {
                        mode: Mode::Permissive,
                        market: self.market,
                    },
                    period,
                )
                .is_ok_and(|r| r.plan_hash().to_string() == t.plan_hash)
        })
    }

    fn with_overlay(&self, dir: &Path) -> Result<Arc<dyn Catalog>, Box<Diagnostic>> {
        let registry = self
            .registry
            .with_overlay(dir)
            .map_err(|e| Box::new(e.diagnostic()))?;
        Ok(Arc::new(Self::new(Arc::new(registry), self.market)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_nem_tables_and_finds_one_by_name() {
        let registry = Registry::load_embedded();
        let cat = RegistryCatalog::new(registry.clone(), Market::Nem);
        assert_eq!(cat.market(), "nem");
        let tables = cat.tables();
        assert!(tables.len() > 200);
        assert!(tables.windows(2).all(|w| w[0].name < w[1].name));
        let scada = cat.table("dispatch_unit_scada").unwrap();
        assert_eq!(scada.name, "DISPATCH_UNIT_SCADA");
        assert_eq!(scada.dir, "dispatch_unit_scada");
        assert_eq!(scada.market, "nem");
        assert_eq!(scada.utc_offset_secs, 36_000);
        assert_eq!(scada.load_mode, LoadMode::Append);
        assert_eq!(
            scada.time_column,
            registry
                .lake_schema(registry.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
                .index_of("SETTLEMENTDATE")
                .ok()
        );
        assert!(cat.table("NO_SUCH_TABLE").is_none());
        assert!(cat.plans_current(&[]));
        assert!(!cat.plans_current(&[Transform::default()]));
        assert!(Arc::clone(cat.registry()).table_by_name("REGION").is_some());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(cat.with_overlay(dir.path()).unwrap().market(), "nem");
    }

    #[test]
    fn a_block_names_its_table_and_a_feed_its_tables() {
        let registry = Registry::load_embedded();
        let of = table_of(registry.clone(), Market::Nem);
        let region = BlockName {
            family: "DISPATCH".into(),
            name: "REGIONSUM".into(),
            version: 1,
        };
        assert_eq!(of(&region).as_deref(), Some("DISPATCHREGIONSUM"));
        let none = BlockName {
            family: "NO".into(),
            name: "SUCH".into(),
            version: 1,
        };
        assert!(of(&none).is_none());
        let serves = serves_feed(registry, "gbb");
        assert!(!serves("NO_SUCH_TABLE"));
    }
}
