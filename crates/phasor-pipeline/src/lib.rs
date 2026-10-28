//! Orchestration (layer L4): list what a request needs, fetch it, ingest each
//! file on a blocking worker, commit the results to the manifest. At most
//! `downloads` fetches and `jobs` parses are in flight.

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

pub use get::{
    GetPlan, GetReport, GetRequest, SourceChoice, SourceGroup, Step, days_of, plan_get, run_get,
    source_groups,
};
pub use plan::{DEFAULT_BATCH_ROWS, Deps, Input, Plan, RunOptions};
pub use report::{RunReport, exit_code};
pub use run::{local_remote, local_remote_in, recorded_remote, run};
