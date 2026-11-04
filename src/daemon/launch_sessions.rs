//! In-memory registry of services running with a launch profile.
//!
//! Every start, stop and lost session is written to the daemon log as an
//! audit line: service, profile, sources and timestamps, never values.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::infrastructure::launch_sessions::{
    LAUNCH_SESSION_LEASE, LaunchSessionDto, LaunchSessionUpdate, format_unix_time, unix_now,
};

#[derive(Clone)]
pub struct LaunchSessions {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    lease: Duration,
}

struct Entry {
    session: LaunchSessionDto,
    last_seen: Instant,
}

impl Default for LaunchSessions {
    fn default() -> Self {
        Self::with_lease(LAUNCH_SESSION_LEASE)
    }
}

impl LaunchSessions {
    fn with_lease(lease: Duration) -> Self {
        Self {
            entries: Arc::default(),
            lease,
        }
    }

    pub fn apply(&self, update: LaunchSessionUpdate) {
        let mut entries = self.lock();
        match update {
            LaunchSessionUpdate::Running { session } => {
                let id = session.id.clone();
                if !entries.contains_key(&id) {
                    log_started(&session);
                }
                entries.insert(
                    id,
                    Entry {
                        session,
                        last_seen: Instant::now(),
                    },
                );
            }
            LaunchSessionUpdate::Stopped { session, reason } => {
                entries.remove(&session.id);
                log_stopped(&session, &reason);
            }
        }
    }

    /// Sessions whose `localify dev` reported within the lease.
    pub fn active(&self) -> Vec<LaunchSessionDto> {
        let mut entries = self.lock();
        self.drop_expired(&mut entries);
        let mut sessions: Vec<_> = entries
            .values()
            .map(|entry| entry.session.clone())
            .collect();
        sessions.sort_by_key(|session| session.started_at);
        sessions
    }

    /// Profile name to advertise for responses from a loopback port.
    pub fn profile_for_port(&self, port: u16) -> Option<String> {
        let mut entries = self.lock();
        self.drop_expired(&mut entries);
        entries
            .values()
            .find(|entry| entry.session.target_port == port)
            .map(|entry| entry.session.profile.clone())
    }

    /// Profile name to show in the browser for pages served from a port.
    pub fn indicator_for_port(&self, port: u16) -> Option<String> {
        let mut entries = self.lock();
        self.drop_expired(&mut entries);
        entries
            .values()
            .find(|entry| entry.session.target_port == port && entry.session.browser_indicator)
            .map(|entry| entry.session.profile.clone())
    }

    fn drop_expired(&self, entries: &mut HashMap<String, Entry>) {
        let lease = self.lease;
        entries.retain(|_, entry| {
            let alive = entry.last_seen.elapsed() <= lease;
            if !alive {
                log_stopped(
                    &entry.session,
                    &format!("no heartbeat for {}s (process killed?)", lease.as_secs()),
                );
            }
            alive
        });
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn log_started(session: &LaunchSessionDto) {
    let expires_at = session
        .expires_at
        .map(format_unix_time)
        .unwrap_or_else(|| "none".into());
    if session.danger {
        warn!(
            target: "localify::audit",
            event = "launch_profile_started",
            project = %session.project_id,
            environment = session.environment_slug.as_deref().unwrap_or("primary"),
            service = %session.service_id,
            profile = %session.profile,
            danger = true,
            port = session.target_port,
            sources = %session.sources.join("; "),
            started_at = %format_unix_time(session.started_at),
            expires_at = %expires_at,
            "Dangerous launch profile started"
        );
    } else {
        info!(
            target: "localify::audit",
            event = "launch_profile_started",
            project = %session.project_id,
            environment = session.environment_slug.as_deref().unwrap_or("primary"),
            service = %session.service_id,
            profile = %session.profile,
            danger = false,
            port = session.target_port,
            sources = %session.sources.join("; "),
            started_at = %format_unix_time(session.started_at),
            "Launch profile started"
        );
    }
}

fn log_stopped(session: &LaunchSessionDto, reason: &str) {
    let stopped_at = unix_now();
    info!(
        target: "localify::audit",
        event = "launch_profile_stopped",
        project = %session.project_id,
        environment = session.environment_slug.as_deref().unwrap_or("primary"),
        service = %session.service_id,
        profile = %session.profile,
        danger = session.danger,
        port = session.target_port,
        reason = %reason,
        started_at = %format_unix_time(session.started_at),
        stopped_at = %format_unix_time(stopped_at),
        "Launch profile stopped"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, port: u16) -> LaunchSessionDto {
        LaunchSessionDto {
            id: id.into(),
            project_id: "acme".into(),
            environment_slug: None,
            service_id: "web".into(),
            profile: "prod".into(),
            danger: true,
            target_port: port,
            started_at: 1,
            expires_at: None,
            sources: vec![],
            browser_indicator: true,
        }
    }

    #[test]
    fn running_sessions_advertise_their_profile_until_stopped() {
        let sessions = LaunchSessions::default();
        sessions.apply(LaunchSessionUpdate::Running {
            session: session("a", 3000),
        });

        assert_eq!(sessions.profile_for_port(3000).as_deref(), Some("prod"));
        assert_eq!(sessions.indicator_for_port(3000).as_deref(), Some("prod"));
        assert_eq!(sessions.profile_for_port(3001), None);
        assert_eq!(sessions.active().len(), 1);

        sessions.apply(LaunchSessionUpdate::Stopped {
            session: session("a", 3000),
            reason: "exited".into(),
        });
        assert_eq!(sessions.profile_for_port(3000), None);
        assert!(sessions.active().is_empty());
    }

    #[test]
    fn sessions_without_heartbeat_expire() {
        let sessions = LaunchSessions::with_lease(Duration::ZERO);
        sessions.apply(LaunchSessionUpdate::Running {
            session: session("a", 3000),
        });
        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(sessions.profile_for_port(3000), None);
        assert!(sessions.active().is_empty());
    }
}
