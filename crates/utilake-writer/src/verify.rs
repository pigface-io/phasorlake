//! `verify`: the lake against the manifest. Every output an ingest lists
//! exists with the size recorded for it, and every file under quarantine
//! belongs to a current ingest.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ManifestError;
use crate::manifest::ManifestHandle;

/// One thing wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    /// The file.
    pub path: String,
    /// The diagnostic code: `verify.size_mismatch`, `verify.missing_file`,
    /// `verify.orphan_quarantine`.
    pub code: String,
    /// One sentence.
    pub problem: String,
}

/// What `verify` found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyReport {
    /// Outputs checked.
    pub files: usize,
    /// Everything wrong, in the order found.
    pub problems: Vec<Problem>,
}

impl VerifyReport {
    /// Whether nothing is wrong.
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Files under `dir`, at any depth, whose path is not in `live`, sorted.
fn orphans_under(dir: &Path, live: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let s = p.display().to_string();
                if !live.contains(&s) {
                    found.push(s);
                }
            }
        }
    }
    found.sort();
    found
}

/// Checks the outputs of every ingest (of one table, when `table` is
/// given) against the disk, and the quarantine directory `quarantine`
/// against the files current ingests recorded. The manifest is read once;
/// the lake is never written.
pub async fn verify_lake(
    manifest: &ManifestHandle,
    quarantine: &Path,
    table: Option<&str>,
) -> Result<VerifyReport, ManifestError> {
    let ingests = manifest
        .list_ingests(table.map(str::to_owned), false, 100_000)
        .await?;
    let live = manifest.live_quarantine().await?;
    let mut report = VerifyReport::default();
    for i in &ingests {
        for o in &i.outputs {
            report.files += 1;
            match std::fs::metadata(Path::new(&o.path)) {
                Ok(m) if m.len() == o.bytes => {}
                Ok(m) => report.problems.push(Problem {
                    path: o.path.clone(),
                    code: "verify.size_mismatch".into(),
                    problem: format!("manifest says {} bytes, disk has {}", o.bytes, m.len()),
                }),
                Err(_) => report.problems.push(Problem {
                    path: o.path.clone(),
                    code: "verify.missing_file".into(),
                    problem: "listed in the manifest but not on disk".into(),
                }),
            }
        }
    }
    // A quarantine file no current ingest recorded was left by a read that
    // has since been superseded: its rows are in the lake or were read again.
    let qdir = match table {
        Some(t) => quarantine.join(t.to_ascii_lowercase()),
        None => quarantine.to_path_buf(),
    };
    report
        .problems
        .extend(orphans_under(&qdir, &live).into_iter().map(|p| Problem {
            path: p,
            code: "verify.orphan_quarantine".into(),
            problem: "no current ingest recorded this quarantine file; it is safe to delete".into(),
        }));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileRecord, IngestEnd, IngestStart, IngestStatus};
    use crate::writer::{Partition, Transform, WrittenFile};

    #[tokio::test]
    async fn finds_missing_and_resized_outputs_and_orphan_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let (m, thread) = ManifestHandle::spawn(&dir.path().join("m.sqlite")).unwrap();
        let run = m.begin_run("ingest x").await.unwrap();
        let file_id = m
            .record_file(FileRecord {
                source: "local".into(),
                url: "file:///x.csv".into(),
                cache_path: "/x.csv".into(),
                sha256: "sha".into(),
                size: 1,
                etag: None,
                last_modified: None,
                last_modified_at: None,
                fetched_at: None,
            })
            .await
            .unwrap();
        let good = dir.path().join("good.parquet");
        std::fs::write(&good, b"12345").unwrap();
        let short = dir.path().join("short.parquet");
        std::fs::write(&short, b"12").unwrap();
        let gone = dir.path().join("gone.parquet");
        let quarantine = dir.path().join("quarantine");
        let kept = quarantine.join("region").join("kept.jsonl");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, b"{}").unwrap();
        let orphan = quarantine.join("region").join("orphan.jsonl");
        std::fs::write(&orphan, b"{}").unwrap();
        let output = |p: &Path, bytes: u64| WrittenFile {
            path: p.display().to_string(),
            partition: Partition::Unknown,
            rows: 1,
            bytes,
            min_time: None,
            max_time: None,
            sorted: true,
        };
        let id = m
            .begin_ingest(IngestStart {
                run_id: run,
                file_id,
                entry: "x.csv".into(),
                table: "REGION".into(),
                schema_version: 1,
                fingerprint: "xxh3:1".into(),
                transform: Transform::default(),
            })
            .await
            .unwrap();
        m.finish_ingest(
            id,
            IngestEnd {
                status: IngestStatus::Ok,
                rows: 3,
                bad_rows: 1,
                quarantine_path: Some(kept.display().to_string()),
                outputs: vec![output(&good, 5), output(&short, 5), output(&gone, 5)],
                error_code: None,
            },
        )
        .await
        .unwrap();
        let r = verify_lake(&m, &quarantine, None).await.unwrap();
        assert_eq!(r.files, 3);
        let codes: Vec<&str> = r.problems.iter().map(|p| p.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "verify.size_mismatch",
                "verify.missing_file",
                "verify.orphan_quarantine"
            ]
        );
        assert!(
            r.problems[0]
                .problem
                .contains("manifest says 5 bytes, disk has 2")
        );
        assert!(r.problems[2].path.ends_with("orphan.jsonl"));
        assert!(!r.is_clean());
        // One table's view looks only under its quarantine directory.
        let r = verify_lake(&m, &quarantine, Some("REGION")).await.unwrap();
        assert_eq!(r.problems.len(), 3);
        let r = verify_lake(&m, &quarantine, Some("OTHER")).await.unwrap();
        assert_eq!((r.files, r.problems.len()), (0, 0));
        assert!(r.is_clean());
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"problems\":[]"));
        drop(m);
        thread.join();
    }
}
