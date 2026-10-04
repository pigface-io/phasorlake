//! Assembling the dependencies a command needs from flags, environment and
//! the config file.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use nemlake_acquire::cache::DiskCache;
use nemlake_acquire::{Fetcher, FetcherConfig, OfflineTransport, ReqwestTransport, Transport};
use nemlake_core::config::Config;
use nemlake_core::paths::Home;
use nemlake_pipeline::Deps;
use nemlake_registry::{Registry, TypeOptions};
use nemlake_writer::{ManifestHandle, ManifestThread};

use crate::cli::Cli;

/// Everything a command may need.
pub struct Context {
    /// Home.
    pub home: Home,
    /// Config.
    pub config: Config,
    /// Registry.
    pub registry: Arc<Registry>,
    /// Whether to print JSON.
    pub json: bool,
    /// Whether to show progress.
    pub progress: bool,
    /// Offline.
    pub offline: bool,
}

impl Context {
    /// Resolves home, config and registry. Cheap: no network, no SQLite.
    pub fn new(cli: &Cli) -> Result<Self> {
        let user_home = std::env::var_os("HOME").map(PathBuf::from);
        let home = Home::resolve(cli.home.as_deref(), None, user_home.as_deref())
            .context("No home directory: pass --home or set NEMLAKE_HOME.")?;
        home.ensure_dirs().with_context(|| {
            format!(
                "The home directory {} could not be created.",
                home.root().display()
            )
        })?;
        let config_path = cli.config.clone().unwrap_or_else(|| home.config_file());
        let config = Config::load(&config_path)
            .map_err(|e| anyhow::anyhow!("{}", nemlake_core::Diagnose::diagnostic(&e).render()))?;
        let mut registry = Registry::load_embedded(TypeOptions {
            decimal: cli.decimal || config.decimal,
        });
        // The user's own tables, when there are any: a file named after a
        // built-in table replaces it, others are added.
        let overlay = home.schemas();
        let has_overlay = !cli.no_user_schemas
            && std::fs::read_dir(&overlay).is_ok_and(|mut d| d.next().is_some());
        if has_overlay {
            registry = Arc::new(registry.with_overlay(&overlay).map_err(|e| {
                anyhow::anyhow!(
                    "{} Fix or remove the file, or pass --no-user-schemas to run without your own tables.",
                    nemlake_core::Diagnose::diagnostic(&e).render()
                )
            })?);
        }
        let progress =
            !cli.no_progress && !cli.json && std::io::IsTerminal::is_terminal(&std::io::stderr());
        Ok(Self {
            home,
            config,
            registry,
            json: cli.json,
            progress,
            offline: cli.offline,
        })
    }

    /// The manifest, on its thread.
    pub fn manifest(&self) -> Result<(ManifestHandle, ManifestThread)> {
        ManifestHandle::spawn(&self.home.manifest())
            .map_err(|e| anyhow::anyhow!("{}", nemlake_core::Diagnose::diagnostic(&e).render()))
    }

    /// The archive of downloads and the cache of listings under the home.
    pub fn store(&self) -> DiskCache {
        DiskCache::at(self.home.cache(), self.home.archive())
    }

    /// A fetcher over the real network, or offline. `requested_by` is the
    /// command being run; it is recorded beside every file it downloads.
    pub fn fetcher(
        &self,
        requested_by: &str,
        refresh: bool,
        events: Option<tokio::sync::mpsc::Sender<nemlake_core::event::Event>>,
    ) -> Result<Arc<Fetcher>> {
        let ua = format!(
            "nemlake/{} (+https://github.com/g0itb4/nemlake){}",
            env!("CARGO_PKG_VERSION"),
            self.config.user_agent_suffix
        );
        let transport: Arc<dyn Transport> = if self.offline {
            Arc::new(OfflineTransport)
        } else {
            Arc::new(
                ReqwestTransport::new(&ua, Duration::from_secs(30), Duration::from_secs(120))
                    .map_err(|e| anyhow::anyhow!("{e}"))?,
            )
        };
        let cfg = FetcherConfig {
            refresh,
            min_interval: Duration::from_millis(self.config.request_interval_ms),
            requested_by: requested_by.to_owned(),
            user_agent: Some(ua),
            ..Default::default()
        };
        let mut f = Fetcher::new(transport, self.store(), cfg);
        if let Some(tx) = events {
            f = f.with_events(tx);
        }
        Ok(Arc::new(f))
    }

    /// Everything the pipeline needs.
    pub fn deps(&self, fetcher: Arc<Fetcher>, manifest: ManifestHandle) -> Deps {
        Deps {
            registry: self.registry.clone(),
            fetcher,
            manifest,
            home: self.home.clone(),
        }
    }
}
