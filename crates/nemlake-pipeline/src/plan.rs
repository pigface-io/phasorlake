//! What a run does and what it does it with.

use std::path::PathBuf;
use std::sync::Arc;

use nemlake_acquire::{Fetcher, Query, Source};
use nemlake_core::paths::Home;
use nemlake_registry::{Mode, Registry};
use nemlake_writer::ManifestHandle;

/// Knobs of a run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Parse workers in flight.
    pub jobs: usize,
    /// Downloads in flight.
    pub downloads: usize,
    /// Registry strictness.
    pub mode: Mode,
    /// Bad rows tolerated per entry.
    pub max_bad_rows: u64,
    /// Stop at the first failure.
    pub fail_fast: bool,
    /// List and plan only.
    pub dry_run: bool,
    /// Treat a footer count mismatch as an error.
    pub strict_footer: bool,
    /// Rows per Arrow batch.
    pub batch_rows: usize,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            jobs: 2,
            downloads: 4,
            mode: Mode::Permissive,
            max_bad_rows: 0,
            fail_fast: false,
            dry_run: false,
            strict_footer: false,
            batch_rows: nemlake_conform::arrow_sink::DEFAULT_BATCH_ROWS,
        }
    }
}

/// Where the files come from.
pub enum Input {
    /// A source listed for a query.
    Remote {
        /// The source.
        source: Arc<dyn Source>,
        /// The query.
        query: Query,
    },
    /// Local zips or CSVs.
    Local(Vec<PathBuf>),
    /// Every source the manifest has recorded that is not wholly in the
    /// lake by the current rules: its rules have changed since it was
    /// read, an earlier read failed, or a Parquet file is gone. Each is
    /// read again from where it is kept on disk; nothing is downloaded.
    Stale,
}

/// A run.
pub struct Plan {
    /// The user's request, for diagnostics and the manifest.
    pub request: String,
    /// Input.
    pub input: Input,
    /// Options.
    pub options: RunOptions,
}

/// What a run needs.
#[derive(Clone)]
pub struct Deps {
    /// The registry.
    pub registry: Arc<Registry>,
    /// The fetcher.
    pub fetcher: Arc<Fetcher>,
    /// The manifest.
    pub manifest: ManifestHandle,
    /// The home.
    pub home: Home,
}
