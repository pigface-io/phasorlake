//! Aggregation of diagnostics over a run: every file is attempted, every
//! failure is kept, and the summary groups by code.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::diag::{Code, Diagnostic, Severity};

/// What a sink tells the caller to do after a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep going.
    Continue,
    /// Stop at the first opportunity (`--fail-fast`).
    Stop,
}

/// Receives diagnostics as they happen.
pub trait DiagSink {
    /// Records one diagnostic and says whether to keep going.
    fn emit(&mut self, d: Diagnostic) -> Flow;
}

/// How many examples a code bucket keeps.
pub const EXAMPLES_PER_CODE: usize = 20;

/// All diagnostics of one code seen in a run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeBucket {
    /// How many times, including deduplicated repeats.
    pub count: u64,
    /// The first [`EXAMPLES_PER_CODE`] occurrences.
    pub examples: Vec<Diagnostic>,
}

/// How one source (a file, or an entry within a zip) ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// Ingested, possibly with warnings.
    Ok,
    /// Already in the lake; nothing done.
    Skipped,
    /// Failed with the given code.
    Failed,
    /// Not attempted because the run stopped first.
    NotAttempted,
}

/// The outcome of one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceOutcome {
    /// URL or path, with entry when it is one entry of a zip.
    pub source: String,
    /// How it ended.
    pub status: SourceStatus,
    /// The code of the error that failed it, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_with: Option<Code>,
    /// Errors raised while processing it.
    pub errors: u32,
    /// Warnings raised while processing it.
    pub warnings: u32,
    /// Rows written.
    pub rows: u64,
    /// Rows quarantined.
    pub bad_rows: u64,
}

/// The aggregate of a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    /// Diagnostics grouped by code.
    pub by_code: BTreeMap<String, CodeBucket>,
    /// One outcome per source, in the order they finished.
    pub per_source: Vec<SourceOutcome>,
    /// Stop at the first error.
    #[serde(skip)]
    pub fail_fast: bool,
    /// Keys of warnings already counted, so a repeated warning is a count
    /// rather than a flood. See [`Report::dedup_key`].
    #[serde(skip)]
    seen_warnings: HashSet<String>,
    /// Whether `Flow::Stop` was returned.
    #[serde(skip)]
    stopped: bool,
}

impl Report {
    /// A report that attempts every source.
    pub fn new() -> Self {
        Self::default()
    }

    /// A report that stops at the first error.
    pub fn fail_fast() -> Self {
        Self {
            fail_fast: true,
            ..Self::default()
        }
    }

    /// Records a finished source.
    pub fn finish_source(&mut self, outcome: SourceOutcome) {
        self.per_source.push(outcome);
    }

    /// Total errors.
    #[cfg(test)]
    pub(crate) fn error_count(&self) -> u64 {
        self.by_code
            .values()
            .filter(|b| b.examples.first().is_some_and(Diagnostic::is_error))
            .map(|b| b.count)
            .sum()
    }

    /// Total warnings (deduplicated repeats counted once each).
    #[cfg(test)]
    pub(crate) fn warning_count(&self) -> u64 {
        self.by_code
            .values()
            .filter(|b| b.examples.first().is_some_and(|d| !d.is_error()))
            .map(|b| b.count)
            .sum()
    }

    /// Whether `Flow::Stop` was returned at some point.
    pub const fn stopped(&self) -> bool {
        self.stopped
    }

    /// Sources that failed.
    pub fn failed(&self) -> impl Iterator<Item = &SourceOutcome> {
        self.per_source
            .iter()
            .filter(|s| s.status == SourceStatus::Failed)
    }

    /// Sources that succeeded.
    pub fn succeeded(&self) -> usize {
        self.per_source
            .iter()
            .filter(|s| s.status == SourceStatus::Ok)
            .count()
    }

    /// Sources that were skipped as already ingested.
    pub fn skipped(&self) -> usize {
        self.per_source
            .iter()
            .filter(|s| s.status == SourceStatus::Skipped)
            .count()
    }

    /// The key under which a warning is deduplicated: the same code for the
    /// same table and version from any file counts once per file, so a
    /// drift that affects 161 files reads as one line with a count.
    fn dedup_key(d: &Diagnostic) -> String {
        format!(
            "{}|{}|{}|{}",
            d.code,
            d.table.as_deref().unwrap_or(""),
            d.version.map_or(String::new(), |v| v.to_string()),
            d.file.as_deref().unwrap_or("")
        )
    }

    /// Lines for a human summary: per code, the count and up to three
    /// example messages.
    pub fn summary_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (code, bucket) in &self.by_code {
            let sev = bucket
                .examples
                .first()
                .map_or(Severity::Error, |d| d.severity);
            out.push(format!("{sev} {code} ×{}", bucket.count));
            for d in bucket.examples.iter().take(3) {
                out.push(format!("    {}", d.render()));
            }
        }
        out
    }
}

impl DiagSink for Report {
    fn emit(&mut self, d: Diagnostic) -> Flow {
        if !d.is_error() {
            let key = Self::dedup_key(&d);
            if !self.seen_warnings.insert(key) {
                if let Some(b) = self.by_code.get_mut(d.code.as_ref()) {
                    b.count += 1;
                }
                return Flow::Continue;
            }
        }
        let bucket = self.by_code.entry(d.code.to_string()).or_default();
        bucket.count += 1;
        let is_error = d.is_error();
        if bucket.examples.len() < EXAMPLES_PER_CODE {
            bucket.examples.push(d);
        }
        if is_error && self.fail_fast {
            self.stopped = true;
            Flow::Stop
        } else {
            Flow::Continue
        }
    }
}

/// A sink that collects everything, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct VecSink(pub Vec<Diagnostic>);

#[cfg(test)]
impl DiagSink for VecSink {
    fn emit(&mut self, d: Diagnostic) -> Flow {
        self.0.push(d);
        Flow::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Category;

    fn err(code: &'static str) -> Diagnostic {
        Diagnostic::error(code, Category::CSV, "Bad.")
    }

    fn warn(code: &'static str, table: &str, file: &str) -> Diagnostic {
        Diagnostic::warning(code, Category::SCHEMA, "Drift.")
            .with_table(table)
            .with_version(2)
            .with_file(file)
    }

    #[test]
    fn groups_by_code_and_keeps_examples() {
        let mut r = Report::new();
        for _ in 0..25 {
            assert_eq!(r.emit(err("csv.field_count_mismatch")), Flow::Continue);
        }
        let b = &r.by_code["csv.field_count_mismatch"];
        assert_eq!(b.count, 25);
        assert_eq!(b.examples.len(), EXAMPLES_PER_CODE);
        assert_eq!(r.error_count(), 25);
        assert_eq!(r.warning_count(), 0);
        assert!(!r.stopped());
    }

    #[test]
    fn deduplicates_warnings_per_table_version_file() {
        let mut r = Report::new();
        for _ in 0..5 {
            r.emit(warn("schema.compatible_drift", "T", "a.zip"));
        }
        r.emit(warn("schema.compatible_drift", "T", "b.zip"));
        let b = &r.by_code["schema.compatible_drift"];
        assert_eq!(b.count, 6);
        assert_eq!(b.examples.len(), 2);
        assert_eq!(r.warning_count(), 6);
        let lines = r.summary_lines();
        assert!(lines[0].starts_with("warning schema.compatible_drift ×6"));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn fail_fast_stops_on_first_error_only() {
        let mut r = Report::fail_fast();
        assert_eq!(r.emit(warn("schema.x", "T", "f")), Flow::Continue);
        assert_eq!(r.emit(err("csv.x")), Flow::Stop);
        assert!(r.stopped());
    }

    #[test]
    fn outcomes_counted() {
        let mut r = Report::new();
        let mk = |status, failed_with| SourceOutcome {
            source: "s".into(),
            status,
            failed_with,
            errors: 0,
            warnings: 0,
            rows: 0,
            bad_rows: 0,
        };
        r.finish_source(mk(SourceStatus::Ok, None));
        r.finish_source(mk(SourceStatus::Skipped, None));
        r.finish_source(mk(
            SourceStatus::Failed,
            Some(Code::Borrowed("zip.bad_entry")),
        ));
        r.finish_source(mk(SourceStatus::NotAttempted, None));
        assert_eq!(r.succeeded(), 1);
        assert_eq!(r.skipped(), 1);
        assert_eq!(r.failed().count(), 1);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"not_attempted\""));
        let mut v = VecSink::default();
        v.emit(err("csv.x"));
        assert_eq!(v.0.len(), 1);
    }
}
