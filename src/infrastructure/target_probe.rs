use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::domain::{ProxyTarget, RouteTarget};

#[allow(dead_code)]
const TARGET_PROBE_TIMEOUT: Duration = Duration::from_millis(150);

#[allow(dead_code)]
pub fn is_route_target_reachable(target: &RouteTarget) -> bool {
    match target {
        RouteTarget::Proxy(proxy_target) => is_proxy_target_reachable(proxy_target),
        RouteTarget::StaticFiles(path) => path.is_dir(),
    }
}

#[allow(dead_code)]
fn is_proxy_target_reachable(target: &ProxyTarget) -> bool {
    let Ok(addresses) = (target.host(), target.port().value()).to_socket_addrs() else {
        return false;
    };

    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, TARGET_PROBE_TIMEOUT).is_ok())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::is_route_target_reachable;
    use crate::domain::RouteTarget;

    #[test]
    fn proxy_target_is_reachable_when_port_is_listening() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = RouteTarget::parse(&format!("127.0.0.1:{port}")).unwrap();

        assert!(is_route_target_reachable(&target));
    }

    #[test]
    fn proxy_target_is_not_reachable_when_port_is_closed() {
        let port = find_closed_loopback_port().unwrap();

        let target = RouteTarget::parse(&format!("127.0.0.1:{port}")).unwrap();

        assert!(!is_route_target_reachable(&target));
    }

    fn find_closed_loopback_port() -> Option<u16> {
        for _ in 0..16 {
            let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
            let port = listener.local_addr().ok()?.port();
            drop(listener);

            let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into();
            let result = TcpStream::connect_timeout(&address, Duration::from_millis(25));
            if result.is_err() {
                return Some(port);
            }
        }

        None
    }

    #[test]
    fn static_target_is_reachable_when_directory_exists() {
        let dir = TempDir::new().unwrap();
        let target = RouteTarget::StaticFiles(PathBuf::from(dir.path()));

        assert!(is_route_target_reachable(&target));
    }

    #[test]
    fn static_target_is_not_reachable_when_directory_is_missing() {
        let target = RouteTarget::StaticFiles(PathBuf::from("/definitely/missing/path"));
        assert!(!is_route_target_reachable(&target));
    }
}
