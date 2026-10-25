//! The zones of writing the lake, for `zonetrace` (feature `zones`): 300
//! to 399.

use zonetrace::Zone;

/// One batch written to an open Parquet file.
pub static WRITE: Zone = Zone::new(300, "parquet.write");
/// The batch's time bounds and order checked.
pub static BOUNDS: Zone = Zone::new(301, "parquet.bounds");
/// The batch encoded by the Parquet writer.
pub static ENCODE: Zone = Zone::new(302, "parquet.encode");
/// A file closed: its footer written, synced and renamed into place.
pub static CLOSE: Zone = Zone::new(303, "parquet.close");
/// A file opened for writing.
pub static OPEN: Zone = Zone::new(304, "parquet.open");
/// A batch split by the market-local month of its time column.
pub static SPLIT: Zone = Zone::new(305, "partition.split");
