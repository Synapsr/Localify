//! When the relay link tries again (`docs/remote-machines.md`,
//! "Reconnects"): exponential backoff with full jitter, server hints, a
//! spread after deploys, and the closes after which it stops.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use super::Timings;
use crate::infrastructure::launch_rev::random_bytes;

/// A server hint gets up to this share of itself on top.
const HINT_JITTER: f64 = 0.2;

/// A uniform random number in `[0, 1)`.
pub fn random_fraction() -> f64 {
    let mut bytes = [0_u8; 8];
    let value = match random_bytes(&mut bytes) {
        Ok(()) => u64::from_le_bytes(bytes),
        // Spreading machines out does not need a secret.
        Err(_) => RandomState::new().build_hasher().finish(),
    };
    (value >> 11) as f64 / (1_u64 << 53) as f64
}

/// A random duration in `[0, max)`.
pub fn up_to(max: Duration) -> Duration {
    max.mul_f64(random_fraction())
}

/// How a connection, or an attempt at one, ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ending {
    /// The relay closed the socket with this code and reason.
    Closed { code: u16, reason: String },
    /// No close frame: the network dropped, or 75 s passed without a frame.
    Lost,
    /// The upgrade was answered with this status.
    Refused {
        status: u16,
        code: Option<String>,
        retry_after: Option<Duration>,
    },
    /// No answer at all: DNS, TCP or TLS failed.
    Unreachable,
    /// Localify Cloud no longer accepts the token: a `401` on a lease.
    Revoked,
    /// This machine signed out here, or signed in as another machine.
    SignedOutHere,
}

/// Why the link stays down until something changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// `4401` or `401`: the token is revoked; a new sign-in starts over.
    SignedOut,
    /// `4402` or `402`: waits for a plan change, or an hour.
    Lapsed,
    /// `4403` or `403`: the account is suspended.
    Banned,
    /// `4002` or `426`: this version is no longer accepted.
    Outdated,
    /// `4001`: another connection of this machine replaced this one.
    Replaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Retry(Duration),
    Stop(Stop),
    /// `409 remote_key_missing`: upload the key again, then retry.
    UploadKey(Duration),
}

/// The attempt counter of one link, and where its jitter comes from.
pub struct Backoff {
    attempt: u32,
    random: Box<dyn FnMut() -> f64 + Send>,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::with_random(Box::new(random_fraction))
    }
}

impl Backoff {
    /// With a given source of numbers in `[0, 1)`, so a test can replay one.
    pub fn with_random(random: Box<dyn FnMut() -> f64 + Send>) -> Self {
        Self { attempt: 0, random }
    }

    fn up_to(&mut self, max: Duration) -> Duration {
        max.mul_f64((self.random)().clamp(0.0, 1.0))
    }

    /// What to do after `ending`, given how long the connection was up
    /// (zero for an attempt that never connected).
    pub fn next(&mut self, ending: &Ending, up_for: Duration, timings: &Timings) -> Next {
        if up_for >= timings.stable_after {
            self.attempt = 0;
        }
        match ending {
            Ending::Closed { code, reason } => self.after_close(*code, reason, timings),
            Ending::Refused {
                status,
                code,
                retry_after,
            } => self.after_refusal(*status, code.as_deref(), *retry_after, timings),
            Ending::Lost | Ending::Unreachable => Next::Retry(self.backoff(None, timings)),
            Ending::Revoked => Next::Stop(Stop::SignedOut),
            // The next pass reads the new sign-in, or finds none.
            Ending::SignedOutHere => Next::Retry(Duration::ZERO),
        }
    }

    fn after_close(&mut self, code: u16, reason: &str, timings: &Timings) -> Next {
        match code {
            1001 | 1012 => Next::Retry(self.up_to(timings.deploy_jitter)),
            4001 => Next::Stop(Stop::Replaced),
            4002 => Next::Stop(Stop::Outdated),
            4401 => Next::Stop(Stop::SignedOut),
            4402 => Next::Stop(Stop::Lapsed),
            4403 => Next::Stop(Stop::Banned),
            _ => Next::Retry(self.backoff(retry_hint(reason), timings)),
        }
    }

    fn after_refusal(
        &mut self,
        status: u16,
        code: Option<&str>,
        retry_after: Option<Duration>,
        timings: &Timings,
    ) -> Next {
        match (status, code) {
            (401, _) => Next::Stop(Stop::SignedOut),
            (402, _) => Next::Stop(Stop::Lapsed),
            (403, _) => Next::Stop(Stop::Banned),
            (426, _) => Next::Stop(Stop::Outdated),
            (409, Some("remote_key_missing")) => Next::UploadKey(self.backoff(None, timings)),
            _ => Next::Retry(self.backoff(retry_after, timings)),
        }
    }

    /// `random(0, min(cap, unit × 2^attempt))`, at least the hint plus up to
    /// 20% of it.
    fn backoff(&mut self, hint: Option<Duration>, timings: &Timings) -> Duration {
        let ceiling = timings
            .backoff_unit
            .saturating_mul(2_u32.saturating_pow(self.attempt))
            .min(timings.backoff_cap);
        self.attempt = self.attempt.saturating_add(1);
        let wait = self.up_to(ceiling);
        match hint {
            Some(hint) => {
                let extra = hint.mul_f64(HINT_JITTER * (self.random)().clamp(0.0, 1.0));
                wait.max(hint + extra)
            }
            None => wait,
        }
    }
}

/// A close reason carrying `{"retryAfter": n}`.
fn retry_hint(reason: &str) -> Option<Duration> {
    let value: serde_json::Value = serde_json::from_str(reason).ok()?;
    let seconds = value.get("retryAfter")?.as_f64()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds.min(3600.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timings() -> Timings {
        Timings::default()
    }

    fn retry(next: Next) -> Duration {
        match next {
            Next::Retry(wait) => wait,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn backoff_grows_with_full_jitter_up_to_five_minutes() {
        let mut backoff = Backoff::default();
        for attempt in 0..20_u32 {
            let ceiling =
                Duration::from_secs(2_u64.pow(attempt.min(9))).min(Duration::from_secs(300));
            let wait = retry(backoff.next(&Ending::Lost, Duration::ZERO, &timings()));
            assert!(wait < ceiling, "attempt {attempt}: {wait:?} ≥ {ceiling:?}");
        }
        assert_eq!(backoff.attempt, 20);
    }

    #[test]
    fn a_connection_up_for_a_minute_resets_the_count() {
        let mut backoff = Backoff::default();
        for _ in 0..8 {
            backoff.next(&Ending::Lost, Duration::ZERO, &timings());
        }
        let wait = retry(backoff.next(&Ending::Lost, Duration::from_secs(61), &timings()));
        assert!(wait < Duration::from_secs(1), "{wait:?}");
    }

    #[test]
    fn a_server_hint_sets_the_least_wait_plus_up_to_a_fifth() {
        let mut backoff = Backoff::default();
        let closed = Ending::Closed {
            code: 4429,
            reason: r#"{"retryAfter": 30}"#.into(),
        };
        let wait = retry(backoff.next(&closed, Duration::ZERO, &timings()));
        assert!(
            wait >= Duration::from_secs(30) && wait <= Duration::from_secs(36),
            "{wait:?}"
        );

        let refused = Ending::Refused {
            status: 429,
            code: Some("rate_limited".into()),
            retry_after: Some(Duration::from_secs(10)),
        };
        let wait = retry(backoff.next(&refused, Duration::ZERO, &timings()));
        assert!(
            wait >= Duration::from_secs(10) && wait <= Duration::from_secs(12),
            "{wait:?}"
        );
    }

    #[test]
    fn final_closes_stop_the_link() {
        let mut backoff = Backoff::default();
        let cases = [
            (4401, Stop::SignedOut),
            (4402, Stop::Lapsed),
            (4403, Stop::Banned),
            (4002, Stop::Outdated),
            (4001, Stop::Replaced),
        ];
        for (code, stop) in cases {
            let ending = Ending::Closed {
                code,
                reason: String::new(),
            };
            assert_eq!(
                backoff.next(&ending, Duration::ZERO, &timings()),
                Next::Stop(stop)
            );
        }
        for (status, stop) in [
            (401, Stop::SignedOut),
            (402, Stop::Lapsed),
            (403, Stop::Banned),
            (426, Stop::Outdated),
        ] {
            let ending = Ending::Refused {
                status,
                code: None,
                retry_after: None,
            };
            assert_eq!(
                backoff.next(&ending, Duration::ZERO, &timings()),
                Next::Stop(stop)
            );
        }
        let missing = Ending::Refused {
            status: 409,
            code: Some("remote_key_missing".into()),
            retry_after: None,
        };
        assert!(matches!(
            backoff.next(&missing, Duration::ZERO, &timings()),
            Next::UploadKey(_)
        ));
    }

    /// A replayable source of `[0, 1)`: a 64-bit linear congruential
    /// generator.
    fn seeded(seed: u64) -> Box<dyn FnMut() -> f64 + Send> {
        let mut state = seed;
        Box::new(move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1_u64 << 53) as f64
        })
    }

    /// The reconnect storm of the PR 2 gate, the daemon's part, on a
    /// simulated clock with replayed randomness: 200 daemons across 20
    /// accounts, closed with `1012` at once. Each makes one attempt, at the
    /// time its backoff chose; together they spread over the 10 seconds,
    /// every account's machines too, and all are back well within 60 s.
    /// (That the Worker sees them spread, and that malformed tokens never
    /// reach D1, is the relay's part of the gate, in #65.)
    #[test]
    fn a_reconnect_storm_of_200_daemons_spreads_over_ten_seconds() {
        let mut attempts: Vec<(usize, Duration)> = Vec::new();
        for daemon in 0..200_usize {
            let account = daemon % 20;
            let mut backoff = Backoff::with_random(seeded(daemon as u64 + 1));
            let ending = Ending::Closed {
                code: 1012,
                reason: String::new(),
            };
            let wait = retry(backoff.next(&ending, Duration::from_secs(3600), &timings()));
            attempts.push((account, wait));
        }
        let mut seconds = [0_u32; 10];
        for (_, wait) in &attempts {
            assert!(*wait < Duration::from_secs(10));
            seconds[wait.as_secs() as usize] += 1;
        }
        // About 20 a second, never a pile-up in one second.
        assert!(
            seconds.iter().all(|&count| (8..=36).contains(&count)),
            "{seconds:?}"
        );
        for account in 0..20 {
            let mut times: Vec<Duration> = attempts
                .iter()
                .filter(|(owner, _)| *owner == account)
                .map(|(_, wait)| *wait)
                .collect();
            times.sort();
            assert!(
                times[9] - times[0] >= Duration::from_secs(3),
                "account {account}: {times:?}"
            );
        }
        let latest = attempts
            .iter()
            .map(|(_, wait)| *wait)
            .max()
            .unwrap_or_default();
        assert!(latest < Duration::from_secs(60));
    }

    /// The same source replays the same waits.
    #[test]
    fn injected_randomness_replays() {
        let ending = Ending::Lost;
        let mut first = Backoff::with_random(seeded(7));
        let mut second = Backoff::with_random(seeded(7));
        for _ in 0..10 {
            assert_eq!(
                first.next(&ending, Duration::ZERO, &timings()),
                second.next(&ending, Duration::ZERO, &timings())
            );
        }
    }

    /// The reconnect storm, with the system's randomness: 200 daemons
    /// closed with `1012` at once come back spread over the 10 seconds.
    #[test]
    fn a_deploy_close_spreads_a_reconnect_storm_over_ten_seconds() {
        let mut seconds = [0_u32; 10];
        let mut latest = Duration::ZERO;
        for _ in 0..200 {
            let mut backoff = Backoff::default();
            let ending = Ending::Closed {
                code: 1012,
                reason: String::new(),
            };
            let wait = retry(backoff.next(&ending, Duration::from_secs(3600), &timings()));
            assert!(wait < Duration::from_secs(10));
            latest = latest.max(wait);
            seconds[wait.as_secs() as usize] += 1;
        }
        // Uniform over 10 s: about 20 a second, and never a pile-up.
        assert!(seconds.iter().all(|&count| count <= 50), "{seconds:?}");
        assert!(latest >= Duration::from_secs(8), "{latest:?}");
        // A storm of failures after that stays bounded by the backoff cap.
        let mut backoff = Backoff::default();
        let total: Duration = (0..30)
            .map(|_| retry(backoff.next(&Ending::Unreachable, Duration::ZERO, &timings())))
            .max()
            .unwrap_or_default();
        assert!(total <= Duration::from_secs(300));
    }

    #[test]
    fn random_fractions_stay_in_range() {
        for _ in 0..1000 {
            let value = random_fraction();
            assert!((0.0..1.0).contains(&value));
        }
    }
}
