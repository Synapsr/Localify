//! The API's JSON types (`docs/public-urls.md`, section "API").

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUser {
    pub login: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudMachine {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionMode {
    Password,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Protection {
    pub mode: ProtectionMode,
    pub exceptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrl {
    pub id: String,
    pub slug: String,
    pub url: String,
    pub local_domain: String,
    pub wildcard: bool,
    /// `None` when no machine serves it.
    pub machine: Option<CloudMachine>,
    pub protection: Protection,
    /// RFC 3339.
    pub created_at: String,
}

/// What the daemon needs to serve this machine's public URLs.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineRuntime {
    pub tunnel_token: String,
    pub ingress_secret: String,
    pub public_urls: Vec<PublicUrl>,
}

/// Leaves both secrets out, so the runtime can appear in logs.
impl fmt::Debug for MachineRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineRuntime")
            .field("tunnel_token", &"<redacted>")
            .field("ingress_secret", &"<redacted>")
            .field("public_urls", &self.public_urls)
            .finish()
    }
}

/// The daemon's answer to the `public_status` management command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicStatus {
    pub signed_in: bool,
    pub tunnel: TunnelState,
    /// Connections `cloudflared` holds to Cloudflare, from its `/ready` endpoint.
    pub connections: u32,
    /// What went wrong, for a person, when `tunnel` is `Error`.
    pub error: Option<String>,
    /// Ids of the public URLs this machine serves.
    pub served_urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelState {
    Stopped,
    Starting,
    Connected,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_url_reads_the_api_shape() {
        let json = r#"{
            "id": "u1", "slug": "journal-a8f3", "url": "https://journal-a8f3.localify.fyi",
            "localDomain": "journal.localify", "wildcard": true,
            "machine": {"id": "m1", "name": "runner"},
            "protection": {"mode": "password", "exceptions": ["/api/webhooks"]},
            "createdAt": "2026-11-05T18:00:00Z"
        }"#;
        let url: PublicUrl = serde_json::from_str(json).unwrap();
        assert_eq!(url.protection.mode, ProtectionMode::Password);
        assert_eq!(url.machine.unwrap().name, "runner");
    }

    #[test]
    fn runtime_debug_output_hides_the_secrets() {
        let runtime = MachineRuntime {
            tunnel_token: "tunnel-secret".into(),
            ingress_secret: "ingress-secret".into(),
            public_urls: Vec::new(),
        };
        let output = format!("{runtime:?}");
        assert!(!output.contains("tunnel-secret") && !output.contains("ingress-secret"));
    }
}
