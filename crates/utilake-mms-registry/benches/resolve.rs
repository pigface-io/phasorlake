//! Registry hot path: resolving an I row, and loading the embedded registry.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use utilake_mms_decode::IRow;
use utilake_mms_registry::{Registry, ResolveOptions};

fn bench(c: &mut Criterion) {
    let r = Registry::load_embedded();
    let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
    let row = IRow {
        report_type: "DISPATCH",
        report_subtype: "UNIT_SCADA",
        version: 1,
        columns: &cols,
    };
    let mut g = c.benchmark_group("registry");
    g.bench_function("resolve_exact", |b| {
        b.iter(|| {
            black_box(
                r.resolve(black_box(&row), ResolveOptions::default())
                    .unwrap()
                    .version,
            )
        });
    });
    let drift_cols = ["SETTLEMENTDATE", "DUID"];
    let drift = IRow {
        report_type: "DISPATCH",
        report_subtype: "UNIT_SCADA",
        version: 1,
        columns: &drift_cols,
    };
    g.bench_function("resolve_drift_cached", |b| {
        b.iter(|| {
            black_box(
                r.resolve(black_box(&drift), ResolveOptions::default())
                    .unwrap()
                    .version,
            )
        });
    });
    g.bench_function("load_embedded_from_blob", |b| {
        b.iter(|| {
            let set = utilake_mms_registry::file_model::blob::SchemaSet::decode(black_box(
                utilake_mms_registry::registry::embedded_blob(),
            ))
            .unwrap();
            black_box(Registry::from_set(set).tables().count())
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
