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
    /// Set for a password; older versions of the API leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
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
    /// Left out by a cloud older than the paid plan.
    #[serde(default)]
    pub state: Option<PublicUrlState>,
    /// RFC 3339.
    pub created_at: String,
}

/// Whether a public URL carries traffic (`docs/public-urls.md`, "Plans").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicUrlState {
    Serving,
    /// Beyond the plan's limits, or without a plan.
    Suspended,
    /// Removed for abuse.
    TakenDown,
}

/// `GET /v1/machines/self/runtime`, which a cloud without `/v2` answers.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeV1 {
    pub tunnel_token: String,
    pub ingress_secret: String,
    pub public_urls: Vec<PublicUrl>,
}

/// Leaves both secrets out, so the runtime can appear in logs.
impl fmt::Debug for RuntimeV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeV1")
            .field("tunnel_token", &"<redacted>")
            .field("ingress_secret", &"<redacted>")
            .field("public_urls", &self.public_urls)
            .finish()
    }
}

/// `GET /v2/machines/self/runtime` (`docs/public-urls.md`, "Runtime").
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeV2 {
    pub state: RuntimeV2State,
    #[serde(default)]
    pub blocked: Option<RuntimeBlock>,
    #[serde(default)]
    pub tunnel: Option<RuntimeTunnel>,
    #[serde(default)]
    pub ingress_secret: Option<String>,
    #[serde(default)]
    pub public_urls: Vec<PublicUrl>,
    /// Seconds.
    #[serde(default)]
    pub retry_after: Option<u64>,
}

/// Leaves both secrets out, so the runtime can appear in logs.
impl fmt::Debug for RuntimeV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeV2")
            .field("state", &self.state)
            .field("blocked", &self.blocked)
            .field("tunnel", &self.tunnel)
            .field(
                "ingress_secret",
                &self.ingress_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("public_urls", &self.public_urls)
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeV2State {
    Idle,
    Provisioning,
    Ready,
    Blocked,
}

/// Why a machine with something to serve may not. `reason` is
/// `subscription_required`, `serving_slots_full` or `capacity`; `message` is
/// for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBlock {
    pub reason: String,
    pub message: String,
    /// What lifts the block, when the account can buy it. Left out by a
    /// cloud older than add-ons, and by a daemon older than this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<Upgrade>,
}

/// The `upgrade` of a refusal or a block: `kind` is `pro`, `addon_machine`
/// or `addon_urls`, and `command` the one a person runs, such as
/// `localify billing` (`docs/public-urls.md`, "API").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upgrade {
    pub kind: String,
    pub command: String,
}

/// The tunnel generation to serve with. The token comes only when the
/// generation differs from the one the daemon said it runs.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTunnel {
    pub generation: u64,
    #[serde(default)]
    pub token: Option<String>,
}

/// Leaves the token out.
impl fmt::Debug for RuntimeTunnel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeTunnel")
            .field("generation", &self.generation)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// One machine of `GET /v1/machines` (`docs/remote-machines.md`, "API").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterMachine {
    pub id: String,
    pub name: String,
    pub created_at: String,
    /// base64url; `None` until the machine uploads one.
    #[serde(default)]
    pub remote_key: Option<String>,
    #[serde(rename = "self", default)]
    pub is_self: bool,
}

/// `GET /v1/remote/lease`: a lifetime in seconds, counted by the client from
/// when it sent the request, and the account's active machine ids.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LeaseGrant {
    pub ttl: u64,
    pub machines: Vec<String>,
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
    /// `None` before the first answer, and from a daemon older than the
    /// runtime states.
    #[serde(default)]
    pub runtime: Option<RuntimeStatus>,
    /// Set when `runtime` is `Blocked`.
    #[serde(default)]
    pub blocked: Option<RuntimeBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelState {
    Stopped,
    Starting,
    Connected,
    Error,
}

/// What the daemon last learned from the runtime, for `public_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeStatus {
    Idle,
    Provisioning,
    Ready,
    Blocked,
    /// The API answered `403 banned`.
    Banned,
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
            "protection": {"mode": "password"},
            "createdAt": "2026-11-05T18:00:00Z"
        }"#;
        let url: PublicUrl = serde_json::from_str(json).unwrap();
        assert_eq!(url.protection.mode, ProtectionMode::Password);
        assert_eq!(url.protection.version, None);
        assert_eq!(url.state, None);
        assert_eq!(url.machine.unwrap().name, "runner");

        let versioned = json.replace(
            r#"{"mode": "password"}"#,
            r#"{"mode": "password", "version": 3}, "state": "taken_down""#,
        );
        let url: PublicUrl = serde_json::from_str(&versioned).unwrap();
        assert_eq!(url.protection.version, Some(3));
        assert_eq!(url.state, Some(PublicUrlState::TakenDown));
    }

    #[test]
    fn a_block_keeps_its_upgrade_through_the_daemon() {
        let block: RuntimeBlock = serde_json::from_str(
            r#"{"reason":"serving_slots_full","message":"Every slot is taken.",
                "upgrade":{"kind":"addon_machine","command":"localify billing"}}"#,
        )
        .unwrap();
        assert_eq!(block.upgrade.as_ref().unwrap().command, "localify billing");

        // `public_status` carries it to the CLI and the app, and leaves it
        // out when there is none, as an older daemon does.
        let again: RuntimeBlock =
            serde_json::from_str(&serde_json::to_string(&block).unwrap()).unwrap();
        assert_eq!(again, block);
        let older: RuntimeBlock =
            serde_json::from_str(r#"{"reason":"capacity","message":"Full."}"#).unwrap();
        assert_eq!(older.upgrade, None);
        assert!(!serde_json::to_string(&older).unwrap().contains("upgrade"));
    }

    #[test]
    fn runtime_debug_output_hides_the_secrets() {
        let runtime = RuntimeV1 {
            tunnel_token: "tunnel-secret".into(),
            ingress_secret: "ingress-secret".into(),
            public_urls: Vec::new(),
        };
        let output = format!("{runtime:?}");
        assert!(!output.contains("tunnel-secret") && !output.contains("ingress-secret"));

        let runtime: RuntimeV2 = serde_json::from_str(
            r#"{"state":"ready","blocked":null,"tunnel":{"generation":2,"token":"tunnel-secret"},
                "ingressSecret":"ingress-secret","publicUrls":[],"retryAfter":null}"#,
        )
        .unwrap();
        let output = format!("{runtime:?}");
        assert!(!output.contains("tunnel-secret") && !output.contains("ingress-secret"));
        assert!(output.contains("generation: 2"), "{output}");
    }

    #[test]
    fn a_status_from_an_older_daemon_reads_without_the_runtime() {
        let status: PublicStatus = serde_json::from_str(
            r#"{"signedIn":true,"tunnel":"connected","connections":4,"error":null,"servedUrls":["u1"]}"#,
        )
        .unwrap();
        assert_eq!((status.runtime, status.blocked), (None, None));
    }
}
