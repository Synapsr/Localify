use serde::Serialize;

use super::{DaemonConnectionError, UpgradeHint};

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

/// What Localify Cloud last told the daemon about this machine's tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeState {
    /// Nothing to serve.
    Idle,
    /// A tunnel is on its way.
    Provisioning,
    Ready,
    /// Something to serve, but no right to: see `TunnelStatus::blocked`.
    Blocked,
    /// The account is suspended.
    Banned,
}

/// Why this machine may not serve. `reason` is `subscription_required`,
/// `serving_slots_full` or `capacity`; `message` is for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBlock {
    pub reason: String,
    pub message: String,
    /// What lifts the block, when the account can buy it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<UpgradeHint>,
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
    /// `None` before the daemon's first answer, or from an older daemon.
    pub runtime: Option<RuntimeState>,
    pub blocked: Option<RuntimeBlock>,
}

/// Port for the daemon's side of public URLs: it runs the tunnel and serves
/// this machine's URLs.
pub trait PublicTunnel {
    fn status(&self) -> Result<TunnelStatus, DaemonConnectionError>;
    /// Makes the daemon fetch this machine's public URLs again, after a
    /// sign-in, a sign-out, a plan change or any change to a URL.
    fn refresh(&self) -> Result<(), DaemonConnectionError>;
}
