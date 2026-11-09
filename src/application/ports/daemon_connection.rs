use crate::domain::DomainRegistration;

#[derive(Debug, Clone)]
#[allow(dead_code)] // The CLI target does not read daemon registration metadata directly.
pub struct DaemonRegistrationInfo {
    pub registration: DomainRegistration,
    pub last_accessed_at: Option<u64>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Used by trait impls; no production caller for status() yet.
pub struct DaemonRuntimeInfo {
    pub pid: u32,
    pub registrations: Vec<DomainRegistration>,
    pub http_port: u16,
    pub https_port: u16,
    pub dns_port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum DaemonConnectionError {
    #[error("Daemon is not running")]
    NotRunning,
    #[error("Connection failed: {0}")]
    ConnectionFailed(#[source] anyhow::Error),
    #[error("Protocol error: {0}")]
    ProtocolError(String),
    /// Another machine, or the way to it, refused: the contract's code and
    /// its message (docs/remote-machines.md, "Errors").
    #[error("{message}")]
    Remote { code: String, message: String },
}

/// Port for communicating with a running daemon process.
/// Complements DaemonControl (lifecycle) with runtime queries.
#[allow(dead_code)] // status()/reload() have impls but no production callers yet.
pub trait DaemonConnection {
    fn status(&self) -> Result<DaemonRuntimeInfo, DaemonConnectionError>;
    fn reload(&self) -> Result<(), DaemonConnectionError>;
    fn list_registrations(&self) -> Result<Vec<DomainRegistration>, DaemonConnectionError>;
    fn list_registration_info(&self) -> Result<Vec<DaemonRegistrationInfo>, DaemonConnectionError> {
        self.list_registrations().map(|registrations| {
            registrations
                .into_iter()
                .map(|registration| DaemonRegistrationInfo {
                    registration,
                    last_accessed_at: None,
                })
                .collect()
        })
    }
}
