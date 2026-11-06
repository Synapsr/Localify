use serde::Serialize;

use super::DaemonConnectionError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelState {
    Stopped,
    Starting,
    Connected,
    Error,
    /// The daemon is not running, or did not answer.
    Unknown,
}

/// How the tunnel that carries this machine's public traffic is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub state: TunnelState,
    /// Connections the tunnel holds to Cloudflare.
    pub connections: u32,
    /// What went wrong, for a person.
    pub error: Option<String>,
}

/// Port for the daemon's side of public URLs: it runs the tunnel and serves
/// this machine's URLs.
pub trait PublicTunnel {
    fn status(&self) -> Result<TunnelStatus, DaemonConnectionError>;
    /// Makes the daemon fetch this machine's public URLs again, after a
    /// sign-in, a sign-out or any change to a URL.
    fn refresh(&self) -> Result<(), DaemonConnectionError>;
}
