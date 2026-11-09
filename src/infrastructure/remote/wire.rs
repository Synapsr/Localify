//! The JSON on the relay socket and inside Noise sessions
//! (`docs/remote-machines.md`, "Relay frames" and "Session messages").
//! Field names are camelCase, and unknown fields are ignored.

use serde::{Deserialize, Serialize};

/// The session protocol versions this build speaks, in `hello`, `presence`
/// and `X-Localify-Proto` (`docs/remote-machines.md`, "Versioning"). The
/// prologue's label does not follow them.
pub const PROTO_MIN: u32 = 1;
pub const PROTO_MAX: u32 = 3;
/// The version that adds access requests.
pub const PROTO_ACCESS: u32 = 2;
/// The version that adds `pair.saved`.
pub const PROTO_PAIR_SAVED: u32 = 3;
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

    /// The session's version: the highest one in both ranges, `None` when
    /// they share none. Each side computes it from the two authenticated
    /// `hello`s, never from `presence`, which the relay could alter.
    pub fn agreed(&self, other: &Self) -> Option<u32> {
        self.overlaps(other).then(|| self.max.min(other.max))
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

impl From<TargetSettings> for crate::application::inventory::TargetSettings {
    fn from(settings: TargetSettings) -> Self {
        Self {
            share: settings.share,
            control: settings.control,
            background_starts: settings.background_starts,
        }
    }
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

impl From<AppPresence> for crate::application::inventory::AppPresence {
    fn from(app: AppPresence) -> Self {
        match app {
            AppPresence::Running => Self::Running,
            AppPresence::Absent => Self::Absent,
        }
    }
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

/// The switches an `access.request` asks for: `backgroundStarts` asks for
/// `control` too. Access never turns `share` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccessWants {
    Control,
    BackgroundStarts,
}

impl AccessWants {
    /// Whether `settings` already allow what is asked.
    pub fn is_granted_by(self, settings: &TargetSettings) -> bool {
        match self {
            Self::Control => settings.control,
            Self::BackgroundStarts => settings.control && settings.background_starts,
        }
    }
}

/// How an `access.request` ended; `busy` and `too_many` come as an `error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessOutcome {
    Granted,
    Declined,
    Nobody,
    Expired,
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

/// What a control asks a target to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlAction {
    Start,
    Stop,
    Restart,
}

impl ControlAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// The service a control names, by its ids only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlService {
    pub project_id: String,
    /// `None` for the primary checkout.
    pub environment: Option<String>,
    pub service_id: String,
}

/// What the viewer saw before it sent a control.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlExpect {
    pub run_id: Option<String>,
    pub config_rev: String,
    /// Protocol 2, restart only: the legacy run to relaunch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_id: Option<String>,
}

/// `control`: start, stop or restart one service, as the viewer saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Control {
    pub id: String,
    pub action: ControlAction,
    pub service: ControlService,
    /// Start only: a profile name, `None` for the effective default.
    pub profile: Option<String>,
    /// 16 random bytes, base64url: names this control across sessions.
    pub request_id: String,
    pub expect: ControlExpect,
    /// RFC 3339, in the target's clock.
    pub expires_at: String,
}

/// The fields a `control` may carry. Anything else is refused with
/// `refused_argument`, so no argument is ever silently dropped.
const CONTROL_FIELDS: &[&str] = &[
    "t",
    "id",
    "action",
    "service",
    "profile",
    "requestId",
    "expect",
    "expiresAt",
];
const CONTROL_SERVICE_FIELDS: &[&str] = &["projectId", "environment", "serviceId"];

/// The first field of a `control` that a session of version `proto` does
/// not know, if any: `expect.legacyId` belongs to protocol 2.
pub fn unknown_control_field(value: &serde_json::Value, proto: u32) -> Option<String> {
    let unknown = |value: Option<&serde_json::Value>, known: &[&str], prefix: &str| {
        value
            .and_then(serde_json::Value::as_object)
            .and_then(|fields| {
                fields
                    .keys()
                    .find(|field| !known.contains(&field.as_str()))
                    .map(|field| format!("{prefix}{field}"))
            })
    };
    let expect: &[&str] = if proto >= PROTO_ACCESS {
        &["runId", "configRev", "legacyId"]
    } else {
        &["runId", "configRev"]
    };
    unknown(Some(value), CONTROL_FIELDS, "")
        .or_else(|| unknown(value.get("service"), CONTROL_SERVICE_FIELDS, "service."))
        .or_else(|| unknown(value.get("expect"), expect, "expect."))
}

/// Whether `id` is 16 bytes in base64url, as a request id is: it names a
/// file on the target.
pub fn is_request_id(id: &str) -> bool {
    id.len() == 22
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Where a control is on the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlPhase {
    /// Handed to the app, or spawned detached; a stop's signal sent.
    Dispatched,
    /// Its launch record says `admitted`.
    Started,
}

/// How a control ended, or where it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlOutcome {
    Done,
    /// Admitted and running, not listening yet.
    Starting,
    Refused,
    Conflict,
    Failed,
    /// Only while the launch has no record and its expiry has not passed.
    Unknown,
}

/// `control.result`, answering a `control` or a `control.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlResult {
    pub re: String,
    pub outcome: ControlOutcome,
    /// A code of "Errors" when the outcome is not `done`.
    pub code: Option<String>,
    pub message: String,
    /// The run now in place, when known.
    pub run_id: Option<String>,
}

/// The session messages this build handles. The access messages and
/// `expect.legacyId` belong to protocol 2, `pair.saved` to protocol 3
/// ([`SessionMessage::proto`]).
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
    #[serde(rename = "control")]
    Control(Box<Control>),
    #[serde(rename = "control.status")]
    ControlStatus { id: String, request_id: String },
    #[serde(rename = "control.progress")]
    ControlProgress { re: String, phase: ControlPhase },
    #[serde(rename = "control.result")]
    ControlResult(ControlResult),
    #[serde(rename = "access.request")]
    AccessRequest { id: String, wants: AccessWants },
    #[serde(rename = "access.cancel")]
    AccessCancel { re: String },
    #[serde(rename = "access.result")]
    AccessResult { re: String, outcome: AccessOutcome },
    #[serde(rename = "pair.commit")]
    PairCommit { c: String },
    #[serde(rename = "pair.nonce")]
    PairNonce { n: String },
    #[serde(rename = "pair.reveal")]
    PairReveal { n: String },
    #[serde(rename = "pair.confirm")]
    PairConfirm { accept: bool },
    /// Protocol 3: this side durably wrote its pin.
    #[serde(rename = "pair.saved")]
    PairSaved,
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
            Self::Control(control) => control.within_bounds(),
            Self::ControlStatus { id, request_id } => id_ok(id) && is_request_id(request_id),
            Self::AccessRequest { id, .. } => id_ok(id),
            Self::AccessCancel { re } => id_ok(re),
            Self::Keep { ids } => ids.len() <= 16 && ids.iter().all(|id| id_ok(id)),
            _ => true,
        }
    }

    /// The first session protocol version that has this message. A session
    /// of an older version never sends it, and treats it as an unknown `t`.
    pub fn proto(&self) -> u32 {
        match self {
            Self::AccessRequest { .. } | Self::AccessCancel { .. } | Self::AccessResult { .. } => {
                PROTO_ACCESS
            }
            Self::PairSaved => PROTO_PAIR_SAVED,
            _ => 1,
        }
    }
}

impl Control {
    /// Ids and names of reasonable length, and a request id that is one.
    pub fn within_bounds(&self) -> bool {
        let short = |text: &str, max: usize| !text.is_empty() && text.len() <= max;
        short(&self.id, MAX_ID)
            && is_request_id(&self.request_id)
            && short(&self.service.project_id, 128)
            && short(&self.service.service_id, 128)
            && self
                .service
                .environment
                .as_deref()
                .is_none_or(|slug| short(slug, 128))
            && self.profile.as_deref().is_none_or(|name| short(name, 64))
            && short(&self.expect.config_rev, 64)
            && self.expect.run_id.as_deref().is_none_or(|id| short(id, 64))
            && self
                .expect
                .legacy_id
                .as_deref()
                .is_none_or(|id| short(id, 128))
            && short(&self.expires_at, 64)
    }
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
        assert!(ours.overlaps(&ProtoRange { min: 2, max: 3 }));
        assert!(ours.overlaps(&ProtoRange { min: 3, max: 4 }));
        assert!(!ours.overlaps(&ProtoRange { min: 4, max: 5 }));
    }

    #[test]
    fn the_session_version_is_the_highest_in_both_ranges() {
        let ours = ProtoRange::OURS;
        assert_eq!(ours.agreed(&ProtoRange { min: 1, max: 2 }), Some(2));
        assert_eq!(ours.agreed(&ProtoRange { min: 1, max: 1 }), Some(1));
        assert_eq!(ours.agreed(&ProtoRange { min: 1, max: 9 }), Some(3));
        assert_eq!(ours.agreed(&ProtoRange { min: 4, max: 9 }), None);
    }

    #[test]
    fn pair_saved_uses_the_contract_name_and_takes_version_3() {
        let saved = serde_json::to_value(SessionMessage::PairSaved).unwrap();
        assert_eq!(saved, json!({"t": "pair.saved"}));
        let parsed: SessionMessage = serde_json::from_value(saved).unwrap();
        assert_eq!(parsed, SessionMessage::PairSaved);
        assert_eq!(parsed.proto(), 3);
    }

    #[test]
    fn access_messages_use_the_contract_names_and_take_version_2() {
        let request: SessionMessage = serde_json::from_value(
            json!({"t": "access.request", "id": "r1", "wants": "backgroundStarts"}),
        )
        .unwrap();
        assert_eq!(
            request,
            SessionMessage::AccessRequest {
                id: "r1".into(),
                wants: AccessWants::BackgroundStarts,
            }
        );
        assert_eq!(request.proto(), 2);
        let result = SessionMessage::AccessResult {
            re: "r1".into(),
            outcome: AccessOutcome::Nobody,
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"t": "access.result", "re": "r1", "outcome": "nobody"})
        );
        let cancel = SessionMessage::AccessCancel { re: "r1".into() };
        assert_eq!(
            serde_json::to_value(&cancel).unwrap(),
            json!({"t": "access.cancel", "re": "r1"})
        );
        assert_eq!(SessionMessage::Kept.proto(), 1);
        assert!(
            !SessionMessage::AccessCancel {
                re: "x".repeat(MAX_ID + 1)
            }
            .within_bounds()
        );
    }

    #[test]
    fn background_starts_are_granted_only_with_control() {
        let settings = |control, background_starts| TargetSettings {
            share: true,
            control,
            background_starts,
        };
        assert!(AccessWants::Control.is_granted_by(&settings(true, false)));
        assert!(!AccessWants::BackgroundStarts.is_granted_by(&settings(true, false)));
        assert!(AccessWants::BackgroundStarts.is_granted_by(&settings(true, true)));
        assert!(!AccessWants::Control.is_granted_by(&settings(false, false)));
    }

    fn control_json() -> serde_json::Value {
        json!({
            "t": "control", "id": "c1", "action": "restart",
            "service": {"projectId": "app", "environment": null, "serviceId": "web"},
            "profile": null, "requestId": "AAAAAAAAAAAAAAAAAAAAAA",
            "expect": {"runId": "BBBBBBBBBBBBBBBBBBBBBB", "configRev": "0123456789abcdef"},
            "expiresAt": "2026-11-09T10:00:30Z"
        })
    }

    #[test]
    fn a_control_and_its_answers_read_as_the_contract_writes_them() {
        let SessionMessage::Control(control) = serde_json::from_value(control_json()).unwrap()
        else {
            panic!("not a control");
        };
        assert_eq!(control.action, ControlAction::Restart);
        assert!(control.within_bounds());
        assert_eq!(
            serde_json::to_value(SessionMessage::Control(control)).unwrap(),
            control_json()
        );

        let result = SessionMessage::ControlResult(ControlResult {
            re: "c1".into(),
            outcome: ControlOutcome::Refused,
            code: Some("busy".into()),
            message: "Busy.".into(),
            run_id: None,
        });
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"t": "control.result", "re": "c1", "outcome": "refused", "code": "busy",
                   "message": "Busy.", "runId": null})
        );
        let progress: SessionMessage = serde_json::from_value(
            json!({"t": "control.progress", "re": "c1", "phase": "started"}),
        )
        .unwrap();
        assert_eq!(
            progress,
            SessionMessage::ControlProgress {
                re: "c1".into(),
                phase: ControlPhase::Started
            }
        );
        let status: SessionMessage = serde_json::from_value(
            json!({"t": "control.status", "id": "s1", "requestId": "AAAAAAAAAAAAAAAAAAAAAA"}),
        )
        .unwrap();
        assert!(status.within_bounds());
        assert_eq!(status.proto(), 1);
    }

    #[test]
    fn a_control_field_the_session_does_not_know_is_named() {
        assert_eq!(unknown_control_field(&control_json(), 2), None);
        let mut command = control_json();
        command["command"] = json!(["rm", "-rf", "/"]);
        assert_eq!(
            unknown_control_field(&command, 2).as_deref(),
            Some("command")
        );
        let mut port = control_json();
        port["service"]["port"] = json!(22);
        assert_eq!(
            unknown_control_field(&port, 2).as_deref(),
            Some("service.port")
        );
        // `legacyId` is protocol 2's.
        let mut relaunch = control_json();
        relaunch["expect"]["legacyId"] = json!("boot.1.2");
        assert_eq!(unknown_control_field(&relaunch, 2), None);
        assert_eq!(
            unknown_control_field(&relaunch, 1).as_deref(),
            Some("expect.legacyId")
        );
    }

    #[test]
    fn a_request_id_is_sixteen_bytes_of_base64url_and_nothing_else() {
        assert!(is_request_id("AAAAAAAAAAAAAAAAAAAAAA"));
        assert!(is_request_id("-_AAAAAAAAAAAAAAAAAAAA"));
        for id in [
            "",
            "short",
            "../../../../etc/passwd",
            "AAAAAAAAAAAAAAAAAAAAA/",
        ] {
            assert!(!is_request_id(id), "{id}");
        }
        let mut control = control_json();
        control["requestId"] = json!("../x");
        let message: SessionMessage = serde_json::from_value(control).unwrap();
        assert!(!message.within_bounds());
    }
}
