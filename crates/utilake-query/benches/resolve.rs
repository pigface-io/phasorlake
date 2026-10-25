//! The read kernel: resolving one unit's rows, and scanning one file.
//!
//! Shapes are the real ones: DISPATCHPRICE (interval-ending, two
//! intervention runs, several sources), P5MIN_REGIONSOLUTION (a run per
//! interval, `known_at`), DUDETAIL (versions across snapshots). Rows are
//! synthetic; the budgets are rows per second.

use std::hint::black_box;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use utilake_mms_registry::file_model::ValidAxis;
use utilake_query::output::Report;
use utilake_query::query::{Bound, BoundSpec, FieldIx, Query, Unit};
use utilake_query::resolve::resolve;
use utilake_query::scan::scan_schema;
use utilake_query::{KnownTime, MarketOffset, Resolution, ValidTime, ValidWindow};

const TZ: &str = "+10:00";
const DAY: i64 = 86_400_000_000;
const FIVE: i64 = 300_000_000;

fn ts(v: Vec<Option<i64>>) -> ArrayRef {
    Arc::new(TimestampMicrosecondArray::from(v).with_timezone(TZ))
}

fn target(fields: &[(&str, DataType)]) -> SchemaRef {
    Arc::new(Schema::new(
        fields
            .iter()
            .map(|(n, t)| Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    ))
}

fn bound(schema: SchemaRef, window: ValidWindow, resolution: Resolution) -> Bound {
    Bound {
        query: Query::new("T", window, resolution),
        table: "T".into(),
        schema_id: Some(0),
        schema,
        spec: None,
        time: None,
        files: vec![],
        entity_filters: vec![],
        value_filters: vec![],
        keep: None,
        offset: MarketOffset(36_000),
        day_start_us: 14_400_000_000,
        warnings: vec![],
    }
}

/// A day of DISPATCHPRICE: 288 intervals × 5 regions × 2 interventions,
/// from `sources` copies with one later run in every tenth interval.
fn dispatchprice(sources: u32) -> (RecordBatch, Bound, BoundSpec) {
    let t = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from(TZ)));
    let schema = target(&[
        ("SETTLEMENTDATE", t.clone()),
        ("RUNNO", DataType::Int64),
        ("REGIONID", DataType::Utf8),
        ("INTERVENTION", DataType::Int64),
        ("RRP", DataType::Int64),
        ("LASTCHANGED", t),
    ]);
    let regions = ["NSW1", "QLD1", "SA1", "TAS1", "VIC1"];
    let (mut sd, mut run, mut reg, mut int, mut rrp, mut lc, mut src) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for s in 0..sources {
        for i in 0..288i64 {
            let at = (i + 1) * FIVE;
            for r in regions {
                for v in 0..2i64 {
                    let runs = if i % 10 == 0 { 2 } else { 1 };
                    for k in 1..=runs {
                        sd.push(Some(at));
                        run.push(k);
                        reg.push(r);
                        int.push(v);
                        rrp.push(i * 10 + k);
                        lc.push(Some(at + k * 30_000_000));
                        src.push(s);
                    }
                }
            }
        }
    }
    let n = sd.len();
    let batch = RecordBatch::try_new(
        scan_schema(&schema, TZ),
        vec![
            ts(sd),
            Arc::new(Int64Array::from(run)),
            Arc::new(StringArray::from(reg)),
            Arc::new(Int64Array::from(int)),
            Arc::new(Int64Array::from(rrp)),
            ts(lc.clone()),
            Arc::new(UInt32Array::from(src)),
            ts(lc),
        ],
    )
    .unwrap();
    assert_eq!(batch.num_rows(), n);
    let b = bound(
        schema,
        ValidWindow::Range {
            after: ValidTime::new(0),
            upto: ValidTime::new(DAY),
        },
        Resolution::Latest { known_at: None },
    );
    let spec = BoundSpec {
        entity: vec![FieldIx(2), FieldIx(3)],
        valid: ValidAxis::IntervalEnd(FieldIx(0)),
        order: vec![FieldIx(1), FieldIx(5)],
        cutoff: Some(FieldIx(5)),
        unit: Unit::Partition,
        collapse_all: false,
    };
    (batch, b, spec)
}

/// A day of P5MIN_REGIONSOLUTION: every interval forecast by the 12 runs
/// before it, 5 regions.
fn p5min() -> (RecordBatch, Bound, BoundSpec) {
    let t = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from(TZ)));
    let schema = target(&[
        ("RUN_DATETIME", t.clone()),
        ("INTERVAL_DATETIME", t.clone()),
        ("REGIONID", DataType::Utf8),
        ("RRP", DataType::Int64),
        ("LASTCHANGED", t),
    ]);
    let regions = ["NSW1", "QLD1", "SA1", "TAS1", "VIC1"];
    let (mut rd, mut id, mut reg, mut rrp, mut lc) = (vec![], vec![], vec![], vec![], vec![]);
    for i in 0..288i64 {
        let at = (i + 1) * FIVE;
        for k in 1..=12i64 {
            for r in regions {
                rd.push(Some(at - k * FIVE));
                id.push(Some(at));
                reg.push(r);
                rrp.push(i + k);
                lc.push(Some(at - k * FIVE + 10_000_000));
            }
        }
    }
    let n = rd.len();
    let batch = RecordBatch::try_new(
        scan_schema(&schema, TZ),
        vec![
            ts(rd),
            ts(id),
            Arc::new(StringArray::from(reg)),
            Arc::new(Int64Array::from(rrp)),
            ts(lc.clone()),
            Arc::new(UInt32Array::from(vec![0u32; n])),
            ts(lc),
        ],
    )
    .unwrap();
    let b = bound(
        schema,
        ValidWindow::Range {
            after: ValidTime::new(0),
            upto: ValidTime::new(DAY),
        },
        Resolution::Latest {
            known_at: Some(KnownTime::new(DAY / 2)),
        },
    );
    let spec = BoundSpec {
        entity: vec![FieldIx(2)],
        valid: ValidAxis::IntervalEnd(FieldIx(1)),
        order: vec![FieldIx(0), FieldIx(4)],
        cutoff: Some(FieldIx(4)),
        unit: Unit::Partition,
        collapse_all: false,
    };
    (batch, b, spec)
}

/// DUDETAIL: 500 units × 24 snapshots × 3 versions of 8 effective dates.
fn dudetail() -> (RecordBatch, Bound, BoundSpec) {
    let t = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from(TZ)));
    let schema = target(&[
        ("EFFECTIVEDATE", t.clone()),
        ("DUID", DataType::Utf8),
        ("VERSIONNO", DataType::Int64),
        ("REGISTEREDCAPACITY", DataType::Int64),
        ("LASTCHANGED", t),
    ]);
    let (mut eff, mut duid, mut ver, mut cap, mut lc, mut src) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    let names: Vec<String> = (0..500).map(|i| format!("UNIT{i:03}")).collect();
    for s in 0..24u32 {
        for (u, name) in names.iter().enumerate() {
            for e in 0..8i64 {
                for v in 1..=3i64 {
                    eff.push(Some(e * 90 * DAY));
                    duid.push(name.as_str());
                    ver.push(v);
                    cap.push(u as i64 + e + v);
                    lc.push(Some(e * 90 * DAY + v * DAY));
                    src.push(s);
                }
            }
        }
    }
    let batch = RecordBatch::try_new(
        scan_schema(&schema, TZ),
        vec![
            ts(eff),
            Arc::new(StringArray::from(duid)),
            Arc::new(Int64Array::from(ver)),
            Arc::new(Int64Array::from(cap)),
            ts(lc.clone()),
            Arc::new(UInt32Array::from(src)),
            ts(lc),
        ],
    )
    .unwrap();
    let b = bound(
        schema,
        ValidWindow::At(ValidTime::new(400 * DAY)),
        Resolution::Latest { known_at: None },
    );
    let spec = BoundSpec {
        entity: vec![FieldIx(1)],
        valid: ValidAxis::From {
            column: FieldIx(0),
            until: None,
        },
        order: vec![FieldIx(2), FieldIx(4)],
        cutoff: Some(FieldIx(4)),
        unit: Unit::Table,
        collapse_all: false,
    };
    (batch, b, spec)
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("query");
    for (name, (batch, b, spec)) in [
        ("resolve_interval_day", dispatchprice(3)),
        ("resolve_known_at", p5min()),
        ("resolve_from_table", dudetail()),
    ] {
        g.throughput(Throughput::Elements(batch.num_rows() as u64));
        g.bench_function(name, |bench| {
            bench.iter(|| {
                let mut report = Report::default();
                black_box(resolve(black_box(&batch), &b, &spec, &mut report).unwrap())
            });
        });
    }
    let (batch, b, spec) = dispatchprice(3);
    let b = Bound {
        query: Query::new(
            "T",
            b.query.window,
            Resolution::Revisions { known_at: None },
        ),
        ..b
    };
    g.throughput(Throughput::Elements(batch.num_rows() as u64));
    g.bench_function("resolve_revisions_day", |bench| {
        bench.iter(|| {
            let mut report = Report::default();
            black_box(resolve(black_box(&batch), &b, &spec, &mut report).unwrap())
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
