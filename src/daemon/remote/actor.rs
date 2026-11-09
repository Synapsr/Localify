//! One relay connection and every session over it
//! (`docs/remote-machines.md`, "Sessions", "Pairing", "Relay frames").
//!
//! A single task owns the sessions, so no lock guards their state: frames
//! from the relay, commands from the management socket and a once-a-second
//! tick all arrive here in turn. When the connection ends, so does every
//! session, and nothing is resent on the next one.
//!
//! As a target, it answers inventory and output requests (`target`) and
//! access requests (`asking`); as a viewer, it carries the management
//! socket's requests (`viewer`).

mod asking;
mod control;
mod target;
mod viewer;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use self::control::ControlEvent;
use self::viewer::Tap;
pub use self::viewer::{TapEvent, TapRequest, WINDOW};
use super::access::Waiting;
use super::backoff::Ending;
use super::lease::{LeaseError, LeaseKeeper};
use super::local::LocalSource;
use super::machines::SessionState;
use super::pacer::{self, Bucket, Outbox};
use super::socket::RelaySocket;
use super::{Shared, View};
use crate::daemon::public_tunnel::Clocks;
use crate::infrastructure::cloud::api::CloudClient;
use crate::infrastructure::remote::noise::{
    HASH_LEN, Handshake, KEY_LEN, SessionContext, SessionId, StaticKeys, Transport, decode_key,
    encode_key,
};
use crate::infrastructure::remote::pairing::{Pairing, Step};
use crate::infrastructure::remote::store::{Pin, now_rfc3339};
use crate::infrastructure::remote::wire::{
    AccessOutcome, AccessWants, ByeReason, DeclineReason, FrameKind, Hello, PresenceState, Purpose,
    RelayFrame, Role, Send, SessionMessage, unknown_control_field,
};

/// A target handles at most this many handshakes at once.
const MAX_HANDSHAKES: usize = 4;
/// A session ends after this many messages, both ways together.
const MAX_MESSAGES: u64 = 100_000;
/// A target accepts at most this many pairing attempts an hour.
const PAIRINGS_PER_HOUR: usize = 5;
const HOUR: Duration = Duration::from_secs(3600);

/// Why something did not work, with the contract's code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// What a `remote_pair` client hears about its pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PairEvent {
    Code {
        pairing_id: String,
        machine_id: String,
        name: String,
        code: String,
    },
    Paired {
        machine_id: String,
        name: String,
    },
    Cancelled {
        reason: String,
        message: String,
    },
}

/// What `remote_events` clients hear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    /// Another machine wants to pair: confirm the code with
    /// `remote_pair_answer`.
    PairRequest {
        pairing_id: String,
        machine_id: String,
        name: String,
        code: String,
    },
    Paired {
        pairing_id: String,
        machine_id: String,
        name: String,
    },
    PairCancelled {
        pairing_id: String,
        machine_id: String,
        reason: String,
    },
    Presence {
        machine_id: String,
        state: PresenceState,
    },
    Revoked {
        machine_id: String,
    },
    Unpaired {
        machine_id: String,
    },
    /// A paired machine asks for remote controls: show the dialog, then
    /// answer with `remote_access_answer`. Heard only by a subscriber that
    /// shows it. `name` and `fingerprint` come from the local pin, never
    /// from the peer.
    AccessRequest {
        request_id: String,
        machine_id: String,
        name: String,
        fingerprint: String,
        wants: AccessWants,
        /// RFC 3339.
        expires_at: String,
    },
    /// The request ended, however it did: close its dialog.
    AccessClosed {
        request_id: String,
    },
}

impl Event {
    /// Whether only a subscriber that shows the access dialog hears it.
    pub fn is_access(&self) -> bool {
        matches!(self, Self::AccessRequest { .. } | Self::AccessClosed { .. })
    }
}

pub enum Command {
    Open {
        peer: String,
        hold: bool,
        reply: oneshot::Sender<Result<Hello, Failure>>,
    },
    Pair {
        peer: String,
        events: mpsc::Sender<PairEvent>,
    },
    Answer {
        pairing_id: String,
        accept: bool,
        reply: oneshot::Sender<bool>,
    },
    Unpair {
        peer: String,
    },
    LeaseReady {
        result: Result<(), LeaseError>,
    },
    LeaseRenewed {
        result: Result<(), LeaseError>,
    },
    /// A lease asked for outside the actor found the token revoked.
    Revoked,
    /// A viewer's request to a peer, carried until it ends.
    Tap {
        peer: String,
        request: TapRequest,
        events: mpsc::Sender<TapEvent>,
        reply: oneshot::Sender<Result<String, Failure>>,
    },
    /// The client of a request went away: unwatch or unfollow.
    Untap {
        peer: String,
        id: String,
    },
    /// The client showed a follow's output up to `offset`.
    Ack {
        peer: String,
        id: String,
        offset: u64,
    },
    /// The switches were just saved: the actor reads them from the file,
    /// never from the message, so a late notice of an older change cannot
    /// undo a newer one. Answered once it acted on them, so a switch turned
    /// off stops every follow before the change is confirmed.
    SettingsChanged {
        reply: oneshot::Sender<()>,
    },
    /// A person's answer in the access dialog.
    AccessAnswer {
        request_id: String,
        allow: bool,
        suppress: bool,
        reply: oneshot::Sender<AccessOutcome>,
    },
    /// Sends a message as the viewer of `peer`, whatever it is.
    #[cfg(test)]
    Raw {
        peer: String,
        message: SessionMessage,
    },
    /// A snapshot of this machine's inventory, built for a viewer.
    Snapshot {
        result: Result<Arc<crate::application::inventory::Snapshot>, String>,
    },
    /// The control executor's progress or result for a viewer's control or
    /// `control.status`, `re` in the session `key`.
    ControlDone {
        key: Key,
        re: String,
        event: ControlEvent,
    },
}

/// What this connection is for.
pub struct Context {
    pub self_id: String,
    /// This machine's name, on inventory pages.
    pub name: String,
    /// What this machine shares, as a target.
    pub source: Arc<dyn LocalSource>,
    /// The token this connection signed in with: another one in `cloud.json`
    /// means this machine signed out, or in again.
    pub token: String,
    /// The account's user id, in every prologue.
    pub account: String,
    pub keys: StaticKeys,
    pub lease: Arc<LeaseKeeper>,
    pub cloud: Arc<CloudClient>,
}

enum Phase {
    /// A target's first handshake message, held until a lease is in hand.
    AwaitLease(Vec<u8>),
    Handshake(Box<Handshake>),
    Open(Transport),
    /// Taken out for a moment while a step runs.
    Busy,
}

/// Who waits on a viewer's session.
enum Holder {
    None,
    Open {
        reply: Option<oneshot::Sender<Result<Hello, Failure>>>,
        hold: bool,
    },
    Pair(mpsc::Sender<PairEvent>),
}

struct Session {
    peer: String,
    sid: SessionId,
    purpose: Purpose,
    role: Role,
    phase: Phase,
    started: Instant,
    opened: Option<Instant>,
    hash: Option<[u8; HASH_LEN]>,
    peer_key: Option<[u8; KEY_LEN]>,
    hello: Option<Hello>,
    /// The version agreed from both `hello`s, once the peer's came.
    proto: Option<u32>,
    messages: u64,
    last_heard: Instant,
    keep_sent: Option<Instant>,
    /// The first `keep` no authenticated message has followed yet.
    unanswered: Option<Instant>,
    last_keep: Instant,
    not_responding: bool,
    pairing: Option<Pairing>,
    code_shown: bool,
    paired: bool,
    holder: Holder,
    /// Target: what the viewer asked for.
    served: target::Served,
    /// Viewer: the requests carried over this session, by id.
    taps: HashMap<String, Tap>,
    /// Target: the access request this viewer waits on.
    access: Option<Waiting>,
    /// Target: the controls and `control.status`es of this viewer still
    /// going, by id.
    controls: HashSet<String>,
    /// Viewer: the target's clock as its `hello` gave it, and when it came,
    /// so a control expires in the target's clock.
    clock: Option<viewer::PeerClock>,
}

impl Session {
    fn new(peer: String, sid: SessionId, purpose: Purpose, role: Role, phase: Phase) -> Self {
        let now = Instant::now();
        Self {
            peer,
            sid,
            purpose,
            role,
            phase,
            started: now,
            opened: None,
            hash: None,
            peer_key: None,
            hello: None,
            proto: None,
            messages: 0,
            last_heard: now,
            keep_sent: None,
            unanswered: None,
            last_keep: now,
            not_responding: false,
            pairing: None,
            code_shown: false,
            paired: false,
            holder: Holder::None,
            served: target::Served::default(),
            taps: HashMap::new(),
            access: None,
            controls: HashSet::new(),
            clock: None,
        }
    }

    fn is_open(&self) -> bool {
        matches!(self.phase, Phase::Open(_))
    }

    /// Whether something still uses the session, so it is not idle.
    fn is_held(&self) -> bool {
        if !self.taps.is_empty()
            || !self.served.is_empty()
            || self.access.is_some()
            || !self.controls.is_empty()
        {
            return true;
        }
        match &self.holder {
            Holder::Open { reply, hold } => *hold || reply.is_some(),
            Holder::Pair(_) => true,
            Holder::None => self
                .pairing
                .as_ref()
                .is_some_and(|pairing| !pairing.is_finished()),
        }
    }
}

type Key = (String, SessionId);

/// How a session ends on this side.
enum End {
    /// Nothing more to send: the peer closed it, or the relay is gone.
    Silent,
    /// A relay `close`: the session never authenticated, or a frame failed.
    Close,
    Bye(ByeReason),
    /// `error` with this code, then `bye`.
    Error(&'static str, String),
}

struct Actor {
    shared: Arc<Shared>,
    ctx: Context,
    out: Outbox,
    commands: mpsc::UnboundedSender<Command>,
    sessions: HashMap<Key, Session>,
    fetching_lease: bool,
    renewing: bool,
    not_before: HashMap<String, Instant>,
    clocks: Clocks,
    next_clock_check: Instant,
    /// Set by a handler that ends the whole connection.
    failed: Option<Ending>,
    /// Lease and roster requests in flight, aborted with the connection.
    tasks: Vec<JoinHandle<()>>,
    /// Target: the snapshot builds and the follows' turn.
    shelf: target::Shelf,
    /// Viewer: numbers the requests it carries.
    taps_made: u64,
    /// Target: how the last access requests ended, so a late answer says.
    access_ended: VecDeque<(String, AccessOutcome)>,
}

pub async fn run(
    shared: Arc<Shared>,
    ctx: Context,
    socket: RelaySocket,
    mut receiver: mpsc::UnboundedReceiver<Command>,
    commands: mpsc::UnboundedSender<Command>,
    cancel: &CancellationToken,
) -> Ending {
    let timings = shared.timings;
    let (sink, mut stream) = socket.split();
    let (out, queue) = pacer::queue();
    let drained = out.drained();
    let bucket = Bucket::paced(Instant::now());
    let mut writer = tokio::spawn(pacer::run(sink, queue, bucket));
    let mut writer_done = false;
    let refresh = Arc::clone(&shared);
    let sharing = shared.store.settings().is_ok_and(|settings| settings.share);
    let mut actor = Actor {
        shared,
        ctx,
        out,
        commands,
        sessions: HashMap::new(),
        fetching_lease: false,
        renewing: false,
        not_before: HashMap::new(),
        clocks: Clocks::now(),
        next_clock_check: Instant::now() + timings.clock_check,
        failed: None,
        tasks: Vec::new(),
        shelf: target::Shelf::new(sharing),
        taps_made: 0,
        access_ended: VecDeque::new(),
    };
    let mut tick = tokio::time::interval(timings.tick);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut poll = tokio::time::interval(timings.follow_poll);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut ping =
        tokio::time::interval_at(Instant::now() + timings.ping_every, timings.ping_every);
    let mut last_frame = Instant::now();
    let ending = loop {
        tokio::select! {
            _ = cancel.cancelled() => break Ending::Lost,
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    last_frame = Instant::now();
                    if let Some(ending) = actor.on_text(text.as_str()) {
                        break ending;
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    break match frame {
                        Some(frame) => Ending::Closed {
                            code: u16::from(frame.code),
                            reason: frame.reason.to_string(),
                        },
                        None => Ending::Closed { code: 1005, reason: String::new() },
                    };
                }
                Some(Ok(_)) => last_frame = Instant::now(),
                Some(Err(error)) => {
                    debug!(error = %error, "The relay socket failed");
                    break Ending::Lost;
                }
                None => break Ending::Lost,
            },
            Some(command) = receiver.recv() => actor.on_command(command),
            _ = tick.tick() => {
                if last_frame.elapsed() >= timings.relay_silence {
                    info!("No frame from the relay in {} s", timings.relay_silence.as_secs());
                    break Ending::Lost;
                }
                actor.on_tick();
            }
            _ = ping.tick() => actor.send_text("ping".to_string()),
            _ = poll.tick() => actor.on_poll(),
            // The writer took a frame: room for pages or output.
            _ = drained.notified() => {}
            _ = refresh.refresh.notified() => {
                if actor.signed_out_here() {
                    info!("This machine signed out; closing its relay connection");
                    break Ending::SignedOutHere;
                }
            }
            _ = &mut writer => {
                writer_done = true;
                break Ending::Lost;
            }
        }
        actor.pump();
        if let Some(ending) = actor.failed.take() {
            break ending;
        }
    };
    actor.end_all(&Failure::new("relay_lost", "The relay connection closed."));
    for task in actor.tasks.drain(..) {
        task.abort();
    }
    drop(actor);
    // A writer stuck on the socket must not hold up the reconnect: what it
    // still holds is dropped.
    if !writer_done {
        writer.abort();
        let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
    }
    ending
}

impl Actor {
    fn view(&self) -> std::sync::MutexGuard<'_, View> {
        self.shared.view()
    }

    /// Queues a frame. A full queue means the socket is stuck: the
    /// connection ends, and the link connects again.
    fn send_text(&mut self, text: String) {
        if !self.out.push(text) && self.failed.is_none() {
            warn!("The relay socket stopped taking frames; reconnecting");
            self.failed = Some(Ending::Lost);
        }
    }

    /// Whether `cloud.json` no longer holds this connection's sign-in.
    fn signed_out_here(&self) -> bool {
        match self.shared.credentials.load() {
            Ok(Some(credentials)) => {
                credentials.token != self.ctx.token || credentials.machine_id != self.ctx.self_id
            }
            _ => true,
        }
    }

    fn spawn(&mut self, task: impl std::future::Future<Output = ()> + std::marker::Send + 'static) {
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio::spawn(task));
    }

    fn send_frame(
        &mut self,
        to: &str,
        sid: SessionId,
        kind: FrameKind,
        purpose: Option<Purpose>,
        data: Option<&[u8]>,
    ) {
        let frame = Send {
            to: to.to_string(),
            sid: sid.encode(),
            kind,
            purpose,
            data: data.map(|data| URL_SAFE_NO_PAD.encode(data)),
        };
        match serde_json::to_string(&frame) {
            Ok(text) => self.send_text(text),
            Err(error) => warn!(error = %error, "Could not encode a relay frame"),
        }
    }

    /// Encrypts and sends one session message. `false` when the session
    /// cannot send: it is not open, or the message is out of bounds.
    fn send_message(&mut self, key: &Key, message: &SessionMessage) -> bool {
        let Ok(plaintext) = serde_json::to_vec(message) else {
            return false;
        };
        let Some(session) = self.sessions.get_mut(key) else {
            return false;
        };
        let Phase::Open(transport) = &mut session.phase else {
            return false;
        };
        let Ok(sealed) = transport.encrypt(&plaintext) else {
            return false;
        };
        session.messages += 1;
        let (peer, sid) = (session.peer.clone(), session.sid);
        self.send_frame(&peer, sid, FrameKind::Msg, None, Some(&sealed));
        true
    }

    // --- Relay frames --------------------------------------------------------

    fn on_text(&mut self, text: &str) -> Option<Ending> {
        if text == "pong" {
            return None;
        }
        let frame: RelayFrame = match serde_json::from_str(text) {
            Ok(frame) => frame,
            Err(error) => {
                debug!(error = %error, "Dropped a relay frame this version cannot read");
                return None;
            }
        };
        match frame {
            RelayFrame::Welcome {
                machine_id, peers, ..
            } => {
                if machine_id != self.ctx.self_id {
                    warn!("The relay welcomed another machine id; closing");
                    return Some(Ending::Closed {
                        code: 4000,
                        reason: "welcome for another machine".into(),
                    });
                }
                let mut view = self.view();
                view.relay = super::machines::RelayState::Connected;
                view.blocked = None;
                view.retry_at = None;
                view.presence = peers
                    .into_iter()
                    .map(|presence| (presence.machine_id.clone(), presence))
                    .collect();
                info!("Connected to the relay");
            }
            RelayFrame::Presence(presence) => self.on_presence(presence),
            RelayFrame::Recv {
                from,
                sid,
                kind,
                purpose,
                data,
            } => self.on_recv(from, &sid, kind, purpose, data.as_deref()),
            RelayFrame::Revoked { machine_id } => self.on_revoked(&machine_id),
            RelayFrame::Error {
                code,
                sid,
                message,
                retry_after,
            } => self.on_relay_error(&code, sid.as_deref(), &message, retry_after),
        }
        None
    }

    fn on_presence(&mut self, presence: crate::infrastructure::remote::wire::Presence) {
        let machine_id = presence.machine_id.clone();
        let state = presence.state;
        if state == PresenceState::Offline {
            self.end_peer(
                &machine_id,
                End::Silent,
                &Failure::new("peer_offline", "It went offline."),
            );
        }
        let known = self
            .view()
            .roster
            .iter()
            .any(|machine| machine.id == machine_id);
        if !known && state == PresenceState::Online {
            self.refresh_roster();
        }
        self.view().presence.insert(machine_id.clone(), presence);
        if state == PresenceState::Offline {
            self.view().presence.remove(&machine_id);
        }
        self.shared.emit(Event::Presence { machine_id, state });
    }

    /// A machine the roster does not list yet: read it again, for the
    /// names pins and pairing requests show.
    fn refresh_roster(&mut self) {
        let shared = Arc::clone(&self.shared);
        let cloud = Arc::clone(&self.ctx.cloud);
        self.spawn(async move {
            if let Ok(roster) = cloud.machines().await {
                shared.view().set_roster(roster);
            }
        });
    }

    fn on_revoked(&mut self, machine_id: &str) {
        info!(
            machine = machine_id,
            "A machine was removed from the account; dropping its pin"
        );
        if let Err(error) = self.shared.store.update_pins(&self.ctx.self_id, |pins| {
            pins.remove(machine_id);
        }) {
            warn!(error = %format!("{error:#}"), "Could not delete a revoked machine's pin");
        }
        self.end_peer(
            machine_id,
            End::Silent,
            &Failure::new("revoked", "It was removed from the account."),
        );
        let mut view = self.view();
        view.presence.remove(machine_id);
        view.roster.retain(|machine| machine.id != machine_id);
        view.settings.remove(machine_id);
        view.inventories.remove(machine_id);
        drop(view);
        self.shared.emit(Event::Revoked {
            machine_id: machine_id.to_string(),
        });
    }

    /// The relay dropped one of this session's frames: the session cannot
    /// skip it, so it ends now, and the next one waits.
    fn on_relay_error(
        &mut self,
        code: &str,
        sid: Option<&str>,
        message: &str,
        retry_after: Option<f64>,
    ) {
        let Some(sid) = sid.and_then(SessionId::parse) else {
            debug!(code, message, "The relay reported an error");
            return;
        };
        let Some(key) = self.sessions.keys().find(|key| key.1 == sid).cloned() else {
            return;
        };
        let wait = retry_after
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .map(|seconds| Duration::from_secs_f64(seconds.min(3600.0)))
            .unwrap_or(self.shared.timings.retry_after_drop);
        self.not_before.insert(key.0.clone(), Instant::now() + wait);
        self.end(&key, End::Close, &Failure::new(code, message));
    }

    fn on_recv(
        &mut self,
        from: String,
        sid: &str,
        kind: FrameKind,
        purpose: Option<Purpose>,
        data: Option<&str>,
    ) {
        let Some(sid) = SessionId::parse(sid) else {
            return;
        };
        if from == self.ctx.self_id {
            return;
        }
        let key = (from, sid);
        let known = self.sessions.contains_key(&key);
        if kind == FrameKind::Close {
            let open = self.sessions.get(&key).is_some_and(Session::is_open);
            if known && !open {
                let failure = Failure::new(
                    "refused",
                    "It refused the session: it is busy, or remote access is not available there.",
                );
                self.end(&key, End::Silent, &failure);
            } else if known {
                self.end(
                    &key,
                    End::Silent,
                    &Failure::new("closed", "The other machine closed the session."),
                );
            }
            return;
        }
        let Some(data) = data.and_then(|data| URL_SAFE_NO_PAD.decode(data).ok()) else {
            if known {
                self.end(
                    &key,
                    End::Close,
                    &Failure::new("bad_frame", "A frame carried no data."),
                );
            }
            return;
        };
        match (known, kind, purpose) {
            (false, FrameKind::Hs, Some(purpose)) => self.accept(key, purpose, data),
            // A frame for an unknown session without a purpose is dropped.
            (false, _, _) => {}
            (true, FrameKind::Hs, _) => self.on_handshake(&key, &data),
            (true, _, _) => self.on_message(&key, &data),
        }
    }

    // --- Handshakes ----------------------------------------------------------

    fn context(
        &self,
        purpose: Purpose,
        initiator: &str,
        responder: &str,
        sid: SessionId,
    ) -> SessionContext {
        SessionContext {
            purpose,
            account: self.ctx.account.clone(),
            initiator: initiator.to_string(),
            responder: responder.to_string(),
            sid,
        }
    }

    /// A viewer's first handshake message.
    fn accept(&mut self, key: Key, purpose: Purpose, first: Vec<u8>) {
        let handshaking = self
            .sessions
            .values()
            .filter(|session| session.role == Role::Target && !session.is_open())
            .count();
        if handshaking >= MAX_HANDSHAKES {
            debug!(peer = %key.0, "Too many handshakes at once; refusing one");
            self.send_frame(&key.0, key.1, FrameKind::Close, None, None);
            return;
        }
        let session = Session::new(
            key.0.clone(),
            key.1,
            purpose,
            Role::Target,
            Phase::AwaitLease(first),
        );
        self.sessions.insert(key.clone(), session);
        if self.ctx.lease.is_valid(Instant::now()) {
            self.respond(&key);
        } else {
            self.fetch_lease();
        }
    }

    fn fetch_lease(&mut self) {
        if self.fetching_lease {
            return;
        }
        self.fetching_lease = true;
        let lease = Arc::clone(&self.ctx.lease);
        let commands = self.commands.clone();
        self.spawn(async move {
            let result = lease.ensure().await;
            let _ = commands.send(Command::LeaseReady { result });
        });
    }

    fn on_lease_ready(&mut self, result: Result<(), LeaseError>) {
        self.fetching_lease = false;
        if result.as_ref().is_err_and(LeaseError::is_revoked) {
            self.failed = Some(Ending::Revoked);
            return;
        }
        let ok = result.is_ok();
        let parked: Vec<Key> = self
            .sessions
            .iter()
            .filter(|(_, session)| matches!(session.phase, Phase::AwaitLease(_)))
            .map(|(key, _)| key.clone())
            .collect();
        for key in parked {
            if ok {
                self.respond(&key);
            } else {
                self.end(&key, End::Close, &Failure::new("unauthorized", "No lease."));
            }
        }
    }

    /// Answers the first handshake message with the second.
    fn respond(&mut self, key: &Key) {
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let Phase::AwaitLease(first) = std::mem::replace(&mut session.phase, Phase::Busy) else {
            return;
        };
        let purpose = session.purpose;
        let context = self.context(purpose, &key.0, &self.ctx.self_id.clone(), key.1);
        let reply = Handshake::responder(&self.ctx.keys, &context).and_then(|mut handshake| {
            handshake.read(&first)?;
            let second = handshake.write()?;
            Ok((handshake, second))
        });
        match reply {
            Ok((handshake, second)) => {
                if let Some(session) = self.sessions.get_mut(key) {
                    session.phase = Phase::Handshake(Box::new(handshake));
                }
                self.send_frame(&key.0, key.1, FrameKind::Hs, None, Some(&second));
            }
            Err(error) => {
                debug!(error = %error, "A handshake failed");
                self.end(
                    key,
                    End::Close,
                    &Failure::new("handshake_failed", "The handshake failed."),
                );
            }
        }
    }

    /// Opens a session as the viewer: the first handshake message. Returns
    /// the new session, or `None` when it could not start.
    fn initiate(&mut self, peer: &str, purpose: Purpose, holder: Holder) -> Option<Key> {
        let failed = |holder: Holder, failure: Failure| {
            notify_failure(holder, &failure);
            None
        };
        let sid = match SessionId::random() {
            Ok(sid) => sid,
            Err(error) => return failed(holder, Failure::new("internal", format!("{error:#}"))),
        };
        let context = self.context(purpose, &self.ctx.self_id.clone(), peer, sid);
        let first = Handshake::initiator(&self.ctx.keys, &context).and_then(|mut handshake| {
            let first = handshake.write()?;
            Ok((handshake, first))
        });
        let (handshake, first) = match first {
            Ok(pair) => pair,
            Err(error) => return failed(holder, Failure::new("internal", format!("{error:#}"))),
        };
        let mut session = Session::new(
            peer.to_string(),
            sid,
            purpose,
            Role::Viewer,
            Phase::Handshake(Box::new(handshake)),
        );
        session.holder = holder;
        self.sessions.insert((peer.to_string(), sid), session);
        if purpose == Purpose::Session {
            self.view()
                .sessions
                .insert(peer.to_string(), SessionState::Handshaking);
        }
        self.send_frame(peer, sid, FrameKind::Hs, Some(purpose), Some(&first));
        Some((peer.to_string(), sid))
    }

    fn on_handshake(&mut self, key: &Key, message: &[u8]) {
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let Phase::Handshake(mut handshake) = std::mem::replace(&mut session.phase, Phase::Busy)
        else {
            return self.end(
                key,
                End::Close,
                &Failure::new("bad_frame", "A handshake frame came out of order."),
            );
        };
        if handshake.is_my_turn() || handshake.read(message).is_err() {
            return self.end(
                key,
                End::Close,
                &Failure::new("handshake_failed", "The handshake failed."),
            );
        }
        let (role, purpose) = (session.role, session.purpose);
        let remote = handshake.remote_static();
        if role == Role::Viewer {
            // The second message carries the target's static key: check it
            // before sending ours.
            if purpose == Purpose::Session
                && let Err(failure) = self.peer_check(&key.0, remote)
            {
                return self.end(key, End::Close, &failure);
            }
            let third = match handshake.write() {
                Ok(third) => third,
                Err(_) => {
                    return self.end(
                        key,
                        End::Close,
                        &Failure::new("handshake_failed", "The handshake failed."),
                    );
                }
            };
            self.send_frame(&key.0, key.1, FrameKind::Hs, None, Some(&third));
        }
        if !handshake.is_finished() {
            return self.end(
                key,
                End::Close,
                &Failure::new("handshake_failed", "The handshake failed."),
            );
        }
        self.finish_handshake(key, *handshake, remote);
    }

    fn finish_handshake(&mut self, key: &Key, handshake: Handshake, remote: Option<[u8; KEY_LEN]>) {
        let Ok((transport, hash)) = handshake.finish() else {
            return self.end(
                key,
                End::Close,
                &Failure::new("handshake_failed", "The handshake failed."),
            );
        };
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let now = Instant::now();
        session.phase = Phase::Open(transport);
        session.hash = Some(hash);
        session.peer_key = remote;
        session.opened = Some(now);
        session.last_heard = now;
        session.last_keep = now;
        let (role, purpose) = (session.role, session.purpose);
        if role == Role::Viewer && purpose == Purpose::Pair {
            session.pairing = Some(Pairing::initiator(hash));
        }
        if role == Role::Target && purpose == Purpose::Session {
            if let Err(failure) = self.peer_check(&key.0, remote) {
                let code = if failure.code == "key_mismatch" {
                    "key_mismatch"
                } else {
                    "not_paired"
                };
                self.send_message(key, &SessionMessage::error(code, failure.message.clone()));
                return self.end(key, End::Close, &failure);
            }
            // One session per viewer: a new one replaces the old.
            let older: Vec<Key> = self
                .sessions
                .iter()
                .filter(|(other, session)| {
                    *other != key
                        && other.0 == key.0
                        && session.role == Role::Target
                        && session.purpose == Purpose::Session
                })
                .map(|(other, _)| other.clone())
                .collect();
            for other in older {
                self.end(
                    &other,
                    End::Bye(ByeReason::Done),
                    &Failure::new("replaced", "A newer session replaced it."),
                );
            }
        }
        if role == Role::Viewer && purpose == Purpose::Session {
            self.view().sessions.insert(
                key.0.clone(),
                SessionState::Open {
                    not_responding: false,
                },
            );
        }
        let hello = self.hello(role, purpose);
        self.send_message(key, &SessionMessage::Hello(hello));
    }

    /// The peer's static key against its pin, by the machine id the relay
    /// says the frame came from.
    fn peer_check(&self, peer: &str, remote: Option<[u8; KEY_LEN]>) -> Result<(), Failure> {
        let pins = self.shared.store.pins().unwrap_or_default();
        let result = match (pins.get(peer), remote) {
            (None, _) => Err(Failure::new(
                "not_paired",
                "This machine is not paired with it.",
            )),
            (Some(pin), Some(remote)) if decode_key(&pin.public_key).ok() == Some(remote) => Ok(()),
            _ => Err(Failure::new(
                "key_mismatch",
                "Its key is not the one paired. Don't trust this machine until you pair it again.",
            )),
        };
        if let Err(failure) = &result {
            self.view()
                .failures
                .insert(peer.to_string(), failure.code.clone());
        }
        result
    }

    fn hello(&self, role: Role, purpose: Purpose) -> Hello {
        // A pairing peer is not trusted yet: it learns no settings.
        let settings = (role == Role::Target && purpose == Purpose::Session).then(|| {
            self.shared
                .store
                .settings()
                .unwrap_or(crate::infrastructure::remote::store::DEFAULT_SETTINGS)
        });
        Hello {
            proto: self.shared.ours(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            now: now_rfc3339(),
            role,
            settings,
            app: settings.map(|_| self.ctx.source.app()),
        }
    }

    // --- Session messages ----------------------------------------------------

    fn on_message(&mut self, key: &Key, message: &[u8]) {
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let agreed = session.proto.unwrap_or(1);
        let Phase::Open(transport) = &mut session.phase else {
            return self.end(
                key,
                End::Close,
                &Failure::new("bad_frame", "A message came before the handshake ended."),
            );
        };
        let plaintext = match transport.decrypt(message) {
            Ok(plaintext) => plaintext,
            Err(_) => {
                // Repeated, missing, reordered or altered: the session
                // cannot go on.
                return self.end(
                    key,
                    End::Close,
                    &Failure::new(
                        "dropped",
                        "A frame was lost, repeated or altered on the way.",
                    ),
                );
            }
        };
        session.messages += 1;
        session.last_heard = Instant::now();
        session.unanswered = None;
        if session.not_responding {
            session.not_responding = false;
            if session.purpose == Purpose::Session && session.role == Role::Viewer {
                self.view().sessions.insert(
                    key.0.clone(),
                    SessionState::Open {
                        not_responding: false,
                    },
                );
            }
        }
        let value: serde_json::Value = match serde_json::from_slice(&plaintext) {
            Ok(value) => value,
            Err(_) => {
                return self.end(
                    key,
                    End::Error("bad_request", "Not JSON.".into()),
                    &bad_request(),
                );
            }
        };
        let kind = value
            .get("t")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        // An argument a control never takes is refused, never dropped.
        let unknown_field = (kind == "control")
            .then(|| unknown_control_field(&value, agreed))
            .flatten();
        let greeted_target = self
            .sessions
            .get(key)
            .is_some_and(|session| session.hello.is_some() && session.role == Role::Target);
        match serde_json::from_value::<SessionMessage>(value.clone()) {
            Ok(SessionMessage::Control(control))
                if greeted_target && control.within_bounds() && unknown_field.is_some() =>
            {
                let field = unknown_field.unwrap_or_default();
                self.refuse_argument(key, &control.id, &field);
            }
            // A type of a later version than the session's is unknown here.
            Ok(message) if message.proto() <= agreed => self.on_session_message(key, message),
            Ok(_) | Err(_) => self.end(
                key,
                End::Error("bad_request", format!("Unknown message {kind}.")),
                &bad_request(),
            ),
        }
    }

    fn on_session_message(&mut self, key: &Key, message: SessionMessage) {
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let role = session.role;
        let greeted = session.hello.is_some()
            || matches!(
                message,
                SessionMessage::Error { .. } | SessionMessage::Bye { .. }
            );
        if !message.within_bounds() {
            return self.end(
                key,
                End::Error("bad_request", "A request is out of bounds.".into()),
                &bad_request(),
            );
        }
        let purpose = session.purpose;
        match message {
            SessionMessage::Hello(hello) if session.hello.is_none() => self.on_hello(key, hello),
            _ if !greeted => self.end(
                key,
                End::Error("bad_request", "hello comes first.".into()),
                &bad_request(),
            ),
            SessionMessage::Hello(_) => self.end(
                key,
                End::Error("bad_request", "hello came twice.".into()),
                &bad_request(),
            ),
            SessionMessage::Keep { ids } if role == Role::Target => {
                session.last_keep = Instant::now();
                self.send_message(key, &SessionMessage::Kept);
                self.renew(key, &ids);
            }
            SessionMessage::Kept if role == Role::Viewer => {}
            message @ (SessionMessage::InventoryPage { .. }
            | SessionMessage::InventoryUnchanged { .. }
            | SessionMessage::OutputData(_))
                if role == Role::Viewer && purpose == Purpose::Session =>
            {
                self.deliver(key, message);
            }
            message @ (SessionMessage::Error { re: Some(_), .. }
            | SessionMessage::AccessResult { .. })
                if role == Role::Viewer && purpose == Purpose::Session =>
            {
                self.deliver(key, message);
            }
            message @ (SessionMessage::ControlProgress { .. }
            | SessionMessage::ControlResult(_))
                if role == Role::Viewer && purpose == Purpose::Session =>
            {
                self.deliver(key, message);
            }
            SessionMessage::Control(control)
                if role == Role::Target && purpose == Purpose::Session =>
            {
                self.on_control(key, *control);
            }
            SessionMessage::ControlStatus { id, request_id }
                if role == Role::Target && purpose == Purpose::Session =>
            {
                self.on_control_status(key, id, request_id);
            }
            SessionMessage::AccessRequest { id, wants }
                if role == Role::Target && purpose == Purpose::Session =>
            {
                self.on_access_request(key, id, wants);
            }
            SessionMessage::AccessCancel { re }
                if role == Role::Target && purpose == Purpose::Session =>
            {
                self.on_access_cancel(key, &re);
            }
            message @ (SessionMessage::InventoryGet { .. }
            | SessionMessage::InventoryWatch { .. }
            | SessionMessage::InventoryUnwatch { .. }
            | SessionMessage::OutputRead { .. }
            | SessionMessage::OutputFollow { .. }
            | SessionMessage::OutputAck { .. }
            | SessionMessage::OutputUnfollow { .. })
                if role == Role::Target && purpose == Purpose::Session =>
            {
                self.on_request(key, message);
            }
            SessionMessage::Bye { reason } => {
                if reason == ByeReason::Unpaired {
                    self.forget_pin(&key.0);
                }
                let code = serde_json::to_value(reason)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| "done".into());
                self.end(
                    key,
                    End::Silent,
                    &Failure::new(&code, "The other machine ended the session."),
                );
            }
            SessionMessage::Error { code, message, .. } => {
                if matches!(code.as_str(), "not_paired" | "key_mismatch") {
                    self.view().failures.insert(key.0.clone(), code.clone());
                }
                self.end(key, End::Silent, &Failure::new(&code, message));
            }
            SessionMessage::PairDeclined { reason, .. } if role == Role::Viewer => {
                let (code, message) = declined(reason);
                self.end(key, End::Silent, &Failure::new(code, message));
            }
            message @ (SessionMessage::PairCommit { .. }
            | SessionMessage::PairNonce { .. }
            | SessionMessage::PairReveal { .. }
            | SessionMessage::PairConfirm { .. }) => self.on_pairing(key, &message),
            _ => self.end(
                key,
                End::Error("bad_request", "Unexpected message.".into()),
                &bad_request(),
            ),
        }
    }

    fn on_hello(&mut self, key: &Key, hello: Hello) {
        let ours = self.shared.ours();
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        // From the two authenticated hellos, never from presence.
        let Some(agreed) = hello.proto.agreed(&ours) else {
            let message = format!(
                "Update Localify: this machine runs {}.",
                env!("CARGO_PKG_VERSION")
            );
            return self.end(
                key,
                End::Error("unsupported", message.clone()),
                &Failure::new("unsupported", message),
            );
        };
        let expected = match session.role {
            Role::Viewer => Role::Target,
            Role::Target => Role::Viewer,
        };
        if hello.role != expected {
            return self.end(
                key,
                End::Error("bad_request", "Wrong role.".into()),
                &bad_request(),
            );
        }
        let settings = hello.settings;
        session.clock =
            time::OffsetDateTime::parse(&hello.now, &time::format_description::well_known::Rfc3339)
                .ok()
                .map(|now| (now, Instant::now()));
        session.hello = Some(hello.clone());
        session.proto = Some(agreed);
        let (role, purpose) = (session.role, session.purpose);
        if !self.ctx.lease.covers(&key.0, Instant::now()) {
            return self.end(key, End::Bye(ByeReason::LeaseExpired), &lease_expired());
        }
        match (role, purpose) {
            (Role::Viewer, Purpose::Session) => {
                let mut view = self.view();
                view.failures.remove(&key.0);
                view.protos.insert(key.0.clone(), agreed);
                if let Some(settings) = settings {
                    view.settings.insert(key.0.clone(), settings);
                }
                drop(view);
                let Some(session) = self.sessions.get_mut(key) else {
                    return;
                };
                let mut hold = !session.taps.is_empty();
                if let Holder::Open { reply, hold: keep } = &mut session.holder {
                    hold |= *keep;
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(Ok(hello));
                    }
                }
                self.send_taps(key);
                if !hold {
                    self.end(
                        key,
                        End::Bye(ByeReason::Done),
                        &Failure::new("done", "Probed."),
                    );
                }
            }
            (Role::Target, Purpose::Pair) => self.start_pairing(key),
            _ => {}
        }
    }

    /// On the target, once both hellos are in: decline, or commit.
    fn start_pairing(&mut self, key: &Key) {
        let now = Instant::now();
        // Kept by the link, so a new connection does not reset the count.
        let mut attempts = self.shared.pair_attempts();
        while attempts
            .front()
            .is_some_and(|at| now.duration_since(*at) >= HOUR)
        {
            attempts.pop_front();
        }
        let oldest = attempts.front().copied();
        let full = attempts.len() >= PAIRINGS_PER_HOUR;
        drop(attempts);
        let decline = |reason, retry_after| SessionMessage::PairDeclined {
            reason,
            retry_after,
        };
        if self.shared.subscribers.load(Ordering::SeqCst) == 0 {
            self.send_message(key, &decline(DeclineReason::NobodyToConfirm, None));
            return self.end(
                key,
                End::Bye(ByeReason::Done),
                &Failure::new("nobody_to_confirm", "Nobody could confirm."),
            );
        }
        let another = self.sessions.iter().any(|(other, session)| {
            other != key
                && session.role == Role::Target
                && session
                    .pairing
                    .as_ref()
                    .is_some_and(|pairing| !pairing.is_finished())
        });
        if another || full {
            let retry_after = match oldest {
                Some(oldest) if !another => HOUR
                    .saturating_sub(now.duration_since(oldest))
                    .as_secs()
                    .max(1),
                _ => 60,
            };
            self.send_message(key, &decline(DeclineReason::Busy, Some(retry_after)));
            return self.end(
                key,
                End::Bye(ByeReason::Done),
                &Failure::new("busy", "Busy."),
            );
        }
        self.shared.pair_attempts().push_back(now);
        let Some(hash) = self.sessions.get(key).and_then(|session| session.hash) else {
            return;
        };
        match Pairing::responder(hash) {
            Ok((pairing, commit)) => {
                if let Some(session) = self.sessions.get_mut(key) {
                    session.pairing = Some(pairing);
                }
                self.send_message(key, &commit);
            }
            Err(error) => self.end(
                key,
                End::Bye(ByeReason::Done),
                &Failure::new("internal", format!("{error:#}")),
            ),
        }
    }

    fn on_pairing(&mut self, key: &Key, message: &SessionMessage) {
        let Some(pairing) = self
            .sessions
            .get_mut(key)
            .and_then(|session| session.pairing.as_mut())
        else {
            return self.end(
                key,
                End::Error("bad_request", "Not a pairing.".into()),
                &bad_request(),
            );
        };
        match pairing.receive(message) {
            Ok(steps) => self.apply(key, steps),
            Err(error) => {
                info!(error = %error, "Pairing abandoned");
                self.end(
                    key,
                    End::Error("bad_request", error.to_string()),
                    &Failure::new("cancelled", error.to_string()),
                );
            }
        }
    }

    fn on_answer(&mut self, pairing_id: &str, accept: bool) -> bool {
        let Some(key) = self
            .sessions
            .keys()
            .find(|key| key.1.encode() == pairing_id)
            .cloned()
        else {
            return false;
        };
        let Some(pairing) = self
            .sessions
            .get_mut(&key)
            .and_then(|session| session.pairing.as_mut())
        else {
            return false;
        };
        match pairing.answer(accept) {
            Ok(steps) => {
                self.apply(&key, steps);
                true
            }
            Err(_) => false,
        }
    }

    fn apply(&mut self, key: &Key, steps: Vec<Step>) {
        for step in steps {
            match step {
                Step::Send(message) => {
                    self.send_message(key, &message);
                }
                Step::ShowCode(code) => self.show_code(key, code),
                Step::Paired => return self.paired(key),
                Step::Cancelled => {
                    return self.end(
                        key,
                        End::Bye(ByeReason::Done),
                        &Failure::new("cancelled", "Pairing cancelled. Nothing was saved."),
                    );
                }
            }
        }
    }

    fn show_code(&mut self, key: &Key, code: String) {
        let name = self.view().name_of(&key.0);
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        session.code_shown = true;
        let pairing_id = key.1.encode();
        match &session.holder {
            Holder::Pair(events) => {
                let _ = events.try_send(PairEvent::Code {
                    pairing_id,
                    machine_id: key.0.clone(),
                    name,
                    code,
                });
            }
            _ => self.shared.emit(Event::PairRequest {
                pairing_id,
                machine_id: key.0.clone(),
                name,
                code,
            }),
        }
    }

    /// Both people accepted: pin the key the handshake gave.
    fn paired(&mut self, key: &Key) {
        let name = self.view().name_of(&key.0);
        let Some(peer_key) = self.sessions.get(key).and_then(|session| session.peer_key) else {
            return;
        };
        let pin = Pin {
            public_key: encode_key(&peer_key),
            name: name.clone(),
            paired_at: now_rfc3339(),
        };
        if let Err(error) = self.shared.store.update_pins(&self.ctx.self_id, |pins| {
            pins.insert(key.0.clone(), pin);
        }) {
            let message = format!("The pin could not be saved: {error:#}");
            return self.end(
                key,
                End::Bye(ByeReason::Done),
                &Failure::new("cancelled", message),
            );
        }
        info!(peer = %key.0, "Paired");
        self.view().failures.remove(&key.0);
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        session.paired = true;
        match &session.holder {
            Holder::Pair(events) => {
                let _ = events.try_send(PairEvent::Paired {
                    machine_id: key.0.clone(),
                    name,
                });
            }
            _ => self.shared.emit(Event::Paired {
                pairing_id: key.1.encode(),
                machine_id: key.0.clone(),
                name,
            }),
        }
        self.end(
            key,
            End::Bye(ByeReason::Done),
            &Failure::new("done", "Paired."),
        );
    }

    fn forget_pin(&mut self, peer: &str) {
        let mut removed = false;
        if let Err(error) = self.shared.store.update_pins(&self.ctx.self_id, |pins| {
            removed = pins.remove(peer).is_some()
        }) {
            warn!(error = %format!("{error:#}"), "Could not delete an unpaired machine's pin");
        }
        self.view().settings.remove(peer);
        self.view().inventories.remove(peer);
        if removed {
            self.shared.emit(Event::Unpaired {
                machine_id: peer.to_string(),
            });
        }
    }

    // --- Commands ------------------------------------------------------------

    fn on_command(&mut self, command: Command) {
        match command {
            Command::Open { peer, hold, reply } => {
                let holder = Holder::Open {
                    reply: Some(reply),
                    hold,
                };
                if let Some(failure) = self.cannot_open(&peer, Purpose::Session) {
                    return notify_failure(holder, &failure);
                }
                let Some(holder) = self.join(&peer, holder) else {
                    return;
                };
                // One session per target: a new one replaces the old.
                self.end_peer_sessions(&peer, Role::Viewer, Purpose::Session);
                self.initiate(&peer, Purpose::Session, holder);
            }
            Command::Tap {
                peer,
                request,
                events,
                reply,
            } => {
                let _ = reply.send(self.on_tap(&peer, request, events));
            }
            Command::Untap { peer, id } => self.untap(&peer, &id),
            Command::Ack { peer, id, offset } => self.ack(&peer, &id, offset),
            Command::Snapshot { result } => self.on_snapshot(result),
            Command::ControlDone { key, re, event } => self.on_control_done(&key, &re, event),
            Command::SettingsChanged { reply } => {
                self.reload_settings();
                let _ = reply.send(());
            }
            Command::AccessAnswer {
                request_id,
                allow,
                suppress,
                reply,
            } => {
                let _ = reply.send(self.on_access_answer(&request_id, allow, suppress));
            }
            #[cfg(test)]
            Command::Raw { peer, message } => {
                if let Some(key) = self.viewer_session(&peer) {
                    self.send_message(&key, &message);
                }
            }
            Command::Pair { peer, events } => {
                let holder = Holder::Pair(events);
                if let Some(failure) = self.cannot_open(&peer, Purpose::Pair) {
                    return notify_failure(holder, &failure);
                }
                self.initiate(&peer, Purpose::Pair, holder);
            }
            Command::Answer {
                pairing_id,
                accept,
                reply,
            } => {
                let _ = reply.send(self.on_answer(&pairing_id, accept));
            }
            Command::Unpair { peer } => {
                let keys: Vec<Key> = self
                    .sessions
                    .keys()
                    .filter(|key| key.0 == peer)
                    .cloned()
                    .collect();
                for key in keys {
                    self.end(
                        &key,
                        End::Bye(ByeReason::Unpaired),
                        &Failure::new("unpaired", "Unpaired."),
                    );
                }
            }
            Command::LeaseReady { result } => self.on_lease_ready(result),
            Command::Revoked => self.failed = Some(Ending::Revoked),
            Command::LeaseRenewed { result } => {
                self.renewing = false;
                if result.as_ref().is_err_and(LeaseError::is_revoked) {
                    self.failed = Some(Ending::Revoked);
                } else if result.is_err() && !self.ctx.lease.is_valid(Instant::now()) {
                    self.end_open(ByeReason::LeaseExpired);
                }
            }
        }
    }

    fn cannot_open(&self, peer: &str, purpose: Purpose) -> Option<Failure> {
        if let Some(at) = self.not_before.get(peer)
            && Instant::now() < *at
        {
            let seconds = at.duration_since(Instant::now()).as_secs().max(1);
            return Some(Failure::new(
                "unavailable",
                format!("The relay dropped a frame; try again in {seconds} s."),
            ));
        }
        if !self.view().presence.contains_key(peer) {
            return Some(Failure::new("peer_offline", "It is offline."));
        }
        if purpose == Purpose::Pair {
            return None;
        }
        let pins = self.shared.store.pins().unwrap_or_default();
        let Some(pin) = pins.get(peer) else {
            return Some(Failure::new(
                "not_paired",
                "This machine is not paired with it.",
            ));
        };
        // The roster never adds trust, but a key there other than the pin
        // means something changed: no session until it is paired again.
        let view = self.view();
        let listed = view.roster.iter().find(|machine| machine.id == peer);
        if let Some(key) = listed.and_then(|machine| machine.remote_key.as_deref())
            && key != pin.public_key
        {
            drop(view);
            self.view()
                .failures
                .insert(peer.to_string(), "key_mismatch".into());
            return Some(Failure::new(
                "key_mismatch",
                "Its key is not the one paired. Don't trust this machine until you pair it again.",
            ));
        }
        None
    }

    fn end_peer_sessions(&mut self, peer: &str, role: Role, purpose: Purpose) {
        let keys: Vec<Key> = self
            .sessions
            .iter()
            .filter(|(key, session)| {
                key.0 == peer && session.role == role && session.purpose == purpose
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            self.end(
                &key,
                End::Bye(ByeReason::Done),
                &Failure::new("replaced", "A newer session replaced it."),
            );
        }
    }

    // --- Timers --------------------------------------------------------------

    fn on_tick(&mut self) {
        let now = Instant::now();
        if now >= self.next_clock_check {
            if self.shared.woke(&self.clocks) {
                info!("This machine woke from sleep; its remote sessions end until a new lease");
                self.ctx.lease.expire();
            }
            self.clocks = Clocks::now();
            self.next_clock_check = now + self.shared.timings.clock_check;
        }
        if self.sessions.values().any(Session::is_open) {
            if !self.ctx.lease.is_valid(now) {
                self.end_open(ByeReason::LeaseExpired);
            } else if self.ctx.lease.needs_renewal(now) && !self.renewing {
                self.renew_lease();
            }
        }
        let keys: Vec<Key> = self.sessions.keys().cloned().collect();
        for key in keys {
            self.check(&key, now);
        }
        self.expire_access(now);
        self.serve_tick(now);
    }

    fn renew_lease(&mut self) {
        self.renewing = true;
        let lease = Arc::clone(&self.ctx.lease);
        let commands = self.commands.clone();
        self.spawn(async move {
            let result = lease.renew().await;
            let _ = commands.send(Command::LeaseRenewed { result });
        });
    }

    /// One session's deadlines.
    fn check(&mut self, key: &Key, now: Instant) {
        let timings = self.shared.timings;
        let subscribers = self.shared.subscribers.load(Ordering::SeqCst);
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let Some(opened) = session.opened else {
            if now.duration_since(session.started) >= timings.handshake_expiry
                && !matches!(session.phase, Phase::AwaitLease(_))
            {
                self.end(
                    key,
                    End::Close,
                    &Failure::new("timeout", "The handshake did not complete in time."),
                );
            }
            return;
        };
        let pairing_open = session
            .pairing
            .as_ref()
            .is_some_and(|pairing| !pairing.is_finished());
        if pairing_open && now.duration_since(session.started) >= timings.pairing_timeout {
            return self.end(
                key,
                End::Bye(ByeReason::Done),
                &Failure::new("timeout", "Pairing cancelled. Nothing was saved."),
            );
        }
        let left = match &session.holder {
            Holder::Pair(events) => events.is_closed(),
            Holder::None if session.role == Role::Target => session.code_shown && subscribers == 0,
            _ => false,
        };
        if pairing_open && left {
            // The person who would answer went away: cancel for them.
            let pairing_id = key.1.encode();
            if !self.on_answer(&pairing_id, false) {
                self.end(
                    key,
                    End::Bye(ByeReason::Done),
                    &Failure::new("cancelled", "Pairing cancelled. Nothing was saved."),
                );
            }
            return;
        }
        if now.duration_since(opened) >= timings.lifetime || session.messages >= MAX_MESSAGES {
            return self.end(
                key,
                End::Bye(ByeReason::Lifetime),
                &Failure::new("lifetime", "The session reached its lifetime."),
            );
        }
        if !session.is_held()
            && now.duration_since(session.last_heard.max(opened)) >= timings.idle_after
        {
            return self.end(
                key,
                End::Bye(ByeReason::Idle),
                &Failure::new("idle", "Nothing used the session."),
            );
        }
        if session.hello.is_some() && !self.ctx.lease.covers(&key.0, now) {
            return self.end(key, End::Bye(ByeReason::LeaseExpired), &lease_expired());
        }
        match session.role {
            Role::Viewer => self.check_viewer(key, now),
            Role::Target => {
                if now.duration_since(session.last_keep) >= timings.target_gives_up_after {
                    return self.end(
                        key,
                        End::Close,
                        &Failure::new("not_responding", "No keep from the viewer."),
                    );
                }
                session.served.expire(now, timings.watch_expiry);
            }
        }
    }

    /// The heartbeat: `keep` every 30 s, `not_responding` 15 s after one
    /// that got no answer, the end after 60 s without one.
    fn check_viewer(&mut self, key: &Key, now: Instant) {
        let timings = self.shared.timings;
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        if now.duration_since(session.last_heard) >= timings.viewer_gives_up_after {
            return self.end(
                key,
                End::Close,
                &Failure::new("not_responding", "It stopped answering."),
            );
        }
        if let Some(since) = session.unanswered
            && now.duration_since(since) >= timings.not_responding_after
            && !session.not_responding
        {
            session.not_responding = true;
            if session.purpose == Purpose::Session {
                self.view().sessions.insert(
                    key.0.clone(),
                    SessionState::Open {
                        not_responding: true,
                    },
                );
            }
        }
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let due = session.keep_sent.unwrap_or(session.opened.unwrap_or(now));
        if session.hello.is_some() && now.duration_since(due) >= timings.keep_every {
            session.keep_sent = Some(now);
            session.unanswered.get_or_insert(now);
            // Renews the watches and follows on the target.
            let ids = viewer::kept_ids(&session.taps);
            self.send_message(key, &SessionMessage::Keep { ids });
        }
    }

    // --- Ending --------------------------------------------------------------

    fn end(&mut self, key: &Key, how: End, failure: &Failure) {
        match how {
            End::Silent => {}
            End::Close => self.send_frame(&key.0, key.1, FrameKind::Close, None, None),
            End::Bye(reason) => {
                if !self.send_message(key, &SessionMessage::Bye { reason }) {
                    self.send_frame(&key.0, key.1, FrameKind::Close, None, None);
                }
            }
            End::Error(code, message) => {
                self.send_message(key, &SessionMessage::error(code, message));
                self.send_message(
                    key,
                    &SessionMessage::Bye {
                        reason: ByeReason::Done,
                    },
                );
            }
        }
        let Some(mut session) = self.sessions.remove(key) else {
            return;
        };
        debug!(peer = %key.0, code = %failure.code, "Session ended");
        for (_, tap) in session.taps.drain() {
            tap.end(failure);
        }
        if session.role == Role::Viewer && session.purpose == Purpose::Session {
            let mut view = self.view();
            let current = !self.sessions.iter().any(|(other, s)| {
                other.0 == key.0 && s.role == Role::Viewer && s.purpose == Purpose::Session
            });
            if current {
                view.sessions.remove(&key.0);
                view.protos.remove(&key.0);
            }
        }
        if let Some(waiting) = session.access.take() {
            // The session is gone: the viewer hears nothing more.
            self.access_closed(&waiting.request_id, AccessOutcome::Cancelled);
        }
        if session.paired {
            return;
        }
        if session.role == Role::Target && session.code_shown {
            self.shared.emit(Event::PairCancelled {
                pairing_id: key.1.encode(),
                machine_id: key.0.clone(),
                reason: failure.code.clone(),
            });
        }
        notify_failure(session.holder, failure);
    }

    fn end_peer(&mut self, peer: &str, how: End, failure: &Failure) {
        let keys: Vec<Key> = self
            .sessions
            .keys()
            .filter(|key| key.0 == peer)
            .cloned()
            .collect();
        for key in keys {
            let how = match &how {
                End::Silent => End::Silent,
                End::Close => End::Close,
                End::Bye(reason) => End::Bye(*reason),
                End::Error(code, message) => End::Error(code, message.clone()),
            };
            self.end(&key, how, failure);
        }
    }

    /// Every open session, with `bye`.
    fn end_open(&mut self, reason: ByeReason) {
        let keys: Vec<Key> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.is_open())
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            self.end(&key, End::Bye(reason), &lease_expired());
        }
    }

    fn end_all(&mut self, failure: &Failure) {
        let keys: Vec<Key> = self.sessions.keys().cloned().collect();
        for key in keys {
            self.end(&key, End::Silent, failure);
        }
    }
}

fn notify_failure(holder: Holder, failure: &Failure) {
    match holder {
        Holder::None => {}
        Holder::Open { reply, .. } => {
            if let Some(reply) = reply {
                let _ = reply.send(Err(failure.clone()));
            }
        }
        Holder::Pair(events) => {
            let message = if failure.message.is_empty() {
                "Pairing cancelled. Nothing was saved.".to_string()
            } else {
                failure.message.clone()
            };
            let _ = events.try_send(PairEvent::Cancelled {
                reason: failure.code.clone(),
                message,
            });
        }
    }
}

fn bad_request() -> Failure {
    Failure::new(
        "bad_request",
        "The other machine sent a message out of order.",
    )
}

fn lease_expired() -> Failure {
    Failure::new("lease_expired", "Remote access is not available right now.")
}

fn declined(reason: DeclineReason) -> (&'static str, &'static str) {
    match reason {
        DeclineReason::NobodyToConfirm => (
            "nobody_to_confirm",
            "Nobody can confirm on that machine. Open Localify on it, or run `localify machines pair --accept` there, then try again.",
        ),
        DeclineReason::Busy => (
            "busy",
            "That machine is busy with another pairing. Try again later.",
        ),
        DeclineReason::Cancelled => ("cancelled", "Pairing cancelled. Nothing was saved."),
    }
}
