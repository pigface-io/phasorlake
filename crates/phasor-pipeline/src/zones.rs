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

/// The run's asynchronous stages, as stretches no guard can hold (they
/// cross awaits and threads): each is a path under `run`, which only
/// places them; its own time is the command's.
pub static RUN: Zone = Zone::new(109, "run");
/// The source listed: what the request needs.
pub static LIST: Zone = Zone::new(110, "run.list");
/// One file fetched, picked or hashed in the fetch stage.
pub static FETCH: Zone = Zone::new(111, "run.fetch");
/// One local file hashed on the blocking pool.
pub static HASH_LOCAL: Zone = Zone::new(112, "run.hash_local");
/// The manifest asked what it holds of a file, before its ingest.
pub static MANIFEST_ASKS: Zone = Zone::new(113, "run.manifest_asks");
/// A file's ingest awaited on the blocking pool: less `ingest.file`, the
/// wait for a thread.
pub static INGEST_AWAIT: Zone = Zone::new(114, "run.ingest_await");
/// One file's results committed to the manifest.
pub static COMMIT: Zone = Zone::new(115, "run.commit");
/// The run finished: the summary and the manifest's last word.
pub static FINISH: Zone = Zone::new(116, "run.finish");
/// A `get` planned: every source listed for its months.
pub static GET_PLAN: Zone = Zone::new(120, "get.plan");
/// The path of a [`LIST`] stretch.
pub static LIST_PATH: [&Zone; 2] = [&RUN, &LIST];
/// The path of a [`FETCH`] stretch.
pub static FETCH_PATH: [&Zone; 2] = [&RUN, &FETCH];
/// The path of a [`MANIFEST_ASKS`] stretch.
pub static MANIFEST_ASKS_PATH: [&Zone; 2] = [&RUN, &MANIFEST_ASKS];
/// The path of an [`INGEST_AWAIT`] stretch.
pub static INGEST_AWAIT_PATH: [&Zone; 2] = [&RUN, &INGEST_AWAIT];
/// The path of a [`COMMIT`] stretch.
pub static COMMIT_PATH: [&Zone; 2] = [&RUN, &COMMIT];
/// The path of a [`FINISH`] stretch.
pub static FINISH_PATH: [&Zone; 2] = [&RUN, &FINISH];
/// The path of a [`GET_PLAN`] stretch.
pub static GET_PLAN_PATH: [&Zone; 1] = [&GET_PLAN];
