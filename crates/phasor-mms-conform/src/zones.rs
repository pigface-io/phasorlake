//! The zones of conforming, for `zonetrace` (feature `zones`): 200 to 299.

use zonetrace::Zone;

/// One row cast field by field into the column builders.
pub static ROW: Zone = Zone::new(200, "conform.row");
/// The builders finished into a typed batch.
pub static FLUSH: Zone = Zone::new(201, "conform.flush");
/// A block's shape resolved through the registry.
pub static BLOCK: Zone = Zone::new(202, "conform.block");
/// One row's fields parsed to their lake types.
pub static PARSE: Zone = Zone::new(203, "conform.parse");
/// One row's parsed values appended to the column builders.
pub static APPEND: Zone = Zone::new(204, "conform.append");
