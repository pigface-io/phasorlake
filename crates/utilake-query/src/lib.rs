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
//! A read is a [`Query`] bound to a table ([`bind()`]) and run ([`read()`],
//! or [`read_with()`] a unit at a time), or written out ([`extract`]). Nothing here opens the manifest:
//! the caller lists the lake's files ([`catalog()`]) and hands them in a
//! [`Lake`].

#![forbid(unsafe_code)]

pub mod bind;
pub mod catalog;
pub mod error;
pub mod intervals;
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
pub use intervals::{Gap, Intervals};
pub use output::{
    Codec, Format, Frame, Frames, RawBatch, Report, Resolved, SourceRef, Timing, WriteOptions,
    Writer, Written,
};
pub use query::{Bound, BoundSpec, FieldIx, Filter, Projection, Query, Resolution, Unit};
pub use sql::Dialect;
pub use time::{
    Known, KnownTime, MarketOffset, Micros, Valid, ValidEnd, ValidTime, ValidWindow, parse_local,
    rfc3339,
};
pub use utilake_iceberg::Target;
use utilake_mms_registry::Registry;
use utilake_mms_registry::file_model::ValidAxis;

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

/// Runs a bound read, handing each unit's frame to `sink` as it resolves,
/// so a caller holds one unit at a time: the frames are in unit order (a
/// partition at a time, or the whole table once). Returns what the read
/// did.
pub fn read_with(
    b: &Bound,
    lake: &Lake<'_>,
    sink: &mut dyn FnMut(Frame) -> Result<(), QueryError>,
) -> Result<Report, QueryError> {
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
    let out = output::output_schema(b);
    // The valid times of an interval table's rows, for the count of
    // intervals the window should hold.
    let stepped = b.spec.as_ref().and_then(|s| match s.valid {
        ValidAxis::IntervalEnd(c) | ValidAxis::Instant(c) | ValidAxis::Day(c) => Some(c),
        _ => None,
    });
    let mut times: Vec<i64> = Vec::new();
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
                let f = Frame::Raw(RawBatch(output::finish(&unit, b, &out, &names)?));
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
                if let Some(c) = stepped
                    && let Some(v) = scan::micros_of(resolved.column(c.0), b.offset)
                {
                    let mut seen: Vec<i64> = v.into_iter().flatten().collect();
                    seen.sort_unstable();
                    seen.dedup();
                    times.extend(seen);
                }
                report.timing.resolve_us += us(t);
                let t = std::time::Instant::now();
                let f = Frame::Resolved(Resolved(output::finish(&resolved, b, &out, &names)?));
                report.timing.finish_us += us(t);
                f
            }
        };
        report.rows_out += frame.batch().num_rows() as u64;
        sink(frame)?;
    }
    report.intervals = intervals::measure(&mut times, &b.query.window, b.offset);
    Ok(report)
}

/// Runs a bound read and returns every frame, in the output schema.
pub fn read(b: &Bound, lake: &Lake<'_>) -> Result<Frames, QueryError> {
    let mut frames = Vec::new();
    let report = read_with(b, lake, &mut |f| {
        frames.push(f);
        Ok(())
    })?;
    Ok(Frames {
        schema: output::output_schema(b),
        frames,
        report,
    })
}

/// A read written out.
#[derive(Debug, Clone)]
pub struct Extract {
    /// What to read.
    pub query: Query,
    /// Where to write it: a file when the extension names a format, else
    /// a directory of parts.
    pub out: PathBuf,
    /// How.
    pub options: WriteOptions,
}

/// Binds, reads and writes, one unit at a time: what `utilake get` does.
pub fn extract(req: Extract, lake: &Lake<'_>) -> Result<Written, QueryError> {
    let b = bind(req.query, lake)?;
    extract_bound(&b, lake, req.options, &req.out)
}

/// Reads a bound query and writes it, one unit at a time.
pub fn extract_bound(
    b: &Bound,
    lake: &Lake<'_>,
    options: WriteOptions,
    out: &std::path::Path,
) -> Result<Written, QueryError> {
    let schema = output::output_schema(b);
    let mut w = output::Writer::open(b, schema, options, out, lake.version)?;
    let report = read_with(b, lake, &mut |f| w.write(&f))?;
    w.close(report)
}
