//! The lease an endpoint holds while it has a session open
//! (`docs/remote-machines.md`, "Entitlement", "Leases"). It lasts `ttl`
//! seconds from the moment its request was sent, on the monotonic clock: no
//! time from the cloud is ever compared with the wall clock. A detected wake
//! from sleep expires it at once, since the monotonic clock may not count
//! sleep.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tokio::time::Instant;
use tracing::info;

use super::Timings;
use super::backoff::{random_fraction, up_to};
use crate::infrastructure::cloud::api::CloudClient;
use crate::infrastructure::cloud::http::CloudError;
use crate::infrastructure::cloud::types::LeaseGrant;

/// A lease never lasts longer than this, whatever the cloud says.
const MAX_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    expires: Instant,
    renew_at: Instant,
    machines: HashSet<String>,
}

impl Lease {
    /// A grant whose request was sent at `sent`.
    pub fn new(grant: LeaseGrant, sent: Instant, timings: &Timings) -> Self {
        let ttl = Duration::from_secs(grant.ttl).min(MAX_TTL);
        // Renew every 45 s, up to 5 s sooner, and never after the lease
        // ends.
        let renew = timings
            .lease_renew
            .saturating_sub(up_to(timings.lease_renew_jitter))
            .min(ttl.mul_f64(0.75));
        Self {
            expires: sent + ttl,
            renew_at: sent + renew,
            machines: grant.machines.into_iter().collect(),
        }
    }

    pub fn is_valid(&self, now: Instant) -> bool {
        now < self.expires
    }

    /// Whether `machine` is among the account's active machines.
    pub fn covers(&self, machine: &str, now: Instant) -> bool {
        self.is_valid(now) && self.machines.contains(machine)
    }
}

/// Why no lease could be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseError {
    /// The cloud said no: `402`, `403`, or `401` for a revoked token.
    Refused { status: u16, code: String },
    /// The cloud could not be reached, or failed; a lease still running
    /// stays.
    Unreachable(String),
}

impl LeaseError {
    /// The token itself is no longer accepted: the machine is signed out.
    pub fn is_revoked(&self) -> bool {
        matches!(self, Self::Refused { status: 401, .. })
    }
}

/// When a renewal that failed is tried again: `base`, doubling with each
/// failure up to 16 times `base`, then between half of that and all of it.
pub fn retry_delay(failures: u32, base: Duration, fraction: f64) -> Duration {
    let ceiling = base.saturating_mul(2_u32.saturating_pow(failures.saturating_sub(1).min(4)));
    ceiling.mul_f64(0.5 + 0.5 * fraction.clamp(0.0, 1.0))
}

#[derive(Debug, Default)]
struct Renewal {
    failures: u32,
    not_before: Option<Instant>,
}

pub struct LeaseKeeper {
    cloud: CloudClient,
    timings: Timings,
    lease: Mutex<Option<Lease>>,
    /// After a failed renewal, the next one waits; the lease keeps its own
    /// expiry meanwhile.
    renewal: Mutex<Renewal>,
    /// One request at a time: a caller that waited finds the fresh lease.
    fetching: tokio::sync::Mutex<()>,
}

impl LeaseKeeper {
    pub fn new(cloud: CloudClient, timings: Timings) -> Self {
        Self {
            cloud,
            timings,
            lease: Mutex::new(None),
            renewal: Mutex::new(Renewal::default()),
            fetching: tokio::sync::Mutex::new(()),
        }
    }

    fn renewal(&self) -> MutexGuard<'_, Renewal> {
        self.renewal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lease(&self) -> MutexGuard<'_, Option<Lease>> {
        self.lease
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A lease valid now, fetched when there is none.
    pub async fn ensure(&self) -> Result<(), LeaseError> {
        if self.is_valid(Instant::now()) {
            return Ok(());
        }
        let _one = self.fetching.lock().await;
        if self.is_valid(Instant::now()) {
            return Ok(());
        }
        self.fetch().await
    }

    /// Asks for a new lease now, whatever the current one. A failure puts
    /// the next renewal off, with backoff and jitter.
    pub async fn renew(&self) -> Result<(), LeaseError> {
        let _one = self.fetching.lock().await;
        let result = self.fetch().await;
        let mut renewal = self.renewal();
        match &result {
            Err(LeaseError::Unreachable(_)) => {
                renewal.failures = renewal.failures.saturating_add(1);
                let delay = retry_delay(
                    renewal.failures,
                    self.timings.lease_retry,
                    random_fraction(),
                );
                renewal.not_before = Some(Instant::now() + delay);
            }
            _ => *renewal = Renewal::default(),
        }
        result
    }

    async fn fetch(&self) -> Result<(), LeaseError> {
        let sent = Instant::now();
        match self.cloud.remote_lease().await {
            Ok(grant) => {
                *self.lease() = Some(Lease::new(grant, sent, &self.timings));
                Ok(())
            }
            Err(CloudError::Api {
                status: status @ 401..=403,
                code,
                ..
            }) => {
                info!(status, code = %code, "Localify Cloud refused a remote lease");
                *self.lease() = None;
                Err(LeaseError::Refused { status, code })
            }
            Err(error) => Err(LeaseError::Unreachable(error.to_string())),
        }
    }

    pub fn is_valid(&self, now: Instant) -> bool {
        self.lease()
            .as_ref()
            .is_some_and(|lease| lease.is_valid(now))
    }

    pub fn covers(&self, machine: &str, now: Instant) -> bool {
        self.lease()
            .as_ref()
            .is_some_and(|lease| lease.covers(machine, now))
    }

    pub fn needs_renewal(&self, now: Instant) -> bool {
        let due = self
            .lease()
            .as_ref()
            .is_none_or(|lease| now >= lease.renew_at);
        due && self.renewal().not_before.is_none_or(|at| now >= at)
    }

    /// After a wake from sleep: sessions end until a renewal succeeds.
    pub fn expire(&self) {
        *self.lease() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(ttl: u64) -> LeaseGrant {
        LeaseGrant {
            ttl,
            machines: vec!["m1".into(), "m2".into()],
        }
    }

    #[test]
    fn a_lease_counts_its_ttl_from_when_its_request_was_sent() {
        let sent = Instant::now();
        let lease = Lease::new(grant(60), sent, &Timings::default());
        assert!(lease.is_valid(sent + Duration::from_secs(59)));
        assert!(!lease.is_valid(sent + Duration::from_secs(60)));
        assert!(lease.covers("m2", sent));
        assert!(!lease.covers("m3", sent));
        assert!(!lease.covers("m2", sent + Duration::from_secs(61)));
        let renew = lease.renew_at - sent;
        assert!(
            renew <= Duration::from_secs(45) && renew > Duration::from_secs(40),
            "{renew:?}"
        );
    }

    #[test]
    fn a_failed_renewal_backs_off_with_jitter_up_to_a_ceiling() {
        let base = Duration::from_secs(1);
        assert_eq!(retry_delay(1, base, 1.0), base);
        assert_eq!(retry_delay(1, base, 0.0), base / 2);
        assert_eq!(retry_delay(3, base, 1.0), 4 * base);
        assert_eq!(retry_delay(9, base, 1.0), 16 * base);
        assert_eq!(retry_delay(u32::MAX, base, 1.0), 16 * base);
    }

    #[test]
    fn a_short_or_huge_ttl_stays_sane() {
        let sent = Instant::now();
        let short = Lease::new(grant(4), sent, &Timings::default());
        assert!(short.renew_at < short.expires);
        let huge = Lease::new(grant(u64::MAX / 4), sent, &Timings::default());
        assert!(!huge.is_valid(sent + Duration::from_secs(601)));
    }
}
