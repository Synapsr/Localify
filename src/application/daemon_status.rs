use std::net::Ipv4Addr;

use anyhow::Result;

use super::ports::{CertificateManager, DaemonControl, NetworkInfo};

/// Snapshot of the daemon's current status (pid, network, CA).
///
/// Domain listing is handled separately by `ListAllDomains`.
pub struct DaemonStatus {
    pub pid: Option<u32>,
    #[allow(dead_code)] // Used by the desktop through the library crate.
    pub lan_ip: Ipv4Addr,
    pub ca_installed: bool,
    /// The CA exists but the system does not trust it yet: HTTP and Node.js work,
    /// browsers warn about HTTPS until the user confirms trust on a screen.
    pub ca_trust_pending: bool,
}

/// Application service for querying daemon status.
pub struct QueryDaemonStatus<'a> {
    daemon: &'a dyn DaemonControl,
    certs: &'a dyn CertificateManager,
    network: &'a dyn NetworkInfo,
}

impl<'a> QueryDaemonStatus<'a> {
    pub fn new(
        daemon: &'a dyn DaemonControl,
        certs: &'a dyn CertificateManager,
        network: &'a dyn NetworkInfo,
    ) -> Self {
        Self {
            daemon,
            certs,
            network,
        }
    }

    /// Collect the current daemon status.
    pub fn execute(&self) -> Result<DaemonStatus> {
        let pid = self.daemon.get_running_pid()?;
        let lan_ip = self.network.lan_ip().unwrap_or(Ipv4Addr::LOCALHOST);
        let ca_installed = self.certs.is_ca_installed().unwrap_or(false);

        Ok(DaemonStatus {
            pid,
            lan_ip,
            ca_installed,
            ca_trust_pending: !ca_installed && self.certs.ca_exists(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::testkit::*;

    #[test]
    fn reports_running_daemon() {
        let daemon = InMemoryDaemonControl::running(42);
        let certs = InMemoryCertificateManager::with_ca_installed();
        let network = InMemoryNetworkInfo::with_ip(Ipv4Addr::new(10, 0, 0, 1));
        let svc = QueryDaemonStatus::new(&daemon, &certs, &network);

        let status = svc.execute().unwrap();

        assert_eq!(status.pid, Some(42));
        assert_eq!(status.lan_ip, Ipv4Addr::new(10, 0, 0, 1));
        assert!(status.ca_installed);
        assert!(!status.ca_trust_pending);
    }

    #[test]
    fn a_ca_the_system_does_not_trust_yet_is_pending() {
        let daemon = InMemoryDaemonControl::stopped();
        let certs = InMemoryCertificateManager::created_but_untrusted();
        let network = InMemoryNetworkInfo::unavailable();
        let svc = QueryDaemonStatus::new(&daemon, &certs, &network);

        let status = svc.execute().unwrap();

        assert!(!status.ca_installed);
        assert!(status.ca_trust_pending);
    }

    #[test]
    fn reports_stopped_daemon() {
        let daemon = InMemoryDaemonControl::stopped();
        let certs = InMemoryCertificateManager::new();
        let network = InMemoryNetworkInfo::unavailable();
        let svc = QueryDaemonStatus::new(&daemon, &certs, &network);

        let status = svc.execute().unwrap();

        assert_eq!(status.pid, None);
        assert_eq!(status.lan_ip, Ipv4Addr::LOCALHOST);
        assert!(!status.ca_installed);
        assert!(!status.ca_trust_pending);
    }
}
