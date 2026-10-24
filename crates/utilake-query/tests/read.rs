//! Reads over the fixture lake: every axis, resolution and window, with
//! the rows each must give.

mod common;

use arrow_array::cast::AsArray;
use arrow_array::types::{Decimal128Type, Float64Type, Int64Type, TimestampMicrosecondType};
use arrow_array::{Array, RecordBatch};
use common::{ARCHIVE, DVD, Fixture, REPORT, ts};
use utilake_query::{
    Filter, Format, Frame, KnownTime, Projection, Query, QueryError, Resolution, Target, ValidTime,
    ValidWindow, bind, extract, read,
};

fn range(a: &str, b: &str) -> ValidWindow {
    ValidWindow::range(ValidTime::new(ts(a)), ValidTime::new(ts(b))).unwrap()
}

fn at(a: &str) -> ValidWindow {
    ValidWindow::At(ValidTime::new(ts(a)))
}

fn known(a: &str) -> Resolution {
    Resolution::Latest {
        known_at: Some(KnownTime::new(ts(a))),
    }
}

const LATEST: Resolution = Resolution::Latest { known_at: None };

/// Every frame as one batch.
fn rows(fx: &Fixture, q: Query) -> (RecordBatch, utilake_query::Report) {
    let lake = fx.lake();
    let b = bind(q, &lake).unwrap();
    let frames = read(&b, &lake).unwrap();
    let batches: Vec<RecordBatch> = frames.frames.iter().map(|f| f.batch().clone()).collect();
    let batch = arrow_select::concat::concat_batches(&frames.schema, &batches).unwrap();
    (batch, frames.report)
}

fn strings(b: &RecordBatch, col: &str) -> Vec<String> {
    let c = b.column_by_name(col).unwrap();
    match c.data_type() {
        arrow_schema::DataType::Dictionary(_, _) => {
            let d = c.as_dictionary::<arrow_array::types::Int32Type>();
            let v = d.values().as_string::<i32>();
            d.keys()
                .iter()
                .map(|k| v.value(k.unwrap() as usize).to_owned())
                .collect()
        }
        _ => c
            .as_string::<i32>()
            .iter()
            .map(|s| s.unwrap_or("").to_owned())
            .collect(),
    }
}

fn ints(b: &RecordBatch, col: &str) -> Vec<Option<i64>> {
    let c = b.column_by_name(col).unwrap();
    match c.data_type() {
        arrow_schema::DataType::Decimal128(_, s) => c
            .as_primitive::<Decimal128Type>()
            .iter()
            .map(|v| v.map(|v| (v / 10i128.pow(u32::from(*s as u8))) as i64))
            .collect(),
        arrow_schema::DataType::Float64 => c
            .as_primitive::<Float64Type>()
            .iter()
            .map(|v| v.map(|v| v as i64))
            .collect(),
        _ => c.as_primitive::<Int64Type>().iter().collect(),
    }
}

fn times(b: &RecordBatch, col: &str) -> Vec<Option<i64>> {
    b.column_by_name(col)
        .unwrap()
        .as_primitive::<TimestampMicrosecondType>()
        .iter()
        .collect()
}

fn t(s: &str) -> Option<i64> {
    Some(ts(s))
}

#[test]
fn interval_table_latest_revisions_and_raw() {
    let mut fx = Fixture::new();
    common::dispatchprice(&mut fx);
    let day = range("2024-01-01T00:00", "2024-01-01T00:15");

    // Latest: one row per (region, interval, intervention); the copy
    // published last wins a tie, the later run wins the interval.
    let (b, report) = rows(&fx, Query::new("DISPATCHPRICE", day, LATEST));
    assert_eq!(b.num_rows(), 4);
    assert_eq!(
        times(&b, "SETTLEMENTDATE"),
        [
            t("2024-01-01T00:05"),
            t("2024-01-01T00:10"),
            t("2024-01-01T00:15"),
            t("2024-01-01T00:15")
        ]
    );
    assert_eq!(
        ints(&b, "RRP"),
        [Some(101), Some(111), Some(120), Some(125)]
    );
    assert_eq!(ints(&b, "RUNNO"), [Some(1), Some(2), Some(1), Some(1)]);
    assert_eq!(
        ints(&b, "INTERVENTION"),
        [Some(0), Some(0), Some(0), Some(1)]
    );
    let sources = strings(&b, "_nl_source");
    assert!(sources[0].starts_with(ARCHIVE), "{}", sources[0]);
    assert!(sources[1].starts_with(DVD), "{}", sources[1]);
    assert_eq!(times(&b, "_nl_known")[1], t("2024-01-01T00:12:00"));
    assert!(b.column_by_name("_nl_valid_to").is_none());
    assert_eq!(report.rows_out, 4);
    assert_eq!(report.units, 1, "one partition read");
    assert_eq!(report.files, 3, "the December file is outside the window");
    assert_eq!(report.sources.len(), 3);
    assert!(report.warnings.is_empty());

    // Field ids ride along.
    let f = b.schema().field_with_name("RRP").unwrap().clone();
    assert_eq!(
        f.metadata().get("PARQUET:field_id").map(String::as_str),
        Some("6")
    );

    // Revisions: every run once, however many copies.
    let (b, _) = rows(&fx, Query::new("DISPATCHPRICE", day, Resolution::Revisions));
    assert_eq!(b.num_rows(), 5);
    assert_eq!(
        ints(&b, "RRP"),
        [Some(101), Some(110), Some(111), Some(120), Some(125)]
    );

    // As known at 00:11: the second run (changed at 00:12) is not yet.
    let (b, report) = rows(
        &fx,
        Query::new("DISPATCHPRICE", day, known("2024-01-01T00:11")),
    );
    assert_eq!(ints(&b, "RRP"), [Some(101), Some(110)]);
    assert_eq!(report.dropped_unknown, 3);

    // Raw: every row of every file in the window, the older schema's
    // file projected (its PRE_AP_LOWER1_PRICE is null, the DVD's is 8).
    let (b, _) = rows(&fx, Query::new("DISPATCHPRICE", day, Resolution::Raw));
    assert_eq!(b.num_rows(), 8);
    let pre: Vec<Option<i64>> = ints(&b, "PRE_AP_LOWER1_PRICE");
    assert_eq!(
        pre.iter().filter(|v| v.is_none()).count(),
        2 + 1,
        "report rows and the archive row"
    );
    assert!(b.column_by_name("_nl_known").is_none());
    assert!(b.column_by_name("_nl_source").is_some());

    // The window is `(after, upto]`: midnight belongs to December, and
    // the two partitions are two units.
    let (b, report) = rows(
        &fx,
        Query::new(
            "DISPATCHPRICE",
            range("2023-12-31T23:55", "2024-01-01T00:05"),
            LATEST,
        ),
    );
    assert_eq!(
        times(&b, "SETTLEMENTDATE"),
        [t("2024-01-01T00:00"), t("2024-01-01T00:05")]
    );
    assert_eq!(ints(&b, "RRP"), [Some(50), Some(101)]);
    assert_eq!(report.units, 2);
    assert!(report.dropped_window > 0);

    // An instant has no interval on this axis.
    let err = bind(
        Query::new("DISPATCHPRICE", at("2024-01-01T00:05"), LATEST),
        &fx.lake(),
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::AtOnIntervalTable { .. }), "{err}");
}

#[test]
fn filters_projection_and_as_of() {
    let mut fx = Fixture::new();
    common::dispatchprice(&mut fx);
    let day = range("2024-01-01T00:00", "2024-01-01T00:15");
    // An entity filter runs first; another runs after and is noted.
    let q = Query::new("DISPATCHPRICE", day, LATEST).with_filters(vec![
        Filter {
            column: "INTERVENTION".into(),
            values: vec!["1".into()],
        },
        Filter {
            column: "RRP".into(),
            values: vec!["125".into(), "999".into()],
        },
    ]);
    let b = bind(q.clone(), &fx.lake()).unwrap();
    assert_eq!(b.entity_filters.len(), 1);
    assert_eq!(b.value_filters.len(), 1);
    assert_eq!(b.warnings.len(), 1);
    assert_eq!(b.warnings[0].code, "query.filter_after_resolution");
    let (rows_, report) = rows(&fx, q);
    assert_eq!(ints(&rows_, "RRP"), [Some(125)]);
    assert_eq!(report.warnings.len(), 1);
    // A filter on a column that is not there, or a value that is not of
    // its type.
    let lake = fx.lake();
    let err = bind(
        Query::new("DISPATCHPRICE", day, LATEST).with_filters(vec![Filter {
            column: "NOPE".into(),
            values: vec![],
        }]),
        &lake,
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::UnknownColumn { .. }));
    let err = bind(
        Query::new("DISPATCHPRICE", day, LATEST).with_filters(vec![Filter {
            column: "RRP".into(),
            values: vec!["high".into()],
        }]),
        &lake,
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::BadFilterValue { .. }));
    // The projection: the named columns, then the `_nl_` ones.
    let (b, _) = rows(
        &fx,
        Query::new("DISPATCHPRICE", day, LATEST)
            .with_columns(vec!["RRP".into(), "REGIONID".into()]),
    );
    let schema = b.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(names, ["RRP", "REGIONID", "_nl_source", "_nl_known"]);
    assert_eq!(b.num_rows(), 4);
    let err = bind(
        Query::new("DISPATCHPRICE", day, LATEST).with_columns(vec!["NOPE".into()]),
        &lake,
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::UnknownColumn { .. }));
    // An older schema as the target: fewer columns, the roles still there.
    let b = bind(
        Query::new("DISPATCHPRICE", day, LATEST).with_target(Target::AsOf(0)),
        &lake,
    )
    .unwrap();
    assert_eq!(b.schema_id, Some(0));
    assert!(b.schema.index_of("PRE_AP_LOWER1_PRICE").is_err());
    let frames = read(&b, &lake).unwrap();
    assert_eq!(frames.report.rows_out, 4);
    let err = bind(
        Query::new("DISPATCHPRICE", day, LATEST).with_target(Target::AsOf(9)),
        &lake,
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::Plan { .. }), "{err}");
    assert!(matches!(
        bind(Query::new("NOBODY", day, LATEST), &lake).unwrap_err(),
        QueryError::UnknownTable(_)
    ));
}

#[test]
fn from_table_versions_in_force() {
    let mut fx = Fixture::new();
    common::dudetail(&mut fx);

    // At an instant: the version in force per unit; a correction wins
    // its version; the "no end" row is never a version's end.
    let (b, _) = rows(&fx, Query::new("DUDETAIL", at("2024-01-15T00:00"), LATEST));
    assert_eq!(strings(&b, "DUID"), ["A", "B", "C"]);
    assert_eq!(
        ints(&b, "REGISTEREDCAPACITY"),
        [Some(21), Some(30), Some(40)]
    );
    assert_eq!(ints(&b, "VERSIONNO"), [Some(2), Some(1), Some(1)]);
    assert_eq!(times(&b, "_nl_valid_to"), [None, None, None]);

    // A range: the version in force at its start, and every one after.
    let (b, _) = rows(
        &fx,
        Query::new(
            "DUDETAIL",
            range("2023-01-01T00:00", "2024-01-01T00:00"),
            LATEST,
        ),
    );
    assert_eq!(strings(&b, "DUID"), ["A", "A", "B", "C"]);
    assert_eq!(
        ints(&b, "REGISTEREDCAPACITY"),
        [Some(10), Some(21), Some(30), Some(40)]
    );
    assert_eq!(
        times(&b, "_nl_valid_to"),
        [t("2023-06-01T00:00"), None, None, None]
    );

    // Every revision: both versions of the corrected date.
    let (b, _) = rows(
        &fx,
        Query::new("DUDETAIL", at("2024-01-15T00:00"), Resolution::Revisions),
    );
    assert_eq!(
        ints(&b, "REGISTEREDCAPACITY"),
        [Some(20), Some(21), Some(30), Some(40)]
    );

    // Before the correction was known.
    let (b, _) = rows(
        &fx,
        Query::new(
            "DUDETAIL",
            at("2024-01-15T00:00"),
            known("2024-01-01T00:00"),
        ),
    );
    assert_eq!(
        ints(&b, "REGISTEREDCAPACITY"),
        [Some(20), Some(30), Some(40)]
    );

    // Before any version: nothing in force.
    let (b, _) = rows(&fx, Query::new("DUDETAIL", at("2019-01-01T00:00"), LATEST));
    assert_eq!(b.num_rows(), 0);
    assert!(b.column_by_name("_nl_valid_to").is_some());
}

#[test]
fn keyless_and_derived_tables() {
    let mut fx = Fixture::new();
    common::keyless(&mut fx);
    let derived = common::derived(&mut fx);
    let any = range("2000-01-01T00:00", "2030-01-01T00:00");

    // No key: rows equal in every column are one row; the window means
    // nothing and says so.
    let (b, report) = rows(&fx, Query::new("SSM_ENABLEMENT_COSTS", any, LATEST));
    assert_eq!(b.num_rows(), 2);
    assert_eq!(strings(&b, "ENABLEMENT_REASON"), ["x", "y"]);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].code, "query.window_ignored");
    let (b, _) = rows(
        &fx,
        Query::new("ssm_enablement_costs", any, Resolution::Raw),
    );
    assert_eq!(b.num_rows(), 4);

    // Derived from a file: raw only, every column text, by pair or name.
    assert_eq!(derived, "UTILAKE_NOBODY");
    let (b, _) = rows(&fx, Query::new("UTILAKE.NOBODY", any, Resolution::Raw));
    assert_eq!(b.num_rows(), 2);
    assert_eq!(strings(&b, "WHAT"), ["a", "a"]);
    let schema = b.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(names, ["WHEN", "WHAT", "_nl_source"]);
    let lake = fx.lake();
    let err = bind(Query::new("UTILAKE.NOBODY", any, LATEST), &lake).unwrap_err();
    assert!(matches!(err, QueryError::RawOnly(_)), "{err}");
    let err = bind(
        Query::new("UTILAKE.NOBODY", at("2024-01-01T00:00"), Resolution::Raw),
        &lake,
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::AtOnIntervalTable { .. }), "{err}");
    let b = bind(
        Query::new("UTILAKE.NOBODY", any, Resolution::Raw).with_filters(vec![Filter {
            column: "WHAT".into(),
            values: vec!["b".into()],
        }]),
        &lake,
    )
    .unwrap();
    assert_eq!(
        b.entity_filters.len(),
        1,
        "a raw read pushes every filter down"
    );
    assert_eq!(read(&b, &lake).unwrap().report.rows_out, 0);
}

#[test]
fn extract_writes_a_file_and_its_sidecar() {
    let mut fx = Fixture::new();
    common::dispatchprice(&mut fx);
    let lake = fx.lake();
    let day = range("2024-01-01T00:00", "2024-01-01T00:15");
    let out = fx.dir.path().join("out");
    let current = fx
        .registry
        .history(fx.id("DISPATCHPRICE"))
        .unwrap()
        .current_schema_id;
    for (format, ext) in [
        (Format::Parquet, "parquet"),
        (Format::Csv, "csv"),
        (Format::Arrow, "arrow"),
    ] {
        let w = extract(
            utilake_query::Extract {
                query: Query::new("DISPATCHPRICE", day, known("2024-01-01T00:11")),
                out: out.join(format!("dp.{ext}")),
                format,
            },
            &lake,
        )
        .unwrap();
        assert_eq!(w.rows, 2);
        assert!(w.path.is_file());
        assert_eq!(w.sidecar, out.join(format!("dp.{ext}.json")));
        let sidecar: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&w.sidecar).unwrap()).unwrap();
        assert_eq!(sidecar["table"], "DISPATCHPRICE");
        assert_eq!(sidecar["resolution"], "latest");
        assert_eq!(sidecar["rows"], 2);
        assert_eq!(sidecar["schema_id"], current);
        assert_eq!(sidecar["window"]["after"], "2024-01-01T00:00:00+10:00");
        assert_eq!(sidecar["known_at"], "2023-12-31T14:11:00Z");
        assert_eq!(sidecar["sources"].as_array().unwrap().len(), 3);
        assert_eq!(sidecar["utilake"], "test");
        assert_eq!(format.extension(), ext);
    }
    // The Parquet file carries the lake's field ids and the read's keys.
    let f = std::fs::File::open(out.join("dp.parquet")).unwrap();
    let r = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
    let kv = r.metadata().file_metadata().key_value_metadata().unwrap();
    let get = |k: &str| kv.iter().find(|x| x.key == k).and_then(|x| x.value.clone());
    assert_eq!(get("utilake.resolution").as_deref(), Some("latest"));
    assert_eq!(get("utilake.table").as_deref(), Some("DISPATCHPRICE"));
    assert_eq!(
        get("utilake.schema_id").as_deref(),
        Some(current.to_string().as_str())
    );
    assert_eq!(
        get("utilake.known_at").as_deref(),
        Some("2023-12-31T14:11:00Z")
    );
    let rrp = r.schema().field_with_name("RRP").unwrap().clone();
    assert_eq!(
        rrp.metadata().get("PARQUET:field_id").map(String::as_str),
        Some("6")
    );
    let batches: Vec<RecordBatch> = r.build().unwrap().map(Result::unwrap).collect();
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
    // A raw extract, and one that writes nothing.
    let w = extract(
        utilake_query::Extract {
            query: Query::new("DISPATCHPRICE", day, Resolution::Raw),
            out: out.join("raw.parquet"),
            format: Format::Parquet,
        },
        &lake,
    )
    .unwrap();
    assert_eq!(w.rows, 8);
    let w = extract(
        utilake_query::Extract {
            query: Query::new(
                "DISPATCHPRICE",
                range("2030-01-01T00:00", "2030-01-02T00:00"),
                LATEST,
            ),
            out: out.join("none.csv"),
            format: Format::Csv,
        },
        &lake,
    )
    .unwrap();
    assert_eq!(w.rows, 0);
    assert!(w.report.sources.is_empty());
    // A path that cannot be written.
    let err = extract(
        utilake_query::Extract {
            query: Query::new("DISPATCHPRICE", day, LATEST),
            out: out.join("dp.parquet").join("x.parquet"),
            format: Format::Parquet,
        },
        &lake,
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::Write { .. }), "{err}");
    // Frames are typed by what they hold.
    let b = bind(Query::new("DISPATCHPRICE", day, LATEST), &lake).unwrap();
    let frames = read(&b, &lake).unwrap();
    assert!(matches!(frames.frames[0], Frame::Resolved(_)));
    let b = bind(Query::new("DISPATCHPRICE", day, Resolution::Raw), &lake).unwrap();
    let frames = read(&b, &lake).unwrap();
    assert!(matches!(frames.frames[0], Frame::Raw(_)));
    assert!(matches!(b.query.columns, Projection::All));
}

#[test]
fn a_file_without_a_publish_time_is_left_out_under_known_at() {
    let mut fx = Fixture::new();
    common::keyless(&mut fx);
    // SSM_ENABLEMENT_COSTS has LASTCHANGED as its cutoff, so its files
    // need no stamp; the derived table has neither, but is raw only. Take
    // DISPATCHPRICE's report file, which has a stamp in its name, and
    // strip it.
    common::dispatchprice(&mut fx);
    for o in &mut fx.outputs {
        o.last_modified_at = None;
    }
    let day = range("2024-01-01T00:00", "2024-01-01T00:15");
    let (b, report) = rows(
        &fx,
        Query::new("DISPATCHPRICE", day, known("2024-01-01T00:11")),
    );
    // The cutoff column decides, so every file is still read.
    assert_eq!(b.num_rows(), 2);
    assert!(report.warnings.is_empty());
    // Without a stamp a file ranks first among copies: the DVD and the
    // archive (no stamp now) lose to the report, stamped by its name.
    let (b, _) = rows(&fx, Query::new("DISPATCHPRICE", day, LATEST));
    assert_eq!(ints(&b, "RRP"), [Some(99), Some(111), Some(120), Some(125)]);
    let sources = strings(&b, "_nl_source");
    assert!(sources[0].starts_with(REPORT), "{}", sources[0]);
    // The DVD's own copy of the 00:10 run 1 row now loses to the report's.
    let (b, _) = rows(&fx, Query::new("DISPATCHPRICE", day, Resolution::Revisions));
    let sources = strings(&b, "_nl_source");
    assert!(sources[1].starts_with(REPORT), "{}", sources[1]);
}

#[test]
fn roles_against_older_schemas_and_instant_tables() {
    let fx = Fixture::new();
    let lake = fx.lake();
    let any = range("2000-01-01T00:00", "2030-01-01T00:00");
    // DISPATCH_UNIT_SCADA gained LASTCHANGED, its cutoff, after schema 0.
    let err = bind(
        Query::new("DISPATCH_UNIT_SCADA", any, LATEST).with_target(Target::AsOf(0)),
        &lake,
    )
    .unwrap_err();
    assert!(
        matches!(&err, QueryError::RoleMissing { role: "order", column, .. } if column == "LASTCHANGED"),
        "{err}"
    );
    // A run's own result is an instant: `--at` is allowed, and a table
    // whose key holds the valid time still resolves whole (a snapshot).
    let b = bind(
        Query::new("MCC_CASESOLUTION", at("2024-01-01T00:00"), LATEST),
        &lake,
    )
    .unwrap();
    let spec = b.spec.as_ref().unwrap();
    assert!(matches!(
        spec.valid,
        utilake_mms_registry::file_model::ValidAxis::Instant(_)
    ));
    assert!(read(&b, &lake).unwrap().frames.is_empty());
    // A table partitioned by its valid time resolves one partition at a time.
    let b = bind(Query::new("DISPATCHPRICE", any, LATEST), &lake).unwrap();
    assert_eq!(
        b.spec.as_ref().unwrap().unit,
        utilake_query::Unit::Partition
    );
    let b = bind(Query::new("DUDETAIL", any, LATEST), &lake).unwrap();
    assert_eq!(b.spec.as_ref().unwrap().unit, utilake_query::Unit::Table);
}

#[test]
fn a_file_of_an_unknown_schema_and_an_extract_at_an_instant() {
    let mut fx = Fixture::new();
    common::dudetail(&mut fx);
    let out = fx.dir.path().join("du.parquet");
    let w = extract(
        utilake_query::Extract {
            query: Query::new("DUDETAIL", at("2024-01-15T00:00"), LATEST),
            out: out.clone(),
            format: Format::Parquet,
        },
        &fx.lake(),
    )
    .unwrap();
    assert_eq!(w.rows, 3);
    let sidecar: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&w.sidecar).unwrap()).unwrap();
    assert_eq!(sidecar["window"]["at"], "2024-01-15T00:00:00+10:00");
    assert!(sidecar.get("known_at").is_none());
    // A file written under a schema the history does not have cannot be
    // projected.
    fx.outputs[0].schema_id = Some(99);
    let err = bind(
        Query::new("DUDETAIL", at("2024-01-15T00:00"), LATEST),
        &fx.lake(),
    )
    .unwrap_err();
    assert!(matches!(err, QueryError::Plan { .. }), "{err}");
}
