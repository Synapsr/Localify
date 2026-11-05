pub mod cloud_account;
pub mod daemon_status;
pub mod install;
pub mod list_all_domains;
pub mod manage_routes;
pub mod ports;
pub mod provider_registry;
pub mod public_urls;
pub mod register_domain;
pub mod restart_daemon;
pub mod start_daemon;
pub mod stop_daemon;
#[cfg(test)]
pub mod testkit;
pub mod uninstall;
pub mod unregister_domain;

use std::fmt;

/// Outcome of a single step in a multi-step operation.
///
/// Used by application services to report partial success/failure
/// so the CLI layer can render feedback appropriately.
#[derive(Debug, Clone)]
pub enum StepOutcome {
    Success(String),
    Warning(String),
    Skipped(String),
    /// The work is done but one last step needs the user, such as confirming CA
    /// trust on a machine with a screen. Unlike a warning, nothing went wrong.
    Pending(String),
}

impl fmt::Display for StepOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success(msg) => write!(f, "{}", msg),
            Self::Warning(msg) => write!(f, "Warning: {}", msg),
            Self::Skipped(msg) => write!(f, "Skipped: {}", msg),
            Self::Pending(msg) => write!(f, "Pending: {}", msg),
        }
    }
}
