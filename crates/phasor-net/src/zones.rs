//! The zones of a request, for `zonetrace` (feature `zones`): 500 to 549.
//! The numbers are fixed, so a report from one revision diffs against the
//! next. The asynchronous stretches are roots of their own: the pacing,
//! the request and the body cross awaits and threads, and no guard can
//! hold them. The tracer caches a zone by `id % 64`: 505, entered per
//! chunk on the blocking pool, falls in slot 57, which the readers' hot
//! zones leave free.

use zonetrace::Zone;

/// The wait before a request, for the host's pace: held under one lock, so
/// its total is the serialisation every request of a fetcher shares.
pub static PACE: Zone = Zone::new(500, "net.pace");
/// The wait before a retry.
pub static BACKOFF: Zone = Zone::new(503, "net.backoff");
/// A body streamed, chunk by chunk, to the spool.
pub static BODY: Zone = Zone::new(504, "net.body");
/// The spool: a body's chunks hashed and written on the blocking pool.
pub static SPOOL: Zone = Zone::new(505, "net.spool");
/// The path of a [`PACE`] stretch.
pub static PACE_PATH: [&Zone; 1] = [&PACE];
/// The path of a [`BACKOFF`] stretch.
pub static BACKOFF_PATH: [&Zone; 1] = [&BACKOFF];
/// The path of a [`BODY`] stretch.
pub static BODY_PATH: [&Zone; 1] = [&BODY];
