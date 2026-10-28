//! `~/.phasor/config.toml`. Unknown keys are errors so a typo never silently
//! falls back to a default.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::diag::{Category, Diagnose, Diagnostic};

/// Settings a user may persist; flags and `PHASOR_*` variables override the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Parse workers; 0 means half the available cores.
    pub jobs: usize,
    /// Concurrent downloads.
    pub downloads: usize,
    /// Minimum gap between two requests to a server, in milliseconds (AEMO blocks fast clients).
    pub request_interval_ms: u64,
    /// Bad rows tolerated per source before it fails.
    pub max_bad_rows: u64,
    /// Fail on every warning.
    pub strict: bool,
    /// Appended to the `User-Agent` header, e.g. a contact address.
    pub user_agent_suffix: String,
    /// How the lake's Parquet files are laid out: `[lake]`.
    pub lake: Lake,
    /// Per-source settings.
    pub sources: Sources,
}

/// How a Parquet file is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    /// zstd: the smallest files at a fair speed, read by every current reader.
    #[default]
    Zstd,
    /// LZ4: larger files, the fastest to read.
    Lz4,
    /// Snappy: larger files, read by older tools too.
    Snappy,
    /// No compression.
    None,
}

impl Codec {
    /// Its name, as config and the command line write it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Lz4 => "lz4",
            Self::Snappy => "snappy",
            Self::None => "none",
        }
    }
}

/// Which statistics a Parquet file keeps of its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Statistics {
    /// None.
    None,
    /// Per column chunk: a row group's least and greatest values.
    Chunk,
    /// Per page, with the column and offset indexes that let a reader skip pages.
    #[default]
    Page,
}

/// How the lake's Parquet files are laid out. Every value is set explicitly, and
/// none changes what a file holds, so changing one never re-reads the lake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Lake {
    /// A file is closed once it holds about this many megabytes.
    pub file_size_mb: u32,
    /// Rows per row group.
    pub row_group_rows: usize,
    /// A data page is closed at about this many kilobytes.
    pub page_kb: u32,
    /// Or at this many rows, whichever comes first.
    pub page_rows: usize,
    /// A column's dictionary may grow to this many kilobytes; past it the column is written plain.
    pub dictionary_page_kb: u32,
    /// The compression.
    pub compression: Codec,
    /// The zstd level, when the compression is zstd.
    pub zstd_level: i32,
    /// The statistics kept.
    pub statistics: Statistics,
}

impl Default for Lake {
    fn default() -> Self {
        Self {
            file_size_mb: 128,
            row_group_rows: 262_144,
            page_kb: 1024,
            page_rows: 20_000,
            dictionary_page_kb: 1024,
            compression: Codec::Zstd,
            zstd_level: 3,
            statistics: Statistics::Page,
        }
    }
}

impl Lake {
    /// The first value out of its range: which key, the value, the range.
    pub fn invalid(&self) -> Option<ConfigError> {
        let checks: [(&str, i64, i64, i64); 6] = [
            ("file_size_mb", i64::from(self.file_size_mb), 1, 4096),
            (
                "row_group_rows",
                self.row_group_rows as i64,
                1024,
                16_777_216,
            ),
            ("page_kb", i64::from(self.page_kb), 8, 65_536),
            ("page_rows", self.page_rows as i64, 1000, 1_048_576),
            (
                "dictionary_page_kb",
                i64::from(self.dictionary_page_kb),
                64,
                65_536,
            ),
            ("zstd_level", i64::from(self.zstd_level), 1, 22),
        ];
        checks
            .into_iter()
            .find(|&(_, v, lo, hi)| v < lo || v > hi)
            .map(|(key, value, lo, hi)| ConfigError::Invalid {
                key: format!("lake.{key}"),
                value: value.to_string(),
                range: format!("{lo} to {hi}"),
            })
    }
}

/// Per-kit settings: `[sources.<kit>]` tables, each read by its kit. A kit falls
/// back to the legacy `*_base_url` keys when its table is absent.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Sources {
    /// NEM base URL, as the earlier config wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nem_base_url: Option<String>,
    /// WEM base URL, as the earlier config wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wem_base_url: Option<String>,
    /// Every `[sources.<kit>]` table, by kit id.
    #[serde(flatten)]
    pub kits: std::collections::BTreeMap<String, toml::Table>,
}

/// Why a kit's table could not be read.
#[derive(Debug, thiserror::Error)]
pub enum SourcesError {
    /// `[sources.<kit>]` is not a table.
    #[error("sources.{kit} must be a table of the kit's settings")]
    NotATable {
        /// The kit.
        kit: String,
    },
    /// The table has a key the kit does not know, or a value of the wrong type.
    #[error("sources.{kit}: {source}")]
    Parse {
        /// The kit.
        kit: String,
        /// What toml said.
        #[source]
        source: toml::de::Error,
    },
}

impl Sources {
    /// The settings of kit `id`, when the file has its table.
    pub fn kit<T: serde::de::DeserializeOwned>(&self, id: &str) -> Result<Option<T>, SourcesError> {
        let Some(table) = self.kits.get(id) else {
            return Ok(None);
        };
        table
            .clone()
            .try_into()
            .map(Some)
            .map_err(|source| SourcesError::Parse {
                kit: id.to_owned(),
                source,
            })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            jobs: 0,
            downloads: 4,
            request_interval_ms: 250,
            max_bad_rows: 0,
            strict: false,
            user_agent_suffix: String::new(),
            lake: Lake::default(),
            sources: Sources::default(),
        }
    }
}

/// Why a config file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but could not be read.
    #[error("The config file {path} could not be read.")]
    Read {
        /// The file.
        path: String,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid for [`Config`].
    #[error("The config file {path} is not valid: {message}")]
    Parse {
        /// The file.
        path: String,
        /// TOML's message, which names the key and line.
        message: String,
        /// Line, when TOML reported one.
        line: Option<u64>,
    },
    /// A value is out of its range.
    #[error("The setting {key} is {value}; it must be {range}.")]
    Invalid {
        /// The key, e.g. `lake.page_kb`.
        key: String,
        /// The value given.
        value: String,
        /// What it may be.
        range: String,
    },
}

impl Diagnose for ConfigError {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Read { path, source } => {
                Diagnostic::error("config.read", Category::CONFIG, self.to_string())
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Parse { path, line, .. } => {
                let mut d = Diagnostic::error("config.parse", Category::CONFIG, self.to_string())
                    .with_file(path.clone());
                if let Some(l) = line {
                    d.line = Some(*l);
                }
                d
            }
            Self::Invalid { range, value, .. } => {
                Diagnostic::error("config.invalid", Category::CONFIG, self.to_string())
                    .expected_observed(range.clone(), value.clone())
            }
        }
    }
}

impl Config {
    /// Loads the file, or the defaults when it does not exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        let config = Self::parse(&text).map_err(|e| ConfigError::Parse {
            path: path.display().to_string(),
            message: e.message().to_owned(),
            line: e.span().map(|span| {
                u64::try_from(text[..span.start].matches('\n').count()).unwrap_or(0) + 1
            }),
        })?;
        config.lake.invalid().map_or(Ok(config), Err)
    }

    /// Parses TOML text.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// The effective number of parse workers.
    pub fn effective_jobs(&self) -> usize {
        if self.jobs > 0 {
            return self.jobs;
        }
        std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_parse() {
        let c = Config::parse("").unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.downloads, 4);
        assert_eq!(c.request_interval_ms, 250);
        let c =
            Config::parse("jobs = 3\n[sources]\nnem_base_url = \"http://localhost:1\"\n").unwrap();
        assert_eq!(c.jobs, 3);
        assert_eq!(c.effective_jobs(), 3);
        assert_eq!(
            c.sources.nem_base_url.as_deref(),
            Some("http://localhost:1")
        );
        assert!(Config::default().effective_jobs() >= 1);
        // A kit's table is read by the kit, with its own keys.
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fake {
            base_url: String,
        }
        let c = Config::parse("[sources.fake]\nbase_url = \"http://f\"\n").unwrap();
        assert_eq!(
            c.sources.kit::<Fake>("fake").unwrap(),
            Some(Fake {
                base_url: "http://f".into()
            })
        );
        assert_eq!(c.sources.kit::<Fake>("other").unwrap(), None);
        let c = Config::parse("[sources.fake]\nbase_url = 1\n").unwrap();
        let e = c.sources.kit::<Fake>("fake").unwrap_err();
        assert!(e.to_string().starts_with("sources.fake:"), "{e}");
        // Anything under [sources] that is not a kit's table is a parse error.
        assert!(Config::parse("[sources]\nfake = 1\n").is_err());
    }

    #[test]
    fn the_lake_layout_has_defaults_and_ranges() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.lake, Lake::default());
        assert_eq!(c.lake.file_size_mb, 128);
        assert!(c.lake.invalid().is_none());
        let c = Config::parse(
            "[lake]\nfile_size_mb = 256\ncompression = \"lz4\"\nstatistics = \"chunk\"\n",
        )
        .unwrap();
        assert_eq!(c.lake.file_size_mb, 256);
        assert_eq!(c.lake.compression, Codec::Lz4);
        assert_eq!(c.lake.statistics, Statistics::Chunk);
        assert!(Config::parse("[lake]\npage_kbs = 1\n").is_err());
        assert!(Config::parse("[lake]\ncompression = \"brotli\"\n").is_err());
        let names: Vec<&str> = [Codec::Zstd, Codec::Lz4, Codec::Snappy, Codec::None]
            .iter()
            .map(|c| c.name())
            .collect();
        assert_eq!(names, ["zstd", "lz4", "snappy", "none"]);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[lake]\npage_kb = 4\n").unwrap();
        let d = Config::load(&p).unwrap_err().diagnostic();
        assert_eq!(d.code, "config.invalid");
        assert!(d.message.contains("lake.page_kb"), "{}", d.message);
        assert!(d.message.contains("8 to 65536"), "{}", d.message);
    }

    #[test]
    fn unknown_key_is_an_error_with_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "downloads = 2\njobz = 1\n").unwrap();
        let e = Config::load(&p).unwrap_err();
        let d = e.diagnostic();
        assert_eq!(d.code, "config.parse");
        assert_eq!(d.line, Some(2));
        assert!(d.message.contains("jobz"), "{}", d.message);
        assert_eq!(d.file.as_deref(), Some(p.to_str().unwrap()));
    }

    #[test]
    fn missing_file_is_default_and_unreadable_is_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(&dir.path().join("none.toml")).unwrap(),
            Config::default()
        );
        // A directory where a file is expected is a read error, not a parse error.
        let e = Config::load(dir.path()).unwrap_err();
        assert_eq!(e.diagnostic().code, "config.read");
        assert!(!e.diagnostic().cause.is_empty());
    }
}
