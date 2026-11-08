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
use super::backoff::up_to;
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
    /// The cloud said no: `402`, `403` or `401`.
    Refused(String),
    /// The cloud could not be reached; a lease still running stays.
    Unreachable(String),
}

pub struct LeaseKeeper {
    cloud: CloudClient,
    timings: Timings,
    lease: Mutex<Option<Lease>>,
    /// One request at a time: a caller that waited finds the fresh lease.
    fetching: tokio::sync::Mutex<()>,
}

impl LeaseKeeper {
    pub fn new(cloud: CloudClient, timings: Timings) -> Self {
        Self {
            cloud,
            timings,
            lease: Mutex::new(None),
            fetching: tokio::sync::Mutex::new(()),
        }
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

    /// Asks for a new lease now, whatever the current one.
    pub async fn renew(&self) -> Result<(), LeaseError> {
        let _one = self.fetching.lock().await;
        self.fetch().await
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
                Err(LeaseError::Refused(code))
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
        self.lease()
            .as_ref()
            .is_none_or(|lease| now >= lease.renew_at)
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
    fn a_short_or_huge_ttl_stays_sane() {
        let sent = Instant::now();
        let short = Lease::new(grant(4), sent, &Timings::default());
        assert!(short.renew_at < short.expires);
        let huge = Lease::new(grant(u64::MAX / 4), sent, &Timings::default());
        assert!(!huge.is_valid(sent + Duration::from_secs(601)));
    }
}
