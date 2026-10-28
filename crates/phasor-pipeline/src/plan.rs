//! What a run does and what it does it with.

use std::path::PathBuf;
use std::sync::Arc;

use phasor_acquire::Fetcher;
use phasor_core::paths::Home;
use phasor_kit::{KitRegistry, Strictness};
use phasor_kit::{Query, Source};
use phasor_writer::ManifestHandle;

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
    pub layout: phasor_core::config::Lake,
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
            layout: phasor_core::config::Lake::default(),
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
    /// Files a source listed already, fetched and read as [`Input::Remote`]'s are.
    Listed {
        /// The source that listed them.
        source: Arc<dyn Source>,
        /// The query it listed them for.
        query: Query,
        /// The files.
        files: Vec<phasor_kit::RemoteFile>,
    },
    /// The archived files of a source for a query, read from disk, never the network.
    Archive {
        /// The source whose archive is read.
        source: Arc<dyn Source>,
        /// The periods and tables.
        query: Query,
    },
    /// Local zips or CSVs.
    Local(Vec<PathBuf>),
    /// Local zips or CSVs, every one read by the kit named, whatever its name says.
    LocalIn {
        /// The kit's id, e.g. `gas`.
        kit: String,
        /// The files.
        paths: Vec<PathBuf>,
    },
    /// Every recorded source not wholly in the lake by the current rules, re-read from disk.
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
