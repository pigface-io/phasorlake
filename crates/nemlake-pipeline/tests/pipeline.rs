//! The pipeline end to end over fixture files: local zips, a mocked remote
//! source, idempotent re-runs, failures that do not stop the run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nemlake_acquire::cache::DiskCache;
use nemlake_acquire::transport::{Canned, StaticTransport};
use nemlake_acquire::{FetchError, Fetcher, FetcherConfig, FileKind, Query, RemoteFile, Source};
use nemlake_core::event::Event;
use nemlake_core::paths::Home;
use nemlake_pipeline::{Deps, Input, Plan, RunOptions, run};
use nemlake_registry::{Mode, Registry, TypeOptions};
use nemlake_writer::manifest::DiagFilter;
use nemlake_writer::{Manifest, ManifestHandle, ManifestThread};
use tokio_util::sync::CancellationToken;
use url::Url;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/csv")
}

fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let f = std::fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    for (name, bytes) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap();
}

struct Env {
    _dir: tempfile::TempDir,
    home: Home,
    deps: Deps,
    transport: Arc<StaticTransport>,
    _thread: ManifestThread,
}

fn env() -> Env {
    env_with(TypeOptions::default())
}

fn env_with(types: TypeOptions) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_dirs().unwrap();
    let (manifest, thread) = ManifestHandle::spawn_with(Manifest::open(&home.manifest()).unwrap());
    let transport = Arc::new(StaticTransport::default());
    let fetcher = Fetcher::new(
        Arc::new(transport.clone()),
        DiskCache::new(home.cache()),
        FetcherConfig {
            attempts: 2,
            backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
            min_interval: Duration::ZERO,
            ..Default::default()
        },
    );
    let deps = Deps {
        registry: Registry::load_embedded(types),
        fetcher: Arc::new(fetcher),
        manifest,
        home: home.clone(),
    };
    Env {
        _dir: dir,
        home,
        deps,
        transport,
        _thread: thread,
    }
}

async fn run_plan(e: &Env, plan: Plan) -> (nemlake_pipeline::RunReport, Vec<Event>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let rep = run(plan, e.deps.clone(), tx, CancellationToken::new()).await;
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    (rep, events)
}

fn parquet_rows(path: &str) -> usize {
    let f = std::fs::File::open(path).unwrap();
    let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
    b.build().unwrap().map(|r| r.unwrap().num_rows()).sum()
}

/// With decimals on, a NUMBER(p,s) column is written as Decimal128(p,s)
/// holding exactly the digits AEMO published; the default is Float64.
#[tokio::test]
async fn decimal_mode_writes_exact_decimals() {
    use arrow_array::Array;
    let csv = fixtures().join("DISPATCHPRICE_v4_head.csv");
    let read = |e: &Env| {
        let files = walk(&e.home.lake());
        assert_eq!(files.len(), 1, "{files:?}");
        let f = std::fs::File::open(&files[0]).unwrap();
        let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
        let schema = b.schema().clone();
        let batch = b.build().unwrap().next().unwrap().unwrap();
        (schema, batch)
    };
    let plan = || Plan {
        request: "ingest price".into(),
        input: Input::Local(vec![csv.clone()]),
        options: RunOptions::default(),
    };

    let exact = env_with(TypeOptions { decimal: true });
    let (rep, _) = run_plan(&exact, plan()).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let (schema, batch) = read(&exact);
    let rrp = schema.field_with_name("RRP").unwrap();
    assert_eq!(rrp.data_type(), &arrow_schema::DataType::Decimal128(15, 5));
    let col = batch
        .column_by_name("RRP")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::Decimal128Array>()
        .unwrap();
    // The first D row of the fixture publishes RRP as 57.98.
    assert_eq!(col.value_as_string(0), "57.98000");
    assert_eq!(col.value(0), 5_798_000);
    assert_eq!(col.null_count(), 0);

    let floats = env();
    let (rep, _) = run_plan(&floats, plan()).await;
    assert_eq!(rep.exit_code, 0);
    let (schema, batch) = read(&floats);
    assert_eq!(
        schema.field_with_name("RRP").unwrap().data_type(),
        &arrow_schema::DataType::Float64
    );
    let col = batch
        .column_by_name("RRP")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::Float64Array>()
        .unwrap();
    assert!((col.value(0) - 57.98).abs() < 1e-12);
    // Both modes carry the same column id, so a reader can line them up.
    assert_eq!(batch.num_rows(), 20);
}

/// An appended table with no time column of its own is partitioned by the
/// month of the file its rows came in.
#[tokio::test]
async fn a_table_without_a_time_column_is_partitioned_by_its_file_month() {
    let mut e = env();
    // Such a table, laid over the embedded registry as a user's would be.
    let cols = ["CONTRACTYEAR", "WEEKNO"];
    let irow = nemlake_core::IRow {
        report_type: "NEMLAKE_TEST",
        report_subtype: "NOTIME",
        version: 1,
        columns: &cols,
    };
    let table = nemlake_registry::infer::draft_table(
        nemlake_registry::Market::Nem,
        &irow,
        &nemlake_registry::infer::Sampler::new(2),
    );
    assert_eq!(table.time_column, None);
    let overlay = e.home.root().join("overlay/nem/mms");
    std::fs::create_dir_all(&overlay).unwrap();
    std::fs::write(
        overlay.join("NEMLAKE_TEST_NOTIME.toml"),
        nemlake_registry::file_model::file::write_table(&table),
    )
    .unwrap();
    e.deps.registry = Arc::new(
        e.deps
            .registry
            .with_overlay(&e.home.root().join("overlay"))
            .unwrap(),
    );
    let csv = "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,NOTIME,1,CONTRACTYEAR,WEEKNO\nD,NEMLAKE_TEST,NOTIME,1,2024,9\nD,NEMLAKE_TEST,NOTIME,1,2024,10\nC,\"END OF REPORT\",5\n";
    let dated = e.home.root().join("PUBLIC_DVD_NOTIME_202403010000.CSV");
    let undated = e.home.root().join("notime.csv");
    std::fs::write(&dated, csv).unwrap();
    std::fs::write(&undated, csv).unwrap();
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest notime".into(),
            input: Input::Local(vec![dated, undated]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.rows, 4);
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(
        files.iter().any(|f| f.contains(
            "nem/nemlake_test_notime/year=2024/month=03/public_dvd_notime_202403010000-v1-"
        )),
        "{files:?}"
    );
    // A file whose name says no month has nowhere better to go.
    assert!(
        files
            .iter()
            .any(|f| f.contains("__HIVE_DEFAULT_PARTITION__") && f.contains("/notime-v1-")),
        "{files:?}"
    );
}

#[tokio::test]
async fn local_zip_ingests_idempotently_and_records_manifest() {
    let e = env();
    let zip_path = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip");
    make_zip(
        &zip_path,
        &[(
            "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV",
            &std::fs::read(fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv")).unwrap(),
        )],
    );
    let plan = || Plan {
        request: "ingest scada.zip".into(),
        input: Input::Local(vec![zip_path.clone()]),
        options: RunOptions::default(),
    };
    let (rep, events) = run_plan(&e, plan()).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.ingested, 1);
    assert_eq!(rep.summary.rows, 20);
    assert_eq!(rep.summary.listed, 1);
    assert_eq!(rep.report.succeeded(), 1);
    assert!(events.iter().any(|ev| matches!(ev, Event::BlockResolved { table, status, .. } if table == "DISPATCH_UNIT_SCADA" && status == "exact")));
    assert!(events.iter().any(|ev| matches!(ev, Event::Summary(_))));
    let files: Vec<_> = walk(&e.home.lake());
    assert_eq!(
        files.len(),
        1,
        "lake {} holds {files:?}; home holds {:?}",
        e.home.lake().display(),
        walk(e.home.root())
    );
    assert!(files[0].contains("nem/dispatch_unit_scada/year=2024/month=01/public_dvd_dispatch_unit_scada_202401010000-v1-"), "{}", files[0]);
    assert_eq!(parquet_rows(&files[0]), 20);
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert_eq!(ingests.len(), 1);
    assert_eq!(ingests[0].rows, 20);
    assert_eq!(ingests[0].outputs.len(), 1);
    assert_eq!(
        e.deps
            .manifest
            .latest_run()
            .await
            .unwrap()
            .unwrap()
            .exit_code,
        Some(0)
    );
    let mtime = std::fs::metadata(&files[0]).unwrap().modified().unwrap();
    // Second run: skipped, nothing rewritten.
    let (rep2, _) = run_plan(&e, plan()).await;
    assert_eq!(rep2.exit_code, 0);
    assert_eq!(rep2.summary.skipped, 1);
    assert_eq!(rep2.summary.ingested, 0);
    assert_eq!(rep2.report.skipped(), 1);
    assert_eq!(
        std::fs::metadata(&files[0]).unwrap().modified().unwrap(),
        mtime
    );
    assert_eq!(walk(&e.home.lake()).len(), 1);
    // Changed source: superseded and replaced.
    make_zip(
        &zip_path,
        &[
            (
                "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV",
                &std::fs::read(fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv")).unwrap(),
            ),
            ("extra.txt", b"x"),
        ],
    );
    let (rep3, _) = run_plan(&e, plan()).await;
    assert_eq!(rep3.exit_code, 0);
    assert_eq!(rep3.summary.ingested, 1);
    assert!(
        rep3.report.by_code.contains_key("manifest.superseded"),
        "{:?}",
        rep3.report.by_code.keys()
    );
    assert_eq!(
        walk(&e.home.lake()).len(),
        1,
        "old file replaced, not duplicated"
    );
}

fn footer(path: &str, key: &str) -> String {
    let f = std::fs::File::open(path).unwrap();
    let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
    b.metadata()
        .file_metadata()
        .key_value_metadata()
        .and_then(|kv| kv.iter().find(|k| k.key == key))
        .and_then(|k| k.value.clone())
        .unwrap_or_default()
}

/// Every output records the shape it was read from and the rules it was
/// read by. A source already in the lake is skipped while those rules
/// hold, and read again, from disk, once they have changed.
#[tokio::test]
async fn a_source_is_read_again_when_its_rules_change() {
    let e = env();
    let csv = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCHPRICE_202401010000.CSV");
    std::fs::copy(fixtures().join("DISPATCHPRICE_v4_head.csv"), &csv).unwrap();
    let floats = e.deps.clone();
    let decimals = Deps {
        registry: Registry::load_embedded(TypeOptions { decimal: true }),
        ..e.deps.clone()
    };
    let go = |deps: &Deps, input: Input, dry_run: bool| {
        let deps = deps.clone();
        async move {
            let (tx, _rx) = tokio::sync::mpsc::channel(4096);
            run(
                Plan {
                    request: "test".into(),
                    input,
                    options: RunOptions {
                        dry_run,
                        ..Default::default()
                    },
                },
                deps,
                tx,
                CancellationToken::new(),
            )
            .await
        }
    };
    let local = || Input::Local(vec![csv.clone()]);

    // First read: the transform is in the manifest and in the footer.
    let rep = go(&floats, local(), false).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert_eq!(ingests.len(), 1);
    let first = ingests[0].transform.clone();
    assert!(
        first.plan_hash.starts_with("plan:") && first.plan_hash.len() == 37,
        "{first:?}"
    );
    assert_eq!(first.schema_source, "registry");
    assert_eq!(first.resolution, "exact");
    assert_eq!(
        (first.report_type.as_str(), first.report_subtype.as_str()),
        ("DISPATCH", "PRICE")
    );
    assert_eq!(first.source_version, 4);
    assert_eq!(first.source_columns[0], "SETTLEMENTDATE");
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 1);
    assert_eq!(footer(&files[0], "nemlake.plan"), first.plan_hash);
    assert_eq!(footer(&files[0], "nemlake.schema_source"), "registry");
    assert_eq!(footer(&files[0], "nemlake.resolution"), "exact");
    assert_eq!(footer(&files[0], "nemlake.report_type"), "DISPATCH");
    assert_eq!(footer(&files[0], "nemlake.report_subtype"), "PRICE");
    assert_eq!(footer(&files[0], "nemlake.source_version"), "4");
    assert_eq!(
        footer(&files[0], "nemlake.source_columns"),
        first.source_columns.join(",")
    );

    // Same rules: skipped, and nothing is behind.
    let rep = go(&floats, local(), false).await;
    assert_eq!((rep.summary.skipped, rep.summary.ingested), (1, 0));
    let rep = go(&floats, Input::Stale, false).await;
    assert_eq!(rep.summary.listed, 0);
    assert_eq!(rep.exit_code, 0);

    // Other rules (decimal types): the same source is read again and its
    // Parquet file replaced, not duplicated.
    let rep = go(&decimals, Input::Stale, true).await;
    assert_eq!(rep.summary.listed, 1, "a dry run lists what is behind");
    assert_eq!(rep.listed[0].url.scheme(), "file");
    assert_eq!(
        footer(&files[0], "nemlake.plan"),
        first.plan_hash,
        "and writes nothing"
    );
    let rep = go(&decimals, local(), false).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!((rep.summary.skipped, rep.summary.ingested), (0, 1));
    assert!(
        rep.report.by_code.contains_key("manifest.rules_changed"),
        "{:?}",
        rep.report.by_code.keys()
    );
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert_eq!(ingests.len(), 1);
    let second = ingests[0].transform.clone();
    assert_ne!(second.plan_hash, first.plan_hash);
    assert_eq!(second.source_columns, first.source_columns);
    assert_eq!(walk(&e.home.lake()), files);
    assert_eq!(footer(&files[0], "nemlake.plan"), second.plan_hash);
    let rep = go(&decimals, Input::Stale, false).await;
    assert_eq!(rep.summary.listed, 0);

    // Back to the first rules through the manifest alone: the same hash
    // as the first read gave.
    let rep = go(&floats, Input::Stale, false).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!((rep.summary.listed, rep.summary.ingested), (1, 1));
    assert!(rep.report.by_code.contains_key("manifest.rules_changed"));
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert_eq!(ingests[0].transform, first);
    assert_eq!(footer(&files[0], "nemlake.plan"), first.plan_hash);

    // A source that is gone cannot be read again, and the run says so.
    std::fs::remove_file(&csv).unwrap();
    let rep = go(&decimals, Input::Stale, false).await;
    assert_eq!(rep.summary.listed, 1);
    assert_eq!(rep.summary.failed, 1);
    assert!(
        rep.report.by_code.contains_key("cache.source_missing"),
        "{:?}",
        rep.report.by_code.keys()
    );
    assert_eq!(walk(&e.home.lake()), files, "what is in the lake stays");
}

/// A person's rule for an interval: the files of those months have a
/// column read into another lake column, as that column's type. The same
/// table, the same column list, read two ways by the month of the source,
/// and the lake says which.
#[tokio::test]
async fn a_rule_for_an_interval_reads_its_files_another_way() {
    use arrow_array::Array;
    let e = env();
    let columns = ["SETTLEMENTDATE", "UNIT", "VALUE"];
    let table = format!(
        r#"schema_format = 1
market = "nem"
source = "mms"
name = "NEMLAKE_TEST_RULED"
package = "TEST"
visibility = "public"
report_pairs = [["NEMLAKE_TEST", "RULED"]]
load_mode = "append"
dedupe = "none"
time_column = "SETTLEMENTDATE"
next_column_id = 5

[[column]]
id = 1
name = "SETTLEMENTDATE"
type = "timestamp"
added_in = 1

[[column]]
id = 2
name = "UNIT"
type = "utf8"
added_in = 1

[[column]]
id = 3
name = "VALUE"
type = "number"
added_in = 1

[[column]]
id = 4
name = "VALUE_TEXT"
type = "utf8"
added_in = 1
src = ["manual"]

[[version]]
n = 1
columns = ["SETTLEMENTDATE", "UNIT", "VALUE"]
fingerprint = "{}"

[[rule]]
note = "Until 2011 the value was published with its unit."
to = "2011-12"

[[rule.column]]
source = "VALUE"
column = "VALUE_TEXT"
"#,
        nemlake_core::Fingerprint::of_names(columns)
    );
    let schemas = e.home.root().join("schemas/nem/mms");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::write(schemas.join("NEMLAKE_TEST_RULED.toml"), table).unwrap();
    let registry = Arc::new(
        e.deps
            .registry
            .with_overlay(&e.home.root().join("schemas"))
            .unwrap(),
    );
    let deps = Deps {
        registry,
        ..e.deps.clone()
    };
    let file = |name: &str, stamp: &str, value: &str| {
        let p = e.home.root().join(name);
        std::fs::write(
            &p,
            format!(
                "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,RULED,1,SETTLEMENTDATE,UNIT,VALUE\nD,NEMLAKE_TEST,RULED,1,\"{stamp}\",A1,{value}\nC,\"END OF REPORT\",4\n"
            ),
        )
        .unwrap();
        p
    };
    let old = file(
        "PUBLIC_DVD_RULED_201003010000.CSV",
        "2010/03/01 00:05:00",
        "1.5 MW",
    );
    let new = file(
        "PUBLIC_DVD_RULED_202403010000.CSV",
        "2024/03/01 00:05:00",
        "2.5",
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    let rep = run(
        Plan {
            request: "ingest ruled".into(),
            input: Input::Local(vec![old, new]),
            options: RunOptions {
                mode: nemlake_registry::Mode::Strict,
                ..Default::default()
            },
        },
        deps.clone(),
        tx,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.bad_rows, 0);

    let read = |month: &str| {
        let files: Vec<String> = walk(&e.home.lake())
            .into_iter()
            .filter(|p| p.contains(month))
            .collect();
        assert_eq!(files.len(), 1, "{month}: {files:?}");
        let f = std::fs::File::open(&files[0]).unwrap();
        let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
        let batch = b.build().unwrap().next().unwrap().unwrap();
        let number = batch
            .column_by_name("VALUE")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow_array::Float64Array>()
            .unwrap()
            .clone();
        let text = batch
            .column_by_name("VALUE_TEXT")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap()
            .clone();
        (files[0].clone(), number, text)
    };
    // 2010: inside the interval. The value is kept as published, as text.
    let (path, number, text) = read("year=2010");
    assert!(number.is_null(0));
    assert_eq!(text.value(0), "1.5 MW");
    assert_eq!(footer(&path, "nemlake.resolution"), "exact+rule");
    assert_eq!(footer(&path, "nemlake.source_period"), "2010-03");
    let old_rules = footer(&path, "nemlake.plan");
    // 2024: outside it. The value is a number.
    let (path, number, text) = read("year=2024");
    assert!((number.value(0) - 2.5).abs() < 1e-12);
    assert!(text.is_null(0));
    assert_eq!(footer(&path, "nemlake.resolution"), "exact");
    assert_eq!(footer(&path, "nemlake.source_period"), "2024-03");
    assert_ne!(footer(&path, "nemlake.plan"), old_rules);

    // The manifest has the same, and with it both sources are current.
    let ingests = deps.manifest.list_ingests(None, false, 10).await.unwrap();
    let by_period = |p: &str| {
        ingests
            .iter()
            .find(|i| i.transform.source_period.as_deref() == Some(p))
            .unwrap()
    };
    assert_eq!(by_period("2010-03").transform.resolution, "exact+rule");
    assert_eq!(by_period("2024-03").transform.resolution, "exact");
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    let rep = run(
        Plan {
            request: "reingest".into(),
            input: Input::Stale,
            options: RunOptions::default(),
        },
        deps.clone(),
        tx,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(rep.summary.listed, 0, "nothing is behind its rules");
    // Under the registry without the rule's table the same sources are
    // behind: their table is gone and they would be read from the file.
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    let rep = run(
        Plan {
            request: "reingest".into(),
            input: Input::Stale,
            options: RunOptions {
                dry_run: true,
                ..Default::default()
            },
        },
        e.deps.clone(),
        tx,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(rep.summary.listed, 2);
}

/// A source whose rows wander over more months than a worker may hold
/// open writes each month in parts: a writer is closed to make room and a
/// further file is started when rows for its month come again. Every row
/// lands, no two files share a name, and a failed entry leaves nothing.
#[tokio::test]
async fn rows_that_wander_over_many_months_are_written_in_parts() {
    assert_eq!(nemlake_pipeline::ingest_file::open_writers(4), 32);
    assert_eq!(nemlake_pipeline::ingest_file::open_writers(0), 128);
    assert_eq!(nemlake_pipeline::ingest_file::open_writers(64), 8);

    let e = env();
    // Twelve months, visited three times over, with sixteen workers
    // allowed: eight writers open at once.
    let wander = |last: &str| {
        let mut csv = String::from(
            "C,NEMP.WORLD,TEST\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n",
        );
        for pass in 0..3 {
            for month in 1..=12 {
                use std::fmt::Write as _;
                let _ = writeln!(
                    csv,
                    "D,DISPATCH,UNIT_SCADA,1,\"2023/{month:02}/0{} 00:05:00\",U{pass},{month}",
                    pass + 1
                );
            }
        }
        csv.push_str(last);
        csv
    };
    let run_it = |name: &str, body: String| {
        let p = e.home.root().join(name);
        std::fs::write(&p, body).unwrap();
        let deps = e.deps.clone();
        async move {
            let (tx, _rx) = tokio::sync::mpsc::channel(4096);
            run(
                Plan {
                    request: "ingest wander".into(),
                    input: Input::Local(vec![p]),
                    options: RunOptions {
                        jobs: 16,
                        // Small batches, so rows reach the writers month
                        // by month as they would from a large file.
                        batch_rows: 4,
                        ..Default::default()
                    },
                },
                deps,
                tx,
                CancellationToken::new(),
            )
            .await
        }
    };
    let rep = run_it("PUBLIC_DVD_WANDER_202401010000.CSV", wander("")).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.rows, 36);
    let files = walk(&e.home.lake());
    assert!(files.len() > 12, "more files than months: {}", files.len());
    assert!(files.iter().any(|f| f.contains("-p2.parquet")), "{files:?}");
    assert!(files.iter().all(|f| !f.contains(".tmp-")), "{files:?}");
    assert_eq!(files.iter().map(|f| parquet_rows(f)).sum::<usize>(), 36);
    // Each month still holds its three rows, over however many files.
    for month in 1..=12 {
        let dir = format!("year=2023/month={month:02}/");
        let rows: usize = files
            .iter()
            .filter(|f| f.contains(&dir))
            .map(|f| parquet_rows(f))
            .sum();
        assert_eq!(rows, 3, "{dir}");
    }
    // The manifest lists every part.
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert_eq!(ingests.len(), 1);
    assert_eq!(ingests[0].outputs.len(), files.len());
    // The same source again is skipped: every part is still there.
    let rep = run_it("PUBLIC_DVD_WANDER_202401010000.CSV", wander("")).await;
    assert_eq!(rep.summary.skipped, 1);

    // An entry refused at its last row leaves no file behind, though
    // writers were closed and their files finished along the way.
    let other = env();
    let p = other.home.root().join("PUBLIC_DVD_WANDER_202401010000.CSV");
    std::fs::write(
        &p,
        wander("D,DISPATCH,UNIT_SCADA,1,\"2023/01/01 00:05:00\",U9,1,extra\n"),
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    let rep = run(
        Plan {
            request: "ingest wander".into(),
            input: Input::Local(vec![p]),
            options: RunOptions {
                jobs: 16,
                batch_rows: 4,
                ..Default::default()
            },
        },
        other.deps.clone(),
        tx,
        CancellationToken::new(),
    )
    .await;
    assert_ne!(rep.exit_code, 0);
    assert_eq!(walk(&other.home.lake()), Vec::<String>::new());
    // The refusal names the row's exact fault, and the row's own error
    // is reported beside it.
    let refusal = &rep.report.by_code["csv.too_many_bad_rows"].examples[0];
    assert_eq!(refusal.observed.as_deref(), Some("4 fields"));
    assert_eq!(refusal.line, Some(39));
    assert!(refusal.message.contains("line 39"), "{}", refusal.message);
    let row = &rep.report.by_code["csv.field_count_mismatch"].examples[0];
    assert!(row.is_error());
    assert_eq!(row.table.as_deref(), Some("DISPATCH_UNIT_SCADA"));
    assert!(row.sample.is_some());
}

/// Reads every row of a Parquet file as one batch.
fn parquet_batch(path: &str) -> arrow_array::RecordBatch {
    let f = std::fs::File::open(path).unwrap();
    let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
    let schema = b.schema().clone();
    let batches: Vec<_> = b.build().unwrap().map(Result::unwrap).collect();
    arrow_select::concat::concat_batches(&schema, &batches).unwrap()
}

/// The text of a column of strings, null as `None`.
fn texts(b: &arrow_array::RecordBatch, name: &str) -> Vec<Option<String>> {
    use arrow_array::Array as _;
    let a = b
        .column_by_name(name)
        .unwrap_or_else(|| panic!("no column {name} in {:?}", b.schema()))
        .as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .unwrap();
    (0..a.len())
        .map(|i| (!a.is_null(i)).then(|| a.value(i).to_owned()))
        .collect()
}

/// The August 2026 head of DUDETAIL, its first REGISTEREDCAPACITY (declared
/// NUMBER(6,0)) changed from 500 to 13.55, zipped as an archive file.
fn dudetail_with_a_fraction(e: &Env) -> PathBuf {
    let head =
        std::fs::read_to_string(fixtures().join(
            "../bootstrap/MMSDM_2026_08/heads/PUBLIC_ARCHIVE#DUDETAIL#FILE01#202608010000.CSV",
        ))
        .unwrap();
    let edited = head.replacen("LD01,1,NLDP1,330,500,", "LD01,1,NLDP1,330,13.55,", 1);
    assert_ne!(edited, head);
    let zip_path = e.home.root().join("PUBLIC_DVD_DUDETAIL_202608010000.zip");
    make_zip(
        &zip_path,
        &[("PUBLIC_DVD_DUDETAIL_202608010000.CSV", edited.as_bytes())],
    );
    zip_path
}

/// A value its column's type cannot hold keeps its row: the column is null
/// there and the text is beside it, in a column of its own.
#[tokio::test]
async fn a_value_that_does_not_fit_keeps_its_row_with_the_text_beside_it() {
    let mut e = env_with(TypeOptions { decimal: true });
    // DUDETAIL as a user's table file might have it: REGISTEREDCAPACITY a
    // whole number of at most six digits, as AEMO's NUMBER(6,0) says.
    let registry = e.deps.registry.clone();
    let mut table = registry
        .table(registry.table_by_name("DUDETAIL").unwrap())
        .clone();
    let cap = table
        .columns
        .iter_mut()
        .find(|c| c.name == "REGISTEREDCAPACITY")
        .unwrap();
    cap.ty = nemlake_registry::ColumnType::Number {
        precision: Some(6),
        scale: Some(0),
    };
    let overlay = e.home.root().join("overlay/nem/mms");
    std::fs::create_dir_all(&overlay).unwrap();
    std::fs::write(
        overlay.join("DUDETAIL.toml"),
        nemlake_registry::file_model::file::write_table(&table),
    )
    .unwrap();
    e.deps.registry = Arc::new(
        registry
            .with_overlay(&e.home.root().join("overlay"))
            .unwrap(),
    );
    let zip_path = dudetail_with_a_fraction(&e);
    let plan = |strict: bool| Plan {
        request: "ingest dudetail".into(),
        input: Input::Local(vec![zip_path.clone()]),
        options: RunOptions {
            mode: if strict {
                Mode::Strict
            } else {
                Mode::Permissive
            },
            ..Default::default()
        },
    };
    // Strict mode refuses the file, naming the column and the line.
    let (rep, _) = run_plan(&e, plan(true)).await;
    assert_eq!(rep.exit_code, 1, "{:?}", rep.report.summary_lines());
    let refusal = &rep.report.by_code["parse.decimal"].examples[0];
    assert!(refusal.is_error());
    assert_eq!(refusal.column.as_deref(), Some("REGISTEREDCAPACITY"));
    assert_eq!(refusal.table.as_deref(), Some("DUDETAIL"));
    assert_eq!(refusal.line, Some(3));
    assert_eq!(refusal.observed.as_deref(), Some("13.55"));
    assert!(walk(&e.home.lake()).is_empty());
    // Otherwise every row is kept.
    let (rep, _) = run_plan(&e, plan(false)).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.rows, 20);
    assert_eq!(rep.summary.bad_rows, 0);
    let kept = &rep.report.by_code["parse.value_kept_raw"];
    assert_eq!(kept.count, 1);
    assert_eq!(
        kept.examples[0].column.as_deref(),
        Some("REGISTEREDCAPACITY")
    );
    assert_eq!(kept.examples[0].line, Some(3));
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 1, "{files:?}");
    let b = parquet_batch(&files[0]);
    assert_eq!(b.num_rows(), 20);
    let cap = b.column_by_name("REGISTEREDCAPACITY").unwrap();
    assert!(matches!(
        cap.data_type(),
        arrow_schema::DataType::Decimal128(6, 0)
    ));
    assert!(cap.is_null(0));
    assert_eq!(cap.null_count(), 1);
    let raw = texts(&b, "REGISTEREDCAPACITY__raw");
    assert_eq!(raw[0].as_deref(), Some("13.55"));
    assert!(raw[1..].iter().all(Option::is_none));
    assert!(walk(&e.home.quarantine()).is_empty());
}

/// A partition whose later batches carry a column of kept text, or a null
/// where the lake allows none, goes on in a further file that has room
/// for them; earlier batches keep their own file, later ones get nulls.
#[tokio::test]
async fn batches_that_need_more_columns_go_on_in_a_further_file() {
    let e = env();
    let csv = "C,NEMP.WORLD,TEST\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:10:00\",B,2\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:15:00\",C,lots\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:20:00\",,4\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:25:00\",E,5\n\
D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:30:00\",F,6\n\
C,\"END OF REPORT\",9\n";
    let p = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV");
    std::fs::write(&p, csv).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    let rep = run(
        Plan {
            request: "ingest".into(),
            input: Input::Local(vec![p]),
            options: RunOptions {
                batch_rows: 2,
                ..Default::default()
            },
        },
        e.deps.clone(),
        tx,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.rows, 6);
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 2, "{files:?}");
    let second_name = files.iter().find(|f| f.ends_with("-p2.parquet")).unwrap();
    let first = parquet_batch(files.iter().find(|f| *f != second_name).unwrap());
    let second = parquet_batch(second_name);
    assert_eq!(first.num_rows(), 2);
    assert_eq!(first.num_columns(), 4);
    assert!(!first.schema().field(1).is_nullable());
    assert_eq!(second.num_rows(), 4);
    assert_eq!(second.num_columns(), 5);
    assert!(second.schema().field(1).is_nullable());
    assert_eq!(
        texts(&second, "SCADAVALUE__raw"),
        [Some("lots".to_owned()), None, None, None]
    );
    assert_eq!(texts(&second, "DUID")[1], None);
}

/// A column the census saw beside the declared ones is declared by the
/// regeneration with an id of its own; the Parquet file of a shape that
/// carries it holds it under that id, like every declared column.
#[tokio::test]
async fn a_census_column_is_written_with_its_field_id() {
    let mut e = env();
    let registry = e.deps.registry.clone();
    let mut table = registry
        .table(registry.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
        .clone();
    let id = table.next_column_id;
    table.columns.push(nemlake_registry::ColumnDef {
        id: nemlake_registry::file_model::ColumnId(id),
        name: "QUALITY".into(),
        aliases: vec![],
        ty: nemlake_registry::ColumnType::Utf8 {
            max_len: None,
            dict: false,
        },
        nullable: true,
        oracle: None,
        loader: None,
        added_in: nemlake_registry::SchemaVersion(1),
        removed_in: None,
        description: None,
        src: vec!["file".into()],
        curated: false,
    });
    table.next_column_id += 1;
    let overlay = e.home.root().join("overlay/nem/mms");
    std::fs::create_dir_all(&overlay).unwrap();
    std::fs::write(
        overlay.join("DISPATCH_UNIT_SCADA.toml"),
        nemlake_registry::file_model::file::write_table(&table),
    )
    .unwrap();
    e.deps.registry = Arc::new(
        registry
            .with_overlay(&e.home.root().join("overlay"))
            .unwrap(),
    );
    let p = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.CSV");
    std::fs::write(
        &p,
        "I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE,QUALITY\nD,DISPATCH,UNIT_SCADA,1,\"2012/01/01 00:05:00\",A,1,GOOD\n",
    )
    .unwrap();
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest".into(),
            input: Input::Local(vec![p]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    let b = parquet_batch(&files[0]);
    let schema = b.schema();
    let quality = schema.field_with_name("QUALITY").unwrap();
    assert_eq!(
        quality
            .metadata()
            .get("PARQUET:field_id")
            .map(String::as_str),
        Some(id.to_string().as_str())
    );
    assert!(!quality.metadata().contains_key("nemlake:source"));
    assert_eq!(texts(&b, "QUALITY"), [Some("GOOD".to_owned())]);
    // Every column of the file has an id.
    assert!(
        schema
            .fields()
            .iter()
            .all(|f| f.metadata().contains_key("PARQUET:field_id")),
        "{schema:?}"
    );
}

/// A source read again leaves no quarantine file of an earlier read behind.
#[tokio::test]
async fn reading_a_source_again_removes_its_old_quarantine() {
    let e = env();
    let p = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV");
    let write = |extra: &str| {
        std::fs::write(
            &p,
            format!("I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1{extra}\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:10:00\",B,2\n"),
        )
        .unwrap();
    };
    let plan = || Plan {
        request: "ingest".into(),
        input: Input::Local(vec![p.clone()]),
        options: RunOptions {
            max_bad_rows: 5,
            ..Default::default()
        },
    };
    write(",extra");
    let (rep, _) = run_plan(&e, plan()).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.bad_rows, 1);
    assert_eq!(walk(&e.home.quarantine()).len(), 1);
    // The source changes and has no bad row any more.
    write("");
    let (rep, _) = run_plan(&e, plan()).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert!(rep.report.by_code.contains_key("manifest.superseded"));
    assert_eq!(rep.summary.bad_rows, 0);
    assert!(walk(&e.home.quarantine()).is_empty());
}

/// What the manifest records of a source is enough to name it again.
#[test]
fn a_recorded_source_becomes_a_file_to_read() {
    use nemlake_writer::manifest::{FileRecord, IngestShape};
    let rec = |source: &str, url: &str, path: &str| FileRecord {
        source: source.into(),
        url: url.into(),
        cache_path: path.into(),
        sha256: "s".into(),
        size: 1,
        etag: None,
        last_modified: None,
    };
    let member = nemlake_pipeline::recorded_remote(&rec(
        "nem-mmsdm",
        "https://h/MMSDM_2012_01.zip#DATA/PUBLIC_DVD_REGION_201201010000.zip",
        "/archive/h/MMSDM_2012_01.zip.members/PUBLIC_DVD_REGION_201201010000.zip",
    ))
    .unwrap();
    assert_eq!(member.source, "nem-mmsdm");
    assert_eq!(member.stem(), "public_dvd_region_201201010000");
    assert_eq!(member.table.as_deref(), Some("REGION"));
    assert_eq!(member.period.unwrap().to_string(), "2012-01");
    assert_eq!(member.kind, FileKind::MmsZip);
    let wem =
        nemlake_pipeline::recorded_remote(&rec("wem-datafiles", "https://w/a.csv", "/a/a.csv"))
            .unwrap();
    assert_eq!(
        (wem.source, wem.kind),
        ("wem-datafiles", FileKind::PlainCsv)
    );
    let local =
        nemlake_pipeline::recorded_remote(&rec("other", "file:///x/a.csv", "/x/a.csv")).unwrap();
    assert_eq!(local.source, "local");
    assert!(nemlake_pipeline::recorded_remote(&rec("local", "not a url", "/x/a.csv")).is_none());

    // An ingest recorded before rules were is never current.
    let reg = Registry::load_embedded(TypeOptions::default());
    let old = IngestShape {
        table: "T".into(),
        transform: Default::default(),
    };
    assert!(!nemlake_pipeline::plans_current(
        &reg,
        std::slice::from_ref(&old)
    ));
    assert!(nemlake_pipeline::plans_current(&reg, &[]));
    // Nor is one whose shape no longer resolves.
    let mut broken = old;
    broken.transform.plan_hash = "plan:0".into();
    assert!(!nemlake_pipeline::plans_current(&reg, &[broken]));
}

#[tokio::test]
async fn multi_entry_zip_with_bad_file_is_partial_and_everything_is_attempted() {
    let e = env();
    let good = std::fs::read(fixtures().join("REGION_v1_head.csv")).unwrap();
    let bad_rows = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1,extra\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",B,1\r\nC,\"END OF REPORT\",4\r\n";
    let zip_path = e.home.root().join("PUBLIC_DVD_MIXED_202401010000.zip");
    make_zip(
        &zip_path,
        &[
            ("GOOD.CSV", &good),
            ("BAD.CSV", bad_rows),
            ("notes.txt", b"hi"),
            (
                "UNIT.CSV",
                &std::fs::read(fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv")).unwrap(),
            ),
        ],
    );
    let opts = RunOptions {
        max_bad_rows: 0,
        ..Default::default()
    };
    let (rep, events) = run_plan(
        &e,
        Plan {
            request: "ingest mixed.zip".into(),
            input: Input::Local(vec![zip_path.clone()]),
            options: opts,
        },
    )
    .await;
    assert_eq!(rep.exit_code, 3, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.ingested, 2);
    assert_eq!(rep.summary.failed, 1);
    let failed: Vec<_> = rep.report.failed().collect();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].source.ends_with("!BAD.CSV"));
    assert_eq!(
        failed[0].failed_with.as_deref(),
        Some("csv.too_many_bad_rows")
    );
    let bad_diag = rep.report.by_code.get("csv.too_many_bad_rows").unwrap();
    let d = &bad_diag.examples[0];
    assert_eq!(d.entry.as_deref(), Some("BAD.CSV"));
    assert!(d.request.as_deref().unwrap().starts_with("ingest"));
    assert!(d.url.as_deref().unwrap().starts_with("file://"));
    // The quarantine holds the bad row with its sample.
    let q: Vec<_> = walk(&e.home.quarantine());
    assert_eq!(q.len(), 1, "{q:?}");
    let qtext = std::fs::read_to_string(&q[0]).unwrap();
    assert!(
        qtext.contains("csv.field_count_mismatch") && qtext.contains("extra"),
        "{qtext}"
    );
    // REGION is a snapshot table: partitioned by archive month.
    let files = walk(&e.home.lake());
    assert!(
        files
            .iter()
            .any(|f| f.contains("nem/region/snapshot=202401/")),
        "{files:?}"
    );
    assert!(
        files
            .iter()
            .any(|f| f.contains("nem/dispatch_unit_scada/year=2024/month=01/")),
        "{files:?}"
    );
    assert!(!files.iter().any(|f| f.contains(".tmp-")), "{files:?}");
    assert_eq!(files.len(), 2, "the failed entry wrote nothing");
    // Manifest has three ingest rows, one failed with its code.
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert_eq!(ingests.len(), 3);
    let failed_ingests = e.deps.manifest.list_ingests(None, true, 10).await.unwrap();
    assert_eq!(failed_ingests.len(), 1);
    assert_eq!(
        failed_ingests[0].error_code.as_deref(),
        Some("csv.too_many_bad_rows")
    );
    let diags = e
        .deps
        .manifest
        .list_diagnostics(DiagFilter {
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        diags
            .iter()
            .any(|(_, _, d)| d.code == "csv.too_many_bad_rows")
    );
    assert!(
        events
            .iter()
            .filter(|ev| matches!(ev, Event::SourceDone(_)))
            .count()
            == 3
    );
    // Fail-fast stops at the first failure.
    let e2 = env();
    let opts = RunOptions {
        max_bad_rows: 0,
        fail_fast: true,
        ..Default::default()
    };
    let (rep, _) = run_plan(
        &e2,
        Plan {
            request: "ingest".into(),
            input: Input::Local(vec![zip_path.clone()]),
            options: opts,
        },
    )
    .await;
    assert!(rep.report.stopped());
    assert!(rep.summary.ingested < 2);
    // Tolerating one bad row makes the entry succeed with the row quarantined.
    let e3 = env();
    let opts = RunOptions {
        max_bad_rows: 5,
        ..Default::default()
    };
    let (rep, _) = run_plan(
        &e3,
        Plan {
            request: "ingest".into(),
            input: Input::Local(vec![zip_path]),
            options: opts,
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.bad_rows, 1);
    assert_eq!(rep.summary.rows, 6 + 20 + 1);
}

#[tokio::test]
async fn month_spill_and_plain_csv_and_bad_paths() {
    let e = env();
    // A CSV whose rows cross a month boundary writes two partitions.
    let csv = b"C,x\r\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/31 23:55:00\",A,1\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/02/01 00:00:00\",A,2\r\nC,\"END OF REPORT\",5\r\n";
    let p = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV");
    std::fs::write(&p, csv).unwrap();
    let missing = e.home.root().join("nope.zip");
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest".into(),
            input: Input::Local(vec![p, missing]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 3, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    assert!(
        files.iter().any(|f| f.contains("month=01"))
            && files.iter().any(|f| f.contains("month=02")),
        "{files:?}"
    );
    assert!(
        rep.report.by_code.contains_key("config.bad_path")
            || rep.report.by_code.contains_key("csv.read_failed"),
        "{:?}",
        rep.report.by_code.keys()
    );
    // Dry run lists and does nothing.
    let e2 = env();
    let (rep, _) = run_plan(
        &e2,
        Plan {
            request: "ingest".into(),
            input: Input::Local(vec![
                e.home
                    .root()
                    .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV"),
            ]),
            options: RunOptions {
                dry_run: true,
                ..Default::default()
            },
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0);
    assert_eq!(rep.listed.len(), 1);
    assert!(walk(&e2.home.lake()).is_empty());
}

struct OneFile(Vec<RemoteFile>);

#[async_trait]
impl Source for OneFile {
    fn id(&self) -> &'static str {
        "test"
    }
    async fn list(&self, _q: &Query, _f: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
        Ok(self.0.clone())
    }
}

struct Broken;

#[async_trait]
impl Source for Broken {
    fn id(&self) -> &'static str {
        "broken"
    }
    async fn list(&self, _q: &Query, _f: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
        Err(FetchError::Offline {
            url: "https://h/".into(),
        })
    }
}

#[tokio::test]
async fn remote_source_downloads_then_serves_from_cache_and_reports_fetch_failures() {
    let e = env();
    let zip_tmp = e.home.root().join("z.zip");
    make_zip(
        &zip_tmp,
        &[(
            "PUBLIC_DVD_TRADINGPRICE_202401010000.CSV",
            &std::fs::read(fixtures().join("TRADINGPRICE_v2_head.csv")).unwrap(),
        )],
    );
    let bytes = std::fs::read(&zip_tmp).unwrap();
    e.transport.add(
        "https://h/PUBLIC_DVD_TRADINGPRICE_202401010000.zip",
        Canned::ok(&bytes),
    );
    e.transport.add(
        "https://h/PUBLIC_DVD_REGION_202401010000.zip",
        Canned::status(404),
    );
    let ym = nemlake_core::YearMonth::new(2024, 1).unwrap();
    let remote = |name: &str, table: &str| RemoteFile {
        url: Url::parse(&format!("https://h/{name}")).unwrap(),
        source: "nem-mmsdm",
        table: Some(table.into()),
        period: Some(ym),
        kind: FileKind::MmsZip,
        size: None,
        modified: None,
        member: None,
    };
    let source = Arc::new(OneFile(vec![
        remote("PUBLIC_DVD_TRADINGPRICE_202401010000.zip", "TRADINGPRICE"),
        remote("PUBLIC_DVD_REGION_202401010000.zip", "REGION"),
    ]));
    let query = Query {
        exclude: Vec::new(),
        tables: vec![],
        from: ym,
        to: ym,
    };
    let plan = || Plan {
        request: "fetch nem".into(),
        input: Input::Remote {
            source: source.clone(),
            query: query.clone(),
        },
        options: RunOptions::default(),
    };
    let (rep, events) = run_plan(&e, plan()).await;
    assert_eq!(rep.exit_code, 3, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.downloaded, 1);
    assert_eq!(rep.summary.ingested, 1);
    assert_eq!(rep.summary.failed, 1);
    let failed: Vec<_> = rep.report.failed().collect();
    assert_eq!(failed[0].failed_with.as_deref(), Some("net.http_status"));
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, Event::DownloadDone { cached: false, .. }))
    );
    assert_eq!(e.transport.request_count(), 2);
    let files = walk(&e.home.lake());
    assert!(
        files.iter().any(|f| f.contains(
            "nem/tradingprice/year=2024/month=01/public_dvd_tradingprice_202401010000-v2-"
        )),
        "{files:?}"
    );
    // Second run: cache answers, manifest skips, the 404 is retried.
    let (rep2, _) = run_plan(&e, plan()).await;
    assert_eq!(rep2.summary.skipped, 1);
    assert_eq!(e.transport.request_count(), 3);
    // A source that fails to list.
    let (rep3, _) = run_plan(
        &e,
        Plan {
            request: "fetch".into(),
            input: Input::Remote {
                source: Arc::new(Broken),
                query: query.clone(),
            },
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep3.exit_code, 4);
    assert!(rep3.report.by_code.contains_key("net.offline"));
}

fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.display().to_string());
            }
        }
    }
    out.sort();
    out
}
