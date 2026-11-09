//! Viewing another machine (`docs/remote-machines.md`, "Daemon"): what
//! `remote_inventory`, `remote_watch`, `remote_output_read`,
//! `remote_output_follow`, `remote_access` and `remote_control` ask a target
//! for, over the actor's one session to it. Everything that came from the
//! target is shown as plain text: control characters and bidirectional
//! overrides are removed here again, whatever the target sent.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::actor::WINDOW;
use super::{Command, Failure, RemoteLink, TapEvent, TapRequest, lease_refused};
use crate::infrastructure::launch_rev::random_bytes;
use crate::infrastructure::remote::wire::{
    AccessOutcome, AccessWants, ControlAction, ControlExpect, ControlOutcome, ControlPhase,
    ControlService, Have, MachineSummary, OutputData, SessionMessage,
};
use crate::infrastructure::runs::output::plain_line;

/// Answers a request may have waiting for its reader: every page of a
/// snapshot, or a window of short output messages.
const TAP_EVENTS: usize = 256;
/// How long a one-shot request waits for its answer.
const ANSWER_WAIT: Duration = Duration::from_secs(30);
/// A follow acknowledges what its client showed once this much is unacked.
const ACK_EVERY: u64 = WINDOW as u64 / 4;
/// How long a control waits for its result before it is `unknown`.
pub const CONTROL_WAIT: Duration = Duration::from_secs(120);
/// How long an `unknown` control keeps asking `control.status`.
const STATUS_WAIT: Duration = Duration::from_secs(60);

/// A target's whole inventory: every page of one revision.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInventory {
    pub machine: MachineSummary,
    pub epoch: String,
    pub rev: u64,
    pub truncated: bool,
    pub items: Vec<Value>,
}

impl RemoteInventory {
    fn have(&self) -> Have {
        Have {
            epoch: self.epoch.clone(),
            rev: self.rev,
        }
    }
}

/// What `remote_watch` streams.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WatchEvent {
    /// Opening a session to the target, or asking it again.
    Connecting,
    Snapshot(RemoteInventory),
    /// The target can't be watched now; asked again in `retry_in` seconds.
    Lost {
        code: String,
        message: String,
        retry_in: u64,
    },
}

/// What `remote_output_follow` streams: lines, then how it ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FollowEvent {
    Data(OutputData),
    /// The follow ended before its run did: the session was lost, the run
    /// restarted (`gone`), sharing was turned off, or the target refused.
    Ended {
        code: String,
        message: String,
    },
}

/// What `remote_access` streams: `waiting`, then one end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AccessEvent {
    /// The request is on its way to the target, or waits for its answer.
    Waiting,
    /// The target's `access.result`.
    Result { outcome: AccessOutcome },
    /// The target refused it: `not_shared`, `busy`, `too_many`, or
    /// `unsupported` for a target of protocol 1.
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
    /// The session ended before an answer, or never opened.
    Lost { code: String, message: String },
}

/// What `remote_control` is asked for: a control, as the viewer saw the
/// service.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlAsk {
    pub action: ControlAction,
    pub service: ControlService,
    #[serde(default)]
    pub profile: Option<String>,
    pub expect: ControlExpect,
}

/// What `remote_control` streams: `sent`, its progress, then its result.
/// A session lost before the result is `unknown`, and the result comes from
/// `control.status` on a new session when it can be had, or `lost` says it
/// could not. A control is never sent again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ControlEvent {
    /// On its way, with the request id that names it on the target.
    Sent {
        request_id: String,
    },
    Progress {
        phase: ControlPhase,
    },
    Result {
        outcome: ControlOutcome,
        code: Option<String>,
        message: String,
        run_id: Option<String>,
    },
    /// No result came: the session was lost, or 120 s passed. Checking.
    Unknown {
        code: String,
        message: String,
    },
    /// The outcome could not be learned; the next snapshot shows the state.
    Lost {
        code: String,
        message: String,
    },
}

impl ControlEvent {
    fn is_last(&self) -> bool {
        matches!(self, Self::Result { .. } | Self::Lost { .. })
    }
}

/// An access request this machine waits on. Dropping it sends
/// `access.cancel` when the target still waits.
pub struct Asking {
    link: RemoteLink,
    peer: String,
    id: String,
    events: mpsc::Receiver<TapEvent>,
    announced: bool,
    ended: bool,
}

impl Asking {
    /// `waiting` first, then how it ended; `None` after that.
    pub async fn next(&mut self) -> Option<AccessEvent> {
        if !self.announced {
            self.announced = true;
            return Some(AccessEvent::Waiting);
        }
        if self.ended {
            return None;
        }
        self.ended = true;
        let event = match self.events.recv().await {
            Some(TapEvent::Answer(SessionMessage::AccessResult { outcome, .. })) => {
                AccessEvent::Result { outcome }
            }
            Some(TapEvent::Answer(SessionMessage::Error {
                code,
                message,
                retry_after,
                ..
            })) => AccessEvent::Error {
                code,
                message: plain_line(&message),
                retry_after,
            },
            Some(TapEvent::Answer(answer)) => lost(answered_with(answer)),
            Some(TapEvent::Ended(failure)) => lost(failure),
            None => lost(closed()),
        };
        Some(event)
    }
}

impl Drop for Asking {
    fn drop(&mut self) {
        self.link.untap(&self.peer, &self.id);
    }
}

fn lost(failure: Failure) -> AccessEvent {
    AccessEvent::Lost {
        code: failure.code,
        message: plain_line(&failure.message),
    }
}

/// What a target's answer to a control says, as plain text.
fn control_answer(answer: SessionMessage) -> ControlEvent {
    match answer {
        SessionMessage::ControlProgress { phase, .. } => ControlEvent::Progress { phase },
        SessionMessage::ControlResult(result) => ControlEvent::Result {
            outcome: result.outcome,
            code: result.code.map(|code| plain_line(&code)),
            message: plain_line(&result.message),
            run_id: result.run_id.map(|run_id| plain_line(&run_id)),
        },
        // A refusal before the executor, `unsupported` among them.
        other => {
            let failure = answered_with(other);
            let outcome = if failure.code == "conflict" {
                ControlOutcome::Conflict
            } else {
                ControlOutcome::Refused
            };
            ControlEvent::Result {
                outcome,
                code: Some(failure.code),
                message: plain_line(&failure.message),
                run_id: None,
            }
        }
    }
}

fn lost_control(failure: Failure) -> ControlEvent {
    ControlEvent::Lost {
        code: failure.code,
        message: plain_line(&failure.message),
    }
}

/// 16 random bytes, base64url: names one control across sessions.
fn new_request_id() -> Result<String, Failure> {
    use base64::Engine;
    let mut bytes = [0_u8; 16];
    random_bytes(&mut bytes).map_err(|error| Failure::new("internal", format!("{error:#}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Pages of one revision, until they are all in. Pages of an older revision
/// are dropped; a newer one starts over.
#[derive(Default)]
struct Pages {
    epoch: String,
    rev: u64,
    truncated: bool,
    machine: Option<MachineSummary>,
    pages: Vec<Option<Vec<Value>>>,
}

impl Pages {
    fn add(&mut self, page: SessionMessage) -> Option<RemoteInventory> {
        let SessionMessage::InventoryPage {
            epoch,
            rev,
            page,
            pages,
            machine,
            items,
            truncated,
            ..
        } = page
        else {
            return None;
        };
        let started = !self.pages.is_empty();
        if !(started && self.epoch == epoch && self.rev == rev) {
            if started && self.epoch == epoch && rev < self.rev {
                return None;
            }
            let count = usize::try_from(pages).unwrap_or(1).clamp(1, 32);
            *self = Self {
                epoch,
                rev,
                truncated,
                machine: None,
                pages: vec![None; count],
            };
        }
        let index = usize::try_from(page).ok()?.checked_sub(1)?;
        *self.pages.get_mut(index)? = Some(items);
        self.machine = Some(machine);
        if self.pages.iter().any(Option::is_none) {
            return None;
        }
        let done = std::mem::take(self);
        let mut inventory = RemoteInventory {
            machine: done.machine?,
            epoch: done.epoch,
            rev: done.rev,
            truncated: done.truncated,
            items: done.pages.into_iter().flatten().flatten().collect(),
        };
        clean_inventory(&mut inventory);
        Some(inventory)
    }
}

/// Every string as plain text.
fn clean(value: &mut Value) {
    match value {
        Value::String(text) => *text = plain_line(text),
        Value::Array(items) => items.iter_mut().for_each(clean),
        Value::Object(fields) => fields.values_mut().for_each(clean),
        _ => {}
    }
}

fn clean_inventory(inventory: &mut RemoteInventory) {
    inventory.items.iter_mut().for_each(clean);
    let machine = &mut inventory.machine;
    machine.name = plain_line(&machine.name);
    machine.version = plain_line(&machine.version);
}

fn clean_output(data: &mut OutputData) {
    for line in &mut data.lines {
        line.text = plain_line(&line.text);
    }
}

fn answered_with(answer: SessionMessage) -> Failure {
    match answer {
        SessionMessage::Error { code, message, .. } => Failure::new(&code, message),
        _ => Failure::new("bad_request", "The other machine answered out of order."),
    }
}

fn closed() -> Failure {
    Failure::new("unavailable", "The relay connection closed.")
}

/// How long a lost watch waits before it asks again.
fn retry_in(failure: &Failure, failures: u32) -> Duration {
    match failure.code.as_str() {
        // Its owner may turn sharing on: no hurry.
        "not_shared" => Duration::from_secs(10),
        _ => Duration::from_secs(2_u64.saturating_mul(1 << failures.min(4)))
            .min(Duration::from_secs(30)),
    }
}

/// An open follow. Dropping it unfollows.
pub struct Following {
    link: RemoteLink,
    peer: String,
    id: String,
    events: mpsc::Receiver<TapEvent>,
    /// Started inside the run's output: its first line is a piece.
    skip_piece: bool,
    acked: u64,
}

impl Following {
    /// The next lines, or how the follow ended; `None` once it is over.
    pub async fn next(&mut self) -> Option<FollowEvent> {
        match self.events.recv().await? {
            TapEvent::Answer(SessionMessage::OutputData(mut data)) => {
                if self.skip_piece {
                    self.skip_piece = false;
                    if data.offset > 0 {
                        let piece = data.lines.iter().position(|line| !line.cut);
                        let skip = piece.map_or(data.lines.len(), |index| index + 1);
                        data.lines.drain(..skip);
                    }
                }
                if self.acked == 0 {
                    self.acked = data.offset;
                }
                clean_output(&mut data);
                Some(FollowEvent::Data(data))
            }
            TapEvent::Answer(answer) => {
                let failure = answered_with(answer);
                Some(FollowEvent::Ended {
                    code: failure.code,
                    message: failure.message,
                })
            }
            TapEvent::Ended(failure) => Some(FollowEvent::Ended {
                code: failure.code,
                message: failure.message,
            }),
        }
    }

    /// The client showed the output up to `next`: acknowledged once enough
    /// is, so the target sends on.
    pub fn shown(&mut self, next: u64) {
        if next.saturating_sub(self.acked) >= ACK_EVERY {
            self.acked = next;
            self.link.ack(&self.peer, &self.id, next);
        }
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        self.link.untap(&self.peer, &self.id);
    }
}

impl RemoteLink {
    /// Carries one request to `peer`; its answers come on the receiver.
    async fn tap(
        &self,
        peer: &str,
        request: TapRequest,
    ) -> Result<(String, mpsc::Receiver<TapEvent>), Failure> {
        let conn = self
            .connected()
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        if let Err(error) = conn.lease.ensure().await {
            return Err(lease_refused(&conn, &error));
        }
        let (events, receiver) = mpsc::channel(TAP_EVENTS);
        let (reply, answer) = oneshot::channel();
        let command = Command::Tap {
            peer: peer.to_string(),
            request,
            events,
            reply,
        };
        conn.commands.send(command).map_err(|_| closed())?;
        let id = answer.await.map_err(|_| closed())??;
        Ok((id, receiver))
    }

    fn untap(&self, peer: &str, id: &str) {
        if let Some(conn) = self.shared.conn() {
            let _ = conn.commands.send(Command::Untap {
                peer: peer.to_string(),
                id: id.to_string(),
            });
        }
    }

    fn ack(&self, peer: &str, id: &str, offset: u64) {
        if let Some(conn) = self.shared.conn() {
            let _ = conn.commands.send(Command::Ack {
                peer: peer.to_string(),
                id: id.to_string(),
                offset,
            });
        }
    }

    fn cached(&self, peer: &str) -> Option<RemoteInventory> {
        self.shared.view().inventories.get(peer).cloned()
    }

    fn keep(&self, peer: &str, inventory: &RemoteInventory) {
        self.shared
            .view()
            .inventories
            .insert(peer.to_string(), inventory.clone());
    }

    /// `remote_inventory`: one snapshot, every page.
    pub async fn inventory(&self, peer: &str) -> Result<RemoteInventory, Failure> {
        let cached = self.cached(peer);
        let have = cached.as_ref().map(RemoteInventory::have);
        let (id, mut events) = self
            .tap(peer, TapRequest::Inventory { watch: false, have })
            .await?;
        let mut pages = Pages::default();
        let answer = tokio::time::timeout(ANSWER_WAIT, async {
            loop {
                match events.recv().await {
                    Some(TapEvent::Answer(page @ SessionMessage::InventoryPage { .. })) => {
                        if let Some(inventory) = pages.add(page) {
                            self.keep(peer, &inventory);
                            return Ok(inventory);
                        }
                    }
                    Some(TapEvent::Answer(SessionMessage::InventoryUnchanged { .. })) => {
                        return cached.clone().ok_or_else(|| {
                            Failure::new("bad_request", "The other machine sent no snapshot.")
                        });
                    }
                    Some(TapEvent::Answer(answer)) => return Err(answered_with(answer)),
                    Some(TapEvent::Ended(failure)) => return Err(failure),
                    None => return Err(closed()),
                }
            }
        })
        .await;
        self.untap(peer, &id);
        answer.unwrap_or_else(|_| Err(Failure::new("timeout", "The machine did not answer.")))
    }

    /// `remote_watch`: a snapshot each time the target's changes, and what
    /// happened when it can't be watched, asked again until the receiver
    /// is dropped.
    pub fn watch(&self, peer: &str) -> mpsc::Receiver<WatchEvent> {
        let (sender, receiver) = mpsc::channel(8);
        let link = self.clone();
        let peer = peer.to_string();
        tokio::spawn(async move {
            let mut failures = 0_u32;
            loop {
                if sender.send(WatchEvent::Connecting).await.is_err() {
                    return;
                }
                let failure = link.watch_once(&peer, &sender, &mut failures).await;
                let Some(failure) = failure else { return };
                let wait = retry_in(&failure, failures);
                failures = failures.saturating_add(1);
                let lost = WatchEvent::Lost {
                    code: failure.code,
                    message: failure.message,
                    retry_in: wait.as_secs(),
                };
                if sender.send(lost).await.is_err() {
                    return;
                }
                tokio::select! {
                    _ = sender.closed() => return,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        });
        receiver
    }

    /// One watch, until it is lost (the failure) or its client is gone
    /// (`None`).
    async fn watch_once(
        &self,
        peer: &str,
        sender: &mpsc::Sender<WatchEvent>,
        failures: &mut u32,
    ) -> Option<Failure> {
        let cached = self.cached(peer);
        let have = cached.as_ref().map(RemoteInventory::have);
        let (id, mut events) = match self
            .tap(peer, TapRequest::Inventory { watch: true, have })
            .await
        {
            Ok(tap) => tap,
            Err(failure) => return Some(failure),
        };
        let mut pages = Pages::default();
        let failure = loop {
            let event = tokio::select! {
                _ = sender.closed() => break None,
                event = events.recv() => event,
            };
            let snapshot = match event {
                Some(TapEvent::Answer(page @ SessionMessage::InventoryPage { .. })) => {
                    let Some(inventory) = pages.add(page) else {
                        continue;
                    };
                    self.keep(peer, &inventory);
                    inventory
                }
                Some(TapEvent::Answer(SessionMessage::InventoryUnchanged { .. })) => {
                    match cached.clone() {
                        Some(inventory) => inventory,
                        None => continue,
                    }
                }
                Some(TapEvent::Answer(answer)) => break Some(answered_with(answer)),
                Some(TapEvent::Ended(failure)) => break Some(failure),
                None => break Some(closed()),
            };
            *failures = 0;
            if sender.send(WatchEvent::Snapshot(snapshot)).await.is_err() {
                break None;
            }
        };
        self.untap(peer, &id);
        failure
    }

    /// `remote_output_follow`: from `offset`, or with no offset, from the
    /// last `tail` bytes.
    pub async fn follow(
        &self,
        peer: &str,
        run_id: &str,
        offset: Option<u64>,
        tail: u64,
    ) -> Result<Following, Failure> {
        let request = TapRequest::Follow {
            run_id: run_id.to_string(),
            offset: offset.unwrap_or(0),
            tail: offset.is_none().then_some(tail),
        };
        let (id, events) = self.tap(peer, request).await?;
        Ok(Following {
            link: self.clone(),
            peer: peer.to_string(),
            id,
            events,
            skip_piece: offset.is_none(),
            acked: 0,
        })
    }

    /// `remote_access`: asks `peer` to turn on `wants`. A person there
    /// decides; this waits for the answer until the returned request is
    /// dropped, which cancels it.
    pub async fn access(&self, peer: &str, wants: AccessWants) -> Result<Asking, Failure> {
        let (id, events) = self.tap(peer, TapRequest::Access { wants }).await?;
        Ok(Asking {
            link: self.clone(),
            peer: peer.to_string(),
            id,
            events,
            announced: false,
            ended: false,
        })
    }

    /// `remote_control`: sends the control once, then streams its progress
    /// and result. When the session is lost first, or no result comes in
    /// [`CONTROL_WAIT`], it says `unknown` and asks `control.status` on a
    /// new session, never sending the control again.
    pub fn control(&self, peer: &str, ask: ControlAsk) -> mpsc::Receiver<ControlEvent> {
        match new_request_id() {
            Ok(request_id) => self.control_as(peer, ask, request_id),
            Err(failure) => {
                let (sender, receiver) = mpsc::channel(1);
                let _ = sender.try_send(lost_control(failure));
                receiver
            }
        }
    }

    /// [`RemoteLink::control`] under a given request id, as a viewer that
    /// sends the same control again would.
    pub(super) fn control_as(
        &self,
        peer: &str,
        ask: ControlAsk,
        request_id: String,
    ) -> mpsc::Receiver<ControlEvent> {
        let (sender, receiver) = mpsc::channel(8);
        let link = self.clone();
        let peer = peer.to_string();
        tokio::spawn(async move {
            let lost = link.control_once(&peer, ask, &request_id, &sender).await;
            let Some(failure) = lost else { return };
            let unknown = ControlEvent::Unknown {
                code: failure.code,
                message: plain_line(&failure.message),
            };
            if sender.send(unknown).await.is_err() {
                return;
            }
            let ended = link.control_status(&peer, &request_id).await;
            let _ = sender.send(ended).await;
        });
        receiver
    }

    /// Sends the control and streams what the target says, until its
    /// result. Returns the failure when no result came.
    async fn control_once(
        &self,
        peer: &str,
        ask: ControlAsk,
        request_id: &str,
        sender: &mpsc::Sender<ControlEvent>,
    ) -> Option<Failure> {
        let request = TapRequest::Control {
            action: ask.action,
            service: ask.service,
            profile: ask.profile,
            request_id: request_id.to_string(),
            expect: ask.expect,
        };
        let (id, mut events) = match self.tap(peer, request).await {
            Ok(tap) => tap,
            // Never sent: nothing to reconcile.
            Err(failure) => {
                let _ = sender.send(lost_control(failure)).await;
                return None;
            }
        };
        let sent = ControlEvent::Sent {
            request_id: request_id.to_string(),
        };
        if sender.send(sent).await.is_err() {
            self.untap(peer, &id);
            return None;
        }
        let deadline = tokio::time::Instant::now() + CONTROL_WAIT;
        let failure = loop {
            let event = tokio::select! {
                _ = sender.closed() => break None,
                event = tokio::time::timeout_at(deadline, events.recv()) => event,
            };
            let event = match event {
                Err(_) => break Some(Failure::new("timeout", "No result came in 120 seconds.")),
                Ok(None) => break Some(closed()),
                Ok(Some(TapEvent::Ended(failure))) => break Some(failure),
                Ok(Some(TapEvent::Answer(answer))) => control_answer(answer),
            };
            let last = event.is_last();
            if sender.send(event).await.is_err() || last {
                break None;
            }
        };
        self.untap(peer, &id);
        failure
    }

    /// `control.status` for `request_id`, on a session of its own, tried
    /// again while the target cannot be reached, for [`STATUS_WAIT`] at most.
    pub(super) async fn control_status(&self, peer: &str, request_id: &str) -> ControlEvent {
        let deadline = tokio::time::Instant::now() + STATUS_WAIT;
        let mut wait = Duration::from_millis(500);
        loop {
            let failure = match self.status_once(peer, request_id).await {
                Ok(event) => return event,
                Err(failure) => failure,
            };
            if tokio::time::Instant::now() + wait >= deadline {
                return lost_control(failure);
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(8));
        }
    }

    async fn status_once(&self, peer: &str, request_id: &str) -> Result<ControlEvent, Failure> {
        let request = TapRequest::ControlStatus {
            request_id: request_id.to_string(),
        };
        let (id, mut events) = self.tap(peer, request).await?;
        let answer = tokio::time::timeout(ANSWER_WAIT, events.recv()).await;
        self.untap(peer, &id);
        match answer {
            Ok(Some(TapEvent::Answer(answer))) => Ok(control_answer(answer)),
            Ok(Some(TapEvent::Ended(failure))) => Err(failure),
            Ok(None) => Err(closed()),
            Err(_) => Err(Failure::new("timeout", "The machine did not answer.")),
        }
    }

    /// `remote_output_read`: one bounded read.
    pub async fn read_output(
        &self,
        peer: &str,
        run_id: &str,
        offset: u64,
        max_bytes: u32,
    ) -> Result<OutputData, Failure> {
        let request = TapRequest::Read {
            run_id: run_id.to_string(),
            offset,
            max_bytes,
        };
        let (id, mut events) = self.tap(peer, request).await?;
        let answer = tokio::time::timeout(ANSWER_WAIT, events.recv()).await;
        self.untap(peer, &id);
        match answer {
            Ok(Some(TapEvent::Answer(SessionMessage::OutputData(mut data)))) => {
                clean_output(&mut data);
                Ok(data)
            }
            Ok(Some(TapEvent::Answer(answer))) => Err(answered_with(answer)),
            Ok(Some(TapEvent::Ended(failure))) => Err(failure),
            Ok(None) => Err(closed()),
            Err(_) => Err(Failure::new("timeout", "The machine did not answer.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::remote::wire::{AppPresence, TargetSettings};
    use serde_json::json;

    fn summary() -> MachineSummary {
        MachineSummary {
            machine_id: "mB".into(),
            name: "teo\u{202e}-mini".into(),
            version: "1.5.0".into(),
            os: "macos".into(),
            settings: TargetSettings {
                share: true,
                control: false,
                background_starts: false,
            },
            app: AppPresence::Absent,
        }
    }

    fn page(rev: u64, page: u32, pages: u32, item: &str) -> SessionMessage {
        SessionMessage::InventoryPage {
            re: "r1".into(),
            epoch: "e".into(),
            rev,
            page,
            pages,
            machine: summary(),
            items: vec![json!({ "service": { "name": item } })],
            truncated: false,
        }
    }

    #[test]
    fn a_snapshot_is_whole_once_every_page_of_one_revision_is_in() {
        let mut pages = Pages::default();
        assert!(pages.add(page(2, 2, 2, "web")).is_none());
        // An older revision is dropped.
        assert!(pages.add(page(1, 1, 2, "old")).is_none());
        let inventory = pages.add(page(2, 1, 2, "api")).unwrap();

        assert_eq!(inventory.rev, 2);
        let names: Vec<_> = inventory
            .items
            .iter()
            .map(|item| item["service"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["api", "web"]);
        // Untrusted text loses its bidirectional override.
        assert_eq!(inventory.machine.name, "teo-mini");
    }

    #[test]
    fn a_newer_revision_starts_over() {
        let mut pages = Pages::default();
        assert!(pages.add(page(1, 1, 2, "old")).is_none());
        let inventory = pages.add(page(2, 1, 1, "new")).unwrap();

        assert_eq!(inventory.rev, 2);
        assert_eq!(inventory.items.len(), 1);
    }

    #[test]
    fn strings_from_another_machine_become_plain_text() {
        let mut value = json!({ "name": "\u{1b}[31mweb\u{1b}[0m\u{7}", "list": ["a\u{2066}b"] });
        clean(&mut value);
        assert_eq!(value, json!({ "name": "web", "list": ["ab"] }));
    }

    #[test]
    fn a_lost_watch_backs_off_and_waits_longer_when_not_shared() {
        let lost = Failure::new("peer_offline", "Offline.");
        assert_eq!(retry_in(&lost, 0), Duration::from_secs(2));
        assert_eq!(retry_in(&lost, 9), Duration::from_secs(30));
        let closed = Failure::new("not_shared", "Off.");
        assert_eq!(retry_in(&closed, 0), Duration::from_secs(10));
    }
}
