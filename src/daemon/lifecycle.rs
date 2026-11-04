use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::Server;
use super::listeners::DaemonListeners;
use crate::application::provider_registry::ProviderRegistry;
use crate::infrastructure::binary_identity::BinaryIdentity;
use crate::infrastructure::config::ConfigStore;
use crate::infrastructure::config::watcher::ConfigFileProvider;
use crate::infrastructure::docker::DockerProvider;
use crate::infrastructure::macos_system::RESTART_EXIT_CODE;
use crate::infrastructure::paths::LocalifyPaths;
use crate::infrastructure::pid::PidFile;
use crate::infrastructure::tracing::{TracingOutput, init_tracing};

/// How long open connections get to finish once the daemon is shutting down.
/// A browser's keep-alive connection would otherwise hold it up indefinitely.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// launchd restarts a failed job within a second. A daemon that cannot start,
/// over a config it cannot read for example, should not spin that fast.
const FAILED_START_DELAY: Duration = Duration::from_secs(5);
/// How often the daemon checks whether an update replaced its binary.
const BINARY_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// Stops the daemon from the inside, and remembers whether a restart was asked for.
#[derive(Clone, Default)]
pub struct ShutdownHandle {
    cancel: CancellationToken,
    restart: Arc<AtomicBool>,
}

impl ShutdownHandle {
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Exits with the code launchd restarts the job on, from the binary at its path.
    pub fn restart(&self) {
        self.restart.store(true, Ordering::SeqCst);
        self.cancel.cancel();
    }

    pub(super) fn exit_code(&self) -> i32 {
        if self.restart.load(Ordering::SeqCst) {
            RESTART_EXIT_CODE
        } else {
            0
        }
    }
}

/// Run the Localify daemon server.
///
/// `service_listeners` are the sockets launchd handed over, when this process is
/// its job. Otherwise the daemon binds its own, which is what a foreground or dev
/// daemon does. launchd restarts a job on its next connection, so when the user
/// has stopped the daemon the job only clears what is waiting, then exits.
pub fn run(
    verbose: bool,
    config_path: &Path,
    paths: &LocalifyPaths,
    service_listeners: Option<DaemonListeners>,
) -> Result<()> {
    let managed = service_listeners.is_some();

    if let Some(listeners) = &service_listeners
        && paths.stop_marker().exists()
    {
        listeners.close_waiting_connections();
        return Ok(());
    }

    let shutdown = ShutdownHandle::default();
    let result = serve(
        verbose,
        config_path,
        paths,
        service_listeners,
        shutdown.clone(),
    );

    if let Err(error) = &result {
        // Under launchd nobody reads stderr, so the log has to say why.
        tracing::error!(error = %format!("{error:#}"), "The daemon stopped");
        if managed {
            std::thread::sleep(FAILED_START_DELAY);
        }
    }
    result?;

    match shutdown.exit_code() {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

#[tokio::main]
async fn serve(
    verbose: bool,
    config_path: &Path,
    paths: &LocalifyPaths,
    service_listeners: Option<DaemonListeners>,
    shutdown: ShutdownHandle,
) -> Result<()> {
    // When running interactively (stdout is a TTY), log to stdout
    // When running as daemon (stdout is /dev/null), log to file
    let output = if std::io::stdout().is_terminal() {
        TracingOutput::Stdout
    } else {
        TracingOutput::File(paths.log_file.clone())
    };
    let config_level = ConfigStore::new(config_path.to_path_buf())
        .load()
        .map(|config| config.daemon.log_level)
        .unwrap_or_else(|_| "info".to_string());
    init_tracing(verbose, &config_level, output);

    info!("Localify daemon started");

    let pid_file = PidFile::new(paths.pid_file.clone());
    pid_file.write()?;

    println!("Starting Localify daemon...");

    // Load config fresh from disk (this path is used by the forked
    // subprocess, so it must re-read from the config file)
    let config_store = ConfigStore::new(config_path.to_path_buf());
    let config = config_store.load()?;

    // Create reload channel for hot-reloading registrations (nudge-based)
    let (reload_tx, reload_rx) = mpsc::channel::<()>(4);

    // Create the config file provider
    let config_provider = Arc::new(ConfigFileProvider::new(config_path.to_path_buf()));

    // Build the provider registry (aggregates all registration sources)
    let mut registry = ProviderRegistry::new();
    registry.add(config_provider.clone());

    // Optionally enable Docker auto-discovery
    let docker_provider = if config.docker.enabled {
        match bollard::Docker::connect_with_local_defaults() {
            Ok(docker) => {
                info!("Docker integration enabled");
                let provider = Arc::new(DockerProvider::new(docker));
                registry.add(provider.clone());
                Some(provider)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Docker integration enabled but could not connect to Docker socket. \
                     Continuing without Docker support."
                );
                None
            }
        }
    } else {
        None
    };

    let registry = Arc::new(registry);

    let managed = service_listeners.is_some();
    let listeners = match service_listeners {
        Some(listeners) => {
            listeners.ensure_ipv4()?;
            listeners
        }
        None => DaemonListeners::bind(&config.daemon)?,
    };

    let server = Server::new(
        &config,
        config_path.to_path_buf(),
        paths,
        reload_rx,
        reload_tx.clone(),
        registry,
        listeners,
    )?;
    let cancel = shutdown.token();

    if managed {
        tokio::spawn(restart_when_binary_is_replaced(shutdown.clone()));
    }

    // Open connections get a moment to finish, then the process exits regardless.
    let cancel_deadline = cancel.clone();
    let shutdown_deadline = shutdown.clone();
    let pid_deadline = PidFile::new(paths.pid_file.clone());
    tokio::spawn(async move {
        cancel_deadline.cancelled().await;
        tokio::time::sleep(SHUTDOWN_GRACE).await;
        warn!("Connections still open after the shutdown delay, exiting anyway");
        let _ = pid_deadline.remove();
        std::process::exit(shutdown_deadline.exit_code());
    });

    // Spawn config file watcher for hot-reload
    let cancel_watcher = cancel.clone();
    let config_reload_tx = reload_tx.clone();
    tokio::spawn(async move {
        config_provider
            .watch(config_reload_tx, cancel_watcher)
            .await;
    });

    // Spawn Docker watcher if enabled
    if let Some(provider) = docker_provider {
        let cancel_docker = cancel.clone();
        let docker_reload_tx = reload_tx;
        tokio::spawn(async move {
            crate::infrastructure::docker::watcher::watch(
                provider.docker().clone(),
                provider.state(),
                docker_reload_tx,
                cancel_docker,
            )
            .await;
        });
    }

    // Spawn signal handler for graceful shutdown
    let cancel_signal = cancel.clone();
    let pid_signal = PidFile::new(paths.pid_file.clone());
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        info!("Shutdown signal received");
        let _ = pid_signal.remove();
        cancel_signal.cancel();
    });

    let result = server.run(shutdown).await;

    let _ = pid_file.remove();
    result
}

/// An update replaces the binary while this process keeps running the old code.
/// launchd starts the new one as soon as this exits, so no approval is needed, and
/// it works however the update arrived: the app's updater, a DMG, Homebrew, cargo.
/// The file must look the same on two checks, so a copy in progress is left alone.
async fn restart_when_binary_is_replaced(shutdown: ShutdownHandle) {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    watch_binary(&executable, BINARY_CHECK_INTERVAL, shutdown).await;
}

async fn watch_binary(executable: &Path, interval: Duration, shutdown: ShutdownHandle) {
    let Some(running) = BinaryIdentity::of(executable) else {
        return;
    };
    let mut previous = running.clone();

    loop {
        tokio::time::sleep(interval).await;
        // Missing during an update: wait for the new file.
        let Some(current) = BinaryIdentity::of(executable) else {
            continue;
        };
        if current != running && current == previous {
            info!("The binary was replaced, restarting on the new build");
            shutdown.restart();
            return;
        }
        previous = current;
    }
}

/// Wait for SIGINT (Ctrl+C) or SIGTERM.
async fn wait_for_shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = ctrl_c => {},
                    _ = sigterm.recv() => {},
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to register SIGTERM handler, using Ctrl+C only");
                ctrl_c.await.ok();
            }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replacing_the_binary_asks_for_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("localify");
        std::fs::write(&binary, b"old build").unwrap();
        let shutdown = ShutdownHandle::default();
        let watcher = tokio::spawn({
            let binary = binary.clone();
            let shutdown = shutdown.clone();
            async move { watch_binary(&binary, Duration::from_millis(20), shutdown).await }
        });

        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!shutdown.token().is_cancelled());

        let staged = dir.path().join("localify.new");
        std::fs::write(&staged, b"new build, longer").unwrap();
        std::fs::rename(&staged, &binary).unwrap();

        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(shutdown.exit_code(), RESTART_EXIT_CODE);
    }
}
