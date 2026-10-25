//! The read as DuckDB SQL: the shape of the query for each kind of read.

mod common;

use common::{Fixture, ts};
use utilake_query::{
    Dialect, Filter, KnownTime, Query, Resolution, ValidTime, ValidWindow, bind, sql::render,
};

fn range(a: &str, b: &str) -> ValidWindow {
    ValidWindow::range(ValidTime::new(ts(a)), ValidTime::new(ts(b))).unwrap()
}

const LATEST: Resolution = Resolution::Latest { known_at: None };

fn render_duck(b: &utilake_query::Bound, lake: &utilake_query::Lake<'_>) -> String {
    render(b, lake, Dialect::DuckDb)
}

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
    let s = render_duck(&bind(q, &lake).unwrap(), &lake);
    assert!(s.contains("read_parquet(['"), "{s}");
    assert!(
        !s.contains("filename"),
        "every row carries its own source: {s}"
    );
    assert!(
        s.contains("CAST(NULL AS DECIMAL(15, 5)) AS \"PRE_AP_LOWER1_PRICE\""),
        "the schema-0 file: {s}"
    );
    assert!(
        s.contains("greatest(\"LASTCHANGED\", \"_nl_published\") AS _nl_known"),
        "{s}"
    );
    assert!(
        s.contains(
            "WHERE _nl_known <= TIMESTAMP WITH TIME ZONE '2023-12-31 14:11:00.000000+00:00'"
        ),
        "{s}"
    );
    assert!(
        s.contains("row_number() OVER (PARTITION BY \"REGIONID\", \"DISPATCHINTERVAL\", \"INTERVENTION\", \"SETTLEMENTDATE\" ORDER BY \"RUNNO\" DESC NULLS LAST, \"LASTCHANGED\" DESC NULLS LAST, \"_nl_published\" DESC NULLS LAST, \"_nl_source\" DESC) AS _nl_rank FROM known) ranked WHERE _nl_rank = 1"),
        "{s}"
    );
    assert!(
        s.contains("WHERE \"SETTLEMENTDATE\" > TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00' AND \"SETTLEMENTDATE\" <= TIMESTAMP WITH TIME ZONE '2023-12-31 14:15:00.000000+00:00'"),
        "{s}"
    );
    assert!(
        s.contains("SELECT \"SETTLEMENTDATE\", \"RUNNO\", \"REGIONID\""),
        "every column, by name: {s}"
    );
    assert!(!s.contains("EXCLUDE") && !s.contains("QUALIFY"), "{s}");
    assert!(!s.contains("lead("), "{s}");

    // Revisions: the order columns join the partition.
    let s = render_duck(
        &bind(
            Query::new(
                "DISPATCHPRICE",
                day,
                Resolution::Revisions { known_at: None },
            ),
            &lake,
        )
        .unwrap(),
        &lake,
    );
    assert!(
        s.contains("PARTITION BY \"REGIONID\", \"DISPATCHINTERVAL\", \"INTERVENTION\", \"SETTLEMENTDATE\", \"RUNNO\", \"LASTCHANGED\" ORDER BY \"_nl_published\" DESC NULLS LAST, \"_nl_source\" DESC) AS _nl_rank"),
        "{s}"
    );
    assert!(!s.contains("WHERE _nl_known"), "{s}");

    // A `from` table: the spans and the in-force window; `--at` too.
    let s = render_duck(
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
    assert!(s.contains("CASE WHEN next_ends.next_valid >= TIMESTAMP WITH TIME ZONE '2262-04-11 23:47:16.854775+00:00' THEN NULL ELSE next_ends.next_valid END AS _nl_valid_to FROM latest JOIN next_ends USING (\"DUID\", \"EFFECTIVEDATE\")"), "{s}");
    assert!(s.ends_with("_nl_known, _nl_valid_to FROM spans WHERE \"EFFECTIVEDATE\" <= TIMESTAMP WITH TIME ZONE '2023-12-31 14:00:00.000000+00:00' AND (_nl_valid_to IS NULL OR _nl_valid_to > TIMESTAMP WITH TIME ZONE '2022-12-31 14:00:00.000000+00:00')"), "{s}");
    let s = render_duck(
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
        s.contains(
            "WHERE \"EFFECTIVEDATE\" <= TIMESTAMP WITH TIME ZONE '2024-01-14 14:00:00.000000+00:00'"
        ),
        "{s}"
    );

    // No key: every column is the partition.
    let any = range("2000-01-01T00:00", "2030-01-01T00:00");
    let s = render_duck(
        &bind(Query::new("SSM_ENABLEMENT_COSTS", any, LATEST), &lake).unwrap(),
        &lake,
    );
    assert!(s.contains("PARTITION BY \"END_TRADINGDATE\", \"ENABLEMENT_REASON\", \"ESTIMATED_COSTS\", \"LASTCHANGED\" ORDER BY \"_nl_published\" DESC NULLS LAST, \"_nl_source\" DESC) AS _nl_rank"), "{s}");
    assert!(
        s.contains("SELECT \"END_TRADINGDATE\", \"ENABLEMENT_REASON\", \"ESTIMATED_COSTS\", \"LASTCHANGED\", \"_nl_source\", _nl_known FROM spans "),
        "{s}"
    );

    // Raw, with a projection, and a derived table.
    let s = render_duck(
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
        s.contains(
            "SELECT \"RRP\", \"_nl_source\" FROM spans WHERE \"SETTLEMENTDATE\" > TIMESTAMP WITH TIME ZONE '"
        ),
        "{s}"
    );
    assert!(!s.contains("QUALIFY"), "{s}");
    let s = render_duck(
        &bind(Query::new("UTILAKE.NOBODY", any, Resolution::Raw), &lake).unwrap(),
        &lake,
    );
    assert!(
        s.ends_with("FROM spans ") && !s.contains("_nl_known"),
        "{s}"
    );

    // No file in the window: a query that reads nothing.
    let s = render_duck(
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
    assert!(
        s.contains("CAST(NULL AS VARCHAR) AS \"_nl_source\", CAST(NULL AS TIMESTAMP WITH TIME ZONE) AS \"_nl_published\" WHERE 1 = 0"),
        "{s}"
    );

    // Postgres: the relation named for the table, carrying `_nl_source`.
    let b = bind(Query::new("DISPATCHPRICE", day, LATEST), &lake).unwrap();
    let s = render(&b, &lake, Dialect::Postgres);
    assert!(
        s.contains("\"_nl_source\", \"_nl_published\" FROM \"DISPATCHPRICE\""),
        "{s}"
    );
    assert!(!s.contains("read_parquet"), "{s}");
    let d = render(&b, &lake, Dialect::DuckDb);
    let after = |s: &str| s[s.find("stamped AS").unwrap()..].to_owned();
    assert_eq!(after(&s), after(&d), "only the raw CTE differs");
}
