//! Parquet write throughput and month-split cost.

use std::hint::black_box;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray, TimestampNanosecondArray};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nemlake_core::Fingerprint;
use nemlake_writer::partition::split_by_month;
use nemlake_writer::{FileMeta, LakeWriter, ParquetHive, Partition, PartitionKey};

const ROWS: usize = 262_144;

fn batch(spill: bool) -> RecordBatch {
    let base = nemlake_core::nemtime::parse_nanos(b"2024/01/31 20:00:00", 36_000)
        .unwrap()
        .0;
    let ts: Vec<i64> = (0..ROWS)
        .map(|i| base + (i as i64) * 300_000_000_000 / if spill { 100 } else { 10_000 })
        .collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "SETTLEMENTDATE",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+10:00".into())),
            false,
        ),
        Field::new("DUID", DataType::Utf8, false),
        Field::new("SCADAVALUE", DataType::Float64, true),
    ]));
    let duids: Vec<String> = (0..ROWS).map(|i| format!("UNIT{:03}", i % 400)).collect();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(TimestampNanosecondArray::from(ts).with_timezone("+10:00")),
            Arc::new(StringArray::from(duids)),
            Arc::new(Float64Array::from(
                (0..ROWS)
                    .map(|i| Some((i as f64) * 0.731))
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap()
}

fn bench(c: &mut Criterion) {
    let b = batch(false);
    let bytes = b.get_array_memory_size() as u64;
    let mut g = c.benchmark_group("store");
    g.sample_size(10);
    g.throughput(Throughput::Bytes(bytes));
    g.bench_function("parquet_write", |bencher| {
        let dir = tempfile::tempdir().unwrap();
        let mut w = ParquetHive::new(dir.path());
        let key = PartitionKey {
            market: "nem".into(),
            table_dir: "t".into(),
            partition: Partition::Month {
                year: 2024,
                month: 1,
            },
        };
        let meta = FileMeta {
            table: "T".into(),
            source_stem: "s".into(),
            schema_version: 1,
            fingerprint: Fingerprint(1),
            source_url: String::new(),
            source_sha256: "abcdef01".into(),
            entry: String::new(),
            time_column: Some(0),
            part: 1,
            transform: nemlake_writer::Transform::default(),
        };
        bencher.iter(|| {
            let id = w.open(&key, b.schema(), &meta).unwrap();
            w.write(id, black_box(&b)).unwrap();
            let f = w.close(id).unwrap();
            let _ = std::fs::remove_file(&f.path);
            black_box(f.rows);
        });
    });
    g.finish();
    let spill = batch(true);
    let mut g = c.benchmark_group("store");
    g.bench_function("partition_split_two_months", |bencher| {
        bencher.iter(|| black_box(split_by_month(black_box(&spill), 0, 36_000).unwrap().len()));
    });
    g.bench_function("partition_split_one_month", |bencher| {
        bencher.iter(|| black_box(split_by_month(black_box(&b), 0, 36_000).unwrap().len()));
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
