use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::branding::BRANDING;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemResolutionCheck {
    pub hostname: String,
    pub healthy: bool,
    pub resolved_ips: Vec<IpAddr>,
    pub message: Option<String>,
}

impl SystemResolutionCheck {
    fn success(hostname: String, resolved_ips: Vec<IpAddr>) -> Self {
        Self {
            hostname,
            healthy: true,
            resolved_ips,
            message: None,
        }
    }

    fn failure(hostname: String, resolved_ips: Vec<IpAddr>, message: String) -> Self {
        Self {
            hostname,
            healthy: false,
            resolved_ips,
            message: Some(message),
        }
    }
}

pub fn check_system_resolution(timeout: Duration) -> SystemResolutionCheck {
    let hostname = format!("localify-healthcheck{}", BRANDING.domain_suffix());
    let lookup_hostname = hostname.clone();
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let result = (lookup_hostname.as_str(), 443)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect::<Vec<_>>())
            .map_err(|error| error.to_string());

        let _ = sender.send(result);
    });

    match receiver.recv_timeout(timeout) {
        Ok(result) => finish_system_resolution_check(hostname, result),
        Err(mpsc::RecvTimeoutError::Timeout) => SystemResolutionCheck::failure(
            hostname.clone(),
            Vec::new(),
            format!("System DNS lookup for {hostname} timed out."),
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => SystemResolutionCheck::failure(
            hostname.clone(),
            Vec::new(),
            format!("System DNS lookup for {hostname} could not complete."),
        ),
    }
}

fn finish_system_resolution_check(
    hostname: String,
    result: Result<Vec<IpAddr>, String>,
) -> SystemResolutionCheck {
    match result {
        Ok(resolved_ips) if resolved_ips.iter().any(IpAddr::is_loopback) => {
            SystemResolutionCheck::success(hostname, resolved_ips)
        }
        Ok(resolved_ips) if resolved_ips.is_empty() => SystemResolutionCheck::failure(
            hostname.clone(),
            resolved_ips,
            format!("System DNS lookup for {hostname} returned no addresses."),
        ),
        Ok(resolved_ips) => SystemResolutionCheck::failure(
            hostname.clone(),
            resolved_ips,
            format!("System DNS lookup for {hostname} did not return a loopback address."),
        ),
        Err(error) => SystemResolutionCheck::failure(
            hostname.clone(),
            Vec::new(),
            format!("System DNS lookup for {hostname} failed: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn loopback_resolution_is_healthy() {
        let check = finish_system_resolution_check(
            "localify-healthcheck.localify".into(),
            Ok(vec![
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ]),
        );

        assert!(check.healthy);
        assert!(check.message.is_none());
    }

    #[test]
    fn non_loopback_resolution_is_unhealthy() {
        let check = finish_system_resolution_check(
            "localify-healthcheck.localify".into(),
            Ok(vec![IpAddr::V4(Ipv4Addr::new(10, 2, 0, 1))]),
        );

        assert!(!check.healthy);
        assert!(
            check
                .message
                .as_deref()
                .unwrap()
                .contains("did not return a loopback address")
        );
    }

    #[test]
    fn lookup_error_is_unhealthy() {
        let check = finish_system_resolution_check(
            "localify-healthcheck.localify".into(),
            Err("nodename nor servname provided".into()),
        );

        assert!(!check.healthy);
        assert!(check.resolved_ips.is_empty());
        assert!(check.message.as_deref().unwrap().contains("failed"));
    }
}
