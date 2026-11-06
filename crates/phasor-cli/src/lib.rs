//! The command: parse flags, assemble dependencies, call the library crates, render.

/// Opens a zone of `zonetrace` until the end of the block, with the
/// feature `zones`; nothing without it. For synchronous code only.
#[allow(unused_macros)]
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = zonetrace::enter(&crate::zones::$z);
    };
}

/// Records a stretch of time on a path of zones when the block ends, with
/// the feature `zones`; nothing without it. Safe across an `.await`.
#[allow(unused_macros)]
macro_rules! stretch {
    ($p:ident) => {
        #[cfg(feature = "zones")]
        let _stretch = zonetrace::stretch(&crate::zones::$p);
    };
}

#[cfg(feature = "zones")]
pub mod zones;

pub mod cli;
pub mod commands;
pub mod context;
pub mod kits;
pub mod render;

pub use cli::Cli;

/// Runs the parsed command and returns the exit code; then, where `--zones`
/// asked for it, the report of where the run's time went.
pub async fn run(cli: Cli) -> i32 {
    let zones = cli.zones.clone();
    let json = cli.json;
    let code = match commands::dispatch(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    zones_report(zones.as_deref(), json);
    code
}

/// The report of the run's zones, where `--zones` asked for it: on stderr
/// for `-` (or `1`, `true`, nothing), else into the file named; NDJSON
/// under `--json`, else the table. A binary built without the feature
/// `zones` says so.
pub fn zones_report(sink: Option<&std::path::Path>, json: bool) {
    let Some(sink) = sink else { return };
    #[cfg(feature = "zones")]
    {
        let report = zonetrace::take_all();
        let overhead = zonetrace::overhead();
        let text = if json {
            report.ndjson(overhead)
        } else {
            report.text(overhead)
        };
        if matches!(sink.to_str(), Some("-" | "1" | "true" | "")) {
            eprint!("{text}");
        } else if let Err(e) = std::fs::write(sink, text) {
            eprintln!("zones: {} could not be written: {e}", sink.display());
        }
    }
    #[cfg(not(feature = "zones"))]
    {
        let _ = (sink, json);
        eprintln!(
            "zones: this binary was built without the feature zones; build with --features zones"
        );
    }
}
