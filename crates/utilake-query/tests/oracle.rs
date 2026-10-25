//! The kernel against DuckDB running the same read as SQL. Needs the
//! `duckdb` binary and `UTILAKE_ORACLE_TESTS=1` (`just oracle-test`).

mod common;

use std::process::Command;

use arrow_array::RecordBatch;
use arrow_array::cast::AsArray;
use arrow_array::types::{Decimal128Type, Float64Type, Int64Type, TimestampMicrosecondType};
use common::{Fixture, ts};
use utilake_query::{
    Dialect, KnownTime, Query, Resolution, ValidTime, ValidWindow, bind, read, sql::render,
};

fn enabled() -> bool {
    std::env::var("UTILAKE_ORACLE_TESTS").is_ok_and(|v| v == "1")
}

/// Rows of a query through DuckDB, as JSON objects.
fn duck(sql: &str) -> Vec<serde_json::Value> {
    let out = Command::new("duckdb")
        .args(["-json", "-c", &format!("SET TimeZone = 'UTC'; {sql}")])
        .output()
        .expect("duckdb runs");
    assert!(
        out.status.success(),
        "{}\n{sql}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// A cell as text: times as microseconds, numbers as integers.
fn cell(b: &RecordBatch, col: &str, i: usize) -> String {
    let c = b.column_by_name(col).unwrap();
    if c.is_null(i) {
        return "null".into();
    }
    match c.data_type() {
        arrow_schema::DataType::Timestamp(_, _) => c
            .as_primitive::<TimestampMicrosecondType>()
            .value(i)
            .to_string(),
        arrow_schema::DataType::Decimal128(_, s) => (c.as_primitive::<Decimal128Type>().value(i)
            / 10i128.pow(u32::from(*s as u8)))
        .to_string(),
        arrow_schema::DataType::Float64 => {
            (c.as_primitive::<Float64Type>().value(i) as i64).to_string()
        }
        arrow_schema::DataType::Int64 => c.as_primitive::<Int64Type>().value(i).to_string(),
        _ => c.as_string::<i32>().value(i).to_owned(),
    }
}

/// Both sides of one read as sorted tuples of `cols`; `times` are the
/// columns DuckDB must give as microseconds.
fn compare(fx: &Fixture, q: Query, cols: &[&str], times: &[&str]) {
    let lake = fx.lake();
    let b = bind(q, &lake).unwrap();
    let frames = read(&b, &lake).unwrap();
    let mut ours: Vec<Vec<String>> = Vec::new();
    for f in &frames.frames {
        let batch = f.batch();
        for i in 0..batch.num_rows() {
            ours.push(cols.iter().map(|c| cell(batch, c, i)).collect());
        }
    }
    ours.sort();
    let select: Vec<String> = cols
        .iter()
        .map(|c| {
            if times.contains(c) {
                format!("epoch_us(\"{c}\")::VARCHAR AS \"{c}\"")
            } else {
                format!("\"{c}\"::VARCHAR AS \"{c}\"")
            }
        })
        .collect();
    let sql = format!(
        "SELECT {} FROM ({})",
        select.join(", "),
        render(&b, &lake, Dialect::DuckDb)
    );
    let mut theirs: Vec<Vec<String>> = duck(&sql)
        .iter()
        .map(|row| {
            cols.iter()
                .map(|c| match &row[*c] {
                    serde_json::Value::Null => "null".to_owned(),
                    serde_json::Value::String(s) => s
                        .parse::<f64>()
                        .ok()
                        .filter(|f| f.fract() == 0.0)
                        .map_or_else(|| s.clone(), |f| (f as i64).to_string()),
                    other => other.to_string(),
                })
                .collect()
        })
        .collect();
    theirs.sort();
    assert_eq!(ours, theirs, "{sql}");
}

fn range(a: &str, b: &str) -> ValidWindow {
    ValidWindow::range(ValidTime::new(ts(a)), ValidTime::new(ts(b))).unwrap()
}

#[test]
fn the_kernel_agrees_with_duckdb() {
    if !enabled() {
        eprintln!("skipped: set UTILAKE_ORACLE_TESTS=1");
        return;
    }
    let mut fx = Fixture::new();
    common::dispatchprice(&mut fx);
    common::dudetail(&mut fx);
    common::keyless(&mut fx);
    let day = range("2024-01-01T00:00", "2024-01-01T00:15");
    let dp = [
        "SETTLEMENTDATE",
        "INTERVENTION",
        "RUNNO",
        "RRP",
        "_nl_known",
    ];
    let dp_times = ["SETTLEMENTDATE", "_nl_known"];
    for res in [
        Resolution::Latest { known_at: None },
        Resolution::Latest {
            known_at: Some(KnownTime::new(ts("2024-01-01T00:11"))),
        },
        Resolution::Revisions { known_at: None },
    ] {
        compare(&fx, Query::new("DISPATCHPRICE", day, res), &dp, &dp_times);
        compare(
            &fx,
            Query::new(
                "DISPATCHPRICE",
                range("2023-12-31T23:55", "2024-01-01T00:05"),
                res,
            ),
            &dp,
            &dp_times,
        );
    }
    compare(
        &fx,
        Query::new("DISPATCHPRICE", day, Resolution::Raw),
        &["SETTLEMENTDATE", "RUNNO", "RRP", "PRE_AP_LOWER1_PRICE"],
        &["SETTLEMENTDATE"],
    );
    let du = [
        "DUID",
        "EFFECTIVEDATE",
        "VERSIONNO",
        "REGISTEREDCAPACITY",
        "_nl_valid_to",
    ];
    let du_times = ["EFFECTIVEDATE", "_nl_valid_to"];
    for res in [
        Resolution::Latest { known_at: None },
        Resolution::Latest {
            known_at: Some(KnownTime::new(ts("2024-01-01T00:00"))),
        },
        Resolution::Revisions { known_at: None },
    ] {
        compare(
            &fx,
            Query::new(
                "DUDETAIL",
                ValidWindow::At(ValidTime::new(ts("2024-01-15T00:00"))),
                res,
            ),
            &du,
            &du_times,
        );
        compare(
            &fx,
            Query::new(
                "DUDETAIL",
                range("2023-01-01T00:00", "2024-01-01T00:00"),
                res,
            ),
            &du,
            &du_times,
        );
    }
    // A day label's trading day, and a version's end column.
    common::biddayoffer(&mut fx);
    common::dudetailsummary(&mut fx);
    let bids = ["SETTLEMENTDATE", "BIDTYPE", "VERSIONNO", "PRICEBAND1"];
    for w in [
        range("2024-01-01T04:00", "2024-01-02T04:00"),
        range("2024-01-01T00:00", "2024-01-02T00:00"),
        ValidWindow::At(ValidTime::new(ts("2024-01-01T02:00"))),
    ] {
        compare(
            &fx,
            Query::new("BIDDAYOFFER_D", w, Resolution::Latest { known_at: None }),
            &bids,
            &["SETTLEMENTDATE"],
        );
    }
    let summary = ["DUID", "START_DATE", "REGIONID", "_nl_valid_to"];
    for w in [
        ValidWindow::At(ValidTime::new(ts("2024-01-15T00:00"))),
        ValidWindow::At(ValidTime::new(ts("2023-07-01T00:00"))),
        range("2021-01-01T00:00", "2022-01-01T00:00"),
    ] {
        compare(
            &fx,
            Query::new("DUDETAILSUMMARY", w, Resolution::Latest { known_at: None }),
            &summary,
            &["START_DATE", "_nl_valid_to"],
        );
    }
    compare(
        &fx,
        Query::new(
            "SSM_ENABLEMENT_COSTS",
            range("2000-01-01T00:00", "2030-01-01T00:00"),
            Resolution::Latest { known_at: None },
        ),
        &["ENABLEMENT_REASON", "ESTIMATED_COSTS"],
        &[],
    );
}
