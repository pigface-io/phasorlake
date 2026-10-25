//! `utilake get`: an interval of some tables, fetched where the market
//! publishes it most finely, read into the lake, and written as one
//! square frame per table through the query engine, with a sidecar that
//! says where every row came from.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use utilake_core::Diagnose as _;
use utilake_pipeline::GetRequest;
use utilake_query::{
    Dialect, Filter, Format, Lake, Query, Resolution, Target, WriteOptions, bind, catalog,
};

use crate::cli::{Cli, DialectArg, FormatArg, GetCmd, ResolveArgs};
use crate::context::Context;
use crate::render;

use super::common::{self, Selection, fail};

/// The resolution and shape the flags ask for, as a query of `table`.
pub fn query(
    table: &str,
    window: utilake_query::ValidWindow,
    args: &ResolveArgs,
    def: &utilake_mms_registry::file_model::MarketDef,
) -> Result<Query> {
    let known_at = args
        .known_at
        .as_deref()
        .map(|t| common::known_at(t, def))
        .transpose()?;
    let resolution = if args.raw {
        Resolution::Raw
    } else if args.revisions {
        Resolution::Revisions { known_at }
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
    Ok(Query::new(table.to_owned(), window, resolution)
        .with_target(args.as_of.map_or(Target::Latest, Target::AsOf))
        .with_columns(args.columns.clone())
        .with_filters(filters))
}

fn write_options(args: &crate::cli::OutputArgs) -> WriteOptions {
    let d = WriteOptions::default();
    WriteOptions {
        format: match args.format {
            FormatArg::Csv => Format::Csv,
            FormatArg::Arrow => Format::Arrow,
            FormatArg::Parquet | FormatArg::Sql => Format::Parquet,
        },
        codec: args.compression.codec(),
        target_file_bytes: args.target_file_bytes.unwrap_or(d.target_file_bytes),
        row_group_rows: d.row_group_rows,
    }
}

/// Where a table's output goes: `-o -` is standard output, a path with a
/// format's extension is one file, anything else a directory of parts
/// under `{out}/{market}/{TABLE}/`.
enum Dest {
    Stdout,
    File(PathBuf),
    Dir(PathBuf),
}

fn dest(out: Option<&Path>, market: &str, table: &str, tables: usize) -> Result<Dest> {
    match out {
        Some(p) if p == Path::new("-") => {
            if tables > 1 {
                bail!("-o - prints one table; name one.");
            }
            Ok(Dest::Stdout)
        }
        Some(p)
            if p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| Format::of_extension(e).is_some() || e == "sql") =>
        {
            if tables > 1 {
                bail!("A file holds one table; name a directory for {tables}.");
            }
            Ok(Dest::File(p.to_path_buf()))
        }
        other => Ok(Dest::Dir(
            other
                .unwrap_or(Path::new("utilake-get"))
                .join(market)
                .join(table),
        )),
    }
}

/// `utilake get TABLES --from A --to B | --at T | --day D`
pub async fn get(cli: &Cli, ctx: &Context, args: &GetCmd) -> Result<i32> {
    let sel: Selection = common::select(ctx, &args.tables)?;
    if sel.ids.is_empty() {
        bail!("Name at least one table; `utilake list` names them.");
    }
    let Some(w) = common::window(&args.window, &sel.def)? else {
        bail!("Give --from and --to, --at, or --day.");
    };
    let request = common::request("get", &sel, Some(&w), &sel.def);
    // The queries are checked before anything is fetched.
    let queries: Vec<Query> = sel
        .names
        .iter()
        .map(|t| query(t, w.valid, &args.resolve, &sel.def))
        .collect::<Result<_>>()?;
    let (manifest, thread) = ctx.manifest()?;
    // Stages one and two: the files that cover the window, fetched and
    // read into the lake, unless the lake is to be read as it is.
    let mut plan = None;
    if !ctx.offline {
        let req = GetRequest {
            kit: sel.kit.clone(),
            tables: sel.names.clone(),
            exclude: args
                .tables
                .exclude
                .iter()
                .map(|t| t.to_ascii_uppercase())
                .collect(),
            range: w.days(&sel.def),
            instants: w.instants,
            source: super::fetch::source_choice(args.fetch.source),
            request: request.clone(),
        };
        let options =
            super::lake::options(cli, ctx, &args.ingest, args.fetch.downloads, args.dry_run);
        let report = match super::acquire::run(cli, ctx, manifest.clone(), req, options).await {
            Ok(Ok(r)) => r,
            Ok(Err(code)) => {
                drop(manifest);
                thread.join();
                return Ok(code);
            }
            Err(e) => {
                drop(manifest);
                thread.join();
                return Err(e);
            }
        };
        if report.exit_code != 0 {
            if ctx.json {
                render::json_line(&super::acquire::json("get", &request, &report));
            } else {
                println!("nothing written: a step failed (exit {})", report.exit_code);
            }
            drop(manifest);
            thread.join();
            return Ok(report.exit_code);
        }
        plan = Some(super::acquire::json("get", &request, &report));
    }
    // Stage three: each table as one frame.
    let options = write_options(&args.output);
    let mut written = Vec::new();
    let mut planned = Vec::new();
    let mut code = 0;
    for (id, q) in sel.ids.iter().zip(queries) {
        let name = ctx.registry.table(*id).name.clone();
        let outputs = manifest
            .live_outputs(Some(&name))
            .await
            .map_err(|e| fail(&e))?;
        let catalog = catalog(&outputs);
        let lake = Lake {
            registry: &ctx.registry,
            files: catalog.files,
            sources: catalog.sources,
            version: env!("CARGO_PKG_VERSION"),
        };
        let b = match bind(q, &lake) {
            Ok(b) => b,
            Err(e) => {
                drop(manifest);
                thread.join();
                return Err(fail(&e));
            }
        };
        if args.dry_run {
            planned.push(serde_json::json!({
                "table": name, "schema_id": b.schema_id, "resolution": b.query.resolution.name(),
                "lake_files": b.files.len(), "window": utilake_query::output::window_text(&b),
            }));
            continue;
        }
        if b.files.is_empty() {
            let e = utilake_query::QueryError::WindowUncovered {
                table: name.clone(),
                window: serde_json::to_string(&utilake_query::output::window_text(&b))
                    .unwrap_or_default(),
            };
            if ctx.offline {
                drop(manifest);
                thread.join();
                return Err(fail(&e));
            }
            // Fetched, and still nothing: the market has nothing for the
            // window; said, not failed, so the other tables are written.
            eprintln!("{}", render::diagnostic_text(&e.diagnostic()));
            code = 3;
            continue;
        }
        let to = dest(
            args.output.out.as_deref(),
            &sel.def.market.to_string(),
            &name,
            sel.ids.len(),
        )?;
        if args.output.format == FormatArg::Sql {
            let dialect = match args.output.dialect {
                DialectArg::Duckdb => Dialect::DuckDb,
                DialectArg::Postgres => Dialect::Postgres,
            };
            let sql = utilake_query::sql::render(&b, &lake, dialect);
            let path = match to {
                Dest::Stdout => {
                    println!("{sql}");
                    continue;
                }
                Dest::File(p) => p,
                Dest::Dir(d) => d.join(format!("{name}.sql")),
            };
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, format!("{sql}\n"))?;
            written.push(serde_json::json!({ "table": name, "files": [path], "rows": null }));
            continue;
        }
        let out = match to {
            Dest::Stdout => bail!("-o - prints SQL only; name a file or directory for rows."),
            Dest::File(p) | Dest::Dir(p) => p,
        };
        let r = utilake_query::extract_bound(&b, &lake, options, &out);
        let x = match r {
            Ok(x) => x,
            Err(e) => {
                drop(manifest);
                thread.join();
                return Err(fail(&e));
            }
        };
        for warning in &x.report.warnings {
            if ctx.json {
                render::json_line(warning);
            } else {
                eprintln!("{}", render::diagnostic_text(warning));
            }
        }
        written.push(serde_json::json!({
            "table": name, "schema_id": b.schema_id, "files": x.parts, "sidecar": x.sidecar, "rows": x.rows,
            "rows_read": x.report.rows_read, "dropped_window": x.report.dropped_window,
            "dropped_unknown": x.report.dropped_unknown, "nan_rows": x.report.nan_rows,
            "lake_files": x.report.files, "units": x.report.units,
            "intervals": x.report.intervals, "timing": x.report.timing,
        }));
    }
    drop(manifest);
    thread.join();
    if ctx.json {
        render::json_line(&serde_json::json!({
            "type": "get",
            "request": request,
            "plan": plan,
            "read_plan": planned,
            "written": written,
            "exit_code": code,
        }));
    } else if args.dry_run {
        let rows: Vec<Vec<String>> = planned
            .iter()
            .map(|p| {
                vec![
                    p["table"].as_str().unwrap_or_default().to_owned(),
                    p["resolution"].as_str().unwrap_or_default().to_owned(),
                    p["lake_files"].to_string(),
                    serde_json::to_string(&p["window"]).unwrap_or_default(),
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(&["table", "resolution", "lake files", "window"], &rows)
        );
        println!("dry run: nothing written");
    } else {
        let rows: Vec<Vec<String>> = written
            .iter()
            .map(|x| {
                let files = x["files"].as_array().map_or(0, Vec::len);
                let first = x["files"][0].as_str().unwrap_or_default();
                let shown = match files {
                    0 | 1 => first.to_owned(),
                    n => format!(
                        "{n} parts under {}",
                        Path::new(first)
                            .parent()
                            .map_or_else(String::new, |d| d.display().to_string())
                    ),
                };
                vec![
                    x["table"].as_str().unwrap_or_default().to_owned(),
                    x["rows"]
                        .as_u64()
                        .map_or_else(|| "-".to_owned(), |r| r.to_string()),
                    x["intervals"]["step"]
                        .as_str()
                        .map(|s| {
                            format!(
                                "{s}, {} of {}",
                                x["intervals"]["present"], x["intervals"]["expected"]
                            )
                        })
                        .unwrap_or_default(),
                    shown,
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(&["table", "rows", "intervals", "written"], &rows)
        );
    }
    Ok(code)
}
