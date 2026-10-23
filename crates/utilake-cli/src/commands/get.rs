//! `utilake get`: an interval of some tables, planned across the market's
//! sources, fetched, read into the lake, and written as one Parquet file
//! per table with a sidecar that says where its rows came from.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use tokio_util::sync::CancellationToken;
use utilake_core::{Diagnose as _, YearMonth};
use utilake_iceberg::{DataFile, Target};
use utilake_mms_registry::Market;
use utilake_pipeline::{GetRequest, SourceChoice, days_of, run_get};
use utilake_writer::{ExtractRequest, extract};

use crate::cli::{Cli, GetArgs, SourceArg};
use crate::context::Context;
use crate::render;

/// An instant from the forms `get` accepts, in a market at `offset_secs`
/// from UTC: `YYYY-MM`, `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS]` (a space
/// for the `T` too), or RFC 3339 with its own offset. As an end, a month
/// or a day means the instant after its last; as a start, its first.
pub fn parse_when(text: &str, offset_secs: i32, end: bool) -> Result<i64> {
    let text = text.trim();
    let bad =
        || anyhow::anyhow!("{text:?} is not a month, a day, a local time or an RFC 3339 time");
    if let Ok(ts) = text.parse::<jiff::Timestamp>() {
        return Ok(ts.as_microsecond());
    }
    if let Ok(ym) = text.parse::<YearMonth>() {
        let m = if end { ym.next() } else { ym };
        return Ok(utilake_core::Period::Month(m)
            .first_day()
            .start_micros(offset_secs));
    }
    if let Ok(day) = text.parse::<utilake_core::CivilDay>() {
        let d = if end { day.next() } else { day };
        return Ok(d.start_micros(offset_secs));
    }
    let local = text
        .replace(' ', "T")
        .parse::<jiff::civil::DateTime>()
        .map_err(|_| bad())?;
    let tz =
        jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_seconds(offset_secs).map_err(|_| bad())?);
    let zoned = local.to_zoned(tz).map_err(|_| bad())?;
    Ok(zoned.timestamp().as_microsecond())
}

/// `utilake get <market> --table T --from A --to B`
pub async fn get(cli: &Cli, ctx: &Context, args: &GetArgs) -> Result<i32> {
    let market = match args.market.to_ascii_lowercase().as_str() {
        "nem" => Market::Nem,
        "wem" => Market::Wem,
        other => bail!("No market named {other}; the markets are nem and wem."),
    };
    let def = ctx
        .registry
        .market(market)
        .context("the registry defines no such market")?;
    let (offset, tz) = (def.utc_offset_secs, def.arrow_tz());
    let from = parse_when(&args.from, offset, false)?;
    let to = parse_when(&args.to, offset, true)?;
    if to <= from {
        bail!("--to {} is not after --from {}.", args.to, args.from);
    }
    let tables: Vec<String> = args.table.iter().map(|t| t.to_ascii_uppercase()).collect();
    for t in &tables {
        if ctx.registry.table_by_name(t).is_none() {
            bail!("No table named {t}; `utilake schema list` names them.");
        }
    }
    let kit = args.market.to_ascii_lowercase();
    let request = format!("get {kit} {} {}..{}", tables.join(","), args.from, args.to);
    let req = GetRequest {
        kit: kit.clone(),
        tables: tables.clone(),
        exclude: args
            .exclude
            .iter()
            .map(|t| t.to_ascii_uppercase())
            .collect(),
        range: days_of((from, to), offset),
        instants: (from, to),
        source: match args.source {
            SourceArg::Auto => SourceChoice::Auto,
            SourceArg::Monthly => SourceChoice::Monthly,
            SourceArg::Daily => SourceChoice::Daily,
        },
        request: request.clone(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel(4096);
    let fetcher = ctx.fetcher(Some(tx.clone()))?;
    let (manifest, thread) = ctx.manifest()?;
    let deps = ctx.deps(fetcher, manifest.clone());
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("interrupted; finishing the files in flight");
            c2.cancel();
        }
    });
    let consumer = tokio::spawn(render::consume_events(
        rx,
        ctx.json,
        ctx.progress,
        cli.verbose > 0,
    ));
    let options = super::lake::options(cli, ctx, &args.ingest, args.downloads, args.dry_run);
    let report = run_get(req, deps, options, tx, cancel).await;
    let _ = consumer.await;
    let report = match report {
        Ok(r) => r,
        Err(d) => {
            drop(manifest);
            thread.join();
            bail!("{}", d.render());
        }
    };
    let json = ctx.json;
    if !json {
        for n in &report.plan.notes {
            println!("plan: {n}");
        }
        for (step, run) in report.plan.steps.iter().zip(&report.runs) {
            println!(
                "{}: {} file(s) over {}, {:.1} MB listed",
                step.source.id(),
                step.files.len(),
                step.query.range,
                step.bytes() as f64 / 1e6
            );
            print!("{}", render::summary_text(&run.summary, &run.report));
        }
    }
    if args.dry_run || report.exit_code != 0 {
        if json {
            render::json_line(&serde_json::json!({
                "type": "get",
                "request": request,
                "notes": report.plan.notes,
                "steps": report.plan.steps.iter().map(|s| serde_json::json!({
                    "source": s.source.id(), "range": s.query.range.to_string(),
                    "files": s.files, "bytes": s.bytes(),
                })).collect::<Vec<_>>(),
                "runs": report.runs.iter().map(|r| serde_json::json!({
                    "run_id": r.run_id, "exit_code": r.exit_code, "summary": r.summary,
                })).collect::<Vec<_>>(),
                "exit_code": report.exit_code,
            }));
        } else if report.exit_code != 0 {
            println!("nothing written: a step failed (exit {})", report.exit_code);
        }
        drop(manifest);
        thread.join();
        return Ok(report.exit_code);
    }
    // One file per table out of the lake.
    let out_dir = args
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("utilake-get"));
    let mut written = Vec::new();
    for table in &tables {
        if req_excluded(&args.exclude, table) {
            continue;
        }
        let id = ctx.registry.table_by_name(table).expect("checked above");
        let history = ctx
            .registry
            .history(id)
            .with_context(|| format!("{table} has no history to read by"))?;
        let outputs = manifest
            .live_outputs(Some(table))
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        let latest = history.current_schema_id;
        let files: Vec<DataFile> = outputs
            .iter()
            .map(|o| DataFile {
                path: o.path.clone(),
                schema_id: o.schema_id.unwrap_or(latest),
                min_us: o.min_time.map(|t| t.div_euclid(1_000)),
                max_us: o.max_time.map(|t| t.div_euclid(1_000)),
            })
            .collect();
        let sources: Vec<serde_json::Value> = outputs
            .iter()
            .map(|o| {
                serde_json::json!({
                    "path": o.path, "url": o.url, "entry": o.entry,
                    "source_period": o.source_period, "rows": o.rows, "schema_id": o.schema_id,
                })
            })
            .collect();
        let out = out_dir.join(&kit).join(format!("{table}.parquet"));
        let time_column = ctx.registry.table(id).time_column.clone();
        let x = extract(&ExtractRequest {
            table,
            history,
            files,
            interval: (from, to),
            target: args.as_of.map_or(Target::Latest, Target::AsOf),
            time_column: time_column.as_deref(),
            tz: &tz,
            max_rows: args.max_rows,
            rows_estimate: outputs.iter().map(|o| o.rows).sum(),
            out: &out,
            version: env!("CARGO_PKG_VERSION"),
            sources: serde_json::Value::Array(sources),
        })
        .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
        written.push(x);
    }
    drop(manifest);
    thread.join();
    if json {
        render::json_line(&serde_json::json!({
            "type": "get",
            "request": request,
            "notes": report.plan.notes,
            "written": written.iter().map(|x| serde_json::json!({
                "path": x.path, "sidecar": x.sidecar, "rows": x.rows, "files": x.files, "schema_id": x.schema_id,
            })).collect::<Vec<_>>(),
            "exit_code": 0,
        }));
    } else {
        let rows: Vec<Vec<String>> = written
            .iter()
            .map(|x| {
                vec![
                    x.path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    x.rows.to_string(),
                    x.files.to_string(),
                    x.schema_id.to_string(),
                    x.path.display().to_string(),
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(&["table", "rows", "lake files", "schema", "written"], &rows)
        );
    }
    Ok(0)
}

fn req_excluded(exclude: &[String], table: &str) -> bool {
    exclude.iter().any(|x| x.eq_ignore_ascii_case(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_of_when_reads_in_market_time() {
        let aest = 36_000;
        let day = |s: &str| s.parse::<utilake_core::CivilDay>().unwrap();
        assert_eq!(
            parse_when("2024-01", aest, false).unwrap(),
            day("2024-01-01").start_micros(aest)
        );
        assert_eq!(
            parse_when("2024-01", aest, true).unwrap(),
            day("2024-02-01").start_micros(aest)
        );
        assert_eq!(
            parse_when("2024-01-15", aest, false).unwrap(),
            day("2024-01-15").start_micros(aest)
        );
        assert_eq!(
            parse_when("2024-01-15", aest, true).unwrap(),
            day("2024-01-16").start_micros(aest)
        );
        // A local time in market time; the same as an end.
        let ten = day("2024-01-15").start_micros(aest) + 10 * 3_600_000_000;
        assert_eq!(parse_when("2024-01-15T10:00", aest, false).unwrap(), ten);
        assert_eq!(parse_when("2024-01-15 10:00:00", aest, true).unwrap(), ten);
        // RFC 3339 carries its own offset.
        assert_eq!(
            parse_when("2024-01-15T00:00:00Z", aest, false).unwrap(),
            day("2024-01-15").start_micros(0)
        );
        assert_eq!(
            parse_when("2024-01-15T10:00:00+10:00", aest, true).unwrap(),
            ten
        );
        for bad in ["yesterday", "2024-13-01", "2024-01-15T25:00", ""] {
            assert!(parse_when(bad, aest, false).is_err(), "{bad}");
        }
        assert!(!req_excluded(&["A".into()], "B"));
        assert!(req_excluded(&["a".into()], "A"));
    }
}
