//! Orchestration: list what a request needs, fetch it, ingest each file on a
//! blocking worker, commit the results to the manifest.
//!
//! Layer L4. Async at the edges, sync in the core: downloads and the manifest
//! are awaited, each file is parsed and written on one `spawn_blocking` task,
//! and the two meet once per file. Backpressure comes from
//! `buffer_unordered`: at most `downloads` fetches and `jobs` parses are in
//! flight. Every file is attempted; a failure marks that file and the run
//! goes on unless `fail_fast` is set.

/// Opens a zone of `zonetrace` until the end of the block, with the
/// feature `zones`; nothing without it.
#[allow(unused_macros)]
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = zonetrace::enter(&crate::zones::$z);
    };
}

#[cfg(feature = "zones")]
pub mod zones;

mod entry_times;
pub mod get;
pub mod ingest_file;
pub mod plan;
pub mod report;
pub mod run;

pub use get::{GetPlan, GetReport, GetRequest, SourceChoice, Step, days_of, plan_get, run_get};
pub use plan::{DEFAULT_BATCH_ROWS, Deps, Input, Plan, RunOptions};
pub use report::{RunReport, exit_code};
pub use run::{local_remote, local_remote_in, recorded_remote, run};
