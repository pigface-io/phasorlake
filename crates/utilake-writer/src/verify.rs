//! `verify`: the lake against the manifest. Every output an ingest lists
//! exists with the size recorded for it, its rows of each source (the
//! count its footer keeps) add up to what the manifest says each entry
//! gave, and every file under quarantine belongs to a current ingest.

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
    /// `verify.sources_mismatch`, `verify.orphan_quarantine`.
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

/// The rows of each source a lake file's footer counts (`utilake.sources`);
/// none when the file cannot be read or has no count.
fn footer_sources(path: &Path) -> Option<Vec<(String, u64)>> {
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let reader = SerializedFileReader::new(std::fs::File::open(path).ok()?).ok()?;
    let text = reader
        .metadata()
        .file_metadata()
        .key_value_metadata()?
        .iter()
        .find(|k| k.key == "utilake.sources")?
        .value
        .clone()?;
    let v: Vec<serde_json::Value> = serde_json::from_str(&text).ok()?;
    v.iter()
        .map(|s| Some((s["source"].as_str()?.to_owned(), s["rows"].as_u64()?)))
        .collect()
}

/// `url!entry`, or the URL alone for a file read whole.
fn source_name(url: &str, entry: &str) -> String {
    if entry.is_empty() {
        url.to_owned()
    } else {
        format!("{url}!{entry}")
    }
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
        // What its files say each source gave, against what the manifest
        // says: only when every file is there to say it.
        let mut counted: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        let mut readable = !i.outputs.is_empty();
        for o in &i.outputs {
            report.files += 1;
            match std::fs::metadata(Path::new(&o.path)) {
                Ok(m) if m.len() == o.bytes => match footer_sources(Path::new(&o.path)) {
                    Some(sources) => {
                        for (s, n) in sources {
                            *counted.entry(s).or_default() += n;
                        }
                    }
                    None => readable = false,
                },
                Ok(m) => {
                    readable = false;
                    report.problems.push(Problem {
                        path: o.path.clone(),
                        code: "verify.size_mismatch".into(),
                        problem: format!("manifest says {} bytes, disk has {}", o.bytes, m.len()),
                    });
                }
                Err(_) => {
                    readable = false;
                    report.problems.push(Problem {
                        path: o.path.clone(),
                        code: "verify.missing_file".into(),
                        problem: "listed in the manifest but not on disk".into(),
                    });
                }
            }
        }
        if readable {
            let said: std::collections::BTreeMap<String, u64> = i
                .entries
                .iter()
                .filter(|e| e.rows > 0)
                .map(|e| (source_name(&i.url, &e.entry), e.rows))
                .collect();
            if said != counted {
                report.problems.push(Problem {
                    path: i.outputs[0].path.clone(),
                    code: "verify.sources_mismatch".into(),
                    problem: format!(
                        "the files of {} {} hold {} rows of {} sources; the manifest says {} rows of {}",
                        i.table,
                        i.url,
                        counted.values().sum::<u64>(),
                        counted.len(),
                        said.values().sum::<u64>(),
                        said.len()
                    ),
                });
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
                entries: vec![crate::manifest::EntryShare {
                    entry: "x.csv".into(),
                    rows: 3,
                    bad_rows: 1,
                    ..Default::default()
                }],
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

    #[tokio::test]
    async fn a_files_sources_must_add_up_to_what_each_entry_gave() {
        use std::sync::Arc;

        use arrow_array::{Int64Array, RecordBatch};
        use arrow_schema::{DataType, Field, Schema};

        use crate::LakeWriter;
        use crate::manifest::EntryShare;
        use crate::provenance::{Stamp, lake_schema, stamped};
        use crate::writer::{FileMeta, PartitionKey};

        let dir = tempfile::tempdir().unwrap();
        let (m, thread) = ManifestHandle::spawn(&dir.path().join("m.sqlite")).unwrap();
        let run = m.begin_run("ingest z").await.unwrap();
        let url = "https://h/z.zip";
        let file_id = m
            .record_file(FileRecord {
                source: "local".into(),
                url: url.into(),
                cache_path: "/z.zip".into(),
                sha256: "abcdef01".into(),
                size: 1,
                etag: None,
                last_modified: None,
                last_modified_at: None,
                fetched_at: None,
            })
            .await
            .unwrap();
        // One file holding two entries' rows: 2 of A, 1 of B.
        let data = Schema::new(vec![Field::new("X", DataType::Int64, true)]);
        let lake = lake_schema(&data);
        let mut hive = crate::ParquetHive::new(dir.path().join("lake"));
        let key = PartitionKey {
            market: "nem".into(),
            table_dir: "t".into(),
            partition: Partition::Unknown,
        };
        let meta = FileMeta {
            table: "T".into(),
            source_stem: "z".into(),
            schema_version: 1,
            fingerprint: utilake_core::Fingerprint(1),
            source_url: url.into(),
            source_sha256: "abcdef01".into(),
            time_column: None,
            part: 1,
            transform: Transform::default(),
        };
        let w = hive.open(&key, lake.clone(), &meta).unwrap();
        for (entry, n) in [("A.CSV", 2), ("B.CSV", 1)] {
            let rows = RecordBatch::try_new(
                Arc::new(data.clone()),
                vec![Arc::new(Int64Array::from(vec![1; n]))],
            )
            .unwrap();
            let stamp = Stamp::new(&format!("{url}!{entry}"), None);
            hive.write(w, &stamped(&rows, &stamp, &lake).unwrap())
                .unwrap();
        }
        let written = hive.close(w).unwrap();
        hive.commit().unwrap();
        let share = |entry: &str, rows| EntryShare {
            entry: entry.into(),
            rows,
            ..Default::default()
        };
        let record = |table: &str, entries: Vec<EntryShare>| {
            let m = &m;
            let written = written.clone();
            let table = table.to_owned();
            async move {
                let id = m
                    .begin_ingest(IngestStart {
                        run_id: run,
                        file_id,
                        table,
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
                        bad_rows: 0,
                        quarantine_path: None,
                        outputs: vec![written],
                        entries,
                        error_code: None,
                    },
                )
                .await
                .unwrap();
            }
        };
        record("T", vec![share("A.CSV", 2), share("B.CSV", 1)]).await;
        let quarantine = dir.path().join("q");
        let r = verify_lake(&m, &quarantine, None).await.unwrap();
        assert!(r.is_clean(), "{:?}", r.problems);
        // The manifest says otherwise of B: the counts disagree.
        record("T", vec![share("A.CSV", 2), share("B.CSV", 5)]).await;
        let r = verify_lake(&m, &quarantine, None).await.unwrap();
        let codes: Vec<&str> = r.problems.iter().map(|p| p.code.as_str()).collect();
        assert_eq!(codes, ["verify.sources_mismatch"]);
        assert!(
            r.problems[0].problem.contains("3 rows of 2 sources"),
            "{}",
            r.problems[0].problem
        );
        drop(m);
        thread.join();
    }
}
