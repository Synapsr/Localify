// Both crates compile this module: the desktop API drives the macOS system changes.
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use crate::application::StepOutcome;
use crate::application::ports::{IosSimulatorTrust, SystemSetup};
use crate::branding::BRANDING;
use crate::infrastructure::ios_simulator_trust::trust_booted_ios_simulators;
use crate::infrastructure::macos_system::{
    self, Approval, SystemRemoval, SystemRequest, run_with_approval,
};
use crate::infrastructure::paths::LocalifyPaths;

/// Infrastructure service for filesystem operations required by Localify.
pub struct FileSystemSetup<'a> {
    paths: &'a LocalifyPaths,
    install: Option<(SystemRequest<'a>, Approval)>,
    removal: Option<(SystemRemoval, Approval)>,
    state_dir: Option<PathBuf>,
}

impl<'a> FileSystemSetup<'a> {
    pub fn new(paths: &'a LocalifyPaths) -> Self {
        Self {
            paths,
            install: None,
            removal: None,
            state_dir: None,
        }
    }

    /// Lets install change what needs an administrator, after one approval.
    pub fn with_system_install(mut self, request: SystemRequest<'a>, approval: Approval) -> Self {
        self.install = Some((request, approval));
        self
    }

    /// Lets uninstall take those changes back out, and remove the user's folder of
    /// Localify state, which on macOS holds the config too.
    pub fn with_system_removal(
        mut self,
        removal: SystemRemoval,
        approval: Approval,
        state_dir: Option<PathBuf>,
    ) -> Self {
        self.removal = Some((removal, approval));
        self.state_dir = state_dir;
        self
    }

    fn ca_cert(&self) -> PathBuf {
        self.paths.data_dir.join("ca.crt")
    }
}

impl SystemSetup for FileSystemSetup<'_> {
    fn create_directories(&self) -> anyhow::Result<()> {
        fs::create_dir_all(&self.paths.data_dir).map_err(|e| {
            anyhow::anyhow!(
                "Failed to create data directory {}: {}",
                self.paths.data_dir.display(),
                e
            )
        })?;

        fs::create_dir_all(&self.paths.certs_dir).map_err(|e| {
            anyhow::anyhow!(
                "Failed to create certs directory {}: {}",
                self.paths.certs_dir.display(),
                e
            )
        })?;

        if let Some(log_dir) = self.paths.log_file.parent() {
            fs::create_dir_all(log_dir).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create log directory {}: {}",
                    log_dir.display(),
                    e
                )
            })?;
        }

        Ok(())
    }

    fn apply_system_changes(&self) -> anyhow::Result<Vec<(String, StepOutcome)>> {
        let Some((request, approval)) = &self.install else {
            return Ok(Vec::new());
        };

        let changes = request.changes()?;
        let label = "System setup".to_string();

        if changes.is_empty() {
            return Ok(vec![(
                label,
                StepOutcome::Skipped("Already in place.".into()),
            )]);
        }

        run_with_approval(&changes.script(), *approval)?;
        Ok(vec![(
            label,
            StepOutcome::Success("Applied with one administrator approval.".into()),
        )])
    }

    fn remove_system_changes(&self) -> anyhow::Result<Vec<(String, StepOutcome)>> {
        let Some((removal, approval)) = &self.removal else {
            return Ok(Vec::new());
        };

        run_with_approval(&removal.script(), *approval)?;

        let mut steps = vec![(
            "Remove system setup".to_string(),
            StepOutcome::Success("Removed with one administrator approval.".into()),
        )];
        if let Some(step) = macos_system::remove_login_keychain_trust(&self.ca_cert()) {
            steps.push(step);
        }
        Ok(steps)
    }

    fn ca_trust_command(&self) -> Option<String> {
        cfg!(target_os = "macos").then(|| BRANDING.admin_command("trust"))
    }

    fn trust_ios_simulators(&self) -> anyhow::Result<IosSimulatorTrust> {
        trust_booted_ios_simulators(&self.ca_cert())
    }

    fn remove_data_directory(&self) -> anyhow::Result<bool> {
        if let Some(state_dir) = &self.state_dir
            && state_dir.exists()
        {
            // Holds the data directory, the config and the logs.
            fs::remove_dir_all(state_dir)?;
            return Ok(true);
        }

        if self.paths.data_dir.exists() {
            fs::remove_dir_all(&self.paths.data_dir)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn remove_pid_file(&self) -> bool {
        fs::remove_file(&self.paths.pid_file).is_ok()
    }

    fn remove_log_directory(&self) -> bool {
        self.paths
            .log_file
            .parent()
            .is_some_and(|log_dir| fs::remove_dir_all(log_dir).is_ok())
    }
}
