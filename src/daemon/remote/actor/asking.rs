//! The target's side of an access request (`docs/remote-machines.md`,
//! "Asking for access"; ADR 0033). The request lives on the session that
//! asked, so it ends with it; the dialog's answer changes the switches only
//! while the request still waits, before its expiry, and over the settings
//! it was recorded with.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use tokio::time::Instant;
use tracing::{info, warn};

use super::{Actor, Event, Key};
use crate::daemon::remote::access::{Admission, Ask, Limits, Waiting};
use crate::infrastructure::launch_rev::random_bytes;
use crate::infrastructure::remote::noise::{decode_key, fingerprint};
use crate::infrastructure::remote::store::{AccessAnswer, Answered, SettingsChange, SettingsFile};
use crate::infrastructure::remote::wire::{AccessOutcome, AccessWants, Role, SessionMessage};

/// How many ended requests a late answer can still learn the end of.
const ENDED_KEPT: usize = 16;

const NOT_SHARED: &str =
    "This machine doesn't share its projects. Turn on sharing in Localify on it first.";
const BUSY: &str = "This machine is answering another request.";
const TOO_SOON: &str = "You asked this machine recently.";

impl Actor {
    /// A viewer's `access.request`, checked in the contract's order.
    pub(super) fn on_access_request(&mut self, key: &Key, id: String, wants: AccessWants) {
        let in_use = self.sessions.get(key).is_some_and(|session| {
            session.served.is_active(&id)
                || session
                    .access
                    .as_ref()
                    .is_some_and(|waiting| waiting.re == id)
        });
        if in_use {
            return self.access_refusal(
                key,
                &id,
                "bad_request",
                "This request id is already in use.",
            );
        }
        let (file, suppressed) = match (
            self.shared.store.settings_file(),
            self.shared.store.suppressions(),
        ) {
            (Ok(file), Ok(suppressed)) => (file, suppressed.contains_key(&key.0)),
            (Err(error), _) | (_, Err(error)) => {
                warn!(error = %format!("{error:#}"), "Could not read the remote settings");
                return self.access_refusal(key, &id, "unavailable", "Try again in a moment.");
            }
        };
        let ask = Ask {
            peer: &key.0,
            wants,
            settings: &file.settings,
            suppressed,
            waiting: self
                .sessions
                .values()
                .any(|session| session.access.is_some()),
            someone_here: self
                .shared
                .access_subscribers
                .load(std::sync::atomic::Ordering::SeqCst)
                > 0,
        };
        let limits = Limits {
            every: self.shared.timings.access_every,
            after_refusal: self.shared.timings.access_after_refusal,
        };
        let now = Instant::now();
        let admission = self.shared.access_history().admit(&ask, limits, now);
        match admission {
            Admission::NotShared => self.access_refusal(key, &id, "not_shared", NOT_SHARED),
            Admission::Granted => self.access_result(key, &id, AccessOutcome::Granted),
            Admission::TooMany { retry_after } => {
                let refusal = SessionMessage::Error {
                    re: Some(id),
                    code: "too_many".into(),
                    message: TOO_SOON.into(),
                    retry_after: Some(retry_after),
                };
                self.send_message(key, &refusal);
            }
            Admission::Suppressed => self.access_result(key, &id, AccessOutcome::Declined),
            Admission::Busy => self.access_refusal(key, &id, "busy", BUSY),
            Admission::Nobody => self.access_result(key, &id, AccessOutcome::Nobody),
            Admission::Ask => self.show_access(key, id, wants, file, now),
        }
    }

    /// Records the request and tells the app, which shows the dialog.
    fn show_access(
        &mut self,
        key: &Key,
        re: String,
        wants: AccessWants,
        seen: SettingsFile,
        now: Instant,
    ) {
        // The asker as this machine pinned it, never as it names itself.
        let pin = self
            .shared
            .store
            .pins()
            .ok()
            .and_then(|pins| pins.get(&key.0).cloned());
        let (Some(pin), Ok(request_id)) = (pin, new_request_id()) else {
            return self.access_refusal(key, &re, "unavailable", "Try again in a moment.");
        };
        let print = decode_key(&pin.public_key)
            .map(|key| fingerprint(&key))
            .unwrap_or_default();
        let expiry = self.shared.timings.access_expiry;
        let expires_at = time::OffsetDateTime::now_utc() + expiry;
        let expires_at = expires_at
            .replace_nanosecond(0)
            .ok()
            .and_then(|at| {
                at.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
            .unwrap_or_default();
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        session.access = Some(Waiting {
            request_id: request_id.clone(),
            re,
            wants,
            expires: now + expiry,
            seen,
            pinned_key: pin.public_key.clone(),
        });
        self.shared.access_history().shown(&key.0, now);
        info!(peer = %key.0, ?wants, "A paired machine asks for remote controls");
        self.shared.emit(Event::AccessRequest {
            request_id,
            machine_id: key.0.clone(),
            name: pin.name,
            fingerprint: print,
            wants,
            expires_at,
        });
    }

    /// `access.cancel`: the viewer stopped waiting.
    pub(super) fn on_access_cancel(&mut self, key: &Key, re: &str) {
        let waiting = self
            .sessions
            .get(key)
            .and_then(|session| session.access.as_ref())
            .is_some_and(|waiting| waiting.re == re);
        if waiting {
            self.close_access(key, AccessOutcome::Cancelled);
        }
    }

    /// `remote_access_answer`. An answer to a request that no longer waits
    /// changes nothing and says how it ended.
    pub(super) fn on_access_answer(
        &mut self,
        request_id: &str,
        allow: bool,
        suppress: bool,
    ) -> AccessOutcome {
        let found = self.sessions.iter().find_map(|(key, session)| {
            session
                .access
                .as_ref()
                .filter(|waiting| waiting.request_id == request_id)
                .map(|waiting| (key.clone(), waiting.clone()))
        });
        let Some((key, waiting)) = found else {
            return self
                .access_ended
                .iter()
                .find(|(id, _)| id == request_id)
                .map_or(AccessOutcome::Cancelled, |(_, outcome)| *outcome);
        };
        if Instant::now() >= waiting.expires {
            self.close_access(&key, AccessOutcome::Expired);
            return AccessOutcome::Expired;
        }
        let outcome = self.apply_answer(&key.0, &waiting, allow, suppress);
        if outcome == AccessOutcome::Declined {
            self.shared.access_history().refused(&key.0, Instant::now());
        }
        self.close_access(&key, outcome);
        if outcome == AccessOutcome::Granted {
            info!(peer = %key.0, wants = ?waiting.wants, "Remote controls allowed here");
            // The switches changed: act on them as on any other change.
            self.reload_settings();
        }
        outcome
    }

    /// Writes the answer under the settings lock, and only while the asker
    /// is still pinned with the key it asked with, the settings are the ones
    /// the request was recorded with, and its expiry has not passed: an
    /// unpair, a settings write or the lock held past the expiry wins, and
    /// the answer then writes nothing, suppression included. An `Allow`
    /// turns on exactly what was asked, never `share`.
    fn apply_answer(
        &self,
        peer: &str,
        waiting: &Waiting,
        allow: bool,
        suppress: bool,
    ) -> AccessOutcome {
        let grant = allow.then_some(SettingsChange {
            share: None,
            control: Some(true),
            background_starts: (waiting.wants == AccessWants::BackgroundStarts).then_some(true),
        });
        let answer = AccessAnswer {
            peer,
            pinned_key: &waiting.pinned_key,
            seen: waiting.seen,
            grant,
            suppress,
        };
        #[cfg(test)]
        self.shared.pause_answer();
        let due = || Instant::now() < waiting.expires;
        match self
            .shared
            .store
            .answer_access(&self.ctx.self_id, answer, due)
        {
            Ok(Answered::Applied) if allow => AccessOutcome::Granted,
            Ok(Answered::Applied) => AccessOutcome::Declined,
            Ok(Answered::Late) => AccessOutcome::Expired,
            Ok(Answered::Overtaken) => AccessOutcome::Cancelled,
            Err(error) => {
                warn!(error = %format!("{error:#}"), "Could not apply an answer to an access request");
                AccessOutcome::Cancelled
            }
        }
    }

    /// Each tick: requests past their expiry end `expired`.
    pub(super) fn expire_access(&mut self, now: Instant) {
        let expired: Vec<Key> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .access
                    .as_ref()
                    .is_some_and(|waiting| now >= waiting.expires)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            self.close_access(&key, AccessOutcome::Expired);
        }
    }

    /// A settings write since a request was recorded cancels it, so it is
    /// never applied over a change made meanwhile.
    pub(super) fn recheck_access(&mut self, generation: Option<u64>) {
        let overtaken: Vec<Key> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .access
                    .as_ref()
                    .is_some_and(|waiting| Some(waiting.seen.generation) != generation)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in overtaken {
            self.close_access(&key, AccessOutcome::Cancelled);
        }
    }

    /// Ends the session's request: `access.result` to the viewer, and
    /// `access_closed` to the app.
    fn close_access(&mut self, key: &Key, outcome: AccessOutcome) {
        let Some(waiting) = self
            .sessions
            .get_mut(key)
            .and_then(|session| session.access.take())
        else {
            return;
        };
        self.access_result(key, &waiting.re, outcome);
        self.access_closed(&waiting.request_id, outcome);
    }

    /// Tells the app the dialog is over, and keeps how it ended.
    pub(super) fn access_closed(&mut self, request_id: &str, outcome: AccessOutcome) {
        if self.access_ended.len() >= ENDED_KEPT {
            self.access_ended.pop_front();
        }
        self.access_ended
            .push_back((request_id.to_string(), outcome));
        self.shared.emit(Event::AccessClosed {
            request_id: request_id.to_string(),
        });
    }

    fn access_result(&mut self, key: &Key, re: &str, outcome: AccessOutcome) {
        let target = self
            .sessions
            .get(key)
            .is_some_and(|session| session.role == Role::Target);
        if target {
            let result = SessionMessage::AccessResult {
                re: re.to_string(),
                outcome,
            };
            self.send_message(key, &result);
        }
    }

    fn access_refusal(&mut self, key: &Key, re: &str, code: &str, message: &str) {
        self.send_message(key, &SessionMessage::refusal(re, code, message));
    }
}

/// 16 random bytes, base64url: what the app answers with.
fn new_request_id() -> anyhow::Result<String> {
    let mut bytes = [0_u8; 16];
    random_bytes(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
