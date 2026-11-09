//! Access requests on the target (`docs/remote-machines.md`, "Asking for
//! access"; ADR 0033): which `access.request` may show a dialog, counted on
//! the daemon's monotonic clock across sessions while it runs, and the
//! request a dialog waits on. The switches change only when a person here
//! allows it, and only if nothing wrote the settings in between.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use tokio::time::Instant;

use crate::infrastructure::remote::store::SettingsFile;
use crate::infrastructure::remote::wire::{AccessWants, TargetSettings};

/// A target shows at most this many dialogs in [`DIALOG_WINDOW`].
const DIALOGS_PER_WINDOW: usize = 5;
const DIALOG_WINDOW: Duration = Duration::from_secs(3600);

/// The delays of the limits; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// A pinned peer asks at most once this often.
    pub every: Duration,
    /// After a refusal, a peer waits this long.
    pub after_refusal: Duration,
}

/// What an `access.request` gets, checked in the contract's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    NotShared,
    /// The switches asked for are already on.
    Granted,
    TooMany {
        retry_after: u64,
    },
    /// A person here stopped this peer's requests: `declined` at once.
    Suppressed,
    /// Another request is waiting on this target.
    Busy,
    /// Nobody here can answer: `nobody` at once.
    Nobody,
    /// A dialog may be shown.
    Ask,
}

/// What the checks look at besides the history.
pub struct Ask<'a> {
    pub peer: &'a str,
    pub wants: AccessWants,
    pub settings: &'a TargetSettings,
    pub suppressed: bool,
    /// Another request waits on this target.
    pub waiting: bool,
    /// An app that shows the dialog is subscribed.
    pub someone_here: bool,
}

/// The requests of each peer, and the dialogs of the last hour. The link
/// keeps it, so a new relay connection does not reset the limits.
#[derive(Debug, Default)]
pub struct History {
    asked: HashMap<String, Instant>,
    refused: HashMap<String, Instant>,
    dialogs: VecDeque<Instant>,
}

impl History {
    /// The checks, in the contract's order: `not_shared`, `granted`,
    /// `too_many`, suppressed, `busy`; then whether anyone can answer.
    pub fn admit(&mut self, ask: &Ask<'_>, limits: Limits, now: Instant) -> Admission {
        if !ask.settings.share {
            return Admission::NotShared;
        }
        if ask.wants.is_granted_by(ask.settings) {
            return Admission::Granted;
        }
        if let Some(wait) = self.wait(ask.peer, limits, now) {
            let retry_after = (wait.as_secs() + u64::from(wait.subsec_nanos() > 0)).max(1);
            return Admission::TooMany { retry_after };
        }
        if ask.suppressed {
            return Admission::Suppressed;
        }
        if ask.waiting {
            return Admission::Busy;
        }
        if !ask.someone_here {
            return Admission::Nobody;
        }
        Admission::Ask
    }

    /// How long `peer` still waits, if it must.
    fn wait(&mut self, peer: &str, limits: Limits, now: Instant) -> Option<Duration> {
        while self
            .dialogs
            .front()
            .is_some_and(|at| now.duration_since(*at) >= DIALOG_WINDOW)
        {
            self.dialogs.pop_front();
        }
        let left = |since: Option<&Instant>, limit: Duration| {
            since
                .map(|at| limit.saturating_sub(now.duration_since(*at)))
                .filter(|left| !left.is_zero())
        };
        let full = (self.dialogs.len() >= DIALOGS_PER_WINDOW)
            .then(|| self.dialogs.front().copied())
            .flatten()
            .and_then(|oldest| left(Some(&oldest), DIALOG_WINDOW));
        [
            left(self.asked.get(peer), limits.every),
            left(self.refused.get(peer), limits.after_refusal),
            full,
        ]
        .into_iter()
        .flatten()
        .max()
    }

    /// A dialog was shown for `peer`'s request.
    pub fn shown(&mut self, peer: &str, now: Instant) {
        self.asked.insert(peer.to_string(), now);
        self.dialogs.push_back(now);
    }

    /// A person here answered `Don't allow`.
    pub fn refused(&mut self, peer: &str, now: Instant) {
        self.refused.insert(peer.to_string(), now);
    }
}

/// The request a dialog waits on: one per target at most, held by the
/// session that asked, so it ends with it.
#[derive(Debug, Clone)]
pub struct Waiting {
    /// What the app answers with: never the viewer's id.
    pub request_id: String,
    /// The viewer's id for it, which its `access.result` answers.
    pub re: String,
    pub wants: AccessWants,
    pub expires: Instant,
    /// The settings and their generation when it was recorded: it applies
    /// only over these.
    pub seen: SettingsFile,
    /// The asker's pinned key when it was recorded: an answer applies only
    /// while the pin still holds it.
    pub pinned_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        every: Duration::from_secs(600),
        after_refusal: Duration::from_secs(3600),
    };

    fn settings(share: bool, control: bool) -> TargetSettings {
        TargetSettings {
            share,
            control,
            background_starts: false,
        }
    }

    fn ask<'a>(peer: &'a str, settings: &'a TargetSettings) -> Ask<'a> {
        Ask {
            peer,
            wants: AccessWants::Control,
            settings,
            suppressed: false,
            waiting: false,
            someone_here: true,
        }
    }

    #[test]
    fn the_checks_come_in_the_contracts_order() {
        let mut history = History::default();
        let now = Instant::now();
        let off = settings(false, false);
        let on = settings(true, true);
        let shared = settings(true, false);

        // `not_shared` before everything, even a suppressed peer.
        let request = Ask {
            suppressed: true,
            waiting: true,
            ..ask("a", &off)
        };
        assert_eq!(history.admit(&request, LIMITS, now), Admission::NotShared);
        // Already on: `granted`, even while limited.
        history.shown("a", now);
        assert_eq!(
            history.admit(&ask("a", &on), LIMITS, now),
            Admission::Granted
        );
        // Limited before suppressed and busy.
        let request = Ask {
            suppressed: true,
            waiting: true,
            ..ask("a", &shared)
        };
        assert_eq!(
            history.admit(&request, LIMITS, now),
            Admission::TooMany { retry_after: 600 }
        );
        // Suppressed before busy.
        let request = Ask {
            suppressed: true,
            waiting: true,
            ..ask("b", &shared)
        };
        assert_eq!(history.admit(&request, LIMITS, now), Admission::Suppressed);
        let request = Ask {
            waiting: true,
            ..ask("b", &shared)
        };
        assert_eq!(history.admit(&request, LIMITS, now), Admission::Busy);
        let request = Ask {
            someone_here: false,
            ..ask("b", &shared)
        };
        assert_eq!(history.admit(&request, LIMITS, now), Admission::Nobody);
        assert_eq!(
            history.admit(&ask("b", &shared), LIMITS, now),
            Admission::Ask
        );
    }

    #[test]
    fn a_peer_asks_once_every_10_minutes_and_once_an_hour_after_a_refusal() {
        let mut history = History::default();
        let start = Instant::now();
        let shared = settings(true, false);
        history.shown("a", start);
        let later = |seconds| start + Duration::from_secs(seconds);

        assert_eq!(
            history.admit(&ask("a", &shared), LIMITS, later(599)),
            Admission::TooMany { retry_after: 1 }
        );
        assert_eq!(
            history.admit(&ask("a", &shared), LIMITS, later(600)),
            Admission::Ask
        );

        history.shown("a", later(600));
        history.refused("a", later(610));
        assert_eq!(
            history.admit(&ask("a", &shared), LIMITS, later(1300)),
            Admission::TooMany { retry_after: 2910 }
        );
        assert_eq!(
            history.admit(&ask("a", &shared), LIMITS, later(4210)),
            Admission::Ask
        );
    }

    #[test]
    fn a_target_shows_at_most_five_dialogs_an_hour() {
        let mut history = History::default();
        let start = Instant::now();
        let shared = settings(true, false);
        for (index, peer) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            history.shown(peer, start + Duration::from_secs(index as u64));
        }
        let now = start + Duration::from_secs(60);
        assert_eq!(
            history.admit(&ask("f", &shared), LIMITS, now),
            Admission::TooMany { retry_after: 3540 }
        );
        // The first one leaves the window.
        let hour = start + DIALOG_WINDOW;
        assert_eq!(
            history.admit(&ask("f", &shared), LIMITS, hour),
            Admission::Ask
        );
    }
}
