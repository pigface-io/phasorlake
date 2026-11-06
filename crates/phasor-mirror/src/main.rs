//! `phasor-mirror`: a copy of every file under nemweb's archive folders, and
//! the catalog of it.

mod cli;
mod commands;
mod render;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the tokio runtime builds on every supported platform");
    let code = match runtime.block_on(commands::run_command(&cli)) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::from(2)
        }
    };
    zones_report(cli.zones.as_deref(), cli.json);
    code
}

/// The report of the run's zones, where `--zones` asked for it: on stderr
/// for `-` (or `1`, `true`, nothing), else into the file named; NDJSON
/// under `--json`, else the table. A binary built without the feature
/// `zones` says so.
fn zones_report(sink: Option<&std::path::Path>, json: bool) {
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
