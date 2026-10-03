//! The pipeline end to end over fixture files: local zips, a mocked remote
//! source, idempotent re-runs, failures that do not stop the run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nemlake_core::event::Event;
use nemlake_core::paths::Home;
use nemlake_fetch::cache::DiskCache;
use nemlake_fetch::transport::{Canned, StaticTransport};
use nemlake_fetch::{FetchError, Fetcher, FetcherConfig, FileKind, Query, RemoteFile, Source};
use nemlake_pipeline::{Deps, Input, Plan, RunOptions, run};
use nemlake_schema::{Registry, TypeOptions};
use nemlake_store::manifest::DiagFilter;
use nemlake_store::{Manifest, ManifestHandle, ManifestThread};
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
    let table = nemlake_schema::infer::draft_table(
        nemlake_schema::Market::Nem,
        &irow,
        &nemlake_schema::infer::Sampler::new(2),
    );
    assert_eq!(table.time_column, None);
    let overlay = e.home.root().join("overlay/nem/mms");
    std::fs::create_dir_all(&overlay).unwrap();
    std::fs::write(
        overlay.join("NEMLAKE_TEST_NOTIME.toml"),
        nemlake_schema::file::write_table(&table),
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

#[tokio::test]
async fn multi_entry_zip_with_bad_file_is_partial_and_everything_is_attempted() {
    let e = env();
    let good = std::fs::read(fixtures().join("REGION_v1_head.csv")).unwrap();
    let bad_rows = b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/13/01 00:05:00\",A,1\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",B,1\r\nC,\"END OF REPORT\",4\r\n";
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
        qtext.contains("parse.timestamp") && qtext.contains("2024/13/01"),
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
