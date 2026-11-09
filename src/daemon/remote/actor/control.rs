//! The target's side of a control (`docs/remote-machines.md`, "Target
//! checks"): checks 1 to 3 here, on the actor's task, then the control
//! executor (`infrastructure::remote::control`) off it, through the
//! machine's [`LocalSource`]. Its progress and its result come back as
//! commands and go to the session that asked, while it is open and this
//! machine shares; a session that ended hears nothing, and its viewer asks
//! `control.status` on its next one.
//!
//! [`LocalSource`]: crate::daemon::remote::local::LocalSource

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::{Actor, Command, Key, Shared};
use crate::application::inventory::Switches;
use crate::infrastructure::remote::control::{Live, Order};
use crate::infrastructure::remote::noise::encode_key;
use crate::infrastructure::remote::requests::Report;
use crate::infrastructure::remote::store::DEFAULT_SETTINGS;
use crate::infrastructure::remote::wire::{Control, ControlPhase, SessionMessage};

/// A viewer has at most this many controls and `control.status`es going on
/// this target at once, and every viewer together this many: past them, a
/// new one is refused `busy` before any work is done for it.
const CONTROLS_PER_PEER: usize = 4;
const CONTROLS_PER_TARGET: usize = 16;
/// What a viewer refused `busy` is told to wait, in seconds.
const BUSY_RETRY: u64 = 5;

/// The controls and `control.status`es going on this target, by peer. The
/// link keeps them, not a session or a connection: a [`ControlPermit`]
/// lives as long as its work, the lease and the blocking execution
/// included, so a viewer that reconnects or opens a new session finds its
/// earlier work still counted.
#[derive(Debug, Default)]
pub struct ControlSlots {
    going: Mutex<HashMap<String, usize>>,
}

impl ControlSlots {
    /// A permit for one more piece of `peer`'s work, within
    /// [`CONTROLS_PER_PEER`] and [`CONTROLS_PER_TARGET`].
    pub fn admit(self: &Arc<Self>, peer: &str) -> Option<ControlPermit> {
        let mut going = self
            .going
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let all: usize = going.values().sum();
        let mine = going.get(peer).copied().unwrap_or(0);
        if mine >= CONTROLS_PER_PEER || all >= CONTROLS_PER_TARGET {
            return None;
        }
        *going.entry(peer.to_string()).or_default() += 1;
        Some(ControlPermit {
            slots: Arc::clone(self),
            peer: peer.to_string(),
        })
    }
}

/// One admitted control or `control.status`, counted until dropped.
#[derive(Debug)]
pub struct ControlPermit {
    slots: Arc<ControlSlots>,
    peer: String,
}

impl Drop for ControlPermit {
    fn drop(&mut self) {
        let mut going = self
            .slots
            .going
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = going.get_mut(&self.peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                going.remove(&self.peer);
            }
        }
    }
}

/// What the executor says about one control, back on the actor's task.
#[derive(Debug)]
pub enum ControlEvent {
    Progress(ControlPhase),
    Result(Report),
}

impl Actor {
    /// Whether `id` names a request of this session still going, whatever
    /// its kind.
    pub(super) fn id_in_use(&self, key: &Key, id: &str) -> bool {
        self.sessions.get(key).is_some_and(|session| {
            session.served.is_active(id)
                || session.controls.contains(id)
                || session
                    .access
                    .as_ref()
                    .is_some_and(|waiting| waiting.re == id)
        })
    }

    /// A `control` that carries a field it may not: refused, never run.
    pub(super) fn refuse_argument(&mut self, key: &Key, id: &str, field: &str) {
        let report = Report::refused(
            "refused_argument",
            format!("A control from another machine never takes '{field}'."),
        );
        self.send_message(key, &SessionMessage::ControlResult(report.answer(id)));
    }

    /// Admits a control or a `control.status` before anything is spawned or
    /// fetched for it, within the link's [`ControlSlots`]. Answers it and
    /// returns `None` otherwise. The permit goes with the work, so the
    /// bounds hold whatever becomes of the session.
    fn admit_control(&mut self, key: &Key, id: &str) -> Option<ControlPermit> {
        if self.id_in_use(key, id) {
            let refusal =
                SessionMessage::refusal(id, "bad_request", "This request id is already in use.");
            self.send_message(key, &refusal);
            return None;
        }
        let Some(permit) = self.shared.control_slots.admit(&key.0) else {
            let busy = SessionMessage::Error {
                re: Some(id.to_string()),
                code: "busy".into(),
                message: "This machine is working on other controls. Try again shortly.".into(),
                retry_after: Some(BUSY_RETRY),
            };
            self.send_message(key, &busy);
            return None;
        };
        let session = self.sessions.get_mut(key)?;
        session.controls.insert(id.to_string());
        Some(permit)
    }

    /// `control`: checks 1 to 3, then the executor, off this task.
    pub(super) fn on_control(&mut self, key: &Key, control: Control) {
        let id = control.id.clone();
        // 1 is the session's: a pinned peer, past `hello`. 2: the switches.
        let settings = self.shared.store.settings().unwrap_or(DEFAULT_SETTINGS);
        let refusal = match (settings.share, settings.control) {
            (false, _) => Some(("not_shared", "This machine doesn't share its projects.")),
            (true, false) => Some((
                "control_disabled",
                "This machine doesn't allow remote controls.",
            )),
            (true, true) => None,
        };
        if let Some((code, message)) = refusal {
            if self.id_in_use(key, &id) {
                let refusal = SessionMessage::refusal(
                    &id,
                    "bad_request",
                    "This request id is already in use.",
                );
                self.send_message(key, &refusal);
                return;
            }
            let report = Report::refused(code, message);
            self.send_message(key, &SessionMessage::ControlResult(report.answer(&id)));
            return;
        }
        let Some(pin_key) = self
            .sessions
            .get(key)
            .and_then(|session| session.peer_key)
            .map(|peer_key| encode_key(&peer_key))
        else {
            return;
        };
        let Some(permit) = self.admit_control(key, &id) else {
            return;
        };
        let peer_name = self
            .shared
            .store
            .pins()
            .ok()
            .and_then(|pins| pins.get(&key.0).map(|pin| pin.name.clone()))
            .unwrap_or_else(|| key.0.clone());
        info!(peer = %key.0, action = control.action.as_str(), service = %control.service.service_id, "A paired machine asks for a control");
        let order = Order {
            peer: key.0.clone(),
            peer_name,
            control,
        };
        let cancel = self
            .control_cancels
            .entry(key.0.clone())
            .or_default()
            .clone();
        let pairing = Pairing { pin_key, cancel };
        self.execute_control(key.clone(), id, order, pairing, permit);
    }

    /// The peer's entry limit, then check 3, a fresh lease, then the
    /// executor on a blocking thread; its progress and result come back as
    /// [`Command::ControlDone`]. An unpairing or a new key cancels it until
    /// it is dispatched. `permit` is held until the executor returns.
    fn execute_control(
        &mut self,
        key: Key,
        id: String,
        order: Order,
        pairing: Pairing,
        permit: ControlPermit,
    ) {
        let lease = Arc::clone(&self.ctx.lease);
        let source = Arc::clone(&self.ctx.source);
        let shared = Arc::clone(&self.shared);
        let commands = self.commands.clone();
        self.spawn(async move {
            let send = |event| {
                let _ = commands.send(Command::ControlDone {
                    key: key.clone(),
                    re: id.clone(),
                    event,
                });
            };
            // A new request id past the peer's entry limit costs no lease.
            let (asked, peer) = (Arc::clone(&source), order.peer.clone());
            let request_id = order.control.request_id.clone();
            let admitted =
                tokio::task::spawn_blocking(move || asked.control_admits(&peer, &request_id)).await;
            if let Ok(Err(refused)) = admitted {
                return send(ControlEvent::Result(refused));
            }
            // 3: a fresh lease, asked for now, that still lists the viewer.
            let renewed = tokio::select! {
                _ = pairing.cancel.cancelled() => None,
                renewed = lease.renew() => Some(renewed.is_ok()),
            };
            let Some(renewed) = renewed.filter(|_| pairing.holds(&shared, &order.peer)) else {
                return send(ControlEvent::Result(not_paired()));
            };
            if !(renewed && lease.covers(&order.peer, Instant::now())) {
                return send(ControlEvent::Result(Report::refused(
                    "unauthorized",
                    "Remote access is not available right now.",
                )));
            }
            let progress_commands = commands.clone();
            let (progress_key, progress_id) = (key.clone(), id.clone());
            let ran = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let switches = || Switches {
                    settings: shared.store.settings().unwrap_or(DEFAULT_SETTINGS).into(),
                    app: source.app().into(),
                };
                let paired = || pairing.holds(&shared, &order.peer);
                let mut progress = |phase| {
                    let _ = progress_commands.send(Command::ControlDone {
                        key: progress_key.clone(),
                        re: progress_id.clone(),
                        event: ControlEvent::Progress(phase),
                    });
                };
                let live = Live {
                    switches: &switches,
                    paired: &paired,
                };
                source.control(&order, &live, &mut progress)
            })
            .await;
            let report = ran.unwrap_or_else(|error| {
                Report::refused("unavailable", format!("The control failed here: {error}"))
            });
            send(ControlEvent::Result(report));
        });
    }

    /// `control.status`: the request's entry, off this task, only while
    /// this machine shares.
    pub(super) fn on_control_status(&mut self, key: &Key, id: String, request_id: String) {
        let sharing = self
            .shared
            .store
            .settings()
            .unwrap_or(DEFAULT_SETTINGS)
            .share;
        if !sharing {
            if self.id_in_use(key, &id) {
                let refusal = SessionMessage::refusal(
                    &id,
                    "bad_request",
                    "This request id is already in use.",
                );
                self.send_message(key, &refusal);
                return;
            }
            let report = Report::refused("not_shared", "This machine doesn't share its projects.");
            self.send_message(key, &SessionMessage::ControlResult(report.answer(&id)));
            return;
        }
        let Some(permit) = self.admit_control(key, &id) else {
            return;
        };
        let source = Arc::clone(&self.ctx.source);
        let commands = self.commands.clone();
        let (key, peer) = (key.clone(), key.0.clone());
        self.spawn(async move {
            let report = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                source.control_status(&peer, &request_id)
            })
            .await
            .unwrap_or_else(|error| {
                Report::refused(
                    "unavailable",
                    format!("The request could not be read here: {error}"),
                )
            });
            let _ = commands.send(Command::ControlDone {
                key,
                re: id,
                event: ControlEvent::Result(report),
            });
        });
    }

    /// The executor's news: to the session that asked, while it is open,
    /// still waits for it, and this machine shares. After a result, viewers
    /// that watch get the fresh snapshot.
    pub(super) fn on_control_done(&mut self, key: &Key, re: &str, event: ControlEvent) {
        // `share` saved off but not acted on yet: the revocation comes
        // first, so this answer is `not_shared` like every other one.
        let sharing = self
            .shared
            .store
            .settings()
            .unwrap_or(DEFAULT_SETTINGS)
            .share;
        if !sharing && self.shelf.sharing {
            self.reload_settings();
        }
        let waiting = self
            .sessions
            .get(key)
            .is_some_and(|session| session.controls.contains(re));
        let message = match event {
            ControlEvent::Progress(phase) => SessionMessage::ControlProgress {
                re: re.to_string(),
                phase,
            },
            ControlEvent::Result(report) => {
                if let Some(session) = self.sessions.get_mut(key) {
                    session.controls.remove(re);
                }
                self.refresh_watchers();
                SessionMessage::ControlResult(report.answer(re))
            }
        };
        if waiting && sharing && !self.send_message(key, &message) {
            warn!(peer = %key.0, "Could not send a control's answer");
        }
    }

    /// `share` was turned off: every control and `control.status` still
    /// going is answered `not_shared` now, and whatever it comes to later
    /// is dropped. A control already dispatched still finishes here.
    pub(super) fn drop_control_answers(&mut self, message: &str) {
        let waiting: Vec<(Key, String)> = self
            .sessions
            .iter_mut()
            .flat_map(|(key, session)| {
                session
                    .controls
                    .drain()
                    .map(|id| (key.clone(), id))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (key, id) in waiting {
            self.send_message(&key, &SessionMessage::refusal(&id, "not_shared", message));
        }
    }

    /// The peer was unpaired here, or paired again with another key: what
    /// it asked for and was not dispatched yet is cancelled.
    pub(super) fn cancel_controls(&mut self, peer: &str) {
        if let Some(cancel) = self.control_cancels.remove(peer) {
            cancel.cancel();
        }
    }

    /// A control changed what runs: every watch hears it at once.
    fn refresh_watchers(&mut self) {
        let watched = self
            .sessions
            .values()
            .any(|session| session.served.watches());
        if watched {
            self.build_snapshot();
        }
    }
}

/// The pairing a control was asked under: the key the session
/// authenticated, and what cancels it.
struct Pairing {
    pin_key: String,
    cancel: CancellationToken,
}

impl Pairing {
    /// Whether the peer is still pinned with that key, and nothing
    /// cancelled the control.
    fn holds(&self, shared: &Shared, peer: &str) -> bool {
        !self.cancel.is_cancelled()
            && shared.store.pins().is_ok_and(|pins| {
                pins.get(peer)
                    .is_some_and(|pin| pin.public_key == self.pin_key)
            })
    }
}

fn not_paired() -> Report {
    Report::refused(
        "not_paired",
        "This machine is no longer paired with that one.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bounds hold for as long as permits live, whoever holds them.
    #[test]
    fn permits_bound_each_peer_and_every_peer_together() {
        let slots = Arc::new(ControlSlots::default());
        let mine: Vec<_> = (0..CONTROLS_PER_PEER)
            .map(|_| slots.admit("mA").unwrap())
            .collect();
        assert!(slots.admit("mA").is_none());
        drop(mine);
        assert!(
            slots.admit("mA").is_some(),
            "a dropped permit frees its slot"
        );

        let all: Vec<_> = (0..CONTROLS_PER_TARGET)
            .map(|n| slots.admit(&format!("m{}", n % 8)).unwrap())
            .collect();
        assert!(slots.admit("mZ").is_none(), "every viewer together");
        drop(all);
        assert!(slots.admit("mZ").is_some());
    }
}
