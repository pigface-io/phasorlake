//! The flag tree.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// A copy of every file under nemweb's archive folders, and its catalog.
#[derive(Parser, Debug)]
#[command(name = "nemweb-mirror", version, about, max_term_width = 100)]
pub struct Cli {
    /// Home directory (default `~/.nemweb-mirror`): the archive, and the
    /// mirror's records with its config under `mirror/`.
    #[arg(long, global = true, env = "NEMWEB_MIRROR_HOME")]
    pub home: Option<PathBuf>,
    /// Machine-readable output: NDJSON, one event or record per line.
    #[arg(long, global = true)]
    pub json: bool,
    /// No progress line.
    #[arg(long, global = true)]
    pub no_progress: bool,
    /// The progress line on stderr even when it is not a terminal.
    #[arg(long, global = true)]
    pub progress: bool,
    /// Never touch the network.
    #[arg(long, global = true)]
    pub offline: bool,
    /// Threads that read zips for the map (default: the config, else half the cores).
    #[arg(long, global = true)]
    pub jobs: Option<usize>,
    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

/// The commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Read every listing page under the roots into the crawl record.
    Crawl {
        /// Roots to crawl instead of nemweb's Data_Archive/ and Reports/ARCHIVE/.
        #[arg(long = "root")]
        roots: Vec<String>,
        /// Move the current crawl aside and read every page again.
        #[arg(long)]
        fresh: bool,
    },
    /// Download every file of the finished crawl into the archive, each once.
    Download {
        /// Only the files whose URL starts with this; repeatable.
        #[arg(long)]
        only: Vec<String>,
    },
    /// Read the central directory of every zip not yet downloaded, by
    /// ranged requests, into peek.ndjson.
    Peek,
    /// Crawl (carrying on from a cut-off crawl), then peek and download
    /// together; when nemweb refuses (HTTP 403/429) or the network fails,
    /// wait and start again.
    Run {
        /// Roots to crawl instead of nemweb's Data_Archive/ and Reports/ARCHIVE/.
        #[arg(long = "root")]
        roots: Vec<String>,
        /// Move the current crawl aside and read every page again.
        #[arg(long)]
        fresh: bool,
        /// How many refusals or network failures in a row to wait out before
        /// giving up; an attempt in which any request succeeded starts the
        /// count again.
        #[arg(long, default_value_t = 8)]
        refusals: u32,
        /// Minutes to wait after a refusal or a network failure.
        #[arg(long, default_value_t = 15)]
        wait_minutes: u64,
        /// Just before each whole-month zip, compare its listing with its
        /// folder, downloaded by then, and when they are the same record it
        /// removed instead of downloading it.
        #[arg(long)]
        delete_duplicates: bool,
    },
    /// Write the map of every folder and file, zips opened as folders, to
    /// map.ndjson and tree.txt under {home}/mirror/map/; offline.
    Map {
        /// The roots the crawl used, when not the default ones.
        #[arg(long = "root")]
        roots: Vec<String>,
    },
    /// Compare each zip nemweb lists beside a folder of the same name (the
    /// whole-month zips) with that folder, both archived.
    Compare {
        /// Remove each zip found the same as its folder, recording it so it
        /// is not downloaded again and stays in the map.
        #[arg(long, conflicts_with = "verify")]
        delete: bool,
        /// Instead, read every removed zip's folder files again and check
        /// them against the zip's listing.
        #[arg(long)]
        verify: bool,
    },
    /// Record the CRC-32 of every downloaded file that has none, reading it
    /// once and checking its sha256 against its sidecar on the way.
    Hash {
        /// Read every downloaded file, recorded CRC or not.
        #[arg(long)]
        all: bool,
    },
    /// Fold the map into patterns (digit runs as `{n}`) and write it as
    /// Markdown, small enough to commit.
    Summary {
        /// Where to write it.
        #[arg(long, default_value = "nemweb/map.md")]
        out: PathBuf,
        /// Fail if the file there is not the summary of the current map.
        #[arg(long)]
        check: bool,
    },
    /// Pages read, and files archived out of those listed, per root.
    Status {
        /// The roots the crawl used, when not the default ones.
        #[arg(long = "root")]
        roots: Vec<String>,
        /// Count from the archive now, even while a run keeps counts.
        #[arg(long)]
        full: bool,
    },
}
