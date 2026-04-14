use std::io::ErrorKind;
use std::net::{TcpListener, UdpSocket};

use anyhow::Result;

fn tcp_probe(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
}

fn udp_probe(port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::bind(("0.0.0.0", port))
}

fn tcp_port_guidance(port: u16) -> String {
    format!("lsof -nP -iTCP:{port} -sTCP:LISTEN")
}

fn udp_port_guidance(port: u16) -> String {
    format!("lsof -nP -iUDP:{port}")
}

fn map_port_error(protocol: &str, port: u16, error: std::io::Error) -> anyhow::Error {
    match error.kind() {
        ErrorKind::AddrInUse => anyhow::anyhow!(
            "{protocol} port {port} is already in use.\nInspect with: {}",
            if protocol == "DNS" {
                udp_port_guidance(port)
            } else {
                tcp_port_guidance(port)
            }
        ),
        ErrorKind::PermissionDenied => {
            anyhow::anyhow!("{protocol} port {port} requires elevated privileges.")
        }
        _ => anyhow::anyhow!("Failed to probe {protocol} port {port}: {error}"),
    }
}

/// Fail fast with an actionable error when the daemon ports cannot be bound.
pub fn ensure_daemon_ports_available(http_port: u16, https_port: u16, dns_port: u16) -> Result<()> {
    let http = tcp_probe(http_port).map_err(|error| map_port_error("HTTP", http_port, error))?;
    let https =
        tcp_probe(https_port).map_err(|error| map_port_error("HTTPS", https_port, error))?;
    let dns = udp_probe(dns_port).map_err(|error| map_port_error("DNS", dns_port, error))?;

    drop(dns);
    drop(https);
    drop(http);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_daemon_ports_available;

    #[test]
    fn reports_tcp_port_in_use() {
        let listener = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        let error = ensure_daemon_ports_available(port, port + 1, port + 2)
            .err()
            .unwrap();

        assert!(error.to_string().contains("already in use"));
        assert!(error.to_string().contains(&port.to_string()));
    }
}
