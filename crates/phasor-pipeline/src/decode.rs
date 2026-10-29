//! Reading one local file through its kit's decoders, wherever the blocks go:
//! the lake ([`crate::ingest_file`]), or a sink of the caller's. The one walk
//! over a file's entries lives here, so every reader of a file reads it alike.

use std::path::Path;

use phasor_core::{Category, Diagnostic};
use phasor_kit::{BlockSink, DecodeCtx, DecodeOptions, DecodeStats, Flow, LocalFile, SourceKit};
use phasor_provenance::layout::listing_offset;
use phasor_zip::{Accepts, ArchiveError, WalkError, Walked, has_extension, walk_entries};

/// Where a file's blocks go, told of each entry around them.
pub trait FileSink: BlockSink {
    /// Entry `entry` is about to be read; `zip_entry_modified` is the zip's stamp for
    /// it in microseconds UTC, none for a file read whole or a zip that records none.
    fn entry_started(&mut self, entry: &str, zip_entry_modified: Option<i64>);
    /// Entry `entry` was read, or failed; [`Flow::Stop`] leaves the rest of the file.
    fn entry_done(&mut self, entry: &str, result: Result<DecodeStats, Box<Diagnostic>>) -> Flow;
}

/// What a file is read with.
pub struct DecodeFile<'a> {
    /// The kit whose decoders read it.
    pub kit: &'a SourceKit,
    /// The file.
    pub local: &'a LocalFile,
    /// How strictly, and how many rows per batch.
    pub options: &'a DecodeOptions,
    /// Scratch for spooled nested zips.
    pub tmp_dir: &'a Path,
    /// Entries (nested paths joined with `!`) to leave unread.
    pub skip: &'a [String],
}

/// Reads the file into `sink`: the file itself when a decoder of the kit reads its
/// name, else every entry of the zip one does, nested zips included, in archive order.
///
/// Returns the entries of a zip that could not be read. The error is the file's own
/// failure: no decoder reads it and it is not a zip, it could not be opened, or the
/// zip could not be walked. Each is placed at the file; the caller adds its request.
pub fn decode_file(
    d: &DecodeFile<'_>,
    sink: &mut dyn FileSink,
) -> Result<Vec<ArchiveError>, Box<Diagnostic>> {
    let path = &d.local.path;
    // The URL's name, so a mirror version `{name}@{stamp}` reads as `{name}`.
    let name = d.local.remote.file_name();
    let mut scratch = Vec::new();
    if d.kit.decoder_for(&name).is_some() {
        let mut f = std::fs::File::open(path).map_err(|e| {
            Box::new(
                Diagnostic::error(
                    "csv.read_failed",
                    Category::CSV,
                    format!("The file {} could not be opened.", path.display()),
                )
                .with_file(path.display().to_string())
                .caused_by(&e),
            )
        })?;
        decode_entry(d, &name, None, &mut f, &mut scratch, sink);
        return Ok(Vec::new());
    }
    if !has_extension(&name, ".zip") {
        return Err(Box::new(
            Diagnostic::error(
                "config.no_decoder",
                Category::CONFIG,
                format!(
                    "The file {name} is not one the kit {} reads; no decoder accepts its name and it is not a zip.",
                    d.kit.id
                ),
            )
            .with_file(path.display().to_string()),
        ));
    }
    let accepts = Accepts {
        accepts: &|n: &str| d.kit.decoder_for(n).is_some() && !d.skip.iter().any(|s| s == n),
        wanted: "readable",
    };
    let result: Result<Walked, WalkError<()>> = walk_entries(
        path,
        d.tmp_dir,
        accepts,
        &mut |e| {
            // The packer's clock: AEST for AEMO's zips, AWST for the WA host's.
            let written = e
                .modified_at(listing_offset(d.local.remote.url.as_str()))
                .map(|s| s.unix() * 1_000_000);
            match decode_entry(d, &e.name, written, e.reader, &mut scratch, sink) {
                Flow::Continue => Ok(()),
                Flow::Stop => Err(()),
            }
        },
        &mut |_| {},
    );
    match result {
        Ok(w) => Ok(w.failed),
        Err(WalkError::Callback(())) => Ok(Vec::new()),
        Err(WalkError::Archive(e)) => Err(Box::new(
            phasor_core::Diagnose::diagnostic(&e).with_file(path.display().to_string()),
        )),
    }
}

/// Reads one entry through the kit's decoder, between the sink's entry calls.
fn decode_entry(
    d: &DecodeFile<'_>,
    entry: &str,
    zip_entry_modified: Option<i64>,
    reader: &mut dyn std::io::Read,
    scratch: &mut Vec<u8>,
    sink: &mut dyn FileSink,
) -> Flow {
    let decoder = d
        .kit
        .decoder_for(entry)
        .expect("only entries a decoder accepts are read");
    sink.entry_started(entry, zip_entry_modified);
    let ctx = DecodeCtx {
        file: d.local,
        entry,
        period: d.local.remote.period,
        options: d.options,
        previous: d.local.previous.as_deref(),
    };
    let blocks: &mut dyn BlockSink = sink;
    let result = decoder.decode(reader, &ctx, blocks, scratch);
    sink.entry_done(entry, result)
}
