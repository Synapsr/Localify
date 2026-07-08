use std::path::PathBuf;

/// Result of attempting to trust the Root CA in booted iOS Simulators.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    #[cfg(target_os = "macos")]
    CurrentUserUnavailable,
    #[cfg(not(target_os = "macos"))]
    UnsupportedPlatform,
}

/// Port for system-level directory and file operations.
pub trait SystemSetup {
    /// Create required data, certs, and log directories.
    fn create_directories(&self) -> anyhow::Result<()>;

    /// Trust the Root CA in booted iOS Simulators when the platform supports it.
    fn trust_ios_simulators(&self) -> anyhow::Result<IosSimulatorTrust>;

    /// Remove the data directory. Returns true if it existed.
    fn remove_data_directory(&self) -> anyhow::Result<bool>;

    /// Remove the PID file. Returns true if it existed.
    fn remove_pid_file(&self) -> bool;

    /// Remove the log directory. Returns true if it existed.
    fn remove_log_directory(&self) -> bool;
}
