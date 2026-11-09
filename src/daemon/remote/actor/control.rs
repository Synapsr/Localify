//! The target's side of a control (`docs/remote-machines.md`, "Target
//! checks"): checks 1 to 3 here, on the actor's task, then the control
//! executor (`infrastructure::remote::control`) off it, through the
//! machine's [`LocalSource`]. Its progress and its result come back as
//! commands and go to the session that asked, while it is open; a session
//! that ended hears nothing, and its viewer asks `control.status` on its
//! next one.
//!
//! [`LocalSource`]: crate::daemon::remote::local::LocalSource

use std::sync::Arc;

use tokio::time::Instant;
use tracing::{info, warn};

use super::{Actor, Command, Key};
use crate::application::inventory::Switches;
use crate::infrastructure::remote::control::Order;
use crate::infrastructure::remote::requests::Report;
use crate::infrastructure::remote::store::DEFAULT_SETTINGS;
use crate::infrastructure::remote::wire::{Control, ControlPhase, SessionMessage};

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

    /// `control`: checks 1 to 3, then the executor, off this task.
    pub(super) fn on_control(&mut self, key: &Key, control: Control) {
        let id = control.id.clone();
        if self.id_in_use(key, &id) {
            let refusal =
                SessionMessage::refusal(&id, "bad_request", "This request id is already in use.");
            self.send_message(key, &refusal);
            return;
        }
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
            let report = Report::refused(code, message);
            self.send_message(key, &SessionMessage::ControlResult(report.answer(&id)));
            return;
        }
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        session.controls.insert(id.clone());
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
        self.execute_control(key.clone(), id, order);
    }

    /// Check 3, a fresh lease, then the executor on a blocking thread; its
    /// progress and result come back as [`Command::ControlDone`].
    fn execute_control(&mut self, key: Key, id: String, order: Order) {
        let lease = Arc::clone(&self.ctx.lease);
        let source = Arc::clone(&self.ctx.source);
        let shared = Arc::clone(&self.shared);
        let commands = self.commands.clone();
        self.spawn(async move {
            // 3: a fresh lease, asked for now, that still lists the viewer.
            let fresh = lease.renew().await.is_ok() && lease.covers(&order.peer, Instant::now());
            let send = |event| {
                let _ = commands.send(Command::ControlDone {
                    key: key.clone(),
                    re: id.clone(),
                    event,
                });
            };
            if !fresh {
                return send(ControlEvent::Result(Report::refused(
                    "unauthorized",
                    "Remote access is not available right now.",
                )));
            }
            let progress_commands = commands.clone();
            let (progress_key, progress_id) = (key.clone(), id.clone());
            let ran = tokio::task::spawn_blocking(move || {
                let switches = || Switches {
                    settings: shared.store.settings().unwrap_or(DEFAULT_SETTINGS).into(),
                    app: source.app().into(),
                };
                let mut progress = |phase| {
                    let _ = progress_commands.send(Command::ControlDone {
                        key: progress_key.clone(),
                        re: progress_id.clone(),
                        event: ControlEvent::Progress(phase),
                    });
                };
                source.control(&order, &switches, &mut progress)
            })
            .await;
            let report = ran.unwrap_or_else(|error| {
                Report::refused("unavailable", format!("The control failed here: {error}"))
            });
            send(ControlEvent::Result(report));
        });
    }

    /// `control.status`: the request's entry, off this task.
    pub(super) fn on_control_status(&mut self, key: &Key, id: String, request_id: String) {
        if self.id_in_use(key, &id) {
            let refusal =
                SessionMessage::refusal(&id, "bad_request", "This request id is already in use.");
            self.send_message(key, &refusal);
            return;
        }
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        session.controls.insert(id.clone());
        let source = Arc::clone(&self.ctx.source);
        let commands = self.commands.clone();
        let (key, peer) = (key.clone(), key.0.clone());
        self.spawn(async move {
            let report =
                tokio::task::spawn_blocking(move || source.control_status(&peer, &request_id))
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

    /// The executor's news: to the session that asked, while it is open.
    /// After a result, viewers that watch get the fresh snapshot.
    pub(super) fn on_control_done(&mut self, key: &Key, re: &str, event: ControlEvent) {
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
        if self.sessions.contains_key(key) && !self.send_message(key, &message) {
            warn!(peer = %key.0, "Could not send a control's answer");
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
