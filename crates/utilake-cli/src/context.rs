//! Assembling the dependencies a command needs from flags, environment and
//! the config file.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use utilake_acquire::{
    Archive, Fetcher, FetcherConfig, OfflineTransport, ReqwestTransport, Transport,
};
use utilake_core::config::Config;
use utilake_core::paths::Home;
use utilake_pipeline::Deps;
use utilake_registry::Registry;
use utilake_writer::{ManifestHandle, ManifestThread};

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
        // `NEMLAKE_HOME` is the variable the tool read under its earlier name.
        let earlier = std::env::var_os("NEMLAKE_HOME").map(PathBuf::from);
        let flag = cli.home.as_deref().or(earlier.as_deref());
        let home = Home::resolve(flag, user_home.as_deref())
            .context("No home directory: pass --home or set UTILAKE_HOME.")?;
        home.ensure_dirs().with_context(|| {
            format!(
                "The home directory {} could not be created.",
                home.root().display()
            )
        })?;
        let config_path = cli.config.clone().unwrap_or_else(|| home.config_file());
        let config = Config::load(&config_path)
            .map_err(|e| anyhow::anyhow!("{}", utilake_core::Diagnose::diagnostic(&e).render()))?;
        let mut registry = Registry::load_embedded();
        // The user's own tables, when there are any: a file named after a
        // built-in table replaces it, others are added.
        let overlay = home.schemas();
        let has_overlay = !cli.no_user_schemas
            && std::fs::read_dir(&overlay).is_ok_and(|mut d| d.next().is_some());
        if has_overlay {
            registry = Arc::new(registry.with_overlay(&overlay).map_err(|e| {
                anyhow::anyhow!(
                    "{} Fix or remove the file, or pass --no-user-schemas to run without your own tables.",
                    utilake_core::Diagnose::diagnostic(&e).render()
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
            .map_err(|e| anyhow::anyhow!("{}", utilake_core::Diagnose::diagnostic(&e).render()))
    }

    /// The archive of downloads under the home.
    pub fn archive(&self) -> Archive {
        Archive::new(self.home.archive())
    }

    /// A fetcher over the real network, or offline, with requests the
    /// configured interval apart.
    pub fn fetcher(
        &self,
        events: Option<tokio::sync::mpsc::Sender<utilake_core::event::Event>>,
    ) -> Result<Arc<Fetcher>> {
        let ua = format!(
            "utilake/{} (+https://github.com/pigface-io/utilake){}",
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
            min_interval: Duration::from_millis(self.config.request_interval_ms),
            ..Default::default()
        };
        let mut f = Fetcher::new(transport, self.archive(), cfg);
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
