//! What a kit knows of its tables.

use std::path::Path;
use std::sync::Arc;

use phasor_core::Diagnostic;

use crate::decoder::TableRef;
use crate::transform::Transform;

/// The tables of one market, as a kit declares them; how it does so is its own.
pub trait Catalog: Send + Sync {
    /// The market directory the tables live under: `nem`.
    fn market(&self) -> &str;
    /// Every table, in name order.
    fn tables(&self) -> Vec<Arc<TableRef>>;
    /// A table by its declared name, in any case.
    fn table(&self, name: &str) -> Option<Arc<TableRef>>;
    /// Whether the catalog's rules today would produce every transform recorded,
    /// decided from the transforms alone. An empty list is current.
    fn plans_current(&self, transforms: &[Transform]) -> bool;
    /// The catalog with the user's tables from `dir` laid over it: a same-named table is replaced.
    fn with_overlay(&self, dir: &Path) -> Result<Arc<dyn Catalog>, Box<Diagnostic>>;
}

/// Names the table that claims a block, when the catalog declares one.
pub type TableOf =
    std::sync::Arc<dyn Fn(&phasor_plain_decode::BlockName) -> Option<String> + Send + Sync>;

/// Whether a table is one a source publishes.
pub type Serves = std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The folders of a host that carry a table's files, as paths under the host
/// (`publicdata/Datasets/Wholesale/DispatchAndPricing/FinalEnergyPrices`):
/// where a source looks for the table instead of walking the whole feed.
/// Empty when the catalog does not know.
pub type FoldersOf = std::sync::Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;
