use anyhow::Result;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::time::Duration;

use crate::application::ports::NetworkInfo;
use crate::branding::BRANDING;
use crate::infrastructure::network::get_network_info;

/// A free port on this machine refuses in well under a millisecond.
const LISTENER_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// The address a daemon started outside launchd listens on.
pub fn listen_address(lan_access: bool) -> IpAddr {
    if lan_access {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    }
}

fn tcp_port_guidance(port: u16) -> String {
    format!("sudo lsof -nP -iTCP:{port} -sTCP:LISTEN")
}

fn udp_port_guidance(port: u16) -> String {
    format!("sudo lsof -nP -iUDP:{port}")
}

/// An actionable message for a port the daemon cannot listen on.
pub fn map_port_error(protocol: &str, port: u16, error: std::io::Error) -> anyhow::Error {
    match error.kind() {
        ErrorKind::AddrInUse => anyhow::anyhow!(
            "{protocol} port {port} is already in use.\nInspect with: {}",
            if protocol == "DNS" {
                udp_port_guidance(port)
            } else {
                tcp_port_guidance(port)
            }
        ),
        ErrorKind::PermissionDenied => anyhow::anyhow!(
            "{protocol} port {port} requires elevated privileges.\n{}",
            low_port_guidance()
        ),
        _ => anyhow::anyhow!("Failed to probe {protocol} port {port}: {error}"),
    }
}

/// macOS lets a user listen on a low port on all interfaces but not on loopback,
/// so a daemon on this path cannot serve 80 and 443 without launchd.
fn low_port_guidance() -> String {
    if cfg!(target_os = "macos") {
        format!(
            "Run `{}` so launchd can open it for the daemon, or use ports from 1024 up.",
            BRANDING.admin_command("install")
        )
    } else {
        format!("Run: {}", BRANDING.admin_command("start"))
    }
}

/// Fail fast with an actionable error when the daemon ports cannot be bound.
pub fn ensure_daemon_ports_available(
    http_port: u16,
    https_port: u16,
    dns_port: u16,
    lan_access: bool,
) -> Result<()> {
    let address = listen_address(lan_access);
    let http = TcpListener::bind((address, http_port))
        .map_err(|error| map_port_error("HTTP", http_port, error))?;
    let https = TcpListener::bind((address, https_port))
        .map_err(|error| map_port_error("HTTPS", https_port, error))?;
    let dns = UdpSocket::bind((address, dns_port))
        .map_err(|error| map_port_error("DNS", dns_port, error))?;

    drop(dns);
    drop(https);
    drop(http);

    Ok(())
}

/// Looks for another listener on these ports before asking for an approval to open
/// them on every interface. A test bind can't tell, because our own service holds
/// them on loopback, and macOS hides the socket table from some programs. A miss only
/// costs the approval: the script finds the port launchd could not open and puts the
/// old service back.
#[allow(dead_code)] // Used by the desktop through the library crate.
pub fn ensure_lan_ports_available(http_port: u16, https_port: u16) -> Result<()> {
    match get_network_info().lan_ip() {
        Some(address) => ensure_no_listener_at(address, http_port, https_port),
        None => Ok(()),
    }
}

/// Our service is not on this address yet, so a free port refuses at once. Anything
/// else belongs to someone else, including a listener that lets the connection hang.
fn ensure_no_listener_at(address: Ipv4Addr, http_port: u16, https_port: u16) -> Result<()> {
    for (protocol, port) in [("HTTP", http_port), ("HTTPS", https_port)] {
        let taken =
            match TcpStream::connect_timeout(&(address, port).into(), LISTENER_PROBE_TIMEOUT) {
                Ok(_) => true,
                Err(error) => error.kind() == ErrorKind::TimedOut,
            };
        if taken {
            anyhow::bail!(
                "Localify cannot listen on the network: {}",
                map_port_error(protocol, port, ErrorKind::AddrInUse.into())
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_tcp_port_in_use() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        let error = ensure_daemon_ports_available(port, port + 1, port + 2, false)
            .err()
            .unwrap();

        assert!(error.to_string().contains("already in use"));
        assert!(error.to_string().contains(&port.to_string()));
    }

    #[test]
    fn a_listener_blocks_the_lan_opt_in_before_any_prompt() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        let error = ensure_no_listener_at(Ipv4Addr::LOCALHOST, port, port)
            .err()
            .unwrap();

        assert!(error.to_string().contains("cannot listen on the network"));
        assert!(error.to_string().contains("already in use"));
    }

    #[test]
    fn a_port_that_refuses_connections_is_free_for_the_lan_opt_in() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        ensure_no_listener_at(Ipv4Addr::LOCALHOST, port, port).unwrap();
    }

    #[test]
    fn the_default_address_stays_on_this_machine() {
        assert!(listen_address(false).is_loopback());
        assert!(listen_address(true).is_unspecified());
    }
}
