//! `mirror`: a copy of every file under nemweb's archive folders.

use std::fmt::Write as _;
use std::io::Write as _;

use anyhow::Result;
use nemlake_acquire::Fetcher;
use nemlake_core::Diagnose;
use nemlake_mirror::{Mirror, MirrorError, Status, Url};

use crate::cli::MirrorCmd;
use crate::context::Context;
use crate::render;

/// `nemlake mirror ...`
pub async fn mirror(ctx: &Context, cmd: &MirrorCmd) -> Result<i32> {
    let mirror = Mirror::new(ctx.home.root().join("mirror"));
    let result = match cmd {
        MirrorCmd::Crawl { roots, fresh } => {
            let fetcher = ctx.fetcher(None)?;
            crawl(ctx, &fetcher, &mirror, roots, *fresh).await
        }
        MirrorCmd::Status { roots } => status(ctx, &mirror, roots),
    };
    match result {
        Ok(code) => Ok(code),
        Err(e) => {
            let d = e.diagnostic();
            if ctx.json {
                render::json_line(&d);
            } else {
                eprintln!("{}", d.render());
            }
            Ok(1)
        }
    }
}

fn roots(ctx: &Context, given: &[String]) -> Result<Vec<Url>, MirrorError> {
    if given.is_empty() {
        nemlake_mirror::default_roots(&ctx.config.sources.nem_base_url)
    } else {
        given.iter().map(|r| nemlake_mirror::root(r)).collect()
    }
}

async fn crawl(
    ctx: &Context,
    fetcher: &Fetcher,
    mirror: &Mirror,
    given: &[String],
    fresh: bool,
) -> Result<i32, MirrorError> {
    let roots = roots(ctx, given)?;
    let json = ctx.json;
    let progress = ctx.progress;
    let summary = nemlake_mirror::crawl(fetcher, mirror, &roots, fresh, |p| {
        if json {
            render::json_line(&serde_json::json!({ "event": "page_read", "page": p }));
        } else if progress {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r{} pages read, {} queued   ", p.pages, p.queued);
            let _ = err.flush();
        }
    })
    .await?;
    if json {
        render::json_line(&serde_json::json!({ "event": "crawl_done", "summary": summary }));
    } else {
        if progress {
            eprintln!();
        }
        if summary.already_done {
            println!("The crawl is complete already; pass --fresh to read every page again.");
        }
        println!(
            "{} folders, {} files ({:.1} GB), {} links out of the roots; {} pages read, {} of them from nemweb",
            summary.dirs,
            summary.files,
            summary.bytes as f64 / 1e9,
            summary.external,
            summary.pages,
            summary.pages_fetched,
        );
    }
    Ok(0)
}

fn status(ctx: &Context, mirror: &Mirror, given: &[String]) -> Result<i32, MirrorError> {
    let roots = roots(ctx, given)?;
    let s = nemlake_mirror::status(mirror, &ctx.archive(), &roots)?;
    if ctx.json {
        render::json_line(&s);
    } else {
        print!("{}", status_text(&s));
    }
    Ok(0)
}

fn status_text(s: &Status) -> String {
    let crawl = if s.crawl_done {
        "complete"
    } else {
        "in progress"
    };
    let mut out = format!("crawl {crawl}: {} pages read\n", s.pages);
    if !s.crawl_done {
        return out;
    }
    let rows: Vec<Vec<String>> = s
        .roots
        .iter()
        .map(|r| {
            vec![
                r.root.clone(),
                format!("{}/{}", r.archived_files, r.files),
                format!(
                    "{:.1}/{:.1}",
                    r.archived_bytes as f64 / 1e9,
                    r.bytes as f64 / 1e9
                ),
            ]
        })
        .collect();
    out.push_str(&render::table(&["root", "files", "GB"], &rows));
    if s.external > 0 {
        let _ = writeln!(out, "{} links out of the roots, not followed", s.external);
    }
    out
}
