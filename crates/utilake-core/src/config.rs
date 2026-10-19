//! `~/.utilake/config.toml`. Unknown keys are errors so a typo never silently
//! falls back to a default.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::diag::{Category, Diagnose, Diagnostic};

/// Settings a user may persist. Every field has a default; flags and
/// `UTILAKE_*` environment variables override the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Parse workers; 0 means half the available cores.
    pub jobs: usize,
    /// Concurrent downloads.
    pub downloads: usize,
    /// Minimum gap between two requests to a server, in milliseconds.
    /// AEMO's web server blocks clients that ask too fast.
    pub request_interval_ms: u64,
    /// Bad rows tolerated per source before it fails.
    pub max_bad_rows: u64,
    /// Fail on every warning.
    pub strict: bool,
    /// Appended to the `User-Agent` header, e.g. a contact address.
    pub user_agent_suffix: String,
    /// Per-source settings.
    pub sources: Sources,
}

/// Base URLs, overridable for mirrors and tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Sources {
    /// NEM archive and reports.
    pub nem_base_url: String,
    /// WEM public data.
    pub wem_base_url: String,
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
            sources: Sources::default(),
        }
    }
}

impl Default for Sources {
    fn default() -> Self {
        Self {
            nem_base_url: "https://www.nemweb.com.au".to_owned(),
            wem_base_url: "https://data.wa.aemo.com.au".to_owned(),
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
}

impl Diagnose for ConfigError {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Read { path, source } => {
                Diagnostic::error("config.read", Category::Config, self.to_string())
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Parse { path, line, .. } => {
                let mut d = Diagnostic::error("config.parse", Category::Config, self.to_string())
                    .with_file(path.clone());
                if let Some(l) = line {
                    d.line = Some(*l);
                }
                d
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
        Self::parse(&text).map_err(|e| ConfigError::Parse {
            path: path.display().to_string(),
            message: e.message().to_owned(),
            line: e.span().map(|span| {
                u64::try_from(text[..span.start].matches('\n').count()).unwrap_or(0) + 1
            }),
        })
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
        assert_eq!(c.sources.nem_base_url, "http://localhost:1");
        assert!(Config::default().effective_jobs() >= 1);
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
