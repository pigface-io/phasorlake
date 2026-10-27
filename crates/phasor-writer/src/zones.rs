//! The zones of writing the lake, for `zonetrace` (feature `zones`): 300
//! to 399.

use zonetrace::Zone;

/// One batch written to an open Parquet file.
pub static WRITE: Zone = Zone::new(300, "parquet.write");
/// The batch's time bounds and order checked.
pub static BOUNDS: Zone = Zone::new(301, "parquet.bounds");
/// The batch encoded by the Parquet writer.
pub static ENCODE: Zone = Zone::new(302, "parquet.encode");
/// A file closed: its footer written and handed to the disk.
pub static CLOSE: Zone = Zone::new(303, "parquet.close");
/// A file opened for writing.
pub static OPEN: Zone = Zone::new(304, "parquet.open");
/// A batch split by the market-local month of its time column.
pub static SPLIT: Zone = Zone::new(305, "partition.split");
/// Every closed file made durable by one flush, and renamed into place.
pub static COMMIT: Zone = Zone::new(306, "parquet.commit");
/// A batch cast to the schema of the file it goes to.
pub static FIT: Zone = Zone::new(307, "session.fit");
/// Closing every open file of a source.
pub static SESSION_CLOSE: Zone = Zone::new(308, "session.close");
