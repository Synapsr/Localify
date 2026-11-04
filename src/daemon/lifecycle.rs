use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::Server;
use super::server::PrivilegedResources;
use crate::application::provider_registry::ProviderRegistry;
use crate::branding::BRANDING;
use crate::infrastructure::certs::OnDemandIssuer;
use crate::infrastructure::config::ConfigStore;
use crate::infrastructure::config::watcher::ConfigFileProvider;
use crate::infrastructure::docker::DockerProvider;
use crate::infrastructure::paths::LocalifyPaths;
use crate::infrastructure::pid::PidFile;
use crate::infrastructure::privilege::{
    ProcessCredentials, UnprivilegedUser, adopt_user_environment, drop_privileges, running_as_root,
};
use crate::infrastructure::tracing::{TracingOutput, init_tracing};

/// Run the Localify daemon server.
///
/// This handles the full daemon lifecycle: tracing initialization, PID file
/// management, signal handling, and server execution.
///
/// Everything that needs root happens first, on this single thread: the log
/// file, the listeners, the leaf keys and the PID file. Then the process drops to
/// `user` for good, and only then starts the runtime and serves anything. When
/// the process is not root, `user` is `None` and there is nothing to drop.
pub fn run(
    verbose: bool,
    config_path: &Path,
    paths: &LocalifyPaths,
    user: Option<UnprivilegedUser>,
) -> Result<()> {
    // When running interactively (stdout is a TTY), log to stdout
    // When running as daemon (stdout is /dev/null), log to file
    let output = if std::io::stdout().is_terminal() {
        TracingOutput::Stdout
    } else {
        TracingOutput::File(paths.log_file.clone())
    };
    init_tracing(verbose, output);

    info!("Localify daemon started");

    run_daemon(config_path, paths, user.as_ref()).inspect_err(|e| {
        // A daemon started in the background has no terminal to show this on.
        error!(error = %format!("{e:#}"), "Localify daemon stopped with an error");
    })
}

fn run_daemon(
    config_path: &Path,
    paths: &LocalifyPaths,
    user: Option<&UnprivilegedUser>,
) -> Result<()> {
    let (config, resources) = start_privileged_phase(config_path, paths, user)?;

    println!("Starting Localify daemon...");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to start the async runtime")?;
    runtime.block_on(serve(config_path, paths, config, resources))
}

/// Takes what needs root, writes the PID file, then gives root up. On return the
/// process can no longer become root.
fn start_privileged_phase(
    config_path: &Path,
    paths: &LocalifyPaths,
    user: Option<&UnprivilegedUser>,
) -> Result<(crate::infrastructure::config::Config, PrivilegedResources)> {
    ensure_not_staying_root(running_as_root(), user.is_some())?;

    // Load config fresh from disk (this path is used by the forked
    // subprocess, so it must re-read from the config file)
    let config = ConfigStore::new(config_path.to_path_buf()).load()?;
    if running_as_root() {
        config
            .daemon
            .validate_root_ports()
            .map_err(anyhow::Error::msg)?;
        keep_config_readable(config_path)?;
    }

    let resources =
        PrivilegedResources::acquire(&config, paths, request_on_demand_issuer(config_path))?;

    let pid_file = PidFile::new(paths.pid_file.clone());
    pid_file.write()?;

    if let Some(user) = user {
        drop_privileges(&mut ProcessCredentials, user).inspect_err(|_| {
            let _ = pid_file.remove();
        })?;
        // SAFETY: the runtime has not started, so this process has one thread.
        unsafe { adopt_user_environment(user) };
        info!(uid = user.uid, gid = user.gid, "Dropped root privileges");
    }

    Ok((config, resources))
}

/// The last check before anything is opened: the daemon never serves as root.
fn ensure_not_staying_root(is_root: bool, has_user: bool) -> Result<()> {
    if is_root && !has_user {
        anyhow::bail!("Refusing to run the daemon as root with no user to drop to.");
    }
    Ok(())
}

/// The daemon re-reads the config as the user. It holds no secrets and the CLI
/// reads it too, but a restrictive umask during a `sudo` install could have made
/// it private. Only ever adds read access.
fn keep_config_readable(config_path: &Path) -> Result<()> {
    let add_read = |path: &Path, bits: u32| -> Result<()> {
        let mode = match std::fs::metadata(path) {
            Ok(metadata) => metadata.permissions().mode(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).context(format!("Failed to inspect {}", path.display())),
        };
        if mode & bits == bits {
            return Ok(());
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | bits))
            .with_context(|| format!("Failed to make {} readable", path.display()))
    };

    add_read(config_path, 0o044)?;
    match config_path.parent() {
        Some(parent) => add_read(parent, 0o055),
        None => Ok(()),
    }
}

/// Asks a separate process for the on-demand issuer, so the Root CA key never
/// enters this one: it goes on to run as the user and serve the network. Without
/// an issuer the daemon still serves the certificates on disk.
fn request_on_demand_issuer(config_path: &Path) -> Option<OnDemandIssuer> {
    let issuer = run_issuer_command(config_path);
    match issuer {
        Ok(issuer) => issuer,
        Err(e) => {
            warn!(error = %format!("{e:#}"), "TLS: on-demand certificates disabled");
            None
        }
    }
}

fn run_issuer_command(config_path: &Path) -> Result<Option<OnDemandIssuer>> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--config")
        .arg(config_path)
        .args(["helper", "issue-daemon-ca"])
        .stdin(Stdio::null());
    if let Some(profile) = BRANDING.active_profile() {
        command.env(BRANDING.profile_env(), profile);
    }

    let output = command
        .output()
        .context("Failed to run the issuer command")?;
    if !output.status.success() {
        anyhow::bail!(
            "The issuer command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("The issuer command returned invalid output")
}

async fn serve(
    config_path: &Path,
    paths: &LocalifyPaths,
    config: crate::infrastructure::config::Config,
    resources: PrivilegedResources,
) -> Result<()> {
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

    let server = Server::new(
        &config,
        config_path.to_path_buf(),
        paths,
        resources,
        reload_rx,
        reload_tx.clone(),
        registry,
    )?;
    let cancel = CancellationToken::new();

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

    // Spawn signal handler for graceful shutdown. After a drop the PID file is in
    // a root-only folder, so removal fails quietly and the helper or next start
    // cleans it up. A stale file is harmless: its PID is no longer running.
    let cancel_signal = cancel.clone();
    let pid_signal = PidFile::new(paths.pid_file.clone());
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        info!("Shutdown signal received");
        let _ = pid_signal.remove();
        cancel_signal.cancel();
    });

    let result = server.run(cancel).await;

    let _ = PidFile::new(paths.pid_file.clone()).remove();
    result
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

    #[test]
    fn root_without_a_user_never_starts_the_daemon() {
        assert!(ensure_not_staying_root(true, false).is_err());
        assert!(ensure_not_staying_root(true, true).is_ok());
        assert!(ensure_not_staying_root(false, false).is_ok());
    }
}
