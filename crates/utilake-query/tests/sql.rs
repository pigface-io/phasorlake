//! The read as DuckDB SQL: the shape of the query for each kind of read.

mod common;

use common::{Fixture, ts};
use utilake_query::{
    Filter, KnownTime, Query, Resolution, ValidTime, ValidWindow, bind, sql::render,
};

fn range(a: &str, b: &str) -> ValidWindow {
    ValidWindow::range(ValidTime::new(ts(a)), ValidTime::new(ts(b))).unwrap()
}

const LATEST: Resolution = Resolution::Latest { known_at: None };

#[test]
fn each_kind_of_read_renders() {
    let mut fx = Fixture::new();
    common::dispatchprice(&mut fx);
    common::dudetail(&mut fx);
    common::keyless(&mut fx);
    common::derived(&mut fx);
    let lake = fx.lake();
    let day = range("2024-01-01T00:00", "2024-01-01T00:15");

    let q = Query::new(
        "DISPATCHPRICE",
        day,
        Resolution::Latest {
            known_at: Some(KnownTime::new(ts("2024-01-01T00:11"))),
        },
    );
    let s = render(&bind(q, &lake).unwrap(), &lake);
    assert!(s.contains("read_parquet(['"), "{s}");
    assert!(s.contains("filename = true"), "{s}");
    assert!(
        s.contains("NULL::DECIMAL(15, 5) AS \"PRE_AP_LOWER1_PRICE\""),
        "the schema-0 file: {s}"
    );
    assert!(
        s.contains("coalesce(\"LASTCHANGED\", published) AS _nl_known"),
        "{s}"
    );
    assert!(s.contains("WHERE _nl_known <= make_timestamptz("), "{s}");
    assert!(
        s.contains("PARTITION BY \"REGIONID\", \"DISPATCHINTERVAL\", \"INTERVENTION\", \"SETTLEMENTDATE\" ORDER BY \"RUNNO\" DESC NULLS LAST, \"LASTCHANGED\" DESC NULLS LAST, published DESC NULLS LAST, filename DESC) = 1"),
        "{s}"
    );
    assert!(
        s.contains("WHERE \"SETTLEMENTDATE\" >= make_timestamptz("),
        "{s}"
    );
    assert!(s.contains("* EXCLUDE (filename, published)"), "{s}");
    assert!(!s.contains("lead("), "{s}");

    // Revisions: the order columns join the partition.
    let s = render(
        &bind(
            Query::new("DISPATCHPRICE", day, Resolution::Revisions),
            &lake,
        )
        .unwrap(),
        &lake,
    );
    assert!(
        s.contains("PARTITION BY \"REGIONID\", \"DISPATCHINTERVAL\", \"INTERVENTION\", \"SETTLEMENTDATE\", \"RUNNO\", \"LASTCHANGED\" ORDER BY published DESC NULLS LAST, filename DESC) = 1"),
        "{s}"
    );
    assert!(!s.contains("WHERE _nl_known"), "{s}");

    // A `from` table: the spans and the in-force window; `--at` too.
    let s = render(
        &bind(
            Query::new(
                "DUDETAIL",
                range("2023-01-01T00:00", "2024-01-01T00:00"),
                LATEST,
            ),
            &lake,
        )
        .unwrap(),
        &lake,
    );
    assert!(s.contains("lead(\"EFFECTIVEDATE\") OVER (PARTITION BY \"DUID\" ORDER BY \"EFFECTIVEDATE\") AS next_valid FROM ends"), "{s}");
    assert!(s.contains("nullif(next_ends.next_valid, make_timestamptz(9223372036854775)) AS _nl_valid_to FROM latest JOIN next_ends USING (\"DUID\", \"EFFECTIVEDATE\")"), "{s}");
    assert!(s.contains("OR _nl_valid_to > make_timestamptz("), "{s}");
    let s = render(
        &bind(
            Query::new(
                "DUDETAIL",
                ValidWindow::At(ValidTime::new(ts("2024-01-15T00:00"))),
                LATEST,
            ),
            &lake,
        )
        .unwrap(),
        &lake,
    );
    assert!(
        s.contains("WHERE \"EFFECTIVEDATE\" <= make_timestamptz("),
        "{s}"
    );

    // No key: every column is the partition.
    let any = range("2000-01-01T00:00", "2030-01-01T00:00");
    let s = render(
        &bind(Query::new("SSM_ENABLEMENT_COSTS", any, LATEST), &lake).unwrap(),
        &lake,
    );
    assert!(s.contains("PARTITION BY \"END_TRADINGDATE\", \"ENABLEMENT_REASON\", \"ESTIMATED_COSTS\", \"LASTCHANGED\" ORDER BY published DESC NULLS LAST, filename DESC) = 1"), "{s}");
    assert!(
        s.contains("SELECT * EXCLUDE (filename, published) FROM spans "),
        "{s}"
    );

    // Raw, with a projection, and a derived table.
    let s = render(
        &bind(
            Query::new("DISPATCHPRICE", day, Resolution::Raw)
                .with_columns(vec!["RRP".into()])
                .with_filters(vec![Filter {
                    column: "REGIONID".into(),
                    values: vec!["NSW1".into()],
                }]),
            &lake,
        )
        .unwrap(),
        &lake,
    );
    assert!(
        s.contains("SELECT \"RRP\" FROM spans WHERE \"SETTLEMENTDATE\" >= make_timestamptz("),
        "{s}"
    );
    assert!(!s.contains("QUALIFY"), "{s}");
    let s = render(
        &bind(Query::new("UTILAKE.NOBODY", any, Resolution::Raw), &lake).unwrap(),
        &lake,
    );
    assert!(
        s.contains("SELECT * EXCLUDE (filename) FROM spans \n") || s.ends_with("FROM spans "),
        "{s}"
    );

    // No file in the window: a query that reads nothing.
    let s = render(
        &bind(
            Query::new(
                "DISPATCHPRICE",
                range("2030-01-01T00:00", "2030-01-02T00:00"),
                LATEST,
            ),
            &lake,
        )
        .unwrap(),
        &lake,
    );
    assert!(s.contains("SELECT NULL AS filename WHERE false"), "{s}");
    assert!(s.contains("VALUES (NULL, NULL::TIMESTAMPTZ)"), "{s}");
}
