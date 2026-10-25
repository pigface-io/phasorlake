//! What a run does and what it does it with.

use std::path::PathBuf;
use std::sync::Arc;

use utilake_acquire::{Fetcher, Query, Source};
use utilake_core::paths::Home;
use utilake_kit::{KitRegistry, Strictness};
use utilake_writer::ManifestHandle;

/// Rows per batch by default.
pub const DEFAULT_BATCH_ROWS: usize = 65_536;

/// Knobs of a run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Parse workers in flight.
    pub jobs: usize,
    /// Downloads in flight.
    pub downloads: usize,
    /// How strictly the catalog is applied.
    pub mode: Strictness,
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
    /// Ingest what is fetched; `false` only downloads into the archive.
    pub ingest: bool,
    /// How the lake's Parquet files are laid out.
    pub layout: utilake_core::config::Lake,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            jobs: 2,
            downloads: 4,
            mode: Strictness::Permissive,
            max_bad_rows: 0,
            fail_fast: false,
            dry_run: false,
            strict_footer: false,
            batch_rows: DEFAULT_BATCH_ROWS,
            ingest: true,
            layout: utilake_core::config::Lake::default(),
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
    /// Files a source listed already, for a query: fetched and read as
    /// [`Input::Remote`]'s are, listed again by no one.
    Listed {
        /// The source that listed them.
        source: Arc<dyn Source>,
        /// The query it listed them for.
        query: Query,
        /// The files.
        files: Vec<utilake_acquire::RemoteFile>,
    },
    /// The archived files of a source for a query: what `fetch`
    /// downloaded, read from disk, never from the network.
    Archive {
        /// The source whose archive is read.
        source: Arc<dyn Source>,
        /// The periods and tables.
        query: Query,
    },
    /// Local zips or CSVs.
    Local(Vec<PathBuf>),
    /// Local zips or CSVs, every one read by the kit named, whatever source
    /// would claim it by its name: for a file whose name or place says
    /// nothing of its market (a renamed download, a copy outside the
    /// mirror's folders).
    LocalIn {
        /// The kit's id, e.g. `gas`.
        kit: String,
        /// The files.
        paths: Vec<PathBuf>,
    },
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
    /// Every kit the binary was assembled with.
    pub kits: Arc<KitRegistry>,
    /// The fetcher.
    pub fetcher: Arc<Fetcher>,
    /// The manifest.
    pub manifest: ManifestHandle,
    /// The home.
    pub home: Home,
}
