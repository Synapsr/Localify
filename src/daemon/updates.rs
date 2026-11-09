//! When the daemon checks for updates (docs/desktop.md, "Automatic
//! updates"): 2 minutes after it starts, then every 6 hours plus or minus
//! 30 minutes, and early when a paired machine announces a newer version.
//! Each check is the updater's one coalesced operation, so its cooldown
//! holds however often this asks.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use semver::Version;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::remote::RemoteLink;
use crate::infrastructure::updates::rules::{self, FIRST_CHECK_DELAY};
use crate::infrastructure::updates::{self, Updater};

/// How often the paired machines' versions are read. Reading them is local.
const HINT_POLL: Duration = Duration::from_secs(60);

/// Starts the checks, unless this build has no updater endpoint.
pub fn spawn(updater: Arc<Updater>, remote: RemoteLink, cancel: CancellationToken) {
    if !updater.checks() {
        return;
    }
    tokio::spawn(async move {
        // A replaced bundle restarts the daemon, so this stays true.
        let installed = match tokio::task::spawn_blocking(updates::installed_version).await {
            Ok(version) => version,
            Err(error) => {
                warn!("Could not read the installed version: {error}");
                return;
            }
        };
        let check = move || {
            let updater = Arc::clone(&updater);
            async move {
                if let Err(error) = tokio::task::spawn_blocking(move || updater.cycle()).await {
                    warn!("The update check stopped: {error}");
                }
            }
        };
        let peers = move || remote.paired_versions();
        run(check, peers, installed, rules::next_check_delay, cancel).await;
    });
}

/// The schedule, apart from what a check does, so a paused clock can test
/// it. `next_delay` picks the wait after each scheduled check from a seed.
async fn run<C, F>(
    check: C,
    peers: impl Fn() -> Vec<String>,
    installed: Version,
    next_delay: fn(u64) -> Duration,
    cancel: CancellationToken,
) where
    C: Fn() -> F,
    F: Future<Output = ()>,
{
    let mut next = Instant::now() + FIRST_CHECK_DELAY;
    let mut hinted: Option<Version> = None;
    let mut hints = tokio::time::interval(HINT_POLL);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep_until(next) => {
                check().await;
                next = Instant::now() + next_delay(updates::random_u64());
            }
            _ = hints.tick() => {
                let versions = peers();
                let hint = rules::peer_hint(versions.iter().map(String::as_str), &installed);
                // Once per newer announced version: the cooldown covers the rest.
                if let Some(version) = hint
                    && hinted.as_ref().is_none_or(|seen| rules::is_newer(&version, seen))
                {
                    info!("A paired machine runs Localify {version}: checking for updates");
                    hinted = Some(version);
                    check().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Probe {
        checks: Arc<AtomicUsize>,
        peers: Arc<Mutex<Vec<String>>>,
        cancel: CancellationToken,
    }

    async fn start() -> Probe {
        let checks = Arc::new(AtomicUsize::new(0));
        let peers = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let counted = Arc::clone(&checks);
        let seen = Arc::clone(&peers);
        tokio::spawn(run(
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async {}
            },
            move || seen.lock().unwrap().clone(),
            Version::new(1, 6, 0),
            |_| Duration::from_secs(6 * 60 * 60),
            cancel.clone(),
        ));
        // The schedule starts counting once the task first runs.
        advance(Duration::ZERO).await;
        Probe {
            checks,
            peers,
            cancel,
        }
    }

    async fn advance(by: Duration) {
        tokio::time::advance(by).await;
        // Lets the woken task run.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_check_is_two_minutes_after_start_then_every_six_hours() {
        let probe = start().await;
        advance(Duration::from_secs(119)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 0);
        advance(Duration::from_secs(1)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 1);
        advance(Duration::from_secs(6 * 60 * 60 - 1)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 1);
        advance(Duration::from_secs(1)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 2);
        probe.cancel.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn a_newer_paired_machine_triggers_one_check_per_version() {
        let probe = start().await;
        *probe.peers.lock().unwrap() = vec!["1.5.0".into(), "1.6.0".into()];
        // Past the first scheduled check, with no newer peer.
        advance(Duration::from_secs(125)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 1);

        *probe.peers.lock().unwrap() = vec!["1.7.0".into()];
        advance(Duration::from_secs(60)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 2);
        // The same hint again, or with other build metadata, asks nothing
        // more.
        advance(Duration::from_secs(60 * 5)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 2);
        *probe.peers.lock().unwrap() = vec!["1.7.0+2".into()];
        advance(Duration::from_secs(60)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 2);

        *probe.peers.lock().unwrap() = vec!["1.7.1".into()];
        advance(Duration::from_secs(60)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 3);
        probe.cancel.cancel();
    }
}
