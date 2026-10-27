//! `list`: what there is, for a consumer. Every table with its roles and
//! what the lake holds of it; one table's columns; the months held and
//! the gaps; the files, the archive, the manifest's ingests and runs.

use std::collections::BTreeMap;

use anyhow::Result;
use phasor_registry::TableId;
use phasor_writer::manifest::LiveOutput;

use crate::cli::ListCmd;
use crate::context::Context;
use crate::render;

use super::common::{fail, rfc3339, table_id};

/// What the lake holds of one table, from its live outputs.
#[derive(Default)]
struct Held {
    files: u64,
    rows: u64,
    min: Option<i64>,
    max: Option<i64>,
}

impl Held {
    fn take(&mut self, o: &LiveOutput) {
        self.files += 1;
        self.rows += o.rows;
        if let Some(t) = o.min_time {
            self.min = Some(self.min.map_or(t, |m| m.min(t)));
        }
        if let Some(t) = o.max_time {
            self.max = Some(self.max.map_or(t, |m| m.max(t)));
        }
    }

    fn span(&self, off: i32) -> String {
        match (self.min, self.max) {
            (Some(a), Some(b)) => format!(
                "{}..{}",
                rfc3339(a.div_euclid(1000), off),
                rfc3339(b.div_euclid(1000), off)
            ),
            _ => String::new(),
        }
    }
}

/// Which side of the publication a lake file came from, by its URL.
fn side(url: &str) -> &'static str {
    if url.contains("/Reports/ARCHIVE/") {
        "daily"
    } else if url.contains("/Reports/CURRENT/") {
        "current"
    } else if url.contains("MMSDM") || url.contains("Data_Archive") {
        "monthly"
    } else if url.starts_with("file:") || url.starts_with('/') {
        "local"
    } else {
        "other"
    }
}

/// `phasor list ...`
pub async fn list(ctx: &Context, args: &ListCmd) -> Result<i32> {
    if args.archive {
        return archive(ctx);
    }
    if args.runs {
        return runs(ctx).await;
    }
    if args.ingests {
        return ingests(ctx, args).await;
    }
    let r = &ctx.registry;
    let id =
        match &args.table {
            Some(t) => Some(table_id(r, t).ok_or_else(|| {
                anyhow::anyhow!("No table is named {t}; `phasor list` names them.")
            })?),
            None => None,
        };
    if args.files {
        return files(ctx, id).await;
    }
    if args.coverage {
        return coverage(ctx, id.expect("--coverage requires a table")).await;
    }
    match id {
        Some(id) => columns(ctx, id),
        None => tables(ctx, args).await,
    }
}

/// Every table.
async fn tables(ctx: &Context, args: &ListCmd) -> Result<i32> {
    let r = &ctx.registry;
    let (m, thread) = ctx.manifest()?;
    let outputs = m.live_outputs(None).await.map_err(|e| fail(&e))?;
    drop(m);
    thread.join();
    let mut held: BTreeMap<&str, Held> = BTreeMap::new();
    for o in &outputs {
        held.entry(o.table.as_str()).or_default().take(o);
    }
    let grep = args.grep.as_deref().map(str::to_ascii_lowercase);
    let mut rows = Vec::new();
    for (id, t) in r.tables() {
        if args
            .market
            .as_deref()
            .is_some_and(|m| !m.eq_ignore_ascii_case(t.market.id()))
        {
            continue;
        }
        if args
            .kind
            .as_deref()
            .is_some_and(|k| k != t.temporal.valid.kind())
        {
            continue;
        }
        if let Some(g) = &grep
            && !(t.name.to_ascii_lowercase().contains(g)
                || t.description
                    .as_deref()
                    .is_some_and(|d| d.to_ascii_lowercase().contains(g)))
        {
            continue;
        }
        let off = r.market_of(id).utc_offset_secs;
        let h = held.get(t.name.as_str());
        let kind = match t.temporal.valid.column() {
            Some(c) => format!("{} {c}", t.temporal.valid.kind()),
            None => t.temporal.valid.kind().to_owned(),
        };
        rows.push((
            t.name.clone(),
            serde_json::json!({
                "table": t.name,
                "market": t.market.id(),
                "kind": t.temporal.valid.kind(),
                "valid": t.temporal.valid.column(),
                "entity": t.temporal.entity,
                "order": t.temporal.order,
                "cutoff": t.temporal.cutoff,
                "mode": format!("{:?}", t.load_mode).to_ascii_lowercase(),
                "lake_files": h.map_or(0, |h| h.files),
                "lake_rows": h.map_or(0, |h| h.rows),
                "lake_span": h.map(|h| h.span(off)).filter(|s| !s.is_empty()),
                "description": t.description,
            }),
            vec![
                t.name.clone(),
                t.market.id().to_owned(),
                kind,
                t.temporal.entity.join(","),
                h.map_or_else(|| "0".to_owned(), |h| h.rows.to_string()),
                h.map_or_else(String::new, |h| h.span(off)),
                t.description.clone().unwrap_or_default(),
            ],
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    if ctx.json {
        for (_, j, _) in &rows {
            render::json_line(j);
        }
    } else {
        let text: Vec<Vec<String>> = rows.into_iter().map(|(_, _, r)| r).collect();
        print!(
            "{}",
            render::table(
                &[
                    "table",
                    "market",
                    "valid",
                    "entity",
                    "rows",
                    "span",
                    "description"
                ],
                &text
            )
        );
    }
    Ok(0)
}

/// One table's columns.
fn columns(ctx: &Context, id: TableId) -> Result<i32> {
    let r = &ctx.registry;
    let t = r.table(id);
    // The data model's words for each column, as phasor-schemas
    // publishes them.
    let docs: BTreeMap<u32, String> = t.published().docs;
    let role = |name: &str| -> &'static str {
        let tp = &t.temporal;
        if tp.entity.iter().any(|e| e == name) {
            "entity"
        } else if tp.valid.column().is_some_and(|c| c == name) {
            "valid"
        } else if tp.order.iter().any(|o| o == name) {
            if tp.cutoff.as_deref() == Some(name) {
                "order, cutoff"
            } else {
                "order"
            }
        } else if tp.cutoff.as_deref() == Some(name) {
            "cutoff"
        } else {
            ""
        }
    };
    let rows: Vec<Vec<String>> = t
        .columns
        .iter()
        .map(|c| {
            vec![
                c.name.clone(),
                c.ty.to_string(),
                role(&c.name).to_owned(),
                c.id.to_string(),
                if c.nullable {
                    String::new()
                } else {
                    "required".into()
                },
                docs.get(&c.id.0).cloned().unwrap_or_default(),
            ]
        })
        .collect();
    if ctx.json {
        for c in &t.columns {
            render::json_line(&serde_json::json!({
                "table": t.name, "column": c.name, "type": c.ty.to_string(),
                "role": role(&c.name), "field_id": c.id.0, "nullable": c.nullable,
                "description": docs.get(&c.id.0),
            }));
        }
    } else {
        if let Some(d) = &t.description {
            println!("{}: {d}", t.name);
        }
        print!(
            "{}",
            render::table(
                &["column", "type", "role", "field", "", "description"],
                &rows
            )
        );
    }
    Ok(0)
}

/// The months the lake holds of a table, and the gaps.
async fn coverage(ctx: &Context, id: TableId) -> Result<i32> {
    let r = &ctx.registry;
    let t = r.table(id);
    let off = r.market_of(id).utc_offset_secs;
    let (m, thread) = ctx.manifest()?;
    let outputs = m.live_outputs(Some(&t.name)).await.map_err(|e| fail(&e))?;
    drop(m);
    thread.join();
    let mut months: BTreeMap<String, (Held, BTreeMap<&str, u64>)> = BTreeMap::new();
    for o in &outputs {
        let e = months.entry(o.partition.clone()).or_default();
        e.0.take(o);
        *e.1.entry(side(&o.url)).or_default() += 1;
    }
    // Gaps: months between the first and the last held with nothing.
    let keys: Vec<String> = months.keys().cloned().collect();
    let mut gaps = Vec::new();
    if let (Some(first), Some(last)) = (keys.first(), keys.last())
        && let (Ok(a), Ok(b)) = (
            first
                .trim_start_matches("year=")
                .replace("/month=", "-")
                .parse::<phasor_core::YearMonth>(),
            last.trim_start_matches("year=")
                .replace("/month=", "-")
                .parse::<phasor_core::YearMonth>(),
        )
    {
        let mut m = a;
        while m <= b {
            let key = format!("year={:04}/month={:02}", m.year(), m.month());
            if !months.contains_key(&key) && !months.contains_key(&m.to_string()) {
                gaps.push(m.to_string());
            }
            m = m.next();
        }
    }
    if ctx.json {
        for (p, (h, sides)) in &months {
            render::json_line(&serde_json::json!({
                "table": t.name, "partition": p, "files": h.files, "rows": h.rows,
                "span": h.span(off), "sides": sides,
            }));
        }
        render::json_line(&serde_json::json!({ "table": t.name, "gaps": gaps }));
    } else {
        let rows: Vec<Vec<String>> = months
            .iter()
            .map(|(p, (h, sides))| {
                vec![
                    p.clone(),
                    h.files.to_string(),
                    h.rows.to_string(),
                    h.span(off),
                    sides
                        .iter()
                        .map(|(s, n)| format!("{s} ×{n}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(&["partition", "files", "rows", "span", "sources"], &rows)
        );
        if gaps.is_empty() {
            println!("{} partition(s), no gap", months.len());
        } else {
            println!("{} partition(s); gaps: {}", months.len(), gaps.join(", "));
        }
    }
    Ok(0)
}

/// The lake's files.
async fn files(ctx: &Context, id: Option<TableId>) -> Result<i32> {
    let r = &ctx.registry;
    let name = id.map(|id| r.table(id).name.clone());
    let (m, thread) = ctx.manifest()?;
    let outputs = m
        .live_outputs(name.as_deref())
        .await
        .map_err(|e| fail(&e))?;
    drop(m);
    thread.join();
    if ctx.json {
        for o in &outputs {
            render::json_line(o);
        }
    } else {
        let rows: Vec<Vec<String>> = outputs
            .iter()
            .map(|o| {
                let off = r
                    .table_by_name(&o.table)
                    .map_or(36_000, |id| r.market_of(id).utc_offset_secs);
                let mut h = Held::default();
                h.take(o);
                vec![
                    o.table.clone(),
                    o.partition.clone(),
                    o.rows.to_string(),
                    h.span(off),
                    side(&o.url).to_owned(),
                    sources_text(&o.url, &o.entries),
                    o.path.clone(),
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(
                &[
                    "table",
                    "partition",
                    "rows",
                    "span",
                    "side",
                    "source",
                    "path"
                ],
                &rows
            )
        );
        println!("{} file(s)", outputs.len());
    }
    Ok(0)
}

/// Every archived download.
fn archive(ctx: &Context) -> Result<i32> {
    let store = ctx.archive();
    let entries = phasor_acquire::archive::entries_under(&store, None);
    if ctx.json {
        for e in &entries {
            render::json_line(&serde_json::json!({ "path": e.path, "record": e.provenance }));
        }
    } else {
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|e| {
                vec![
                    format!("{:.1} MB", e.provenance.content.size as f64 / 1e6),
                    e.provenance.fetched_at().rfc3339(),
                    e.provenance
                        .modified()
                        .map(phasor_core::timetext::Stamp::http_date)
                        .unwrap_or_default(),
                    e.provenance
                        .modified()
                        .map(|s| s.rfc3339())
                        .unwrap_or_default(),
                    e.provenance.url().to_owned(),
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(
                &["size", "fetched", "published", "published_at", "url"],
                &rows,
            )
        );
        println!(
            "{} file(s), {:.1} MB, under {}",
            entries.len(),
            entries
                .iter()
                .map(|e| e.provenance.content.size)
                .sum::<u64>() as f64
                / 1e6,
            store.root().display()
        );
    }
    Ok(0)
}

/// The latest run.
async fn runs(ctx: &Context) -> Result<i32> {
    let (m, thread) = ctx.manifest()?;
    let latest = m.latest_run().await.map_err(|e| fail(&e))?;
    drop(m);
    thread.join();
    if ctx.json {
        render::json_line(&latest);
    } else if let Some(r) = latest {
        print!(
            "{}",
            render::table(
                &["run", "started", "finished", "exit", "argv"],
                &[vec![
                    r.id.to_string(),
                    r.started_at,
                    r.finished_at.unwrap_or_default(),
                    r.exit_code.map(|c| c.to_string()).unwrap_or_default(),
                    r.argv
                ]]
            )
        );
    } else {
        println!("no runs yet");
    }
    Ok(0)
}

/// The manifest's ingests.
async fn ingests(ctx: &Context, args: &ListCmd) -> Result<i32> {
    let table = args
        .table
        .as_deref()
        .map(|t| table_id(&ctx.registry, t).map(|id| ctx.registry.table(id).name.clone()))
        .map(|t| {
            t.ok_or_else(|| {
                anyhow::anyhow!(
                    "No table is named {}.",
                    args.table.as_deref().unwrap_or_default()
                )
            })
        })
        .transpose()?;
    let (m, thread) = ctx.manifest()?;
    let ingests = m
        .list_ingests(table, args.failed, args.limit)
        .await
        .map_err(|e| fail(&e))?;
    drop(m);
    thread.join();
    if ctx.json {
        for i in &ingests {
            render::json_line(i);
        }
    } else {
        let rows: Vec<Vec<String>> = ingests
            .iter()
            .map(|i| {
                vec![
                    i.id.to_string(),
                    i.table.clone(),
                    i.schema_version.to_string(),
                    format!("{:?}", i.status).to_ascii_lowercase(),
                    i.rows.to_string(),
                    i.bad_rows.to_string(),
                    i.outputs.len().to_string(),
                    i.error_code.clone().unwrap_or_default(),
                    sources_text(&i.url, &i.entries),
                ]
            })
            .collect();
        print!(
            "{}",
            render::table(
                &[
                    "id", "table", "ver", "status", "rows", "bad", "files", "error", "source"
                ],
                &rows
            )
        );
    }
    Ok(0)
}

/// A source file's name and what of it was read: its one entry, or how
/// many entries.
fn sources_text(url: &str, entries: &[phasor_writer::manifest::EntryShare]) -> String {
    let name = url.rsplit('/').next().unwrap_or(url);
    match entries {
        [] => name.to_owned(),
        [e] if e.entry.is_empty() => name.to_owned(),
        [e] => format!("{name}!{}", e.entry),
        many => format!("{name} ({} entries)", many.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_by_url() {
        assert_eq!(
            side("https://nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/x.zip"),
            "daily"
        );
        assert_eq!(
            side("https://nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/x.zip"),
            "current"
        );
        assert_eq!(
            side("https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/x.zip"),
            "monthly"
        );
        assert_eq!(side("/home/x/a.zip"), "local");
        assert_eq!(side("https://example.com/a.zip"), "other");
        let mut h = Held::default();
        assert_eq!(h.span(36_000), "");
        h.take(&LiveOutput {
            table: "T".into(),
            path: "p".into(),
            rows: 2,
            bytes: 1,
            schema_id: None,
            source_version: 1,
            source_period: None,
            url: "u".into(),
            entries: vec![phasor_writer::manifest::EntryShare {
                entry: "e".into(),
                ..Default::default()
            }],
            min_time: Some(1_704_031_200_000_000_000),
            max_time: Some(1_704_031_500_000_000_000),
            partition: "year=2024/month=01".into(),
            last_modified_at: None,
        });
        assert_eq!(
            h.span(36_000),
            "2024-01-01T00:00:00+10:00..2024-01-01T00:05:00+10:00"
        );
        assert_eq!((h.files, h.rows), (1, 2));
    }
}
