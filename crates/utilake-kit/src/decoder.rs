//! A decoder reads one entry of one file into typed blocks.
//!
//! The pipeline opens the entry and hands its bytes to the decoder; the
//! decoder hands back batches of rows, each with the table they belong to
//! and the transform they were read by. Which rows fit, which are kept as
//! text and which are quarantined is the decoder's business; where the
//! batches go is the sink's.

use std::io::Read;
use std::sync::Arc;

use crate::source::LocalFile;
use arrow_array::RecordBatch;
use utilake_core::{Diagnostic, Fingerprint, Period};

use crate::transform::Transform;

/// Whether a file adds to a table or replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadMode {
    /// Each file adds rows for its period.
    Append,
    /// Each file is a full snapshot of standing data.
    Snapshot,
}

/// How strictly a decoder reads: what a source publishes that the catalog
/// does not describe is kept, or refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Strictness {
    /// Keep every row; an undeclared table or column is kept as text.
    #[default]
    Permissive,
    /// Refuse a block the catalog does not describe exactly.
    Strict,
}

/// A table of the lake, as a block names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    /// The market directory: `nem`, `wem`.
    pub market: String,
    /// The table's name, as the catalog declares it: `DISPATCH_UNIT_SCADA`.
    pub name: String,
    /// The directory under the market the table's files go to.
    pub dir: String,
    /// Seconds east of UTC of the market's time, which partitions by month.
    pub utc_offset_secs: i32,
    /// Append or snapshot.
    pub load_mode: LoadMode,
    /// The index of the time column in the lake schema, when the table has one.
    pub time_column: Option<usize>,
}

/// A batch of rows from one block of one entry.
#[derive(Debug)]
pub struct Block {
    /// The table the rows belong to.
    pub table: Arc<TableRef>,
    /// The table's version the block was read against.
    pub version: u16,
    /// The fingerprint of the block's header: two column lists of one
    /// version are two shapes.
    pub fingerprint: Fingerprint,
    /// The transform the rows were read by.
    pub transform: Arc<Transform>,
    /// The rows, in the table's lake schema (with any raw columns beside it).
    pub batch: RecordBatch,
}

/// Where a row is in its entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowAt {
    /// 1-based line.
    pub line: u64,
    /// 0-based byte offset of the line.
    pub byte_offset: u64,
}

/// Whether to go on after a bad row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep decoding.
    Continue,
    /// Stop now.
    Stop,
}

/// Receives what a decoder reads.
pub trait BlockSink {
    /// A block began: its table, version and transform are known, and
    /// `status` says how the block matched the catalog.
    fn block_started(
        &mut self,
        table: &Arc<TableRef>,
        version: u16,
        fingerprint: Fingerprint,
        transform: &Arc<Transform>,
        status: &str,
    );
    /// A batch of rows of the current block is ready.
    fn batch(&mut self, b: Block) -> Result<(), Box<Diagnostic>>;
    /// A warning raised while reading (a deviation, an unseen version).
    fn warning(&mut self, d: Diagnostic);
    /// A row the decoder rejected: its bytes, where it is, and why.
    fn bad_row(&mut self, raw: &[u8], at: RowAt, d: Diagnostic) -> Flow;
}

/// What a decoder reads with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeOptions {
    /// How strictly the catalog is applied.
    pub strictness: Strictness,
    /// Bad rows tolerated per entry before the entry fails.
    pub max_bad_rows: u64,
    /// Rows per batch.
    pub batch_rows: usize,
}

/// The entry a decoder is reading.
pub struct DecodeCtx<'a> {
    /// The file the entry is in.
    pub file: &'a LocalFile,
    /// The entry's name, nested names joined with `!`.
    pub entry: &'a str,
    /// The period the file covers, when known: it decides which version
    /// of a table a block is read against.
    pub period: Option<Period>,
    /// Options.
    pub options: &'a DecodeOptions,
    /// The version of the file published before this one, when the source
    /// knows it ([`LocalFile::previous`]): a decoder of a feed that
    /// restates its files reads only the rows absent from it.
    pub previous: Option<&'a std::path::Path>,
}

/// What a decoder counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStats {
    /// Lines read, headers and footer included.
    pub lines: u64,
    /// Bytes read.
    pub bytes: u64,
    /// Blocks begun.
    pub blocks: u64,
    /// Rows accepted.
    pub rows: u64,
    /// Rows rejected.
    pub bad_rows: u64,
    /// Rows the version of the file before this one already had, left out
    /// by a feed that restates its files.
    pub rows_repeated: u64,
    /// The line count the file declared of itself, when it did.
    pub footer_declared: Option<u64>,
}

impl DecodeStats {
    /// Whether the file declared a line count other than what was read.
    pub fn footer_mismatch(&self) -> bool {
        self.footer_declared.is_some_and(|d| d != self.lines)
    }
}

/// Reads one format into blocks.
///
/// A trait object, not an enum: the pipeline must not depend on the crate
/// of any one format, and a kit registers its decoder at assembly. The
/// dynamic call is per entry and per batch, never per row.
pub trait Decoder: Send + Sync {
    /// The format's id, recorded with what it reads: `mms`.
    fn id(&self) -> &'static str;
    /// Whether an entry of this name is one this decoder reads.
    fn accepts(&self, entry_name: &str) -> bool;
    /// Reads `reader` to the end, handing blocks to `sink`. `scratch` is
    /// a read buffer the caller keeps between entries.
    fn decode(
        &self,
        reader: &mut dyn Read,
        ctx: &DecodeCtx<'_>,
        sink: &mut dyn BlockSink,
        scratch: &mut Vec<u8>,
    ) -> Result<DecodeStats, Box<Diagnostic>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_and_defaults() {
        let s = DecodeStats {
            lines: 5,
            footer_declared: Some(6),
            ..Default::default()
        };
        assert!(s.footer_mismatch());
        assert!(
            !DecodeStats {
                footer_declared: Some(5),
                ..s
            }
            .footer_mismatch()
        );
        assert!(!DecodeStats::default().footer_mismatch());
        assert_eq!(Strictness::default(), Strictness::Permissive);
        assert_eq!(RowAt::default().line, 0);
        assert_ne!(Flow::Continue, Flow::Stop);
        assert_ne!(LoadMode::Append, LoadMode::Snapshot);
    }
}
