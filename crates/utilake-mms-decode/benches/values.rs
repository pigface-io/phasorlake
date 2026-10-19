//! The two value parsers that run once per field on the hot path.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("parse_values");
    g.bench_function("timestamp", |b| {
        b.iter(|| {
            utilake_mms_decode::mmstime::parse_nanos(black_box(b"2024/01/07 13:45:00"), 36_000)
                .unwrap()
        });
    });
    g.bench_function("float_short", |b| {
        b.iter(|| utilake_core::num::parse_f64(black_box(b"57.98")).unwrap());
    });
    g.bench_function("float_long", |b| {
        b.iter(|| {
            utilake_core::num::parse_f64(black_box(b"13878.2169050000000000000000")).unwrap()
        });
    });
    g.bench_function("integer", |b| {
        b.iter(|| utilake_core::num::parse_i64(black_box(b"20231231241")).unwrap());
    });
    g.bench_function("decimal", |b| {
        b.iter(|| utilake_core::num::parse_decimal(black_box(b"13878.216905"), 16, 6).unwrap());
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
