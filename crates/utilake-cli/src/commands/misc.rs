//! `wem`.

use anyhow::Result;
use utilake_acquire::{Query, Source};
use utilake_core::{Diagnose, YearMonth};
use utilake_wem::WemDatafiles;

use crate::cli::WemCmd;
use crate::context::Context;
use crate::render;

/// `utilake wem ...`
pub async fn wem(ctx: &Context, cmd: &WemCmd) -> Result<i32> {
    match cmd {
        WemCmd::Manifest { dataset, from, to } => {
            let base = utilake_wem::WemConfig::from_sources(&ctx.config.sources)
                .map_err(|e| anyhow::anyhow!("{e} (in the config file)"))?
                .base_url;
            let w = WemDatafiles::new(&base, vec![dataset.clone()])
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            let fetcher = ctx.fetcher(None)?;
            let from = from.unwrap_or(YearMonth::new(2000, 1).expect("valid"));
            let to = to.unwrap_or(YearMonth::new(2100, 12).expect("valid"));
            let files = w
                .list(
                    &Query {
                        exclude: Vec::new(),
                        tables: vec![],
                        range: utilake_core::TimeRange::months(from, to),
                        instants: None,
                    },
                    &fetcher,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.diagnostic().render()))?;
            if ctx.json {
                for f in &files {
                    render::json_line(f);
                }
            } else {
                let rows: Vec<Vec<String>> = files
                    .iter()
                    .map(|f| {
                        vec![
                            f.period.map(|p| p.to_string()).unwrap_or_default(),
                            f.modified.clone().unwrap_or_default(),
                            f.modified_at.map(|s| s.rfc3339()).unwrap_or_default(),
                            f.url.to_string(),
                        ]
                    })
                    .collect();
                print!(
                    "{}",
                    render::table(&["period", "generated", "generated_at", "url"], &rows)
                );
                println!(
                    "{} file(s); downloading WEM data is not implemented yet",
                    files.len()
                );
            }
            Ok(0)
        }
    }
}
