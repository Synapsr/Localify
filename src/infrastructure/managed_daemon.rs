//! Starting, stopping and restarting the daemon when launchd or systemd owns it.
//!
//! A user cannot drive a system service with `launchctl` or `systemctl`, and both start
//! an on-demand service again on its next connection. So the service keeps running in
//! the manager's eyes and the user steers it from the outside:
//!
//! - **stop**: write a marker file, then ask the daemon to exit cleanly. While
//!   the marker exists, each connection wakes the service, it closes what is waiting,
//!   and exits. The ports stay with the manager, and nothing else keeps running.
//! - **start**: remove the marker and open a connection, which wakes the service.
//! - **restart**: ask the daemon to exit with the restart code. The manager brings it
//!   back from the binary at its path, within its 1 second delay.
// Both crates compile this module: the CLI and the desktop API each use it.
#![allow(dead_code)]

use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::application::ports::DaemonConnectionError;
use crate::branding::BRANDING;
use crate::config::{DaemonConfig, LocalifyPaths};
use crate::infrastructure::mgmt_client::{MgmtSocketClient, ShutdownKind};
use crate::infrastructure::system_service::service_runs_config;

/// The manager may hold a restart back by its 1 second delay. This also covers a
/// slow disk at boot and the daemon's own startup.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const WAKE_INTERVAL: Duration = Duration::from_millis(500);

pub struct ManagedDaemon {
    mgmt: MgmtSocketClient,
    stop_marker: std::path::PathBuf,
    wake_address: SocketAddr,
    log_file: std::path::PathBuf,
}

impl ManagedDaemon {
    /// The service-managed daemon for this config, if there is one. Everything else,
    /// such as the dev profile, a custom `--config` or root on Linux, starts as a plain
    /// process.
    pub fn find(config_path: &Path, paths: &LocalifyPaths, daemon: &DaemonConfig) -> Option<Self> {
        service_runs_config(config_path).then(|| Self::new(paths, daemon))
    }

    fn new(paths: &LocalifyPaths, daemon: &DaemonConfig) -> Self {
        Self {
            mgmt: MgmtSocketClient::new(&paths.socket_path),
            stop_marker: paths.stop_marker(),
            wake_address: SocketAddr::from(([127, 0, 0, 1], daemon.http_port)),
            log_file: paths.log_file.clone(),
        }
    }

    /// Lets the job run and returns its PID. Safe to call when it already runs.
    pub fn start(&self) -> Result<u32> {
        self.clear_stop_marker()?;
        self.wait_for_new_daemon(None)
    }

    /// Stops the daemon until `start` or `restart`, including across a reboot.
    pub fn stop(&self) -> Result<()> {
        if let Some(parent) = self.stop_marker.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::write(&self.stop_marker, "").with_context(|| {
            format!(
                "Failed to write the stop marker at {}",
                self.stop_marker.display()
            )
        })?;

        let pid = match self.mgmt.running_pid() {
            Ok(pid) => pid,
            Err(DaemonConnectionError::NotRunning) => return Ok(()),
            Err(error) => return Err(anyhow!("Could not reach the daemon: {error}")),
        };
        self.mgmt
            .shutdown(ShutdownKind::Stop)
            .map_err(|error| anyhow!("Could not ask the daemon (PID {pid}) to stop: {error}"))?;
        self.wait_until_gone(pid)
    }

    /// Runs the daemon on the binary now at its path. Needs no approval.
    pub fn restart(&self) -> Result<u32> {
        self.clear_stop_marker()?;
        let previous = self.mgmt.running_pid().ok();
        if previous.is_some() {
            self.mgmt
                .shutdown(ShutdownKind::Restart)
                .map_err(|error| anyhow!("Could not ask the daemon to restart: {error}"))?;
        }
        self.wait_for_new_daemon(previous)
    }

    fn clear_stop_marker(&self) -> Result<()> {
        match fs::remove_file(&self.stop_marker) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "Failed to remove the stop marker at {}",
                    self.stop_marker.display()
                )
            }),
        }
    }

    /// Waits for a daemon other than `previous` to answer, knocking on its port so
    /// the manager wakes a service that is waiting for a connection.
    fn wait_for_new_daemon(&self, previous: Option<u32>) -> Result<u32> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut next_wake = Instant::now();

        loop {
            if let Ok(pid) = self.mgmt.running_pid()
                && previous != Some(pid)
            {
                return Ok(pid);
            }
            if Instant::now() >= deadline {
                bail!(not_ready_message(&self.log_file));
            }
            if Instant::now() >= next_wake {
                let _ = TcpStream::connect_timeout(&self.wake_address, WAKE_INTERVAL);
                next_wake = Instant::now() + WAKE_INTERVAL;
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn wait_until_gone(&self, pid: u32) -> Result<()> {
        let deadline = Instant::now() + STOP_TIMEOUT;
        while matches!(self.mgmt.running_pid(), Ok(running) if running == pid) {
            if Instant::now() >= deadline {
                bail!(
                    "The daemon (PID {pid}) did not stop within {} seconds.",
                    STOP_TIMEOUT.as_secs()
                );
            }
            thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }
}

fn not_ready_message(log_file: &Path) -> String {
    let platform_hint = if cfg!(target_os = "macos") {
        format!(
            "If macOS shows {} as turned off under System Settings > General > Login Items & Extensions, turn it on.\n",
            BRANDING.app_name
        )
    } else {
        format!(
            "Check the service with: systemctl status {}\n",
            BRANDING.slug()
        )
    };
    format!(
        "{0} did not answer within {1} seconds.\n\
         Check the log at {2}.\n\
         {platform_hint}\
         To reinstall the service, run: {3}",
        BRANDING.app_name,
        READY_TIMEOUT.as_secs(),
        log_file.display(),
        BRANDING.admin_command("install"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(dir: &Path) -> LocalifyPaths {
        LocalifyPaths {
            data_dir: dir.join("data"),
            pid_file: dir.join("localify.pid"),
            log_file: dir.join("logs/localify.log"),
            certs_dir: dir.join("data/certs"),
            socket_path: dir.join("s.sock"),
        }
    }

    #[test]
    fn stopping_with_no_daemon_running_leaves_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let daemon = ManagedDaemon::new(&paths, &DaemonConfig::default());

        daemon.stop().unwrap();

        assert!(paths.stop_marker().exists());
    }

    #[test]
    fn starting_clears_the_marker_even_when_the_service_does_not_answer() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        fs::write(paths.stop_marker(), "").unwrap();

        ManagedDaemon::new(&paths, &DaemonConfig::default())
            .clear_stop_marker()
            .unwrap();

        assert!(!paths.stop_marker().exists());
    }

    #[test]
    fn the_failure_message_points_at_the_log_and_the_background_setting() {
        let message = not_ready_message(Path::new("/tmp/localify.log"));

        assert!(message.contains("/tmp/localify.log"));
        assert!(message.contains(if cfg!(target_os = "macos") {
            "Login Items"
        } else {
            "systemctl status"
        }));
    }
}
