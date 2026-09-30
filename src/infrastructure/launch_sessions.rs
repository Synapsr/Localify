//! Running services that were launched with a launch profile.
//!
//! `localify dev` reports its session to the daemon so the proxy can add the
//! `X-Localify-Profile` header, the desktop app can show a badge, and the
//! daemon log keeps an audit trail. Sessions carry names and counts only,
//! never variable values.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How often a running `localify dev` renews its session.
#[allow(dead_code)] // Used by the CLI through the library crate.
pub const LAUNCH_SESSION_HEARTBEAT: Duration = Duration::from_secs(15);

/// How long the daemon keeps a session without news. Covers a `localify dev`
/// that was killed before it could report its stop.
pub const LAUNCH_SESSION_LEASE: Duration = Duration::from_secs(45);

/// HTTP response header the proxy adds for services launched with a profile.
pub const LAUNCH_PROFILE_HEADER: &str = "x-localify-profile";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSessionDto {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub environment_slug: Option<String>,
    pub service_id: String,
    pub profile: String,
    pub danger: bool,
    /// Loopback port the service listens on; the proxy matches on it.
    pub target_port: u16,
    /// Unix seconds.
    pub started_at: u64,
    /// Unix seconds, when the profile has a `max_duration`.
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// Source labels with key counts, for the audit log.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Show the launch profile in the browser (tab title and a pill).
    #[serde(default)]
    pub browser_indicator: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LaunchSessionUpdate {
    /// First report and every heartbeat.
    Running { session: LaunchSessionDto },
    Stopped {
        session: LaunchSessionDto,
        reason: String,
    },
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// RFC 3339 timestamp for logs, falling back to raw seconds.
pub fn format_unix_time(seconds: u64) -> String {
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| seconds.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_time_as_rfc3339() {
        assert_eq!(format_unix_time(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn update_round_trips_as_tagged_json() {
        let update = LaunchSessionUpdate::Stopped {
            session: LaunchSessionDto {
                id: "1".into(),
                project_id: "acme".into(),
                environment_slug: Some("main".into()),
                service_id: "web".into(),
                profile: "prod".into(),
                danger: true,
                target_port: 3000,
                started_at: 10,
                expires_at: None,
                sources: vec!["file .env (3 keys)".into()],
                browser_indicator: true,
            },
            reason: "exited".into(),
        };

        let json = serde_json::to_string(&update).unwrap();
        assert!(json.contains("\"state\":\"stopped\""));
        assert_eq!(
            serde_json::from_str::<LaunchSessionUpdate>(&json).unwrap(),
            update
        );
    }
}
