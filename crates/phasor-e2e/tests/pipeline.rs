//! The pipeline end to end over fixture files: local zips, a mocked remote
//! source, idempotent re-runs, failures that do not stop the run.

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use phasor_acquire::transport::{Canned, StaticTransport};
use phasor_acquire::{Archive, FetchError, Fetcher, FetcherConfig};
use phasor_core::TimeRange;
use phasor_core::event::Event;
use phasor_core::paths::Home;
use phasor_kit::{Catalog as _, KitRegistry, Strictness};
use phasor_kit::{Granularity, Listings, Query, RemoteFile, Source};
use phasor_pipeline::{Deps, Input, Plan, RunOptions, run};
use phasor_registry::RegistryCatalog;
use phasor_registry::file_model::{
    Column, ColumnId, LakeType, Market, RoleOrigin, Run, Table, TableSchema, Temporal, ValidAxis,
};
use phasor_registry::{LoadMode, Registry};
use phasor_writer::manifest::DiagFilter;
use phasor_writer::{Manifest, ManifestHandle, ManifestThread};
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
    registry: Arc<Registry>,
    deps: Deps,
    transport: Arc<StaticTransport>,
    _thread: ManifestThread,
}

impl Env {
    /// Reads with `registry` from now on.
    fn set_registry(&mut self, registry: Arc<Registry>) {
        self.deps.kits = kits_for(&registry);
        self.registry = registry;
    }
}

/// The WEM kit, whose sources know a file by its path, then the NEM kit
/// over `registry`, with the archive at `https://h` as its source (what
/// names a local file's table and month): last, as the binary assembles
/// it, so a file no source claims is read as MMS rows.
fn kits_for(registry: &Arc<Registry>) -> Arc<KitRegistry> {
    Arc::new(KitRegistry::new(vec![
        phasor_wem::kit(&phasor_wem::WemConfig::default(), Registry::load_embedded()).unwrap(),
        phasor_nem::kit(registry.clone(), "https://h").unwrap(),
    ]))
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_dirs().unwrap();
    let (manifest, thread) = ManifestHandle::spawn_with(Manifest::open(&home.manifest()).unwrap());
    let transport = Arc::new(StaticTransport::default());
    let fetcher = Fetcher::new(
        Arc::new(transport.clone()),
        Archive::new(home.archive()),
        FetcherConfig {
            attempts: 2,
            backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
            min_interval: Duration::ZERO,
        },
    );
    let registry = Registry::load_embedded();
    let deps = Deps {
        kits: kits_for(&registry),
        fetcher: Arc::new(fetcher),
        manifest,
        home: home.clone(),
    };
    Env {
        _dir: dir,
        home,
        registry,
        deps,
        transport,
        _thread: thread,
    }
}

/// A table of text columns, with schemas of `(from, columns)`: one run
/// each, from that month.
fn text_table(
    name: &str,
    pair: (&str, &str),
    columns: &[&str],
    schemas: &[(&str, &[&str])],
    time_column: Option<&str>,
) -> Table {
    let id = |c: &str| ColumnId(columns.iter().position(|n| *n == c).unwrap() as u32 + 1);
    Table {
        market: Market::Nem,
        name: name.into(),
        description: None,
        report_pairs: vec![(pair.0.into(), pair.1.into())],
        with_columns: Vec::new(),
        older_names: Vec::new(),
        published: String::new(),
        load_mode: LoadMode::Append,
        temporal: Temporal {
            origin: RoleOrigin::Hand,
            entity: vec![],
            valid: ValidAxis::Timeless,
            order: vec![],
            cutoff: None,
        },
        primary_key: vec![],
        time_column: time_column.map(str::to_owned),
        columns: columns
            .iter()
            .enumerate()
            .map(|(i, c)| Column {
                id: ColumnId(i as u32 + 1),
                name: (*c).into(),
                ty: if Some(*c) == time_column {
                    LakeType::Timestamp
                } else {
                    LakeType::String
                },
                nullable: true,
                note: None,
            })
            .collect(),
        schemas: schemas
            .iter()
            .enumerate()
            .map(|(i, (_, cols))| TableSchema {
                id: i as u16,
                fields: cols.iter().map(|c| (id(c), false)).collect(),
                key: Vec::new(),
            })
            .collect(),
        runs: schemas
            .iter()
            .enumerate()
            .map(|(i, (from, _))| Run {
                schema: i as u16,
                report: (pair.0.into(), pair.1.into()),
                version: 1,
                kinds: Vec::new(),
                from: from.parse().unwrap(),
                to: from.parse().unwrap(),
                files: 0,
                rows: 0,
            })
            .collect(),
        current_schema: schemas.len().saturating_sub(1) as u16,
    }
}

/// Lays `t` over the registry as a user's own table under `{root}`, as
/// `{root}/nem/{TABLE}/table.toml`.
fn overlay(registry: &Registry, root: &Path, t: &Table) -> Arc<Registry> {
    let dir = root.join("nem").join(&t.name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("table.toml"),
        phasor_registry::file_model::file::write_table(t),
    )
    .unwrap();
    Arc::new(registry.with_overlay(root).unwrap())
}

async fn run_plan(e: &Env, plan: Plan) -> (phasor_pipeline::RunReport, Vec<Event>) {
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

/// A number is written as its table's history types it: RRP is
/// decimal(15, 5), exact.
#[tokio::test]
async fn numbers_are_written_as_their_historys_types() {
    use arrow_array::Array;
    let csv = fixtures().join("DISPATCHPRICE_v4_head.csv");
    let floats = env();
    let (rep, _) = run_plan(
        &floats,
        Plan {
            request: "ingest price".into(),
            input: Input::Local(vec![csv]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let files = walk(&floats.home.lake());
    assert_eq!(files.len(), 1, "{files:?}");
    let batch = parquet_batch(&files[0]);
    assert_eq!(
        batch.schema().field_with_name("RRP").unwrap().data_type(),
        &arrow_schema::DataType::Decimal128(15, 5)
    );
    let col = batch
        .column_by_name("RRP")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::Decimal128Array>()
        .unwrap();
    // The first D row of the fixture publishes RRP as 57.98.
    assert_eq!(col.value(0), 5_798_000);
    assert_eq!(col.null_count(), 0);
    assert_eq!(batch.num_rows(), 20);
}

/// An appended table with no time column of its own is partitioned by the
/// month of the file its rows came in.
#[tokio::test]
async fn a_table_without_a_time_column_is_partitioned_by_its_file_month() {
    let mut e = env();
    // Such a table, laid over the embedded registry as a user's would be.
    let cols = ["CONTRACTYEAR", "WEEKNO"];
    let irow = phasor_mms_decode::IRow {
        report_type: "PHASOR_TEST",
        report_subtype: "NOTIME",
        version: 1,
        columns: &cols,
    };
    let table = text_table(
        "PHASOR_TEST_NOTIME",
        (irow.report_type, irow.report_subtype),
        &cols,
        &[("2009-07", &cols)],
        None,
    );
    e.set_registry(overlay(&e.registry, &e.home.root().join("overlay"), &table));
    let csv = "C,NEMP.WORLD,TEST\nI,PHASOR_TEST,NOTIME,1,CONTRACTYEAR,WEEKNO\nD,PHASOR_TEST,NOTIME,1,2024,9\nD,PHASOR_TEST,NOTIME,1,2024,10\nC,\"END OF REPORT\",5\n";
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
            "nem/phasor_test_notime/data/year=2024/month=03/public_dvd_notime_202403010000-v0-"
        )),
        "{files:?}"
    );
    // A file whose name says no month has nowhere better to go.
    assert!(
        files
            .iter()
            .any(|f| f.contains("__HIVE_DEFAULT_PARTITION__") && f.contains("/notime-v0-")),
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
    assert!(events.iter().any(|ev| matches!(ev, Event::BlockResolved { table, status, .. } if table == "DISPATCH_UNIT_SCADA" && status == "conforms")));
    assert!(events.iter().any(|ev| matches!(ev, Event::Summary(_))));
    let files: Vec<_> = walk(&e.home.lake());
    assert_eq!(
        files.len(),
        1,
        "lake {} holds {files:?}; home holds {:?}",
        e.home.lake().display(),
        walk(e.home.root())
    );
    assert!(files[0].contains("nem/dispatch_unit_scada/data/year=2024/month=01/public_dvd_dispatch_unit_scada_202401010000-v0-"), "{}", files[0]);
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

/// FNV-1a over `bytes`: a hash whose value no toolchain changes.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// How a file is laid out: each row group's rows, and a hash of its data
/// columns' bytes, which says where their pages end and how they are
/// encoded and compressed. The provenance columns, which name the source's
/// path, are left out.
fn layout(path: &str) -> (Vec<i64>, u64) {
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let bytes = std::fs::read(path).unwrap();
    let reader = SerializedFileReader::new(std::fs::File::open(path).unwrap()).unwrap();
    let meta = reader.metadata();
    let mut chunks = Vec::new();
    for g in meta.row_groups() {
        // The provenance columns are the ones whose names begin `_`.
        for c in g
            .columns()
            .iter()
            .filter(|c| !c.column_path().string().starts_with('_'))
        {
            let (start, len) = c.byte_range();
            chunks.extend_from_slice(&bytes[start as usize..(start + len) as usize]);
        }
    }
    (
        meta.row_groups()
            .iter()
            .map(parquet::file::metadata::RowGroupMetaData::num_rows)
            .collect(),
        fnv1a(&chunks),
    )
}

/// The same rows are written as the same bytes, wherever they are
/// ingested: where batches, row groups and pages end follows only from the
/// rows and the layout. A change here changes every file a re-ingest
/// writes; review it, then pin the new value.
#[tokio::test]
async fn the_same_rows_are_laid_out_byte_for_byte_the_same() {
    let e = env();
    let fixture =
        std::fs::read_to_string(fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv")).unwrap();
    let mut lines = fixture.lines();
    let (header, irow) = (lines.next().unwrap(), lines.next().unwrap());
    let rows: Vec<&str> = lines.filter(|l| l.starts_with("D,")).collect();
    let mut csv = format!("{header}\r\n{irow}\r\n");
    let n = 150_000;
    for i in 0..n {
        let (prefix, _) = rows[i % rows.len()].rsplit_once(',').unwrap();
        let minute = (i / rows.len()) * 5;
        let at = format!(
            "2024/01/{:02} {:02}:{:02}:00",
            minute / 1440 % 28 + 1,
            minute % 1440 / 60,
            minute % 60
        );
        let prefix = prefix.replacen("2024/01/01 00:05:00", &at, 1);
        write!(csv, "{prefix},{:.6}\r\n", (i as f64) * 0.731).unwrap();
    }
    write!(csv, "C,\"END OF REPORT\",{}\r\n", n + 3).unwrap();
    let zip_path = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip");
    make_zip(
        &zip_path,
        &[(
            "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV",
            csv.as_bytes(),
        )],
    );
    let plan = Plan {
        request: "ingest".into(),
        input: Input::Local(vec![zip_path]),
        options: RunOptions::default(),
    };
    let (rep, _) = run_plan(&e, plan).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(
        layout(&files[0]),
        (vec![150_000], 1_031_190_125_928_677_621)
    );
}

/// An entry of a rollup that the mirror also holds from its own address
/// went public when that address served it, which its record keeps: its
/// rows are stamped with that time, and an entry held only in the rollup
/// with the rollup's.
#[tokio::test]
async fn an_entry_held_alone_is_stamped_when_it_first_went_public() {
    let e = env();
    let scada = std::fs::read(fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv")).unwrap();
    let zip_path = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip");
    let held = "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV";
    let other = "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401020000.CSV";
    make_zip(&zip_path, &[(held, &scada), (other, &scada)]);
    let zip_url = format!("file://{}", zip_path.display());
    // The record of the entry held from its own address, which names the
    // rollup's entry as the same bytes.
    let record = format!(
        r#"{{"size":1,"sha256":"{sha}","tool":"t","addresses":[{{"url":"https://h/CURRENT/{held}","seen":"2024-01-01T00:06:00Z","via":"fetched","served":{{"modified":"2024-01-01T00:05:31Z"}}}},{{"url":"{zip_url}!{held}","seen":"2024-01-02T00:00:00Z","via":"entry"}}]}}"#,
        sha = "0".repeat(64)
    );
    std::fs::write(
        e.home.root().join(format!("{held}.provenance.json")),
        record,
    )
    .unwrap();
    // A record that does not name the rollup's entry says nothing of it.
    let unrelated = format!(
        r#"{{"size":1,"sha256":"{sha}","tool":"t","addresses":[{{"url":"https://h/CURRENT/{other}","seen":"2024-01-02T00:06:00Z","via":"fetched","served":{{"modified":"2024-01-02T00:05:31Z"}}}}]}}"#,
        sha = "0".repeat(64)
    );
    std::fs::write(
        e.home.root().join(format!("{other}.provenance.json")),
        unrelated,
    )
    .unwrap();
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest rollup".into(),
            input: Input::Local(vec![zip_path.clone()]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 1, "both entries in one file: {files:?}");
    let b = parquet_batch(&files[0]);
    use arrow_array::Array as _;
    let source = b
        .column_by_name("_phasor_source")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::DictionaryArray<arrow_array::types::UInt16Type>>()
        .unwrap();
    let names = source
        .values()
        .as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .unwrap();
    let published = b
        .column_by_name("_phasor_server_last_modified")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::TimestampMicrosecondArray>()
        .unwrap();
    // 2024-01-01T00:05:31Z.
    let first = 1_704_067_531_000_000;
    for i in 0..b.num_rows() {
        let s = names.value(usize::from(source.keys().value(i)));
        if s.ends_with(&format!("!{held}")) {
            assert_eq!(published.value(i), first, "{s}");
        } else {
            assert!(s.ends_with(&format!("!{other}")), "{s}");
            // A monthly archive's name has no time and a local file no
            // server: nothing says when it went public.
            assert!(published.is_null(i), "{s}");
        }
    }
    // The manifest says so too, entry by entry.
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    let shares = &ingests[0].entries;
    assert_eq!(shares.len(), 2);
    assert_eq!(
        shares
            .iter()
            .find(|x| x.entry == held)
            .unwrap()
            .server_last_modified,
        Some(first)
    );
    assert_eq!(
        shares
            .iter()
            .find(|x| x.entry == other)
            .unwrap()
            .server_last_modified,
        None
    );
}

#[tokio::test]
async fn a_file_whose_parquet_cannot_be_put_in_place_fails_and_leaves_nothing() {
    let scada = |e: &Env| {
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
        Plan {
            request: "ingest scada.zip".into(),
            input: Input::Local(vec![zip_path]),
            options: RunOptions::default(),
        }
    };
    // Where the file goes, learnt from a lake of its own.
    let first = env();
    let (rep, _) = run_plan(&first, scada(&first)).await;
    assert_eq!(rep.exit_code, 0);
    let written = walk(&first.home.lake()).remove(0);
    let place = Path::new(&written)
        .strip_prefix(first.home.lake())
        .unwrap()
        .to_path_buf();
    // A folder there: the commit cannot rename the file into place.
    let e = env();
    std::fs::create_dir_all(e.home.lake().join(&place).join("x")).unwrap();
    let (rep, _) = run_plan(&e, scada(&e)).await;
    assert_ne!(rep.exit_code, 0);
    assert_eq!(rep.summary.failed, 1);
    assert!(
        rep.report.by_code.contains_key("store.rename"),
        "{:?}",
        rep.report.by_code.keys()
    );
    let left: Vec<String> = walk(&e.home.lake());
    assert!(left.is_empty(), "{left:?}");
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

/// Every output records the shape it was read from and the plan it was
/// read by. A source already in the lake is skipped while that plan holds,
/// and read again, from disk, once it has changed.
#[tokio::test]
async fn a_source_is_read_again_when_its_plan_changes() {
    let e = env();
    let csv = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCHPRICE_202401010000.CSV");
    std::fs::copy(fixtures().join("DISPATCHPRICE_v4_head.csv"), &csv).unwrap();
    let floats = e.deps.clone();
    // Another registry, in which RRP is text: another plan.
    let mut price = e
        .registry
        .table(e.registry.table_by_name("DISPATCHPRICE").unwrap())
        .clone();
    price
        .columns
        .iter_mut()
        .find(|c| c.name == "RRP")
        .unwrap()
        .ty = LakeType::String;
    let decimals = Deps {
        kits: kits_for(&overlay(
            &e.registry,
            &e.home.root().join("retyped"),
            &price,
        )),
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
    assert_eq!(first.resolution, "conforms");
    assert_eq!(
        (first.block_family.as_str(), first.block_name.as_str()),
        ("DISPATCH", "PRICE")
    );
    assert_eq!(first.source_version, 4);
    assert_eq!(first.source_columns[0], "SETTLEMENTDATE");
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 1);
    assert_eq!(footer(&files[0], "phasor.plan"), first.plan_hash);
    assert_eq!(footer(&files[0], "phasor.schema_source"), "registry");
    assert_eq!(footer(&files[0], "phasor.resolution"), "conforms");
    assert_eq!(footer(&files[0], "phasor.schema_version"), "1");
    assert_eq!(footer(&files[0], "phasor.report_type"), "DISPATCH");
    assert_eq!(footer(&files[0], "phasor.report_subtype"), "PRICE");
    assert_eq!(footer(&files[0], "phasor.source_version"), "4");
    assert_eq!(
        footer(&files[0], "phasor.source_columns"),
        first.source_columns.join(",")
    );

    // Same rules: skipped, and nothing is behind.
    let rep = go(&floats, local(), false).await;
    assert_eq!((rep.summary.skipped, rep.summary.ingested), (1, 0));
    let rep = go(&floats, Input::Stale, false).await;
    assert_eq!(rep.summary.listed, 0);
    assert_eq!(rep.exit_code, 0);

    // Another plan (RRP as text): the same source is read again and its
    // Parquet file replaced, not duplicated.
    let rep = go(&decimals, Input::Stale, true).await;
    assert_eq!(rep.summary.listed, 1, "a dry run lists what is behind");
    assert_eq!(rep.listed[0].url.scheme(), "file");
    assert_eq!(
        footer(&files[0], "phasor.plan"),
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
    assert_eq!(second.schema_source, "user");
    assert_eq!(walk(&e.home.lake()), files);
    assert_eq!(footer(&files[0], "phasor.plan"), second.plan_hash);
    let rep = go(&decimals, Input::Stale, false).await;
    assert_eq!(rep.summary.listed, 0);

    // Back to the first plan through the manifest alone: the same hash
    // as the first read gave.
    let rep = go(&floats, Input::Stale, false).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!((rep.summary.listed, rep.summary.ingested), (1, 1));
    assert!(rep.report.by_code.contains_key("manifest.rules_changed"));
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert_eq!(ingests[0].transform, first);
    assert_eq!(footer(&files[0], "phasor.plan"), first.plan_hash);

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

/// A block is read with the schema of its table's history its columns
/// are: the same table, two shapes, both conforming, and the lake says
/// which schema read each.
#[tokio::test]
async fn the_columns_decide_the_schema() {
    let e = env();
    let three = ["SETTLEMENTDATE", "UNIT", "VALUE"];
    let four = ["SETTLEMENTDATE", "UNIT", "VALUE", "QUALITY"];
    let table = text_table(
        "PHASOR_TEST_GROWN",
        ("PHASOR_TEST", "GROWN"),
        &four,
        &[("2009-07", &three), ("2020-01", &four)],
        Some("SETTLEMENTDATE"),
    );
    let deps = Deps {
        kits: kits_for(&overlay(
            &e.registry,
            &e.home.root().join("schemas"),
            &table,
        )),
        ..e.deps.clone()
    };
    let file = |name: &str, stamp: &str, cols: &[&str]| {
        let p = e.home.root().join(name);
        let values = ["A1", "1.5", "GOOD"][..cols.len() - 1].join(",");
        std::fs::write(
            &p,
            format!(
                "C,NEMP.WORLD,TEST\nI,PHASOR_TEST,GROWN,1,{}\nD,PHASOR_TEST,GROWN,1,\"{stamp}\",{values}\nC,\"END OF REPORT\",4\n",
                cols.join(",")
            ),
        )
        .unwrap();
        p
    };
    let old = file(
        "PUBLIC_DVD_GROWN_201003010000.CSV",
        "2010/03/01 00:05:00",
        &three,
    );
    let new = file(
        "PUBLIC_DVD_GROWN_202403010000.CSV",
        "2024/03/01 00:05:00",
        &four,
    );
    let go = |input: Input, mode: Strictness, deps: &Deps| {
        let deps = deps.clone();
        async move {
            let (tx, _rx) = tokio::sync::mpsc::channel(4096);
            run(
                Plan {
                    request: "ingest grown".into(),
                    input,
                    options: RunOptions {
                        mode,
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
    let rep = go(Input::Local(vec![old, new]), Strictness::Strict, &deps).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.bad_rows, 0);
    let path = |month: &str| {
        let files: Vec<String> = walk(&e.home.lake())
            .into_iter()
            .filter(|p| p.contains(month))
            .collect();
        assert_eq!(files.len(), 1, "{month}: {files:?}");
        files[0].clone()
    };
    let (p2010, p2024) = (path("year=2010"), path("year=2024"));
    assert_eq!(footer(&p2010, "phasor.schema_version"), "0");
    assert_eq!(footer(&p2024, "phasor.schema_version"), "1");
    for p in [&p2010, &p2024] {
        assert_eq!(footer(p, "phasor.resolution"), "conforms");
        assert_eq!(footer(p, "phasor.source_version"), "1");
    }
    assert_eq!(footer(&p2010, "phasor.source_period"), "2010-03");
    assert_ne!(footer(&p2010, "phasor.plan"), footer(&p2024, "phasor.plan"));
    // The 2010 file is null in the column it lacks, as the lake has it.
    assert_eq!(texts(&parquet_batch(&p2010), "QUALITY"), [None]);

    // Three columns in another order fit no run: strict mode refuses
    // them, and otherwise they are kept.
    let late = file(
        "PUBLIC_DVD_GROWN_202405010000.CSV",
        "2024/05/01 00:05:00",
        &["SETTLEMENTDATE", "VALUE", "UNIT"],
    );
    let rep = go(Input::Local(vec![late.clone()]), Strictness::Strict, &deps).await;
    assert_ne!(rep.exit_code, 0);
    assert!(
        rep.report.by_code.contains_key("schema.deviates"),
        "{:?}",
        rep.report.by_code.keys()
    );
    let rep = go(Input::Local(vec![late]), Strictness::Permissive, &deps).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let ingests = deps.manifest.list_ingests(None, false, 10).await.unwrap();
    let by_period = |p: &str| {
        ingests
            .iter()
            .find(|i| i.transform.source_period.as_deref() == Some(p))
            .unwrap()
    };
    assert_eq!(by_period("2010-03").schema_version, 0);
    assert_eq!(by_period("2024-03").schema_version, 1);
    assert_eq!(by_period("2024-05").transform.resolution, "deviates");

    // With the manifest, every source is current; under the registry
    // without the table they are behind: they would be read from the file.
    let rep = go(Input::Stale, Strictness::Permissive, &deps).await;
    assert_eq!(rep.summary.listed, 0, "nothing is behind its plan");
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
    assert_eq!(rep.summary.listed, 3);
}

/// A source whose rows wander over more months than a worker may hold
/// open writes each month in parts: a writer is closed to make room and a
/// further file is started when rows for its month come again. Every row
/// lands, no two files share a name, and a failed entry leaves nothing.
#[tokio::test]
async fn rows_that_wander_over_many_months_are_written_in_parts() {
    assert_eq!(phasor_pipeline::ingest_file::open_writers(4), 32);
    assert_eq!(phasor_pipeline::ingest_file::open_writers(0), 128);
    assert_eq!(phasor_pipeline::ingest_file::open_writers(64), 8);

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
    let mut e = env();
    // DUDETAIL as a user's table file might have it: REGISTEREDCAPACITY a
    // whole number, as AEMO's NUMBER(6,0) says.
    let registry = e.registry.clone();
    let mut table = registry
        .table(registry.table_by_name("DUDETAIL").unwrap())
        .clone();
    let cap = table
        .columns
        .iter_mut()
        .find(|c| c.name == "REGISTEREDCAPACITY")
        .unwrap();
    cap.ty = LakeType::Long;
    e.set_registry(overlay(&registry, &e.home.root().join("overlay"), &table));
    let zip_path = dudetail_with_a_fraction(&e);
    let plan = |strict: bool| Plan {
        request: "ingest dudetail".into(),
        input: Input::Local(vec![zip_path.clone()]),
        options: RunOptions {
            mode: if strict {
                Strictness::Strict
            } else {
                Strictness::Permissive
            },
            ..Default::default()
        },
    };
    // Strict mode refuses the file, naming the column and the line.
    let (rep, _) = run_plan(&e, plan(true)).await;
    assert_eq!(rep.exit_code, 1, "{:?}", rep.report.summary_lines());
    let refusal = &rep.report.by_code["parse.integer"].examples[0];
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
    assert!(matches!(cap.data_type(), arrow_schema::DataType::Int64));
    assert!(cap.is_null(0));
    assert_eq!(cap.null_count(), 1);
    let raw = texts(&b, "REGISTEREDCAPACITY__raw");
    assert_eq!(raw[0].as_deref(), Some("13.55"));
    assert!(raw[1..].iter().all(Option::is_none));
    assert!(walk(&e.home.quarantine()).is_empty());
}

/// A file of many blocks, each I row a different table, as most AEMO
/// files are: every block lands in its own table, every row is kept.
#[tokio::test]
async fn each_block_of_a_file_goes_to_its_own_table() {
    let e = env();
    let p = e.home.root().join("PUBLIC_DISPATCHIS_202401010005.CSV");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/csv/multi_block_dispatchis.csv"
        ),
        &p,
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    let rep = run(
        Plan {
            request: "ingest".into(),
            input: Input::Local(vec![p]),
            options: RunOptions::default(),
        },
        e.deps.clone(),
        tx,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.rows, 19);
    let tables: std::collections::BTreeSet<String> = walk(&e.home.lake())
        .iter()
        .map(|f| {
            // lake/{market}/{table}/...
            let rel = Path::new(f).strip_prefix(e.home.lake()).unwrap();
            let table = rel.components().nth(1).unwrap().as_os_str();
            table.to_string_lossy().into_owned()
        })
        .collect();
    assert_eq!(tables.len(), 7, "{tables:?}");
    assert!(tables.contains("dispatchprice"), "{tables:?}");
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
    // The table's columns, then each row's source and its three times.
    assert_eq!(first.num_columns(), 4 + 4);
    assert!(!first.schema().field(1).is_nullable());
    assert_eq!(second.num_rows(), 4);
    assert_eq!(second.num_columns(), 5 + 4);
    let names: Vec<String> = second
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(
        &names[4..],
        [
            "SCADAVALUE__raw",
            "_phasor_source",
            "_phasor_server_last_modified",
            "_phasor_zip_entry_modified",
            "_phasor_filename_timestamp"
        ]
    );
    assert!(second.schema().field(1).is_nullable());
    assert_eq!(
        texts(&second, "SCADAVALUE__raw"),
        [Some("lots".to_owned()), None, None, None]
    );
    assert_eq!(texts(&second, "DUID")[1], None);
}

/// A column a user's table declares beside the history's has an id of its
/// own; the Parquet file of a shape that carries it holds it under that
/// id, like every declared column.
#[tokio::test]
async fn a_declared_column_is_written_with_its_field_id() {
    let mut e = env();
    let registry = e.registry.clone();
    let mut table = registry
        .table(registry.table_by_name("DISPATCH_UNIT_SCADA").unwrap())
        .clone();
    let id = table.columns.iter().map(|c| c.id.0).max().unwrap() + 1;
    table.columns.push(Column {
        id: ColumnId(id),
        name: "QUALITY".into(),
        ty: LakeType::String,
        nullable: true,
        note: Some("Seen beside the declared columns.".into()),
    });
    e.set_registry(overlay(&registry, &e.home.root().join("overlay"), &table));
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
    assert!(!quality.metadata().contains_key("phasor:source"));
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

/// A source read again that fails leaves the earlier read's Parquet in
/// place: nothing is replaced by a read that wrote nothing.
#[tokio::test]
async fn a_failed_read_again_keeps_what_the_earlier_read_wrote() {
    let e = env();
    let p = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV");
    let write = |b: u32, bad: usize| {
        let mut csv = String::from(
            "I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:10:00\",B,\n",
        );
        csv.insert_str(csv.len() - 1, &b.to_string());
        for i in 0..bad {
            let _ = writeln!(
                csv,
                "D,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:15:00\",C{i},3,extra"
            );
        }
        std::fs::write(&p, csv).unwrap();
    };
    let plan = || Plan {
        request: "ingest".into(),
        input: Input::Local(vec![p.clone()]),
        options: RunOptions {
            max_bad_rows: 2,
            ..Default::default()
        },
    };
    write(2, 0);
    let (rep, _) = run_plan(&e, plan()).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let before = walk(&e.home.lake());
    assert!(!before.is_empty());
    // The source changes, and its new read is refused: too many bad rows.
    write(2, 5);
    let (rep, _) = run_plan(&e, plan()).await;
    assert_ne!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert!(!rep.report.by_code.contains_key("manifest.superseded"));
    let parquet = |files: Vec<String>| -> Vec<String> {
        files
            .into_iter()
            .filter(|f| f.ends_with(".parquet"))
            .collect()
    };
    let after = parquet(walk(&e.home.lake()));
    let kept = parquet(before);
    assert!(!kept.is_empty());
    assert_eq!(after, kept, "the earlier read's Parquet stays");
    // Read again once it is good: the earlier read is replaced, so the lake
    // holds this read's rows only.
    write(9, 0);
    let (rep, _) = run_plan(&e, plan()).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert!(rep.report.by_code.contains_key("manifest.superseded"));
    assert_eq!(parquet(walk(&e.home.lake())).len(), kept.len());
    assert_ne!(parquet(walk(&e.home.lake())), kept);
}

/// What the manifest records of a source is enough to name it again.
#[test]
fn a_recorded_source_becomes_a_file_to_read() {
    use phasor_writer::manifest::FileRecord;
    let rec = |source: &str, url: &str, path: &str| FileRecord {
        source: source.into(),
        url: url.into(),
        cache_path: path.into(),
        sha256: "s".into(),
        size: 1,
        etag: None,
        last_modified: None,
        last_modified_at: None,
        fetched_at: None,
    };
    let kits = kits_for(&Registry::load_embedded());
    let (kit, archived) = phasor_pipeline::recorded_remote(
        &kits,
        &rec(
            "nem-mmsdm",
            "https://h/2024/MMSDM_2024_01/DATA/PUBLIC_DVD_REGION_202401010000.zip",
            "/archive/h/2024/MMSDM_2024_01/DATA/PUBLIC_DVD_REGION_202401010000.zip",
        ),
    )
    .unwrap();
    assert_eq!((kit, archived.source), ("nem", "nem-mmsdm"));
    assert_eq!(archived.stem(), "public_dvd_region_202401010000");
    assert_eq!(archived.table.as_deref(), Some("REGION"));
    assert_eq!(archived.period.unwrap().to_string(), "2024-01");
    let (kit, local) =
        phasor_pipeline::recorded_remote(&kits, &rec("other", "file:///x/a.csv", "/x/a.csv"))
            .unwrap();
    assert_eq!((kit, local.source), ("nem", "local"));
    assert!(local.table.is_none());
    assert!(
        phasor_pipeline::recorded_remote(&kits, &rec("local", "not a url", "/x/a.csv")).is_none()
    );
    assert!(
        phasor_pipeline::recorded_remote(&kits, &rec("local", "file:///x/a.txt", "/x/a.txt"))
            .is_none()
    );
    assert!(phasor_pipeline::local_remote(&kits, Path::new("/x/a.ctl")).is_none());

    // An ingest recorded before rules were is never current.
    let catalog = RegistryCatalog::new(Registry::load_embedded(), Market::Nem);
    let old = phasor_kit::Transform::default();
    assert!(!catalog.plans_current(std::slice::from_ref(&old)));
    assert!(catalog.plans_current(&[]));
    // Nor is one whose shape no longer resolves.
    let mut broken = old;
    broken.plan_hash = "plan:0".into();
    assert!(!catalog.plans_current(&[broken]));
}

/// A file a kit's source names goes to that kit and is read by its rules:
/// a WEM datafile outside the feed's folders names no block, and the kit
/// says so.
#[tokio::test]
async fn a_file_of_a_kit_without_a_decoder_is_refused() {
    let mut e = env();
    let csv = e.home.root().join("facility-scada-2023-10.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    // A kit whose source claims the file but which has no decoder for it.
    let mut wem =
        phasor_wem::kit(&phasor_wem::WemConfig::default(), Registry::load_embedded()).unwrap();
    wem.decoders.clear();
    e.deps.kits = Arc::new(KitRegistry::new(vec![
        wem,
        phasor_nem::kit(e.registry.clone(), "https://h").unwrap(),
    ]));
    let plan = Plan {
        request: "ingest wem".into(),
        input: Input::Local(vec![csv.clone()]),
        options: RunOptions::default(),
    };
    let (rep, _) = run_plan(&e, plan).await;
    assert_eq!(rep.exit_code, 1, "{:?}", rep.report.summary_lines());
    let failed: Vec<_> = rep.report.failed().collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].failed_with.as_deref(), Some("config.no_decoder"));
    // With its decoders, the WEM kit reads the file by its rules: outside
    // the feed's folders its name says no block.
    e.set_registry(e.registry.clone());
    let plan = Plan {
        request: "ingest wem".into(),
        input: Input::Local(vec![csv.clone()]),
        options: RunOptions::default(),
    };
    let (rep, _) = run_plan(&e, plan).await;
    assert_eq!(rep.exit_code, 1, "{:?}", rep.report.summary_lines());
    let failed: Vec<_> = rep.report.failed().collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].failed_with.as_deref(), Some("csv.unnamed_block"));
    // The same bytes under a name no kit claims go to the fallback kit,
    // which reads them as MMS rows.
    let other = e.home.root().join("notes.csv");
    std::fs::write(&other, "a,b\n1,2\n").unwrap();
    let plan = Plan {
        request: "ingest other".into(),
        input: Input::Local(vec![other]),
        options: RunOptions::default(),
    };
    let (rep, _) = run_plan(&e, plan).await;
    assert_ne!(
        rep.report
            .failed()
            .next()
            .and_then(|f| f.failed_with.clone())
            .as_deref(),
        Some("config.no_decoder")
    );
}

/// A file handed to a kit the binary was not assembled with is refused
/// whole, before anything is read or written.
#[test]
fn a_file_for_a_kit_not_assembled_is_refused() {
    use phasor_pipeline::ingest_file::{IngestInput, ingest_file};
    let e = env();
    let csv = e.home.root().join("x.csv");
    std::fs::write(&csv, "C,NEMP.WORLD,TEST\n").unwrap();
    let remote = RemoteFile {
        url: Url::from_file_path(&csv).unwrap(),
        source: "local",
        table: None,
        period: None,
        size: None,
        modified: None,
        modified_at: None,
    };
    let rep = ingest_file(IngestInput {
        local: phasor_pipeline::run::local_file(remote, &csv).unwrap(),
        kits: e.deps.kits.clone(),
        kit: "nobody",
        lake_root: e.home.lake(),
        quarantine_root: e.home.quarantine(),
        tmp_dir: e.home.tmp(),
        options: RunOptions::default(),
        request: "ingest x.csv".into(),
        events: None,
        rules_changed: false,
        skip: Vec::new(),
    });
    let d = rep.file_error.unwrap();
    assert_eq!(d.code, "config.no_decoder");
    assert_eq!(d.request.as_deref(), Some("ingest x.csv"));
    assert!(rep.results.is_empty() && rep.entries.is_empty());
    assert!(walk(&e.home.lake()).is_empty());
}

#[tokio::test]
async fn a_zip_with_a_bad_entry_is_refused_whole_and_every_entry_is_read() {
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
    // Every entry was read, and the file is refused whole: the bad entry
    // with its own error, the others discarded for it.
    assert_ne!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.ingested, 0);
    assert_eq!(rep.summary.failed, 3);
    let failed: Vec<_> = rep.report.failed().collect();
    assert_eq!(failed.len(), 3);
    let bad = failed
        .iter()
        .find(|f| f.source.ends_with("!BAD.CSV"))
        .unwrap();
    assert_eq!(bad.failed_with.as_deref(), Some("csv.too_many_bad_rows"));
    for f in failed.iter().filter(|f| !f.source.ends_with("!BAD.CSV")) {
        assert_eq!(f.failed_with.as_deref(), Some("store.file_discarded"));
    }
    let bad_diag = rep.report.by_code.get("csv.too_many_bad_rows").unwrap();
    let d = &bad_diag.examples[0];
    assert_eq!(d.entry.as_deref(), Some("BAD.CSV"));
    assert!(d.request.as_deref().unwrap().starts_with("ingest"));
    assert!(d.url.as_deref().unwrap().starts_with("file://"));
    let discarded = &rep
        .report
        .by_code
        .get("store.file_discarded")
        .unwrap()
        .examples[0];
    assert!(
        discarded.message.contains("BAD.CSV"),
        "{}",
        discarded.message
    );
    assert!(discarded.request.as_deref().unwrap().starts_with("ingest"));
    // No Parquet is left, nor a temporary file. The quarantine keeps the
    // bad row to show why, and each shape read is a failed ingest, with
    // no files, naming the refusal.
    assert!(
        walk(&e.home.lake()).is_empty(),
        "{:?}",
        walk(&e.home.lake())
    );
    let q: Vec<_> = walk(&e.home.quarantine());
    assert_eq!(q.len(), 1, "{q:?}");
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    assert!(!ingests.is_empty());
    for i in &ingests {
        assert_eq!(i.status, phasor_writer::manifest::IngestStatus::Failed);
        assert!(i.outputs.is_empty());
        assert_eq!(i.error_code.as_deref(), Some("csv.too_many_bad_rows"));
    }
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
    assert_eq!(rep.summary.ingested, 0);
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
    // The quarantine holds the bad row, naming the entry it came from.
    let q: Vec<_> = walk(&e3.home.quarantine());
    assert_eq!(q.len(), 1, "{q:?}");
    let qtext = std::fs::read_to_string(&q[0]).unwrap();
    assert!(
        qtext.contains("csv.field_count_mismatch") && qtext.contains("extra"),
        "{qtext}"
    );
    assert!(
        qtext.contains("PUBLIC_DVD_MIXED_202401010000.zip!BAD.CSV"),
        "{qtext}"
    );
    // REGION is a snapshot table: partitioned by archive month; every
    // file ends with each row's source and publish time.
    let files = walk(&e3.home.lake());
    assert!(
        files
            .iter()
            .any(|f| f.contains("nem/region/data/snapshot=202401/")),
        "{files:?}"
    );
    assert!(!files.iter().any(|f| f.contains(".tmp-")), "{files:?}");
    let ingests = e3
        .deps
        .manifest
        .list_ingests(None, false, 10)
        .await
        .unwrap();
    let entries: Vec<&str> = ingests
        .iter()
        .flat_map(|i| &i.entries)
        .map(|x| x.entry.as_str())
        .collect();
    assert!(
        entries.contains(&"BAD.CSV") && entries.contains(&"GOOD.CSV"),
        "{entries:?}"
    );
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
        "test-one"
    }
    async fn list(&self, _q: &Query, _f: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
        Ok(self.0.clone())
    }
}

struct Broken;

#[async_trait]
impl Source for Broken {
    fn id(&self) -> &'static str {
        "test-broken"
    }
    async fn list(&self, _q: &Query, _f: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
        Err(FetchError::Offline {
            url: "https://h/".into(),
        })
    }
}

/// `fetch` downloads into the archive and writes nothing else; `ingest
/// --from` then reads the month from the archive, without the network.
#[tokio::test]
async fn fetch_only_downloads_and_ingest_reads_the_archived_month() {
    let e = env();
    let zip_tmp = e.home.root().join("z.zip");
    make_zip(
        &zip_tmp,
        &[(
            "PUBLIC_DVD_TRADINGPRICE_202401010000.CSV",
            &std::fs::read(fixtures().join("TRADINGPRICE_v2_head.csv")).unwrap(),
        )],
    );
    let nem = phasor_nem::MmsdmArchive::new("https://h").unwrap();
    let ym = phasor_core::YearMonth::new(2024, 1).unwrap();
    let url = nem
        .month_root(ym)
        .unwrap()
        .join("DATA/PUBLIC_DVD_TRADINGPRICE_202401010000.zip")
        .unwrap();
    e.transport
        .add(url.as_str(), Canned::ok(&std::fs::read(&zip_tmp).unwrap()));
    let query = |tables: &[&str]| Query {
        exclude: Vec::new(),
        tables: tables.iter().map(|t| (*t).to_owned()).collect(),
        range: TimeRange::months(ym, ym),
        instants: None,
    };
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "fetch".into(),
            input: Input::Remote {
                source: Arc::new(OneFile(vec![RemoteFile {
                    url: url.clone(),
                    source: "nem-mmsdm",
                    table: Some("TRADINGPRICE".into()),
                    period: Some(ym.into()),
                    size: None,
                    modified: None,
                    modified_at: None,
                }])),
                query: query(&[]),
            },
            options: RunOptions {
                ingest: false,
                ..Default::default()
            },
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!((rep.summary.downloaded, rep.summary.ingested), (1, 0));
    assert!(walk(&e.home.lake()).is_empty(), "nothing is written");
    assert!(
        e.deps
            .fetcher
            .archive()
            .lookup(url.as_str())
            .unwrap()
            .is_some()
    );

    let ingest = |tables: &[&str]| Plan {
        request: "ingest nem".into(),
        input: Input::Archive {
            source: Arc::new(nem.clone()),
            query: query(tables),
        },
        options: RunOptions::default(),
    };
    let requests = e.transport.request_count();
    let (rep, _) = run_plan(&e, ingest(&["REGION"])).await;
    assert_eq!(rep.summary.listed, 0, "another table's files are left");
    let (rep, _) = run_plan(&e, ingest(&[])).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!((rep.summary.listed, rep.summary.ingested), (1, 1));
    assert!(rep.summary.rows > 0);
    assert_eq!(e.transport.request_count(), requests, "no request");
    assert!(!walk(&e.home.lake()).is_empty());
}

/// Two URLs the archive keeps at one path (a folder listed on both sides
/// of nemweb) are fetched once in a run, never both into one `.part`.
#[tokio::test]
async fn urls_the_archive_keeps_at_one_path_are_fetched_once() {
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
    let ym = phasor_core::YearMonth::new(2024, 1).unwrap();
    let remote = |side: &str| {
        let url = format!(
            "https://www.nemweb.com.au/Reports/{side}/Trading/PUBLIC_DVD_TRADINGPRICE_202401010000.zip"
        );
        e.transport.add(&url, Canned::ok(&bytes));
        RemoteFile {
            url: Url::parse(&url).unwrap(),
            source: "nem-mmsdm",
            table: Some("TRADINGPRICE".into()),
            period: Some(ym.into()),
            size: None,
            modified: None,
            modified_at: None,
        }
    };
    let source = Arc::new(OneFile(vec![remote("CURRENT"), remote("ARCHIVE")]));
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "fetch nem".into(),
            input: Input::Remote {
                source,
                query: Query {
                    exclude: Vec::new(),
                    tables: vec![],
                    range: TimeRange::months(ym, ym),
                    instants: None,
                },
            },
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(
        rep.summary.downloaded,
        1,
        "{:?}",
        rep.report.summary_lines()
    );
    assert_eq!(e.transport.request_count(), 1);
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
    let ym = phasor_core::YearMonth::new(2024, 1).unwrap();
    let remote = |name: &str, table: &str| RemoteFile {
        url: Url::parse(&format!("https://h/{name}")).unwrap(),
        source: "nem-mmsdm",
        table: Some(table.into()),
        period: Some(ym.into()),
        size: None,
        modified: None,
        modified_at: None,
    };
    let source = Arc::new(OneFile(vec![
        remote("PUBLIC_DVD_TRADINGPRICE_202401010000.zip", "TRADINGPRICE"),
        remote("PUBLIC_DVD_REGION_202401010000.zip", "REGION"),
    ]));
    let query = Query {
        exclude: Vec::new(),
        tables: vec![],
        range: TimeRange::months(ym, ym),
        instants: None,
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
            "nem/tradingprice/data/year=2024/month=01/public_dvd_tradingprice_202401010000-v1-"
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

/// A whole-month zip read by its directory: the source picks the nested
/// zips of the tables asked for, or wants the zip whole.
struct RangedMonth {
    url: Url,
    whole: bool,
}

#[async_trait]
impl Source for RangedMonth {
    fn id(&self) -> &'static str {
        "nem-mmsdm"
    }
    async fn list(&self, _q: &Query, _f: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
        Ok(vec![RemoteFile {
            url: self.url.clone(),
            source: "nem-mmsdm",
            table: None,
            period: Some(phasor_core::YearMonth::new(2024, 1).unwrap().into()),
            size: None,
            modified: None,
            modified_at: None,
        }])
    }
    fn wants_ranged(&self, _remote: &RemoteFile, _query: &Query) -> bool {
        true
    }
    fn pick(
        &self,
        remote: &RemoteFile,
        entries: &[phasor_kit::Entry],
        query: &Query,
    ) -> phasor_kit::Picks {
        if self.whole {
            return phasor_kit::Picks::Whole;
        }
        phasor_kit::Picks::Entries(
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| query.tables.iter().any(|t| e.name.contains(t.as_str())))
                .map(|(index, e)| phasor_kit::Pick {
                    index,
                    remote: phasor_nem::describe_name(
                        Url::parse(&format!("{}!{}", remote.url, e.name)).unwrap(),
                    )
                    .unwrap(),
                })
                .collect(),
        )
    }
}

/// An entry fetched alone out of a whole-month zip lands in the lake under
/// its own name; a later read of the whole zip leaves it alone; the entry
/// read again alone after that is nothing new.
#[tokio::test]
async fn an_entry_picked_out_of_a_zip_and_the_whole_zip_do_not_double_up() {
    let e = env();
    let inner = |name: &str, fixture: &str| {
        let p = e.home.root().join(name);
        make_zip(
            &p,
            &[(
                &name.replace(".zip", ".CSV"),
                &std::fs::read(fixtures().join(fixture)).unwrap(),
            )],
        );
        std::fs::read(&p).unwrap()
    };
    let trading = inner(
        "PUBLIC_DVD_TRADINGPRICE_202401010000.zip",
        "TRADINGPRICE_v2_head.csv",
    );
    let dispatch = inner(
        "PUBLIC_DVD_DISPATCHPRICE_202401010000.zip",
        "DISPATCHPRICE_v4_head.csv",
    );
    let whole = e.home.root().join("MMSDM_2024_01.zip");
    {
        let f = std::fs::File::create(&whole).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("PUBLIC_DVD_TRADINGPRICE_202401010000.zip", stored)
            .unwrap();
        w.write_all(&trading).unwrap();
        w.start_file("PUBLIC_DVD_DISPATCHPRICE_202401010000.zip", stored)
            .unwrap();
        w.write_all(&dispatch).unwrap();
        // Padding, so one table's zip is well under half of the whole.
        w.start_file("README.txt", stored).unwrap();
        w.write_all(&vec![b'.'; 2 * (trading.len() + dispatch.len())])
            .unwrap();
        w.finish().unwrap();
    }
    let url =
        Url::parse("https://h/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01.zip")
            .unwrap();
    e.transport.add(
        url.as_str(),
        Canned::ranged(&std::fs::read(&whole).unwrap()),
    );
    let ym = phasor_core::YearMonth::new(2024, 1).unwrap();
    let plan = |whole: bool, tables: &[&str]| Plan {
        request: "get".into(),
        input: Input::Remote {
            source: Arc::new(RangedMonth {
                url: url.clone(),
                whole,
            }),
            query: Query {
                tables: tables.iter().map(|t| (*t).to_owned()).collect(),
                exclude: Vec::new(),
                range: TimeRange::months(ym, ym),
                instants: None,
            },
        },
        options: RunOptions::default(),
    };
    // 1. One table, picked out: its nested zip alone is fetched and read.
    let (rep, _) = run_plan(&e, plan(false, &["TRADINGPRICE"])).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!((rep.summary.downloaded, rep.summary.ingested), (1, 1));
    let entry_url = format!("{url}!PUBLIC_DVD_TRADINGPRICE_202401010000.zip");
    let archive = e.deps.fetcher.archive();
    assert!(archive.lookup(&entry_url).unwrap().is_some());
    assert!(
        archive.lookup(url.as_str()).unwrap().is_none(),
        "the zip is not downloaded whole"
    );
    let outputs = e
        .deps
        .manifest
        .live_outputs(Some("TRADINGPRICE"))
        .await
        .unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].url, entry_url);
    let rows_before: usize = outputs.iter().map(|o| parquet_rows(&o.path)).sum();
    // 2. The whole zip: the entry read before is left as it is, the rest
    // is read.
    let (rep, _) = run_plan(&e, plan(true, &[])).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert!(
        rep.report.by_code.contains_key("manifest.already_ingested"),
        "{:?}",
        rep.report.by_code.keys()
    );
    let outputs = e
        .deps
        .manifest
        .live_outputs(Some("TRADINGPRICE"))
        .await
        .unwrap();
    assert_eq!(outputs.len(), 1, "no second copy of the table's rows");
    assert_eq!(
        outputs.iter().map(|o| parquet_rows(&o.path)).sum::<usize>(),
        rows_before
    );
    assert!(
        !e.deps
            .manifest
            .live_outputs(Some("DISPATCHPRICE"))
            .await
            .unwrap()
            .is_empty()
    );
    // 3. The entry alone again: the archive has the zip whole and the lake
    // has its rows; nothing is fetched or read.
    let (rep, _) = run_plan(&e, plan(false, &["TRADINGPRICE"])).await;
    assert_eq!(
        (
            rep.summary.downloaded,
            rep.summary.ingested,
            rep.summary.skipped
        ),
        (0, 0, 1),
        "{:?}",
        rep.report.summary_lines()
    );
    // 4. The archive pruned of the whole zip, and another table asked for
    // alone: its entry is fetched, and then found to be in the lake from
    // the read of the whole zip, so it is not read again.
    std::fs::remove_file(archive.path_for(url.as_str())).unwrap();
    std::fs::remove_file(phasor_acquire::Archive::record_path(
        &archive.path_for(url.as_str()),
    ))
    .unwrap();
    let (rep, _) = run_plan(&e, plan(false, &["DISPATCHPRICE"])).await;
    assert_eq!(
        (
            rep.summary.downloaded,
            rep.summary.ingested,
            rep.summary.skipped
        ),
        (0, 0, 1),
        "{:?}",
        rep.report.summary_lines()
    );
    assert!(
        archive
            .lookup(&format!("{url}!PUBLIC_DVD_DISPATCHPRICE_202401010000.zip"))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        e.deps
            .manifest
            .live_outputs(Some("DISPATCHPRICE"))
            .await
            .unwrap()
            .len(),
        1
    );
}

/// A monthly source whose archive has published up to, not including,
/// `unpublished_from`: one file per table per month.
struct Monthly {
    unpublished_from: Option<phasor_core::YearMonth>,
    whole_zip: Option<Url>,
}

/// A monthly source that says a month is missing that was not asked for.
struct OddMonthly;

#[async_trait]
impl Source for OddMonthly {
    fn id(&self) -> &'static str {
        "nem-mmsdm"
    }
    async fn list(&self, _q: &Query, _f: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
        Err(FetchError::MonthMissing {
            period: "no such".into(),
            url: "https://h/year/".into(),
            available: String::new(),
        })
    }
}

#[async_trait]
impl Source for Monthly {
    fn id(&self) -> &'static str {
        "nem-mmsdm"
    }
    async fn list(&self, q: &Query, _f: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        for m in q.range.months_iter() {
            if self.unpublished_from.is_some_and(|u| m >= u) {
                return Err(FetchError::MonthMissing {
                    period: m.to_string(),
                    url: "https://h/year/".into(),
                    available: "earlier months".into(),
                });
            }
            if let Some(whole) = &self.whole_zip {
                out.push(RemoteFile {
                    url: whole.clone(),
                    source: "nem-mmsdm",
                    table: None,
                    period: Some(m.into()),
                    size: Some(3000),
                    modified: None,
                    modified_at: None,
                });
                continue;
            }
            for t in &q.tables {
                out.push(RemoteFile {
                    url: Url::parse(&format!("https://h/m/{m}/{t}.zip")).unwrap(),
                    source: "nem-mmsdm",
                    table: Some(t.clone()),
                    period: Some(m.into()),
                    size: Some(1000),
                    modified: None,
                    modified_at: None,
                });
            }
        }
        Ok(out)
    }
    fn wants_ranged(&self, remote: &RemoteFile, _query: &Query) -> bool {
        self.whole_zip.as_ref() == Some(&remote.url)
    }
    fn pick(
        &self,
        remote: &RemoteFile,
        entries: &[phasor_kit::Entry],
        query: &Query,
    ) -> phasor_kit::Picks {
        phasor_kit::Picks::Entries(
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| query.tables.iter().any(|t| e.name.contains(t.as_str())))
                .map(|(index, e)| phasor_kit::Pick {
                    index,
                    remote: phasor_nem::describe_name(
                        Url::parse(&format!("{}!{}", remote.url, e.name)).unwrap(),
                    )
                    .unwrap(),
                })
                .collect(),
        )
    }
}

/// A daily source: one file per day for every table but ONLY_MONTHLY.
struct Daily;

#[async_trait]
impl Source for Daily {
    fn id(&self) -> &'static str {
        "nem-reports"
    }
    fn granularity(&self) -> phasor_kit::Granularity {
        phasor_kit::Granularity::Day
    }
    async fn list(&self, q: &Query, _f: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
        if q.tables.iter().any(|t| t == "BROKEN") {
            return Err(FetchError::Offline {
                url: "https://h/Reports/".into(),
            });
        }
        if let Some(t) = q.tables.iter().find(|t| *t == "ONLY_MONTHLY") {
            return Err(FetchError::NoMatch {
                url: "https://h/Reports/".into(),
                entries: 0,
                table: t.clone(),
                period: q.range.to_string(),
                nearest: "no report folder carries it; the monthly archive does".into(),
            });
        }
        Ok(q.range
            .days_iter()
            .map(|d| RemoteFile {
                url: Url::parse(&format!("https://h/d/{d}.zip")).unwrap(),
                source: "nem-reports",
                table: None,
                period: Some(d.into()),
                size: Some(10),
                modified: None,
                modified_at: None,
            })
            .collect())
    }
}

fn kit_with(e: &Env, sources: Vec<Arc<dyn Source>>) -> Arc<KitRegistry> {
    let mut nem = phasor_nem::kit(e.registry.clone(), "https://h").unwrap();
    nem.sources = sources;
    Arc::new(KitRegistry::new(vec![nem]))
}

fn get_request(
    tables: &[&str],
    from: &str,
    to: &str,
    source: phasor_pipeline::SourceChoice,
) -> phasor_pipeline::GetRequest {
    let day = |s: &str| s.parse::<phasor_core::CivilDay>().unwrap();
    let (a, b) = (day(from), day(to));
    phasor_pipeline::GetRequest {
        kit: "nem".into(),
        tables: tables.iter().map(|t| (*t).to_owned()).collect(),
        exclude: Vec::new(),
        range: TimeRange::days(a, b),
        instants: (a.start_micros(36_000), b.next().start_micros(36_000)),
        source,
        request: "get".into(),
    }
}

fn step_names(plan: &phasor_pipeline::GetPlan) -> Vec<String> {
    plan.steps
        .iter()
        .map(|s| format!("{} {} {}", s.source.id(), s.query.range, s.files.len()))
        .collect()
}

#[tokio::test]
async fn an_interval_is_cut_into_published_months_and_the_days_around_them() {
    use phasor_pipeline::{SourceChoice, plan_get};
    let mut e = env();
    let month = |y, m| phasor_core::YearMonth::new(y, m).unwrap();
    // Everything published: February whole from the archive, the ends daily.
    e.deps.kits = kit_with(
        &e,
        vec![
            Arc::new(Monthly {
                unpublished_from: None,
                whole_zip: None,
            }),
            Arc::new(Daily),
        ],
    );
    let plan = plan_get(
        &get_request(&["A", "B"], "2024-01-15", "2024-03-10", SourceChoice::Auto),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(
        step_names(&plan),
        [
            "nem-mmsdm 2024-02 2",
            "nem-reports 2024-01-15..2024-01-31 17",
            "nem-reports 2024-03-01..2024-03-10 10"
        ]
    );
    assert!(plan.notes.is_empty(), "{:?}", plan.notes);
    assert!(plan.steps[0].query.instants.is_none());
    assert_eq!(
        plan.steps[1].query.instants,
        Some(get_request(&[], "2024-01-15", "2024-03-10", SourceChoice::Auto).instants)
    );
    assert_eq!(plan.steps[0].bytes(), 2000);
    // February not published yet: its days go daily too.
    e.deps.kits = kit_with(
        &e,
        vec![
            Arc::new(Monthly {
                unpublished_from: Some(month(2024, 2)),
                whole_zip: None,
            }),
            Arc::new(Daily),
        ],
    );
    let plan = plan_get(
        &get_request(&["A"], "2024-01-15", "2024-03-10", SourceChoice::Auto),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(
        step_names(&plan),
        [
            "nem-reports 2024-01-15..2024-01-31 17",
            "nem-reports 2024-02-01..2024-02-29 29",
            "nem-reports 2024-03-01..2024-03-10 10"
        ]
    );
    assert_eq!(
        plan.notes,
        ["2024-02 is not published in the monthly archive yet"]
    );
    // Two whole months, the later one unpublished: the first from the
    // archive, the second daily.
    e.deps.kits = kit_with(
        &e,
        vec![
            Arc::new(Monthly {
                unpublished_from: Some(month(2024, 3)),
                whole_zip: None,
            }),
            Arc::new(Daily),
        ],
    );
    let plan = plan_get(
        &get_request(&["A"], "2024-02-01", "2024-03-31", SourceChoice::Auto),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(
        step_names(&plan),
        [
            "nem-mmsdm 2024-02 1",
            "nem-reports 2024-03-01..2024-03-31 31"
        ]
    );
    // A table no report folder carries: its days come from the archive's
    // months; the other table's stay daily.
    e.deps.kits = kit_with(
        &e,
        vec![
            Arc::new(Monthly {
                unpublished_from: None,
                whole_zip: None,
            }),
            Arc::new(Daily),
        ],
    );
    let plan = plan_get(
        &get_request(
            &["A", "ONLY_MONTHLY"],
            "2024-01-15",
            "2024-01-20",
            SourceChoice::Auto,
        ),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(
        step_names(&plan),
        [
            "nem-reports 2024-01-15..2024-01-20 6",
            "nem-mmsdm 2024-01 1"
        ]
    );
    assert_eq!(plan.steps[0].query.tables, ["A"]);
    assert_eq!(plan.steps[1].query.tables, ["ONLY_MONTHLY"]);
    assert!(
        plan.notes
            .iter()
            .any(|n| n.starts_with("ONLY_MONTHLY: no report folder")),
        "{:?}",
        plan.notes
    );
    // Forced sides.
    let plan = plan_get(
        &get_request(&["A"], "2024-01-15", "2024-03-10", SourceChoice::Monthly),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(step_names(&plan), ["nem-mmsdm 2024-01..2024-03 3"]);
    let plan = plan_get(
        &get_request(&["A"], "2024-01-15", "2024-03-10", SourceChoice::Daily),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(step_names(&plan), ["nem-reports 2024-01-15..2024-03-10 56"]);
    assert_eq!(plan.notes, ["every day from the report folders, as asked"]);
    // A kit without a daily source: whole months cover the days.
    e.deps.kits = kit_with(
        &e,
        vec![Arc::new(Monthly {
            unpublished_from: None,
            whole_zip: None,
        })],
    );
    let plan = plan_get(
        &get_request(&["A"], "2024-01-15", "2024-01-20", SourceChoice::Auto),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(step_names(&plan), ["nem-mmsdm 2024-01 1"]);
    assert!(
        plan.notes.iter().any(|n| n.contains("no daily source")),
        "{:?}",
        plan.notes
    );
    let e2 = plan_get(
        &get_request(&["A"], "2024-01-15", "2024-01-20", SourceChoice::Daily),
        &e.deps,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(e2.message.contains("no daily source"), "{}", e2.message);
    // A kit without a monthly source: every day daily.
    e.deps.kits = kit_with(&e, vec![Arc::new(Daily)]);
    let plan = plan_get(
        &get_request(&["A"], "2024-01-15", "2024-03-10", SourceChoice::Auto),
        &e.deps,
    )
    .await
    .unwrap();
    assert_eq!(step_names(&plan), ["nem-reports 2024-01-15..2024-03-10 56"]);
    let e3 = plan_get(
        &get_request(&["A"], "2024-01-15", "2024-01-20", SourceChoice::Monthly),
        &e.deps,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(e3.message.contains("no monthly source"), "{}", e3.message);
    // A daily source that fails on a table: reported, not worked around.
    e.deps.kits = kit_with(
        &e,
        vec![
            Arc::new(Monthly {
                unpublished_from: None,
                whole_zip: None,
            }),
            Arc::new(Daily),
        ],
    );
    let e4 = plan_get(
        &get_request(&["BROKEN"], "2024-01-15", "2024-01-20", SourceChoice::Auto),
        &e.deps,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert_eq!(e4.code, "net.offline");
    // An archive that names a month nobody asked for as missing.
    e.deps.kits = kit_with(&e, vec![Arc::new(OddMonthly), Arc::new(Daily)]);
    let e5 = plan_get(
        &get_request(&["A"], "2024-02-01", "2024-02-29", SourceChoice::Auto),
        &e.deps,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert_eq!(e5.code, "listing.month_missing");
    assert!(e5.message.contains("no such"), "{}", e5.message);
    // An unknown kit.
    let mut bad = get_request(&["A"], "2024-01-15", "2024-01-20", SourceChoice::Auto);
    bad.kit = "gas".into();
    assert!(
        plan_get(&bad, &e.deps)
            .await
            .map(|_| ())
            .unwrap_err()
            .message
            .contains("No kit named gas")
    );
    // The days an interval touches, in market time.
    let d = |s: &str| s.parse::<phasor_core::CivilDay>().unwrap();
    let start = d("2024-01-15").start_micros(36_000);
    assert_eq!(
        phasor_pipeline::days_of((start, start + 7_200_000_000), 36_000),
        TimeRange::days(d("2024-01-15"), d("2024-01-15"))
    );
    assert_eq!(
        phasor_pipeline::days_of((start, d("2024-01-17").start_micros(36_000)), 36_000),
        TimeRange::days(d("2024-01-15"), d("2024-01-16")),
        "an end at midnight is not a new day"
    );
    assert_eq!(
        phasor_pipeline::days_of((start, start), 36_000),
        TimeRange::days(d("2024-01-15"), d("2024-01-15"))
    );
}

/// `get` over a whole-month zip read by its directory: the plan's one step
/// runs over the files the archive listed, and the table's entry alone
/// lands in the lake.
#[tokio::test]
async fn get_runs_each_step_over_what_its_source_listed() {
    use phasor_pipeline::{SourceChoice, run_get};
    let mut e = env();
    let inner = |name: &str, fixture: &str| {
        let p = e.home.root().join(name);
        make_zip(
            &p,
            &[(
                &name.replace(".zip", ".CSV"),
                &std::fs::read(fixtures().join(fixture)).unwrap(),
            )],
        );
        std::fs::read(&p).unwrap()
    };
    let trading = inner(
        "PUBLIC_DVD_TRADINGPRICE_202401010000.zip",
        "TRADINGPRICE_v2_head.csv",
    );
    let dispatch = inner(
        "PUBLIC_DVD_DISPATCHPRICE_202401010000.zip",
        "DISPATCHPRICE_v4_head.csv",
    );
    let whole = e.home.root().join("MMSDM_2024_01.zip");
    {
        let f = std::fs::File::create(&whole).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("PUBLIC_DVD_TRADINGPRICE_202401010000.zip", stored)
            .unwrap();
        w.write_all(&trading).unwrap();
        w.start_file("PUBLIC_DVD_DISPATCHPRICE_202401010000.zip", stored)
            .unwrap();
        w.write_all(&dispatch).unwrap();
        w.start_file("README.txt", stored).unwrap();
        w.write_all(&vec![b'.'; 2 * (trading.len() + dispatch.len())])
            .unwrap();
        w.finish().unwrap();
    }
    let url =
        Url::parse("https://h/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01.zip")
            .unwrap();
    e.transport.add(
        url.as_str(),
        Canned::ranged(&std::fs::read(&whole).unwrap()),
    );
    e.deps.kits = kit_with(
        &e,
        vec![Arc::new(Monthly {
            unpublished_from: None,
            whole_zip: Some(url.clone()),
        })],
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let report = run_get(
        get_request(
            &["TRADINGPRICE"],
            "2024-01-01",
            "2024-01-31",
            SourceChoice::Auto,
        ),
        e.deps.clone(),
        RunOptions::default(),
        tx,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let mut events = 0;
    while rx.try_recv().is_ok() {
        events += 1;
    }
    assert!(events > 0);
    assert_eq!(
        report.exit_code,
        0,
        "{:?}",
        report.runs[0].report.summary_lines()
    );
    assert_eq!(step_names(&report.plan), ["nem-mmsdm 2024-01 1"]);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(
        (
            report.runs[0].summary.downloaded,
            report.runs[0].summary.ingested
        ),
        (1, 1)
    );
    assert!(
        e.deps
            .fetcher
            .archive()
            .lookup(&format!("{url}!PUBLIC_DVD_TRADINGPRICE_202401010000.zip"))
            .unwrap()
            .is_some()
    );
    assert!(
        e.deps
            .fetcher
            .archive()
            .lookup(url.as_str())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        e.deps
            .manifest
            .live_outputs(Some("TRADINGPRICE"))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        e.deps
            .manifest
            .live_outputs(Some("DISPATCHPRICE"))
            .await
            .unwrap()
            .is_empty()
    );
    // Dry run: the plan and the listing, nothing fetched.
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    let report = run_get(
        get_request(
            &["DISPATCHPRICE"],
            "2024-01-01",
            "2024-01-31",
            SourceChoice::Auto,
        ),
        e.deps.clone(),
        RunOptions {
            dry_run: true,
            ..RunOptions::default()
        },
        tx,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.runs[0].listed.len(), 1);
    assert_eq!(report.runs[0].summary.downloaded, 0);
    assert!(
        e.deps
            .fetcher
            .archive()
            .lookup(&format!("{url}!PUBLIC_DVD_DISPATCHPRICE_202401010000.zip"))
            .unwrap()
            .is_none()
    );
    // A cancelled run stops after its first step.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (tx, _rx) = tokio::sync::mpsc::channel(4096);
    e.deps.kits = kit_with(
        &e,
        vec![
            Arc::new(Monthly {
                unpublished_from: None,
                whole_zip: None,
            }),
            Arc::new(Daily),
        ],
    );
    let report = run_get(
        get_request(&["A"], "2024-01-15", "2024-03-10", SourceChoice::Auto),
        e.deps.clone(),
        RunOptions::default(),
        tx,
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(report.plan.steps.len(), 3);
    assert_eq!(report.runs.len(), 1, "stopped after the first step");
}

/// A snapshot table fed from a source that cuts its files by day (a
/// registry a feed publishes daily) is partitioned by that day.
#[tokio::test]
async fn a_days_snapshot_is_partitioned_by_its_day() {
    let e = env();
    let url = Url::parse("https://h/daily/PUBLIC_DVD_REGION_202401070000.CSV").unwrap();
    e.transport.add(
        url.as_str(),
        Canned::ok(&std::fs::read(fixtures().join("REGION_v1_head.csv")).unwrap()),
    );
    let day: phasor_core::CivilDay = "2024-01-07".parse().unwrap();
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest daily".into(),
            input: Input::Remote {
                source: Arc::new(OneFile(vec![RemoteFile {
                    url: url.clone(),
                    source: "test-one",
                    table: Some("REGION".into()),
                    period: Some(phasor_core::Period::Day(day)),
                    size: None,
                    modified: None,
                    modified_at: None,
                }])),
                query: Query {
                    exclude: Vec::new(),
                    tables: Vec::new(),
                    range: TimeRange::days(day, day),
                    instants: None,
                },
            },
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    assert!(
        files
            .iter()
            .any(|f| f.contains("nem/region/data/snapshot=20240107/")),
        "{files:?}"
    );
    let outputs = e.deps.manifest.live_outputs(Some("REGION")).await.unwrap();
    assert!(
        outputs.iter().any(|o| o.path.contains("snapshot=20240107")),
        "{outputs:?}"
    );
}

/// A feed whose files restate the one before: the archive holds two
/// versions of a GBB file, as the mirror keeps them; the second ingests
/// only the rows the first lacked, and the manifest tells the versions
/// apart.
#[tokio::test]
async fn a_restated_file_adds_only_its_new_rows() {
    let mut e = env();
    let registry = e.registry.clone();
    e.deps.kits = Arc::new(KitRegistry::new(vec![
        phasor_gas::kit(registry.clone(), &phasor_gas::GasConfig::default()).unwrap(),
        phasor_nem::kit(registry.clone(), "https://h").unwrap(),
    ]));
    let archive = Archive::new(e.home.archive());
    let url = "https://www.nemweb.com.au/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV";
    let v1 = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/plain/gbb_actual_flow_storage_last31.csv"),
    )
    .unwrap();
    let text = String::from_utf8(v1.clone()).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    let added = lines[1].replace("2026/09/20", "2026/09/21");
    lines.push(&added);
    let v2 = lines.join("\n");
    let keep = |version: Option<&str>, bytes: &[u8], listed: &str| {
        let base = archive.path_for(url);
        let path = match version {
            Some(v) => {
                let mut name = base.file_name().unwrap().to_os_string();
                name.push(format!("@{v}"));
                base.with_file_name(name)
            }
            None => base,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let record = phasor_acquire::Provenance::new(
            phasor_acquire::Content::of(bytes),
            "test",
            phasor_acquire::Address {
                url: url.to_owned(),
                seen: phasor_core::timetext::Stamp::new(1_792_540_800, 0),
                gone: None,
                via: phasor_acquire::Via::Fetched {
                    listed: Some(phasor_acquire::archive::Listed {
                        size: None,
                        at: phasor_core::timetext::parse_iis_listing(listed, 36_000),
                    }),
                    served: None,
                },
            },
        );
        phasor_acquire::Archive::write(&path, &record).unwrap();
    };
    keep(None, &v1, "Monday, October 20, 2026 10:00 AM");
    keep(
        Some("20261021T191100+1000"),
        v2.as_bytes(),
        "Tuesday, October 21, 2026 7:11 PM",
    );
    let gbb = e
        .deps
        .kits
        .source("gas-gbb")
        .map(|(_, s)| s.clone())
        .unwrap();
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest gas".into(),
            input: Input::Archive {
                source: gbb,
                query: Query {
                    exclude: Vec::new(),
                    tables: vec!["GBB_ACTUAL_FLOW_STORAGE".into()],
                    range: TimeRange::months(
                        phasor_core::YearMonth::new(2026, 10).unwrap(),
                        phasor_core::YearMonth::new(2026, 10).unwrap(),
                    ),
                    instants: None,
                },
            },
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.ingested, 2);
    // Two rows from the first version, one new from the second.
    assert_eq!(rep.summary.rows, 3, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    assert!(
        files
            .iter()
            .all(|f| f.contains("gas/gbb_actual_flow_storage/data/year=2026/month=09/")),
        "{files:?}"
    );
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    let urls: Vec<&str> = ingests.iter().map(|i| i.url.as_str()).collect();
    assert!(urls.contains(&url), "{urls:?}");
    assert!(
        urls.iter().any(|u| u.ends_with("#20261021T191100+1000")),
        "{urls:?}"
    );
    // The kept version as a local path is the file's URL with the stamp as
    // its fragment, claimed by the GBB source.
    let version = archive
        .path_for(url)
        .with_file_name("GasBBActualFlowStorageLast31.CSV@20261021T191100+1000");
    let (kit, local) = phasor_pipeline::local_remote(&e.deps.kits, &version).unwrap();
    assert_eq!(kit, "gas");
    assert!(
        local
            .url
            .as_str()
            .ends_with("/GasBBActualFlowStorageLast31.CSV#20261021T191100+1000"),
        "{}",
        local.url
    );
    assert_eq!(local.table.as_deref(), Some("GBB_ACTUAL_FLOW_STORAGE"));
}

/// Files read by a named kit, whatever claims them by name: a WEM
/// datafile's name read as MMS rows by the NEM kit; a kit the binary lacks
/// reads nothing and says so.
#[tokio::test]
async fn local_files_read_by_a_named_kit() {
    let e = env();
    let csv = e.home.root().join("facility-scada-2023-10.csv");
    std::fs::copy(fixtures().join("REGION_v1_head.csv"), &csv).unwrap();
    let (kit, remote) = phasor_pipeline::local_remote_in(&e.deps.kits, "nem", &csv).unwrap();
    assert_eq!((kit, remote.source), ("nem", "local"));
    assert!(phasor_pipeline::local_remote_in(&e.deps.kits, "mars", &csv).is_none());
    let txt = e.home.root().join("notes.txt");
    std::fs::write(&txt, "x").unwrap();
    assert!(phasor_pipeline::local_remote_in(&e.deps.kits, "nem", &txt).is_none());
    let run_in = |kit: &str, paths: Vec<PathBuf>| Plan {
        request: format!("import --kit {kit}"),
        input: Input::LocalIn {
            kit: kit.into(),
            paths,
        },
        options: RunOptions::default(),
    };
    let (rep, _) = run_plan(&e, run_in("nem", vec![csv.clone()])).await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.ingested, 1);
    let (rep, _) = run_plan(&e, run_in("mars", vec![csv])).await;
    assert_eq!(rep.summary.listed, 0);
    let d = &rep.report.by_code["config.bad_path"].examples[0];
    assert!(d.message.contains("the mars kit"), "{}", d.message);
}

/// One field's text in an MMS row, for row `i` of three, by the type its
/// table gives it: the widest value the type holds, an awkward one, and
/// the empty field where the column allows it.
fn synthetic_value(ty: LakeType, i: usize, required: bool) -> String {
    let empty_or = |v: &str| {
        if required {
            v.to_owned()
        } else {
            String::new()
        }
    };
    match (ty, i) {
        (LakeType::String, 0) => "\"a, \"\"b\"\"\"".into(),
        (LakeType::String, 1) => "X".into(),
        (LakeType::String, _) => empty_or("Y"),
        (LakeType::Double, 0) => "1.5".into(),
        (LakeType::Double, 1) => "-2.5E3".into(),
        (LakeType::Double, _) => empty_or("0"),
        (LakeType::Long, 0) => "9223372036854775807".into(),
        (LakeType::Long, 1) => "-7".into(),
        (LakeType::Long, _) => empty_or("0"),
        (LakeType::Boolean, 0) => "1".into(),
        (LakeType::Boolean, 1) => "0".into(),
        (LakeType::Boolean, _) => empty_or("1"),
        (LakeType::Date, n) => format!("\"2024/01/0{} 00:00:00\"", n + 1),
        (LakeType::Timestamp, n) => format!("\"2024/01/15 10:{n}5:00\""),
        (LakeType::Decimal { precision, scale }, 0) => {
            let int = usize::from(precision - scale);
            let int = if int == 0 {
                "0".to_owned()
            } else {
                "9".repeat(int)
            };
            if scale == 0 {
                int
            } else {
                format!("{int}.{}", "9".repeat(usize::from(scale)))
            }
        }
        (LakeType::Decimal { scale, .. }, 1) if scale > 0 => format!("-1E-{scale}"),
        (LakeType::Decimal { .. }, 1) => "-1".into(),
        (LakeType::Decimal { .. }, _) => empty_or("0"),
    }
}

/// A file of one run of a table: its I row as the run's report pair and
/// version, three D rows of every column, and the footer.
fn synthetic_mms(table: &Table, run: &Run) -> String {
    let schema = table
        .schemas
        .iter()
        .find(|s| s.id == run.schema)
        .expect("a run's schema");
    let (rt, st) = &run.report;
    let columns: Vec<(&Column, bool)> = schema
        .fields
        .iter()
        .map(|(id, required)| {
            let c = table.column_by_id(*id).expect("a schema's column");
            (
                c,
                *required || table.primary_key.contains(&c.name) || !c.nullable,
            )
        })
        .collect();
    let names: Vec<&str> = columns.iter().map(|(c, _)| c.name.as_str()).collect();
    let mut out = format!(
        "C,NEMP.WORLD,SYNTHETIC\nI,{rt},{st},{},{}\n",
        run.version,
        names.join(",")
    );
    for i in 0..3 {
        let values: Vec<String> = columns
            .iter()
            .map(|(c, required)| synthetic_value(c.ty, i, *required))
            .collect();
        writeln!(out, "D,{rt},{st},{},{}", run.version, values.join(",")).unwrap();
    }
    out.push_str("C,\"END OF REPORT\",6\n");
    out
}

/// Every NEM table of the registry, as its latest and its oldest runs
/// write it, goes into the lake: one zip, an entry per run, three rows of
/// every column each. Each file holds its table's columns under their field ids, then
/// each row's source and publish time; no value is turned to text; each
/// source's rows are all there.
#[tokio::test]
async fn every_table_survives_the_lake() {
    use arrow_array::Array as _;
    let e = env();
    let registry = e.registry.clone();
    let tables: Vec<(&str, String, String)> = registry
        .tables()
        .filter(|(id, t)| {
            t.market == Market::Nem && !t.runs.is_empty() && !registry.is_derived(*id)
        })
        .flat_map(|(_, t)| {
            // The latest run, and the oldest where its shape differs.
            let last = t.runs.last().unwrap();
            let first = t.runs.first().unwrap();
            let mut runs = vec![("LAST", last)];
            if first.schema != last.schema || first.version != last.version {
                runs.push(("FIRST", first));
            }
            runs.into_iter().map(move |(which, run)| {
                (
                    t.name.as_str(),
                    format!("SYN_{}_{which}.CSV", t.name),
                    synthetic_mms(t, run),
                )
            })
        })
        .collect();
    assert!(tables.len() > 200, "{}", tables.len());
    let zip_path = e.home.root().join("PUBLIC_DVD_SYNTHETIC_202401010000.zip");
    let entries: Vec<(&str, &[u8])> = tables
        .iter()
        .map(|(_, entry, text)| (entry.as_str(), text.as_bytes()))
        .collect();
    make_zip(&zip_path, &entries);
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest every table".into(),
            input: Input::Local(vec![zip_path.clone()]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.summary.failed, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.bad_rows, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.rows, 3 * tables.len() as u64);
    let files = walk(&e.home.lake());
    let mut names: Vec<&str> = tables.iter().map(|(n, _, _)| *n).collect();
    names.dedup();
    let mut seen = 0;
    for name in names {
        let entries: Vec<&str> = tables
            .iter()
            .filter(|(n, _, _)| *n == name)
            .map(|(_, e, _)| e.as_str())
            .collect();
        let id = registry.table_by_name(name).unwrap();
        let dir = format!("/{}/{}/", Market::Nem, registry.table_dir(id));
        let mine: Vec<&String> = files.iter().filter(|f| f.contains(&dir)).collect();
        assert!(!mine.is_empty(), "{name}: no file under {dir}");
        let lake = registry.lake_schema(id);
        let id_of = |f: &arrow_schema::Field| {
            f.metadata()
                .get("PARQUET:field_id")
                .cloned()
                .unwrap_or_default()
        };
        let mut rows = 0;
        for f in mine {
            let b = parquet_batch(f);
            let schema = b.schema();
            let fields = schema.fields();
            let n = fields.len();
            let tail: Vec<&str> = fields[n - 4..].iter().map(|f| f.name().as_str()).collect();
            assert_eq!(
                tail,
                [
                    "_phasor_source",
                    "_phasor_server_last_modified",
                    "_phasor_zip_entry_modified",
                    "_phasor_filename_timestamp"
                ],
                "{name}"
            );
            let ids: Vec<String> = fields[n - 4..].iter().map(|f| id_of(f)).collect();
            assert_eq!(
                ids,
                ["2147483000", "2147483001", "2147483002", "2147483003"],
                "{name}"
            );
            for field in &fields[..n - 4] {
                assert!(
                    !field.name().ends_with("__raw"),
                    "{name}: {} kept as text",
                    field.name()
                );
                let declared = lake
                    .field_with_name(field.name())
                    .unwrap_or_else(|_| panic!("{name}: {} not in the lake schema", field.name()));
                assert_eq!(id_of(field), id_of(declared), "{name}.{}", field.name());
            }
            let source = b
                .column_by_name("_phasor_source")
                .unwrap()
                .as_any()
                .downcast_ref::<arrow_array::DictionaryArray<arrow_array::types::UInt16Type>>()
                .unwrap()
                .clone();
            let values = source
                .values()
                .as_any()
                .downcast_ref::<arrow_array::StringArray>()
                .unwrap();
            for k in source.keys().values() {
                let v = values.value(usize::from(*k));
                assert!(
                    entries.iter().any(|e| v.ends_with(&format!("!{e}"))),
                    "{name}: {v}"
                );
            }
            rows += b.num_rows();
        }
        assert_eq!(rows, 3 * entries.len(), "{name}");
        seen += entries.len();
    }
    assert_eq!(seen, tables.len());
}

/// A file is closed for the next once it holds `file_size_mb`: the rows
/// go on in a further part, each still naming its source, none lost.
#[tokio::test]
async fn a_file_is_cut_at_its_size_and_the_rows_go_on() {
    let e = env();
    let mut csv =
        String::from("C,NEMP.WORLD,TEST\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\n");
    let rows = 300_000;
    // Values that do not compress, so the file grows past its size.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..rows {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        writeln!(
            csv,
            "D,DISPATCH,UNIT_SCADA,1,\"2024/01/{:02} {:02}:{:02}:00\",U{x:016x},{}.{:03}",
            1 + (i / 7_200) % 28,
            (i / 60) % 24,
            i % 60,
            x % 100_000,
            (x >> 20) % 1000
        )
        .unwrap();
    }
    writeln!(csv, "C,\"END OF REPORT\",{}", rows + 3).unwrap();
    let zip_path = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip");
    make_zip(&zip_path, &[("A.CSV", csv.as_bytes()), ("B.CSV", b"C,NEMP.WORLD,TEST\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/31 00:05:00\",Z,1\nC,\"END OF REPORT\",4\n")]);
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest big".into(),
            input: Input::Local(vec![zip_path]),
            options: RunOptions {
                layout: phasor_core::config::Lake {
                    file_size_mb: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    let sizes: Vec<u64> = files
        .iter()
        .map(|f| std::fs::metadata(f).unwrap().len())
        .collect();
    assert!(files.len() >= 2, "{files:?} {sizes:?}");
    assert!(
        files.iter().any(|f| f.ends_with("-p2.parquet")),
        "{files:?}"
    );
    let total: usize = files.iter().map(|f| parquet_rows(f)).sum();
    assert_eq!(total, rows + 1);
    // The manifest has both entries' shares, adding up to every row.
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    let shares: u64 = ingests
        .iter()
        .flat_map(|i| &i.entries)
        .map(|x| x.rows)
        .sum();
    assert_eq!(shares, rows as u64 + 1);
    assert_eq!(ingests[0].outputs.len(), files.len());
}

/// A plain CSV row whose every field is empty is a blank line, and a
/// blank line is no row in any CSV: a one-column file with an empty value
/// reads as one row fewer. Not a loss: AEMO cannot publish such a row
/// either, as the bytes are the same.
#[tokio::test]
async fn a_blank_line_in_a_plain_file_is_no_row() {
    let mut e = env();
    let registry = e.registry.clone();
    e.deps.kits = Arc::new(KitRegistry::new(vec![
        phasor_gas::kit(registry.clone(), &phasor_gas::GasConfig::default()).unwrap(),
        phasor_nem::kit(registry.clone(), "https://h").unwrap(),
    ]));
    let dir = e.home.archive().join("www.nemweb.com.au/Reports/VicGas");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("int157_13.csv");
    std::fs::write(&path, "gas_date\n02 Oct 2026\n\n03 Oct 2026\n").unwrap();
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest a blank line".into(),
            input: Input::Local(vec![path]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    assert_eq!(rep.summary.rows, 2);
    assert_eq!(rep.summary.bad_rows, 0);
}

/// Every table's real file, as AEMO published it (the first rows of one
/// file of each, in `testdata/heads`), goes into the lake whole: no file
/// fails, no row is lost or quarantined, no value is turned to text, and
/// every lake file ends with each row's source and publish time.
#[tokio::test]
async fn every_real_head_survives_the_lake() {
    use arrow_array::Array as _;
    let mut e = env();
    let registry = e.registry.clone();
    e.deps.kits = Arc::new(KitRegistry::new(vec![
        phasor_gas::kit(registry.clone(), &phasor_gas::GasConfig::default()).unwrap(),
        phasor_nem::kit(registry.clone(), "https://h").unwrap(),
    ]));
    let heads = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/heads")
        .canonicalize()
        .unwrap();
    let mut files = Vec::new();
    let mut stack = vec![heads.clone()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    assert!(files.len() > 400, "{}", files.len());
    // The rows each file holds: an MMS file's D rows, a plain one's lines
    // after its header.
    let held = |p: &Path| -> u64 {
        let text = std::fs::read_to_string(p).unwrap();
        if text.starts_with("C,") {
            text.lines().filter(|l| l.starts_with("D,")).count() as u64
        } else {
            text.lines().skip(1).filter(|l| !l.is_empty()).count() as u64
        }
    };
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest every real head".into(),
            input: Input::Local(files.clone()),
            options: RunOptions::default(),
        },
    )
    .await;
    let failed: Vec<String> = rep
        .report
        .failed()
        .map(|f| {
            format!(
                "{} {:?}",
                f.source.rsplit('/').next().unwrap_or(""),
                f.failed_with
            )
        })
        .collect();
    assert!(failed.is_empty(), "{} failed: {failed:#?}", failed.len());
    assert_eq!(rep.summary.bad_rows, 0, "{:#?}", rep.report.summary_lines());
    let lake = walk(&e.home.lake());
    // AEMO's area reports share report pairs with the regional ones; each
    // kind of row lands in its own table.
    for t in [
        "rooftop_pv_actual_area",
        "rooftop_pv_forecast_area",
        "demandoperationalactual_area",
        "demandoperationalforecast_area",
    ] {
        assert!(
            lake.iter().any(|f| f.contains(&format!("/nem/{t}/"))),
            "no lake file of {t}"
        );
    }
    let mut raw = Vec::new();
    for f in &lake {
        let b = parquet_batch(f);
        let schema = b.schema();
        let n = schema.fields().len();
        let tail: Vec<&str> = schema.fields()[n - 4..]
            .iter()
            .map(|x| x.name().as_str())
            .collect();
        assert_eq!(
            tail,
            [
                "_phasor_source",
                "_phasor_server_last_modified",
                "_phasor_zip_entry_modified",
                "_phasor_filename_timestamp"
            ],
            "{f}"
        );
        raw.extend(
            schema
                .fields()
                .iter()
                .filter(|x| x.name().ends_with("__raw"))
                .map(|x| format!("{}: {}", f.rsplit('/').nth(3).unwrap_or(""), x.name())),
        );
    }
    assert!(raw.is_empty(), "values kept as text: {raw:#?}");
    // Each head is a CSV read whole: no zip entry, so no zip time; a name
    // stamp exactly where its file name holds a run of fourteen digits.
    let fourteen = |name: &str| {
        name.split(|c: char| !c.is_ascii_digit())
            .any(|run| run.len() == 14)
    };
    let (mut named, mut unnamed) = (0, 0);
    for f in &lake {
        let b = parquet_batch(f);
        let column = |n: &str| b.column_by_name(n).unwrap().clone();
        assert_eq!(
            column("_phasor_zip_entry_modified").null_count(),
            b.num_rows(),
            "{f}"
        );
        let source = column("_phasor_source");
        let source = source
            .as_any()
            .downcast_ref::<arrow_array::DictionaryArray<arrow_array::types::UInt16Type>>()
            .unwrap();
        let values = source
            .values()
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        let stamp = column("_phasor_filename_timestamp");
        for row in 0..b.num_rows() {
            let key = usize::from(source.keys().value(row));
            let name = values.value(key).rsplit('/').next().unwrap();
            assert_eq!(fourteen(name), stamp.is_valid(row), "{name}");
            if stamp.is_valid(row) {
                named += 1;
            } else {
                unnamed += 1;
            }
        }
    }
    assert!(named > 0 && unnamed > 0, "{named} named, {unnamed} not");
    // Every row of every file is in the lake, by the file's own share:
    // the same count, but for a feed that unpivots its forecast columns
    // into rows of their own, which gives a whole number of rows per row.
    let ingests = e
        .deps
        .manifest
        .list_ingests(None, false, 10_000)
        .await
        .unwrap();
    let mut by_path: std::collections::BTreeMap<PathBuf, u64> = std::collections::BTreeMap::new();
    for i in &ingests {
        let path = Url::parse(&i.url).unwrap().to_file_path().unwrap();
        *by_path.entry(path).or_default() += i.entries.iter().map(|x| x.rows).sum::<u64>();
    }
    let mut short = Vec::new();
    for p in &files {
        let want = held(p);
        let got = by_path.get(p).copied().unwrap_or(0);
        let unpivoted = p.to_string_lossy().contains("ForecastUtilisation");
        let whole = if unpivoted {
            got > want && got % want == 0
        } else {
            got == want
        };
        if !whole {
            short.push(format!("{}: file {want}, lake {got}", p.display()));
        }
    }
    assert!(short.is_empty(), "{short:#?}");
}

/// Every time a source was stamped with is kept on its rows, each as
/// found: the zip's record of when the entry was written (read in AEST,
/// the packer's clock), the creation stamp in the entry's name, and the
/// server's Last-Modified, here none for a local file. Nothing is chosen
/// between them; the manifest's shares say the same.
#[tokio::test]
async fn each_time_a_source_was_stamped_with_is_kept_on_its_rows() {
    use arrow_array::Array as _;
    let e = env();
    let scada = std::fs::read(fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv")).unwrap();
    let zip_path = e
        .home
        .root()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip");
    let written = "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV";
    let named = "PUBLIC_DISPATCH_UNIT_SCADA_202401010000_20240101000731.CSV";
    {
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let at = zip::DateTime::from_date_and_time(2024, 1, 1, 0, 5, 30).unwrap();
        w.start_file(
            written,
            zip::write::SimpleFileOptions::default().last_modified_time(at),
        )
        .unwrap();
        w.write_all(&scada).unwrap();
        let at = zip::DateTime::from_date_and_time(2024, 1, 1, 0, 7, 40).unwrap();
        w.start_file(
            named,
            zip::write::SimpleFileOptions::default().last_modified_time(at),
        )
        .unwrap();
        w.write_all(&scada).unwrap();
        w.finish().unwrap();
    }
    let (rep, _) = run_plan(
        &e,
        Plan {
            request: "ingest stamped".into(),
            input: Input::Local(vec![zip_path]),
            options: RunOptions::default(),
        },
    )
    .await;
    assert_eq!(rep.exit_code, 0, "{:?}", rep.report.summary_lines());
    let files = walk(&e.home.lake());
    assert_eq!(files.len(), 1, "{files:?}");
    let b = parquet_batch(&files[0]);
    let source = b
        .column_by_name("_phasor_source")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::DictionaryArray<arrow_array::types::UInt16Type>>()
        .unwrap()
        .clone();
    let names = source
        .values()
        .as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .unwrap()
        .clone();
    let time = |c: &str| {
        b.column_by_name(c)
            .unwrap()
            .as_any()
            .downcast_ref::<arrow_array::TimestampMicrosecondArray>()
            .unwrap()
            .clone()
    };
    let (server, zip_entry, filename) = (
        time("_phasor_server_last_modified"),
        time("_phasor_zip_entry_modified"),
        time("_phasor_filename_timestamp"),
    );
    // 2024-01-01 00:05:30 and 00:07:40 AEST; the name's 00:07:31 AEST.
    let (written_at, named_at, named_zip) = (
        1_704_031_530_000_000,
        1_704_031_651_000_000,
        1_704_031_660_000_000,
    );
    for i in 0..b.num_rows() {
        let s = names.value(usize::from(source.keys().value(i)));
        assert!(server.is_null(i), "a local file has no server: {s}");
        if s.ends_with(&format!("!{written}")) {
            assert_eq!(zip_entry.value(i), written_at, "{s}");
            assert!(filename.is_null(i), "twelve digits are no time: {s}");
        } else {
            assert!(s.ends_with(&format!("!{named}")), "{s}");
            assert_eq!(zip_entry.value(i), named_zip, "{s}");
            assert_eq!(filename.value(i), named_at, "{s}");
        }
    }
    let ingests = e.deps.manifest.list_ingests(None, false, 10).await.unwrap();
    let share = |entry: &str| {
        ingests[0]
            .entries
            .iter()
            .find(|x| x.entry == entry)
            .unwrap()
            .clone()
    };
    let w = share(written);
    assert_eq!(
        (
            w.server_last_modified,
            w.zip_entry_modified,
            w.filename_timestamp
        ),
        (None, Some(written_at), None)
    );
    let n = share(named);
    assert_eq!(
        (
            n.server_last_modified,
            n.zip_entry_modified,
            n.filename_timestamp
        ),
        (None, Some(named_zip), Some(named_at))
    );
}

/// A feed of a kit: a source of one granularity serving some tables.
struct Feed(&'static str, Granularity, &'static [&'static str]);

#[async_trait]
impl Source for Feed {
    fn id(&self) -> &'static str {
        self.0
    }
    fn granularity(&self) -> Granularity {
        self.1
    }
    fn serves(&self, table: &str) -> bool {
        self.2.contains(&table)
    }
    async fn list(&self, _q: &Query, _f: &dyn Listings) -> Result<Vec<RemoteFile>, FetchError> {
        Ok(Vec::new())
    }
}

/// Tables go with the sources that serve them; with none, a kit of several
/// feeds reads every feed, and a kit of one monthly and one daily source
/// reads them as one.
#[test]
fn every_feed_a_request_needs_is_read() {
    use phasor_pipeline::source_groups;
    let ids =
        |gs: Vec<phasor_pipeline::SourceGroup>| -> Vec<(Option<&str>, Option<&str>, Vec<String>)> {
            gs.iter()
                .map(|g| {
                    (
                        g.monthly.as_ref().map(|s| s.id()),
                        g.daily.as_ref().map(|s| s.id()),
                        g.tables.clone(),
                    )
                })
                .collect()
        };
    let t = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let gas: Vec<Arc<dyn Source>> = vec![
        Arc::new(Feed("gsh", Granularity::Month, &["GSH_A"])),
        Arc::new(Feed("gbb", Granularity::Month, &["GBB_A", "GBB_B"])),
        Arc::new(Feed("sttm", Granularity::Month, &["STTM_A"])),
    ];
    assert_eq!(
        ids(source_groups(&gas, &[])),
        [
            (Some("gsh"), None, t(&[])),
            (Some("gbb"), None, t(&[])),
            (Some("sttm"), None, t(&[])),
        ]
    );
    assert_eq!(
        ids(source_groups(&gas, &t(&["GBB_A", "STTM_A", "GBB_B"]))),
        [
            (Some("gbb"), None, t(&["GBB_A", "GBB_B"])),
            (Some("sttm"), None, t(&["STTM_A"])),
        ]
    );
    let nem: Vec<Arc<dyn Source>> = vec![
        Arc::new(Feed("mmsdm", Granularity::Month, &["A", "B"])),
        Arc::new(Feed("reports", Granularity::Day, &["A"])),
    ];
    assert_eq!(
        ids(source_groups(&nem, &[])),
        [(Some("mmsdm"), Some("reports"), t(&[]))]
    );
    assert_eq!(
        ids(source_groups(&nem, &t(&["A", "B", "C"]))),
        [
            (Some("mmsdm"), Some("reports"), t(&["A"])),
            (Some("mmsdm"), None, t(&["B"])),
            (None, None, t(&["C"])),
        ]
    );
}
