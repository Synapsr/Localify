//! Viewing another machine (`docs/remote-machines.md`, "Daemon"): what
//! `remote_inventory`, `remote_watch`, `remote_output_read` and
//! `remote_output_follow` ask a target for, over the actor's one session to
//! it. Everything that came from the target is shown as plain text: control
//! characters and bidirectional overrides are removed here again, whatever
//! the target sent.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::actor::WINDOW;
use super::{Command, Failure, RemoteLink, TapEvent, TapRequest, lease_refused};
use crate::infrastructure::remote::wire::{Have, MachineSummary, OutputData, SessionMessage};
use crate::infrastructure::runs::output::plain_line;

/// Answers a request may have waiting for its reader: every page of a
/// snapshot, or a window of short output messages.
const TAP_EVENTS: usize = 256;
/// How long a one-shot request waits for its answer.
const ANSWER_WAIT: Duration = Duration::from_secs(30);
/// A follow acknowledges what its client showed once this much is unacked.
const ACK_EVERY: u64 = WINDOW as u64 / 4;

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
