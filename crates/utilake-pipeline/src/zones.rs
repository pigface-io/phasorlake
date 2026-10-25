//! The zones of an ingest, for `zonetrace` (feature `zones`). The numbers
//! are fixed, so a report from one revision diffs against the next; the
//! pipeline's are 100 to 199.

use zonetrace::Zone;

/// One local file, from open to its last entry written.
pub static FILE: Zone = Zone::new(100, "ingest.file");
/// One entry of a file: decoded, conformed and written.
pub static ENTRY: Zone = Zone::new(101, "ingest.entry");
/// The kit's decoder over one entry: scanning, conforming, and the sink.
pub static DECODE: Zone = Zone::new(102, "decode");
/// One batch handed to the lake: widened, fitted and written.
pub static BATCH: Zone = Zone::new(103, "sink.batch");
/// A batch cast to the schema of the file it goes to.
pub static FIT: Zone = Zone::new(104, "sink.fit");
/// Closing every open file of an entry.
pub static CLOSE: Zone = Zone::new(105, "sink.close");
/// A new part file opened for a batch.
pub static OPEN: Zone = Zone::new(106, "sink.open");
