//! Assembling a command's dependencies from flags, environment and the config file.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use phasor_acquire::{
    Archive, Fetcher, FetcherConfig, OfflineTransport, ReqwestTransport, Transport,
};
use phasor_core::config::Config;
use phasor_core::paths::Home;
use phasor_kit::KitRegistry;
use phasor_pipeline::Deps;
use phasor_registry::Registry;
use phasor_writer::{ManifestHandle, ManifestThread};

use crate::cli::Cli;

/// Everything a command may need.
pub struct Context {
    /// Home.
    pub home: Home,
    /// Config.
    pub config: Config,
    /// Registry.
    pub registry: Arc<Registry>,
    /// Every kit this binary was assembled with.
    pub kits: Arc<KitRegistry>,
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
        let home = Home::resolve(cli.home.as_deref(), user_home.as_deref())
            .context("No home directory: pass --home or set PHASOR_HOME.")?;
        home.ensure_dirs().with_context(|| {
            format!(
                "The home directory {} could not be created.",
                home.root().display()
            )
        })?;
        let config_path = cli.config.clone().unwrap_or_else(|| home.config_file());
        let config = Config::load(&config_path)
            .map_err(|e| anyhow::anyhow!("{}", phasor_core::Diagnose::diagnostic(&e).render()))?;
        let mut registry = phasor_nem::registry();
        let overlay = home.schemas();
        let has_overlay = !cli.no_user_schemas
            && std::fs::read_dir(&overlay).is_ok_and(|mut d| d.next().is_some());
        if has_overlay {
            registry = Arc::new(registry.with_overlay(&overlay).map_err(|e| {
                anyhow::anyhow!(
                    "{} Fix or remove the file, or pass --no-user-schemas to run without your own tables.",
                    phasor_core::Diagnose::diagnostic(&e).render()
                )
            })?);
        }
        let progress =
            !cli.no_progress && !cli.json && std::io::IsTerminal::is_terminal(&std::io::stderr());
        let kits = crate::kits::assemble(&config, registry.clone())?;
        Ok(Self {
            home,
            config,
            registry,
            kits,
            json: cli.json,
            progress,
            offline: cli.offline,
        })
    }

    /// The manifest, on its thread.
    pub fn manifest(&self) -> Result<(ManifestHandle, ManifestThread)> {
        ManifestHandle::spawn(&self.home.manifest())
            .map_err(|e| anyhow::anyhow!("{}", phasor_core::Diagnose::diagnostic(&e).render()))
    }

    /// The archive of downloads under the home.
    pub fn archive(&self) -> Archive {
        Archive::new(self.home.archive())
    }

    /// A fetcher over the network, or offline, its requests the configured interval apart.
    pub fn fetcher(
        &self,
        events: Option<tokio::sync::mpsc::Sender<phasor_core::event::Event>>,
    ) -> Result<Arc<Fetcher>> {
        let ua = format!(
            "phasor/{} (+https://github.com/pigface-io/phasorlake){}",
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
            kits: self.kits.clone(),
            fetcher,
            manifest,
            home: self.home.clone(),
        }
    }
}
