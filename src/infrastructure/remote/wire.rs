//! The JSON on the relay socket and inside Noise sessions
//! (`docs/remote-machines.md`, "Relay frames" and "Session messages").
//! Field names are camelCase, and unknown fields are ignored.

use serde::{Deserialize, Serialize};

/// The session protocol versions this build speaks, in `hello` and the
/// prologue.
pub const PROTO_MIN: u32 = 1;
pub const PROTO_MAX: u32 = 1;
/// A relay frame, as text, never exceeds this.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// A session message, before encryption, never exceeds this.
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Pair,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameKind {
    Hs,
    Msg,
    Close,
}

/// The one frame a daemon sends besides `ping`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename = "send")]
pub struct Send {
    pub to: String,
    pub sid: String,
    pub kind: FrameKind,
    /// On the first `hs` frame of a session only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<Purpose>,
    /// base64url Noise message; absent for `close`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtoRange {
    pub min: u32,
    pub max: u32,
}

impl ProtoRange {
    pub const OURS: Self = Self {
        min: PROTO_MIN,
        max: PROTO_MAX,
    };

    /// Whether two ranges share a version.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.min <= other.max && other.min <= self.max
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceState {
    Online,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub machine_id: String,
    pub state: PresenceState,
    pub version: String,
    pub proto: ProtoRange,
    /// RFC 3339.
    pub since: String,
}

/// What the relay sends a daemon, besides `pong`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum RelayFrame {
    Welcome {
        v: u32,
        conn_id: String,
        machine_id: String,
        peers: Vec<Presence>,
    },
    Presence(Presence),
    Recv {
        from: String,
        sid: String,
        kind: FrameKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<Purpose>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    Revoked {
        machine_id: String,
    },
    Error {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sid: Option<String>,
        message: String,
        /// Seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<f64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSettings {
    pub share: bool,
    pub control: bool,
    pub background_starts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppPresence {
    Running,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub proto: ProtoRange,
    pub version: String,
    /// RFC 3339, the sender's clock.
    pub now: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<TargetSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppPresence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByeReason {
    Done,
    Unpaired,
    Lifetime,
    Idle,
    NotShared,
    LeaseExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclineReason {
    NobodyToConfirm,
    Busy,
    Cancelled,
}

/// The longest request id a viewer may pick.
pub const MAX_ID: usize = 32;
/// The most an `output.read` may ask for.
pub const MAX_READ_BYTES: u32 = 16 * 1024;
/// The largest window an `output.follow` may ask for.
pub const MAX_WINDOW: u32 = 128 * 1024;

/// A revision of a target's inventory.
pub use crate::application::inventory::Have;

/// On every inventory page: who the target is and what it allows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineSummary {
    pub machine_id: String,
    pub name: String,
    pub version: String,
    /// `macos` or `linux`.
    pub os: String,
    pub settings: TargetSettings,
    pub app: AppPresence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputLine {
    pub text: String,
    /// The line goes on in the next item.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rotated {
    pub first_offset: u64,
}

/// A run's lines, from `offset` to `next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputData {
    pub re: String,
    pub run_id: String,
    pub offset: u64,
    pub next: u64,
    pub lines: Vec<OutputLine>,
    /// The asked offset was dropped; data starts there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotated: Option<Rotated>,
    /// The run ended and everything kept was sent.
    pub ended: bool,
    /// With `ended`: capture stopped early.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

/// The session messages this build handles. Controls (`control`,
/// `control.status`) come with a later version of the daemon and are
/// answered `unsupported` (see [`is_later_message`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all_fields = "camelCase")]
pub enum SessionMessage {
    #[serde(rename = "hello")]
    Hello(Hello),
    #[serde(rename = "keep")]
    Keep { ids: Vec<String> },
    #[serde(rename = "kept")]
    Kept,
    #[serde(rename = "error")]
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        re: Option<String>,
        code: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
    #[serde(rename = "bye")]
    Bye { reason: ByeReason },
    #[serde(rename = "inventory.get")]
    InventoryGet {
        id: String,
        #[serde(default)]
        have: Option<Have>,
    },
    #[serde(rename = "inventory.watch")]
    InventoryWatch {
        id: String,
        #[serde(default)]
        have: Option<Have>,
    },
    #[serde(rename = "inventory.unwatch")]
    InventoryUnwatch { id: String },
    #[serde(rename = "inventory.page")]
    InventoryPage {
        re: String,
        epoch: String,
        rev: u64,
        page: u32,
        pages: u32,
        machine: MachineSummary,
        /// `InventoryItem`s, handed to the app as they came.
        items: Vec<serde_json::Value>,
        truncated: bool,
    },
    #[serde(rename = "inventory.unchanged")]
    InventoryUnchanged { re: String, epoch: String, rev: u64 },
    #[serde(rename = "output.read")]
    OutputRead {
        id: String,
        run_id: String,
        offset: u64,
        max_bytes: u32,
    },
    #[serde(rename = "output.follow")]
    OutputFollow {
        id: String,
        run_id: String,
        offset: u64,
        window: u32,
        /// An optional field this build adds: start at most this many bytes
        /// before the end, instead of at `offset`, so a viewer opening a
        /// long run reads its tail and not all of it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail: Option<u64>,
    },
    #[serde(rename = "output.ack")]
    OutputAck { re: String, offset: u64 },
    #[serde(rename = "output.unfollow")]
    OutputUnfollow { re: String },
    #[serde(rename = "output.data")]
    OutputData(OutputData),
    #[serde(rename = "pair.commit")]
    PairCommit { c: String },
    #[serde(rename = "pair.nonce")]
    PairNonce { n: String },
    #[serde(rename = "pair.reveal")]
    PairReveal { n: String },
    #[serde(rename = "pair.confirm")]
    PairConfirm { accept: bool },
    #[serde(rename = "pair.declined")]
    PairDeclined {
        reason: DeclineReason,
        /// Seconds, with `busy`: an optional field this build adds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
}

impl SessionMessage {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            re: None,
            code: code.to_string(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// An error answering the request `re`: the session goes on.
    pub fn refusal(re: &str, code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            re: Some(re.to_string()),
            code: code.to_string(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// Whether a viewer's request stays within the contract's bounds.
    pub fn within_bounds(&self) -> bool {
        let id_ok = |id: &str| !id.is_empty() && id.len() <= MAX_ID;
        match self {
            Self::InventoryGet { id, .. }
            | Self::InventoryWatch { id, .. }
            | Self::InventoryUnwatch { id } => id_ok(id),
            Self::OutputRead { id, max_bytes, .. } => {
                id_ok(id) && (1..=MAX_READ_BYTES).contains(max_bytes)
            }
            Self::OutputFollow { id, window, .. } => id_ok(id) && (1..=MAX_WINDOW).contains(window),
            Self::OutputAck { re, .. } | Self::OutputUnfollow { re } => id_ok(re),
            Self::Keep { ids } => ids.len() <= 16 && ids.iter().all(|id| id_ok(id)),
            _ => true,
        }
    }
}

/// Message types of protocol 1 that this build does not handle yet.
pub fn is_later_message(t: &str) -> bool {
    matches!(t, "control" | "control.status")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_send_frame_matches_the_contract_byte_for_byte() {
        let frame = Send {
            to: "m2".into(),
            sid: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            kind: FrameKind::Hs,
            purpose: Some(Purpose::Pair),
            data: Some("abc".into()),
        };
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            json!({"t": "send", "to": "m2", "sid": "AAAAAAAAAAAAAAAAAAAAAA", "kind": "hs", "purpose": "pair", "data": "abc"})
        );
        let close = Send {
            to: "m2".into(),
            sid: "s".into(),
            kind: FrameKind::Close,
            purpose: None,
            data: None,
        };
        assert_eq!(
            serde_json::to_value(&close).unwrap(),
            json!({"t": "send", "to": "m2", "sid": "s", "kind": "close"})
        );
    }

    #[test]
    fn relay_frames_parse_with_unknown_fields_ignored() {
        let welcome: RelayFrame = serde_json::from_value(json!({
            "t": "welcome", "v": 1, "connId": "c1", "machineId": "m1", "future": true,
            "peers": [{"t": "presence", "machineId": "m2", "state": "online",
                       "version": "1.4.0", "proto": {"min": 1, "max": 1},
                       "since": "2026-11-08T10:00:00Z"}],
        }))
        .unwrap();
        let RelayFrame::Welcome { peers, .. } = welcome else {
            panic!("not a welcome");
        };
        assert_eq!(peers[0].machine_id, "m2");

        let error: RelayFrame = serde_json::from_value(json!({
            "t": "error", "code": "unavailable", "sid": "s", "message": "D1", "retryAfter": 5
        }))
        .unwrap();
        assert!(matches!(error, RelayFrame::Error { retry_after: Some(r), .. } if r == 5.0));

        let revoked: RelayFrame =
            serde_json::from_value(json!({"t": "revoked", "machineId": "m3"})).unwrap();
        assert_eq!(
            revoked,
            RelayFrame::Revoked {
                machine_id: "m3".into()
            }
        );
    }

    /// The shapes `apps/cloud/src/relay.ts` sends: `since` from
    /// `toISOString()`, `error` without `sid` or `retryAfter` when it has
    /// none, `recv` without `purpose` or `data` on a `close`.
    #[test]
    fn frames_as_the_relay_builds_them_parse() {
        let transcript = [
            r#"{"t":"welcome","v":1,"connId":"3f2a","machineId":"mA","peers":[{"t":"presence","machineId":"mB","state":"online","version":"1.4.0","proto":{"min":1,"max":1},"since":"2026-11-08T10:12:00.123Z"}]}"#,
            r#"{"t":"presence","machineId":"mB","state":"offline","version":"1.4.0","proto":{"min":1,"max":1},"since":"2026-11-08T10:13:00.000Z"}"#,
            r#"{"t":"recv","from":"mB","sid":"AAAAAAAAAAAAAAAAAAAAAA","kind":"hs","purpose":"session","data":"AQ"}"#,
            r#"{"t":"recv","from":"mB","sid":"AAAAAAAAAAAAAAAAAAAAAA","kind":"close"}"#,
            r#"{"t":"error","code":"peer_offline","sid":"AAAAAAAAAAAAAAAAAAAAAA","message":"mB is not connected."}"#,
            r#"{"t":"error","code":"rate_limited","message":"Slow down.","retryAfter":2}"#,
            r#"{"t":"revoked","machineId":"mB"}"#,
        ];
        for frame in transcript {
            serde_json::from_str::<RelayFrame>(frame)
                .unwrap_or_else(|error| panic!("{frame}: {error}"));
        }
    }

    #[test]
    fn session_messages_use_the_contract_names() {
        let kept = serde_json::to_value(SessionMessage::Kept).unwrap();
        assert_eq!(kept, json!({"t": "kept"}));
        let bye = serde_json::to_value(SessionMessage::Bye {
            reason: ByeReason::LeaseExpired,
        })
        .unwrap();
        assert_eq!(bye, json!({"t": "bye", "reason": "lease_expired"}));
        let declined = serde_json::to_value(SessionMessage::PairDeclined {
            reason: DeclineReason::NobodyToConfirm,
            retry_after: None,
        })
        .unwrap();
        assert_eq!(
            declined,
            json!({"t": "pair.declined", "reason": "nobody_to_confirm"})
        );
        let hello: SessionMessage = serde_json::from_value(json!({
            "t": "hello", "proto": {"min": 1, "max": 1}, "version": "1.4.0",
            "now": "2026-11-08T10:00:00Z", "role": "target",
            "settings": {"share": true, "control": false, "backgroundStarts": false},
            "app": "absent",
        }))
        .unwrap();
        let SessionMessage::Hello(hello) = hello else {
            panic!("not a hello");
        };
        assert!(hello.settings.unwrap().share);
    }

    #[test]
    fn ranges_overlap_only_when_they_share_a_version() {
        let ours = ProtoRange::OURS;
        assert!(ours.overlaps(&ProtoRange { min: 1, max: 3 }));
        assert!(!ours.overlaps(&ProtoRange { min: 2, max: 3 }));
    }
}
