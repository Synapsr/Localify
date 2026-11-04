use crate::application::StepOutcome;
use std::path::PathBuf;

/// Result of attempting to trust the Root CA in booted iOS Simulators.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum IosSimulatorTrust {
    Trusted {
        device_names: Vec<String>,
    },
    NoBootedDevices,
    CertificateMissing {
        path: PathBuf,
    },
    ToolUnavailable {
        reason: String,
    },
    #[cfg(not(target_os = "macos"))]
    UnsupportedPlatform,
}

/// Port for system-level directory and file operations.
pub trait SystemSetup {
    /// Create required data, certs, and log directories.
    fn create_directories(&self) -> anyhow::Result<()>;

    /// Applies what only an administrator can change, asking for at most one
    /// approval, and reports each result. Where the process already has the rights
    /// there is nothing to report here.
    fn apply_system_changes(&self) -> anyhow::Result<Vec<(String, StepOutcome)>>;

    /// Takes those changes back out, asking for at most one approval.
    fn remove_system_changes(&self) -> anyhow::Result<Vec<(String, StepOutcome)>>;

    /// The command that finishes CA trust by hand, where trusting it can wait.
    fn ca_trust_command(&self) -> Option<String>;

    /// Trust the Root CA in booted iOS Simulators when the platform supports it.
    fn trust_ios_simulators(&self) -> anyhow::Result<IosSimulatorTrust>;

    /// Remove the data directory. Returns true if it existed.
    fn remove_data_directory(&self) -> anyhow::Result<bool>;

    /// Remove the PID file. Returns true if it existed.
    fn remove_pid_file(&self) -> bool;

    /// Remove the log directory. Returns true if it existed.
    fn remove_log_directory(&self) -> bool;
}
