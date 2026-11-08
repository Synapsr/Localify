use anyhow::{Context, Result, bail};
use std::fs;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::{Duration, Instant};

use super::mgmt_client::{MgmtSocketClient, ShutdownKind};
use super::process::process_exists;
use crate::application::ports::{DaemonConnectionError, DaemonControl};
use crate::config::LocalifyPaths;

pub struct PidFile {
    path: PathBuf,
}

impl PidFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Write current process PID to file
    pub fn write(&self) -> Result<()> {
        let pid = process::id();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, pid.to_string()).context("Failed to write PID file")?;
        Ok(())
    }

    /// Read PID from file
    pub fn read(&self) -> Result<Option<u32>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&self.path).context("Failed to read PID file")?;
        let pid: u32 = content.trim().parse().context("Invalid PID in file")?;
        Ok(Some(pid))
    }

    /// Check if process with stored PID is running
    pub fn is_running(&self) -> Result<bool> {
        match self.read()? {
            Some(pid) => Ok(process_exists(pid)),
            None => Ok(false),
        }
    }

    /// Remove PID file
    pub fn remove(&self) -> Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path).context("Failed to remove PID file")?;
        }
        Ok(())
    }

    /// Get stored PID if daemon is running
    pub fn get_running_pid(&self) -> Result<Option<u32>> {
        match self.read()? {
            Some(pid) if process_exists(pid) => Ok(Some(pid)),
            _ => Ok(None),
        }
    }
}

/// The daemon as a process. Its PID file only says whether one seems to be running;
/// anything that acts on the daemon goes through its management socket, which only
/// the same user can open. A PID in a file the user can edit may by now belong to
/// any other process, so this never signals one.
pub struct DaemonHandle {
    pid_file: PidFile,
    mgmt: MgmtSocketClient,
}

impl DaemonHandle {
    pub fn new(paths: &LocalifyPaths) -> Self {
        Self {
            pid_file: PidFile::new(paths.pid_file.clone()),
            mgmt: MgmtSocketClient::new(&paths.socket_path),
        }
    }

    /// Asks the daemon to exit and waits until it has. A daemon that no longer
    /// answers on its socket has gone, so a leftover PID file is simply removed.
    pub fn stop_gracefully(&self, timeout: Duration) -> Result<()> {
        let Some(pid) = self.pid_file.get_running_pid()? else {
            return Ok(());
        };

        match self.mgmt.shutdown(ShutdownKind::Stop) {
            Ok(()) => {}
            Err(DaemonConnectionError::NotRunning) => return self.pid_file.remove(),
            Err(error) => bail!(
                "Could not ask the daemon (PID {pid}) to stop: {error}\n\
                 If a build older than this one is running, stop it with: sudo kill {pid}"
            ),
        }

        let deadline = Instant::now() + timeout;
        while process_exists(pid) {
            if Instant::now() >= deadline {
                bail!(
                    "The daemon (PID {pid}) did not stop within {} seconds.\n\
                     Check what it is doing, and stop it with: kill {pid}",
                    timeout.as_secs()
                );
            }
            thread::sleep(Duration::from_millis(50));
        }

        self.pid_file.remove()
    }
}

impl DaemonControl for DaemonHandle {
    fn is_running(&self) -> Result<bool> {
        self.pid_file.is_running()
    }

    fn get_running_pid(&self) -> Result<Option<u32>> {
        self.pid_file.get_running_pid()
    }

    fn stop_gracefully(&self, timeout: Duration) -> Result<()> {
        DaemonHandle::stop_gracefully(self, timeout)
    }
}
