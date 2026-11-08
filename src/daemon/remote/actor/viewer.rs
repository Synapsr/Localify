//! The viewer's side of a session (`docs/remote-machines.md`, "Daemon"):
//! the management socket's requests, carried to a target over one session
//! per target, and the answers routed back to them by `re`. The session
//! opens with the first request and closes with the last one.

use std::collections::HashMap;

use tokio::sync::mpsc;

use super::{Actor, End, Failure, Holder, Key};
use crate::infrastructure::remote::wire::{ByeReason, Have, Purpose, Role, SessionMessage};

/// The window a viewer gives each follow.
pub const WINDOW: u32 = 64 * 1024;

/// What a management socket client asked a target for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapRequest {
    Inventory {
        watch: bool,
        have: Option<Have>,
    },
    Follow {
        run_id: String,
        offset: u64,
        tail: Option<u64>,
    },
    Read {
        run_id: String,
        offset: u64,
        max_bytes: u32,
    },
}

#[derive(Debug)]
pub enum TapEvent {
    /// The target's answer: pages, `unchanged`, output, or an error.
    Answer(SessionMessage),
    /// The session ended, and the request with it.
    Ended(Failure),
}

/// One request carried over a session, until it ends.
pub struct Tap {
    request: TapRequest,
    events: mpsc::Sender<TapEvent>,
    sent: bool,
}

impl Tap {
    pub(super) fn end(self, failure: &Failure) {
        let _ = self.events.try_send(TapEvent::Ended(failure.clone()));
    }

    /// Watches and follows live as long as `keep` names them.
    fn renewed(&self) -> bool {
        self.sent
            && matches!(
                self.request,
                TapRequest::Inventory { watch: true, .. } | TapRequest::Follow { .. }
            )
    }

    fn message(&self, id: &str) -> SessionMessage {
        let id = id.to_string();
        match self.request.clone() {
            TapRequest::Inventory { watch: true, have } => {
                SessionMessage::InventoryWatch { id, have }
            }
            TapRequest::Inventory { watch: false, have } => {
                SessionMessage::InventoryGet { id, have }
            }
            TapRequest::Follow {
                run_id,
                offset,
                tail,
            } => SessionMessage::OutputFollow {
                id,
                run_id,
                offset,
                window: WINDOW,
                tail,
            },
            TapRequest::Read {
                run_id,
                offset,
                max_bytes,
            } => SessionMessage::OutputRead {
                id,
                run_id,
                offset,
                max_bytes,
            },
        }
    }

    /// What tells the target this request is over, if anything.
    fn cancel(&self, id: &str) -> Option<SessionMessage> {
        match self.request {
            TapRequest::Inventory { watch: true, .. } => {
                Some(SessionMessage::InventoryUnwatch { id: id.to_string() })
            }
            TapRequest::Follow { .. } => {
                Some(SessionMessage::OutputUnfollow { re: id.to_string() })
            }
            _ => None,
        }
    }

    /// Whether `answer` is the last one this request gets.
    fn finished_by(&self, answer: &SessionMessage) -> bool {
        match (answer, &self.request) {
            (SessionMessage::Error { .. }, _) => true,
            (
                SessionMessage::InventoryPage { page, pages, .. },
                TapRequest::Inventory { watch: false, .. },
            ) => page >= pages,
            (SessionMessage::InventoryUnchanged { .. }, TapRequest::Inventory { watch, .. }) => {
                !watch
            }
            (SessionMessage::OutputData(data), TapRequest::Follow { .. }) => data.ended,
            (SessionMessage::OutputData(_), TapRequest::Read { .. }) => true,
            _ => false,
        }
    }
}

/// The ids a `keep` renews on the target.
pub(super) fn kept_ids(taps: &HashMap<String, Tap>) -> Vec<String> {
    let mut ids: Vec<String> = taps
        .iter()
        .filter(|(_, tap)| tap.renewed())
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids.truncate(16);
    ids
}

/// The request an answer is for.
fn answer_to(answer: &SessionMessage) -> Option<&str> {
    match answer {
        SessionMessage::InventoryPage { re, .. }
        | SessionMessage::InventoryUnchanged { re, .. } => Some(re),
        SessionMessage::OutputData(data) => Some(&data.re),
        SessionMessage::Error { re: Some(re), .. } => Some(re),
        _ => None,
    }
}

impl Actor {
    /// This machine's session as a viewer of `peer`, open or opening.
    pub(super) fn viewer_session(&self, peer: &str) -> Option<Key> {
        self.sessions
            .iter()
            .find(|(key, session)| {
                key.0 == peer && session.role == Role::Viewer && session.purpose == Purpose::Session
            })
            .map(|(key, _)| key.clone())
    }

    /// An `Open` joins the session already there, so it never cuts the
    /// requests carried over it. Returns the holder when there is none to
    /// join.
    pub(super) fn join(&mut self, peer: &str, holder: Holder) -> Option<Holder> {
        let Some(key) = self.viewer_session(peer) else {
            return Some(holder);
        };
        let Some(session) = self.sessions.get_mut(&key) else {
            return Some(holder);
        };
        let Holder::Open { reply, hold } = holder else {
            return Some(holder);
        };
        if let Some(hello) = session.hello.clone() {
            if hold {
                session.holder = Holder::Open { reply: None, hold };
            }
            if let Some(reply) = reply {
                let _ = reply.send(Ok(hello));
            }
            return None;
        }
        if matches!(session.holder, Holder::None) {
            session.holder = Holder::Open { reply, hold };
            return None;
        }
        Some(Holder::Open { reply, hold })
    }

    /// Carries a request to `peer`, over its session or a new one. Returns
    /// the request's id.
    pub(super) fn on_tap(
        &mut self,
        peer: &str,
        request: TapRequest,
        events: mpsc::Sender<TapEvent>,
    ) -> Result<String, Failure> {
        let key = match self.viewer_session(peer) {
            Some(key) => key,
            None => {
                if let Some(failure) = self.cannot_open(peer, Purpose::Session) {
                    return Err(failure);
                }
                self.initiate(peer, Purpose::Session, Holder::None)
                    .ok_or_else(|| Failure::new("internal", "The session could not start."))?
            }
        };
        self.taps_made += 1;
        let id = format!("r{}", self.taps_made);
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or_else(|| Failure::new("closed", "The session ended."))?;
        let tap = Tap {
            request,
            events,
            sent: false,
        };
        session.taps.insert(id.clone(), tap);
        self.send_taps(&key);
        Ok(id)
    }

    /// Once the target said hello: the requests that waited for it, in the
    /// order they came, as long as the send queue has room for them.
    pub(super) fn send_taps(&mut self, key: &Key) {
        let Some(session) = self
            .sessions
            .get(key)
            .filter(|session| session.hello.is_some())
        else {
            return;
        };
        let mut waiting: Vec<(u64, String)> = session
            .taps
            .iter()
            .filter(|(_, tap)| !tap.sent)
            .map(|(id, _)| (id[1..].parse().unwrap_or(u64::MAX), id.clone()))
            .collect();
        waiting.sort();
        for (_, id) in waiting {
            if !self.out.has_room_for_requests() {
                return;
            }
            self.send_tap(key, &id);
        }
    }

    /// Requests that waited for room in the send queue.
    pub(super) fn pump_taps(&mut self) {
        let keys: Vec<Key> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.taps.values().any(|tap| !tap.sent))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            self.send_taps(&key);
        }
    }

    fn send_tap(&mut self, key: &Key, id: &str) {
        let Some(tap) = self
            .sessions
            .get_mut(key)
            .and_then(|session| session.taps.get_mut(id))
        else {
            return;
        };
        tap.sent = true;
        let message = tap.message(id);
        self.send_message(key, &message);
    }

    /// An answer from the target, to the request it names.
    pub(super) fn deliver(&mut self, key: &Key, answer: SessionMessage) {
        let Some(re) = answer_to(&answer).map(str::to_string) else {
            return;
        };
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        // An answer to a request already over: dropped.
        let Some(tap) = session.taps.get(&re) else {
            return;
        };
        let finished = tap.finished_by(&answer);
        let delivered = tap.events.try_send(TapEvent::Answer(answer)).is_ok();
        if finished || !delivered {
            let removed = session.taps.remove(&re);
            if !finished && let Some(cancel) = removed.and_then(|tap| tap.cancel(&re)) {
                // Its client is gone or stopped reading.
                self.send_message(key, &cancel);
            }
            self.close_if_unused(key);
        }
    }

    /// The client of a request went away.
    pub(super) fn untap(&mut self, peer: &str, id: &str) {
        let Some(key) = self.viewer_session(peer) else {
            return;
        };
        let Some(tap) = self
            .sessions
            .get_mut(&key)
            .and_then(|session| session.taps.remove(id))
        else {
            return;
        };
        if tap.sent
            && let Some(cancel) = tap.cancel(id)
        {
            self.send_message(&key, &cancel);
        }
        self.close_if_unused(&key);
    }

    pub(super) fn ack(&mut self, peer: &str, id: &str, offset: u64) {
        let Some(key) = self.viewer_session(peer) else {
            return;
        };
        let following = self
            .sessions
            .get(&key)
            .and_then(|session| session.taps.get(id))
            .is_some_and(|tap| tap.sent && matches!(tap.request, TapRequest::Follow { .. }));
        if following {
            let ack = SessionMessage::OutputAck {
                re: id.to_string(),
                offset,
            };
            self.send_message(&key, &ack);
        }
    }

    /// The daemon closes a session once nothing uses it.
    fn close_if_unused(&mut self, key: &Key) {
        let unused = self
            .sessions
            .get(key)
            .is_some_and(|session| session.hello.is_some() && !session.is_held());
        if unused {
            self.end(
                key,
                End::Bye(ByeReason::Done),
                &Failure::new("done", "Nothing uses the session any more."),
            );
        }
    }
}
