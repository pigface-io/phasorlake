//! One square frame of one table over an interval.
//!
//! The lake keeps every row of every file. A frame is what an analyst
//! wants instead: every file projected onto one schema by field id (a
//! column a file lacks is a typed null), one row per fact (the newest
//! revision, as known at a time, or every revision each once), and the
//! versions of a standing-data table that were in force across the
//! window. The rules come from the table's published roles
//! (`utilake.temporal.*`: entity, valid, order, cutoff), so the same
//! kernel reads every declared table of every market.
//!
//! A read is a [`Query`] bound to a table ([`bind()`]) and run ([`read()`]),
//! or written as one file ([`extract`]). Nothing here opens the manifest:
//! the caller lists the lake's files ([`catalog()`]) and hands them in a
//! [`Lake`].

#![forbid(unsafe_code)]

pub mod bind;
pub mod catalog;
pub mod error;
pub mod output;
pub mod query;
pub mod resolve;
pub mod scan;
pub mod sql;
pub mod stamp;
pub mod time;
pub mod window;

use std::path::PathBuf;

use arrow_array::RecordBatch;
pub use bind::bind;
pub use catalog::{LakeFile, SourceStamp, catalog};
pub use error::QueryError;
pub use output::{Format, Frame, Frames, RawBatch, Report, Resolved, SourceRef, Timing, Written};
pub use query::{Bound, BoundSpec, FieldIx, Filter, Projection, Query, Resolution, Unit};
pub use time::{
    Known, KnownTime, MarketOffset, Micros, Valid, ValidEnd, ValidTime, ValidWindow, parse_local,
    rfc3339,
};
pub use utilake_iceberg::Target;
use utilake_mms_registry::Registry;

/// Parquet's field-id metadata key, the one the lake writes.
pub const FIELD_ID_KEY: &str = utilake_mms_registry::arrow::FIELD_ID_KEY;

/// The lake, as a read sees it: the registry and the files of the tables
/// it may read.
#[derive(Debug, Clone)]
pub struct Lake<'a> {
    /// The tables and their roles.
    pub registry: &'a Registry,
    /// The files, from the manifest's live outputs ([`catalog()`]).
    pub files: Vec<LakeFile>,
    /// The utilake version, written into every output.
    pub version: &'a str,
}

/// Runs a bound read: scans each unit's files, resolves them, and
/// returns the frames in the output schema.
pub fn read(b: &Bound, lake: &Lake<'_>) -> Result<Frames, QueryError> {
    let mut report = Report {
        warnings: b.warnings.clone(),
        ..Report::default()
    };
    let known_at = match b.query.resolution {
        Resolution::Latest { known_at } => known_at,
        _ => None,
    };
    // Units, in partition order (which is time order, as the directories
    // are named); one unit when the table resolves whole.
    let by_partition = b.spec.as_ref().is_some_and(|s| s.unit == Unit::Partition);
    let mut units: Vec<(String, Vec<&query::FileRead>)> = Vec::new();
    for fr in &b.files {
        let key = if by_partition {
            lake.files[fr.file].partition.clone()
        } else {
            String::new()
        };
        match units.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(fr),
            None => units.push((key, vec![fr])),
        }
    }
    units.sort_by(|a, b| a.0.cmp(&b.0));
    for fr in &b.files {
        let f = &lake.files[fr.file];
        report.sources.push(SourceRef {
            url: f.url.to_string(),
            entry: f.entry.to_string(),
            source_period: f.source_period.clone(),
            published: f.published.map(|p| rfc3339(p.get(), 0)),
        });
    }
    let names = output::source_names(lake);
    let scan_schema = scan::scan_schema(&b.schema, &b.offset.tz());
    let mut frames = Vec::new();
    let mut out_schema = None;
    let us = |t: std::time::Instant| u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
    for (_, reads) in &units {
        let mut batches: Vec<RecordBatch> = Vec::new();
        let t = std::time::Instant::now();
        for fr in reads {
            let ix = u32::try_from(fr.file).expect("fewer than 2^32 files");
            batches.extend(scan::scan(b, lake, fr, ix, known_at, &mut report)?);
            report.files += 1;
        }
        report.timing.scan_us += us(t);
        if batches.is_empty() {
            continue;
        }
        let t = std::time::Instant::now();
        let unit = arrow_select::concat::concat_batches(&scan_schema, &batches)
            .map_err(|e| QueryError::arrow(&b.table, e))?;
        drop(batches);
        report.timing.concat_us += us(t);
        report.units += 1;
        let frame = match (&b.query.resolution, &b.spec) {
            (Resolution::Raw, _) | (_, None) => {
                let t = std::time::Instant::now();
                let out =
                    out_schema.get_or_insert_with(|| output::output_schema(b, &unit.schema()));
                let f = Frame::Raw(RawBatch(output::finish(&unit, b, out, &names)?));
                report.timing.finish_us += us(t);
                f
            }
            (_, Some(spec)) => {
                let t = std::time::Instant::now();
                let mut resolved = resolve::resolve(&unit, b, spec, &mut report)?;
                if !b.value_filters.is_empty() {
                    resolved = scan::apply_filters(&resolved, &b.value_filters)
                        .map_err(|e| QueryError::arrow(&b.table, e))?;
                }
                report.timing.resolve_us += us(t);
                let t = std::time::Instant::now();
                let out =
                    out_schema.get_or_insert_with(|| output::output_schema(b, &resolved.schema()));
                let f = Frame::Resolved(Resolved(output::finish(&resolved, b, out, &names)?));
                report.timing.finish_us += us(t);
                f
            }
        };
        report.rows_out += frame.batch().num_rows() as u64;
        frames.push(frame);
    }
    let schema = out_schema.unwrap_or_else(|| output::output_schema(b, &scan_schema));
    Ok(Frames {
        schema,
        frames,
        report,
    })
}

/// A read written as one file.
#[derive(Debug, Clone)]
pub struct Extract {
    /// What to read.
    pub query: Query,
    /// Where to write it.
    pub out: PathBuf,
    /// As what.
    pub format: Format,
}

/// Binds, reads and writes: what `utilake read -o` and `utilake get` do.
pub fn extract(req: Extract, lake: &Lake<'_>) -> Result<Written, QueryError> {
    let b = bind(req.query, lake)?;
    let frames = read(&b, lake)?;
    output::write(&frames, &b, req.format, &req.out, lake.version)
}
