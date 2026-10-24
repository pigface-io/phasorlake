//! `read` and `sql`: one frame of one table over an interval, run or
//! printed as DuckDB SQL.

use std::path::PathBuf;

use anyhow::Result;
use utilake_core::Diagnose;
use utilake_query::{
    Dialect, Extract, Filter, Format, Known, Lake, MarketOffset, Query, QueryError, Resolution,
    Target, Valid, ValidWindow, WriteOptions, bind, catalog, parse_local,
};

use crate::cli::ReadArgs;
use crate::context::Context;
use crate::render;

fn fail(e: &impl Diagnose) -> anyhow::Error {
    anyhow::anyhow!("{}", e.diagnostic().render())
}

/// The query the flags ask for, and the market clock the times were read in.
fn query(ctx: &Context, args: &ReadArgs) -> Result<(Query, MarketOffset)> {
    let r = &ctx.registry;
    let id = r
        .table_by_name(&args.table)
        .or_else(|| {
            let (rt, st) = args.table.split_once('.')?;
            r.table_by_pair(rt, st)
        })
        .ok_or_else(|| fail(&QueryError::UnknownTable(args.table.clone())))?;
    let off = MarketOffset(r.market_of(id).utc_offset_secs);
    let valid = |t: &str| parse_local::<Valid>(t, off).map_err(|e| fail(&e));
    let window = match (&args.at, &args.from, &args.to) {
        (Some(at), _, _) => ValidWindow::At(valid(at)?),
        (None, Some(from), Some(to)) => {
            ValidWindow::range(valid(from)?, valid(to)?).map_err(|e| fail(&e))?
        }
        _ => anyhow::bail!("Give --from and --to, or --at."),
    };
    let known_at = args
        .known_at
        .as_deref()
        .map(|t| parse_local::<Known>(t, off).map_err(|e| fail(&e)))
        .transpose()?;
    let resolution = if args.raw {
        Resolution::Raw
    } else if args.revisions {
        Resolution::Revisions
    } else {
        Resolution::Latest { known_at }
    };
    let filters = args
        .filters
        .iter()
        .map(|f| {
            let (column, values) = f
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("A filter is COL=v1,v2; {f:?} is not."))?;
            Ok(Filter {
                column: column.to_owned(),
                values: values.split(',').map(str::to_owned).collect(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let q = Query::new(r.table(id).name.clone(), window, resolution)
        .with_target(args.as_of.map_or(Target::Latest, Target::AsOf))
        .with_columns(args.columns.clone())
        .with_filters(filters);
    Ok((q, off))
}

/// The lake's files of a table, from the manifest.
async fn lake<'a>(ctx: &'a Context, table: &str, off: MarketOffset) -> Result<Lake<'a>> {
    let (manifest, thread) = ctx.manifest()?;
    let outputs = manifest
        .live_outputs(Some(table))
        .await
        .map_err(|e| fail(&e))?;
    drop(manifest);
    thread.join();
    Ok(Lake {
        registry: &ctx.registry,
        files: catalog(&outputs, off),
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// `utilake read`.
pub async fn read(ctx: &Context, args: &ReadArgs) -> Result<i32> {
    let (q, off) = query(ctx, args)?;
    let lake = lake(ctx, &q.table, off).await?;
    let format = match args.format.as_str() {
        "csv" => Format::Csv,
        "arrow" => Format::Arrow,
        _ => Format::Parquet,
    };
    let report = match &args.out {
        Some(out) => {
            let written = utilake_query::extract(
                Extract {
                    query: q,
                    out: PathBuf::from(out),
                    options: WriteOptions {
                        format,
                        ..WriteOptions::default()
                    },
                },
                &lake,
            )
            .map_err(|e| fail(&e))?;
            if ctx.json {
                render::json_line(&serde_json::json!({
                    "files": written.parts, "sidecar": written.sidecar, "rows": written.rows,
                }));
            } else {
                println!(
                    "{} rows written to {} (sidecar {})",
                    written.rows,
                    written
                        .parts
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    written.sidecar.display()
                );
            }
            written.report
        }
        None => {
            let b = bind(q, &lake).map_err(|e| fail(&e))?;
            let frames = utilake_query::read(&b, &lake).map_err(|e| fail(&e))?;
            let columns: Vec<String> = frames
                .schema
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect();
            let r = &frames.report;
            if ctx.json {
                render::json_line(&serde_json::json!({
                    "table": b.table, "schema_id": b.schema_id, "resolution": b.query.resolution.name(),
                    "rows": r.rows_out, "rows_read": r.rows_read, "units": r.units, "files": r.files,
                    "dropped_window": r.dropped_window, "dropped_unknown": r.dropped_unknown,
                    "nan_rows": r.nan_rows, "columns": columns, "sources": r.sources,
                    "intervals": r.intervals, "timing": r.timing,
                }));
            } else {
                let mut rows = vec![
                    vec!["table".to_owned(), b.table.clone()],
                    vec![
                        "resolution".to_owned(),
                        b.query.resolution.name().to_owned(),
                    ],
                    vec![
                        "schema".to_owned(),
                        b.schema_id
                            .map_or_else(|| "none (derived)".to_owned(), |s| s.to_string()),
                    ],
                    vec!["rows".to_owned(), r.rows_out.to_string()],
                    vec!["rows read".to_owned(), r.rows_read.to_string()],
                    vec!["files".to_owned(), r.files.to_string()],
                    vec!["units".to_owned(), r.units.to_string()],
                    vec![
                        "outside the window".to_owned(),
                        r.dropped_window.to_string(),
                    ],
                    vec!["not yet known".to_owned(), r.dropped_unknown.to_string()],
                    vec!["columns".to_owned(), columns.join(", ")],
                    vec![
                        "time".to_owned(),
                        format!(
                            "scan {} ms, concat {} ms, resolve {} ms, finish {} ms",
                            r.timing.scan_us / 1000,
                            r.timing.concat_us / 1000,
                            r.timing.resolve_us / 1000,
                            r.timing.finish_us / 1000
                        ),
                    ],
                ];
                if let Some(i) = &r.intervals {
                    rows.push(vec![
                        "intervals".to_owned(),
                        format!(
                            "step {}, {} present{}, {} gap(s)",
                            i.step,
                            i.present,
                            i.expected.map_or_else(String::new, |e| format!(" of {e}")),
                            i.gaps.len() as u64 + i.more_gaps
                        ),
                    ]);
                }
                print!("{}", render::table(&["", ""], &rows));
            }
            frames.report
        }
    };
    for w in &report.warnings {
        if ctx.json {
            render::json_line(w);
        } else {
            eprintln!("{}", render::diagnostic_text(w));
        }
    }
    Ok(0)
}

/// `utilake sql`: the read as DuckDB SQL; raw unless a resolution is asked for.
pub async fn sql(ctx: &Context, args: &ReadArgs) -> Result<i32> {
    let (mut q, off) = query(ctx, args)?;
    if !(args.latest || args.revisions || args.known_at.is_some()) {
        q.resolution = Resolution::Raw;
    }
    let lake = lake(ctx, &q.table, off).await?;
    let b = bind(q, &lake).map_err(|e| fail(&e))?;
    println!("{}", utilake_query::sql::render(&b, &lake, Dialect::DuckDb));
    Ok(0)
}
