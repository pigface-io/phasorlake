//! The outcome of a run and how it becomes an exit code.

use phasor_core::diag::exit;
use phasor_core::event::Summary;
use phasor_core::report::Report;
use phasor_kit::RemoteFile;

// The family's exit codes (`phasor_diag::exit`), as the `i32` a process
// exits with.
/// Exit code: success.
pub const EXIT_OK: i32 = exit::OK as i32;
/// Exit code: nothing succeeded, or the run could not start.
pub const EXIT_FAILURE: i32 = exit::FAILURE as i32;
/// Exit code: some files failed.
pub const EXIT_PARTIAL: i32 = exit::PARTIAL as i32;
/// Exit code: offline and a file was not cached.
pub const EXIT_OFFLINE: i32 = exit::OFFLINE as i32;
/// Exit code: a schema could not be resolved in strict mode.
pub const EXIT_SCHEMA: i32 = exit::SCHEMA as i32;
/// Exit code: Ctrl-C stopped the run, the files in flight finished. Not one
/// of the family's codes: the shell's own for a process stopped by SIGINT
/// (128 + 2), so a script tells an interrupted run from a finished one.
pub const EXIT_INTERRUPTED: i32 = 130;

/// What a run produced.
#[derive(Debug)]
pub struct RunReport {
    /// Manifest run id.
    pub run_id: Option<i64>,
    /// Totals.
    pub summary: Summary,
    /// Every diagnostic and per-source outcome.
    pub report: Report,
    /// The files the source listed (also for `--dry-run`).
    pub listed: Vec<RemoteFile>,
    /// The exit code.
    pub exit_code: i32,
}

/// Derives the exit code from the report's per-source outcomes.
pub fn exit_code(report: &Report) -> i32 {
    let attempted = report
        .per_source
        .iter()
        .filter(|s| s.status != phasor_core::report::SourceStatus::NotAttempted)
        .count();
    let failed = report.failed().count();
    let codes: Vec<String> = report
        .failed()
        .filter_map(|s| s.failed_with.as_ref().map(ToString::to_string))
        .collect();
    if failed == 0 {
        return EXIT_OK;
    }
    if codes.iter().all(|c| c == "net.offline") {
        return EXIT_OFFLINE;
    }
    if codes.iter().all(|c| c.starts_with("schema.")) && report.succeeded() == 0 {
        return EXIT_SCHEMA;
    }
    if failed >= attempted || report.succeeded() + report.skipped() == 0 {
        EXIT_FAILURE
    } else {
        EXIT_PARTIAL
    }
}

#[cfg(test)]
mod tests {
    use phasor_core::report::{SourceOutcome, SourceStatus};

    use super::*;

    fn outcome(status: SourceStatus, code: Option<&'static str>) -> SourceOutcome {
        SourceOutcome {
            source: "s".into(),
            status,
            failed_with: code.map(std::borrow::Cow::Borrowed),
            errors: 0,
            warnings: 0,
            rows: 0,
            bad_rows: 0,
        }
    }

    #[test]
    fn codes() {
        let mut r = Report::new();
        assert_eq!(exit_code(&r), EXIT_OK);
        r.finish_source(outcome(SourceStatus::Ok, None));
        r.finish_source(outcome(SourceStatus::Failed, Some("zip.bad_entry")));
        assert_eq!(exit_code(&r), EXIT_PARTIAL);
        let mut r = Report::new();
        r.finish_source(outcome(SourceStatus::Failed, Some("zip.bad_entry")));
        assert_eq!(exit_code(&r), EXIT_FAILURE);
        let mut r = Report::new();
        r.finish_source(outcome(SourceStatus::Failed, Some("net.offline")));
        r.finish_source(outcome(SourceStatus::Skipped, None));
        assert_eq!(exit_code(&r), EXIT_OFFLINE);
        let mut r = Report::new();
        r.finish_source(outcome(SourceStatus::Failed, Some("schema.unknown_table")));
        assert_eq!(exit_code(&r), EXIT_SCHEMA);
        let mut r = Report::new();
        r.finish_source(outcome(SourceStatus::Skipped, None));
        r.finish_source(outcome(SourceStatus::Failed, Some("csv.read_failed")));
        assert_eq!(exit_code(&r), EXIT_PARTIAL);
    }
}
