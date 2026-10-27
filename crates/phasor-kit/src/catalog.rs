//! What a kit knows of its tables.

use std::path::Path;
use std::sync::Arc;

use phasor_core::Diagnostic;

use crate::decoder::TableRef;
use crate::transform::Transform;

/// The tables of one market, as a kit declares them.
///
/// The pipeline and the commands see no registry: how a kit declares
/// tables, resolves blocks and plans casts is its own. What they ask is
/// listed here.
pub trait Catalog: Send + Sync {
    /// The market directory the tables live under: `nem`.
    fn market(&self) -> &str;
    /// Every table, in name order.
    fn tables(&self) -> Vec<Arc<TableRef>>;
    /// A table by its declared name, in any case.
    fn table(&self, name: &str) -> Option<Arc<TableRef>>;
    /// Whether every transform recorded of a source would be produced by
    /// the catalog's rules today: decided from the transforms alone, the
    /// source is not opened. An empty list is current.
    fn plans_current(&self, transforms: &[Transform]) -> bool;
    /// The catalog with the user's own tables from `dir` laid over it: a
    /// file named after a declared table replaces it, others are added.
    fn with_overlay(&self, dir: &Path) -> Result<Arc<dyn Catalog>, Box<Diagnostic>>;
}

/// Names the table that claims a block, when the catalog declares one.
pub type TableOf =
    std::sync::Arc<dyn Fn(&phasor_plain_decode::BlockName) -> Option<String> + Send + Sync>;

/// Whether a table is one a source publishes.
pub type Serves = std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;
