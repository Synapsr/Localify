use std::net::{Ipv6Addr, TcpListener};

use anyhow::{Context, Result};

use crate::config::DaemonConfig;
use crate::infrastructure::launchd_sockets::{self, HTTP_SOCKETS, HTTPS_SOCKETS};
use crate::infrastructure::port_check::{listen_address, map_port_error};

/// The HTTP and HTTPS listeners the daemon serves on, however it got them.
#[derive(Debug)]
pub struct DaemonListeners {
    pub http: Vec<TcpListener>,
    pub https: Vec<TcpListener>,
    /// Whether other devices can reach them. Under launchd the plist decides, so
    /// this reads the real sockets rather than a config the user may have edited.
    pub lan_access: bool,
}

impl DaemonListeners {
    /// Takes the sockets launchd holds for this job. `None` means this process is
    /// not that job: a foreground or dev daemon, which binds for itself instead.
    pub fn from_launchd() -> Result<Option<Self>> {
        let Some(http) = launchd_sockets::activate(HTTP_SOCKETS)
            .context("launchd could not hand over the HTTP sockets")?
        else {
            return Ok(None);
        };
        let https = launchd_sockets::activate(HTTPS_SOCKETS)
            .context("launchd could not hand over the HTTPS sockets")?
            .context("The launchd service has no HTTPS sockets. Reinstall it.")?;

        let lan_access = http.iter().chain(&https).any(|listener| {
            listener
                .local_addr()
                .is_ok_and(|address| address.ip().is_unspecified())
        });

        Ok(Some(Self {
            http,
            https,
            lan_access,
        }))
    }

    /// launchd loads a job even when a port is taken, and the job then has no IPv4
    /// socket for it. Those connections reach the other program, so the daemon refuses
    /// to run rather than look healthy on IPv6 alone.
    pub fn ensure_ipv4(&self) -> Result<()> {
        for (protocol, listeners) in [("HTTP", &self.http), ("HTTPS", &self.https)] {
            let has_ipv4 = listeners
                .iter()
                .any(|listener| listener.local_addr().is_ok_and(|address| address.is_ipv4()));
            anyhow::ensure!(
                has_ipv4,
                "launchd could not open the {protocol} port on IPv4, because another program holds it. Check with: sudo lsof -nP -iTCP -sTCP:LISTEN"
            );
        }
        Ok(())
    }

    /// Binds as a normal process. The default stays on this machine: loopback,
    /// plus IPv6 loopback when the system has it, and every interface only for an
    /// explicit opt-in.
    pub fn bind(config: &DaemonConfig) -> Result<Self> {
        Ok(Self {
            http: bind_port("HTTP", config.http_port, config.lan_access)?,
            https: bind_port("HTTPS", config.https_port, config.lan_access)?,
            lan_access: config.lan_access,
        })
    }

    /// Closes every connection waiting on the sockets, without serving any. While
    /// the user has the daemon stopped, this answers the connection that woke the
    /// job. Leaving it queued would make launchd start the job again straight away.
    pub fn close_waiting_connections(&self) {
        for listener in self.http.iter().chain(&self.https) {
            if listener.set_nonblocking(true).is_err() {
                continue;
            }
            while listener.accept().is_ok() {}
        }
    }
}

fn bind_port(protocol: &str, port: u16, lan_access: bool) -> Result<Vec<TcpListener>> {
    let mut listeners = vec![
        TcpListener::bind((listen_address(lan_access), port))
            .map_err(|error| map_port_error(protocol, port, error))?,
    ];
    if !lan_access && let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
        listeners.push(listener);
    }
    Ok(listeners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpStream;

    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn config(lan_access: bool) -> DaemonConfig {
        let http_port = free_port();
        let https_port = free_port();
        DaemonConfig {
            http_port,
            https_port,
            lan_access,
            ..DaemonConfig::default()
        }
    }

    #[test]
    fn a_port_without_an_ipv4_socket_stops_the_daemon_and_is_named() {
        let ipv4 = || TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let listeners = |http, https| DaemonListeners {
            http,
            https,
            lan_access: false,
        };

        listeners(vec![ipv4()], vec![ipv4()]).ensure_ipv4().unwrap();

        let error = listeners(vec![ipv4()], Vec::new())
            .ensure_ipv4()
            .unwrap_err();
        assert!(error.to_string().contains("HTTPS port"), "{error}");

        // Not every machine has IPv6 loopback, CI containers included.
        if let Ok(ipv6) = TcpListener::bind(("::1", 0)) {
            let error = listeners(vec![ipv6], vec![ipv4()])
                .ensure_ipv4()
                .unwrap_err();
            assert!(error.to_string().contains("HTTP port"), "{error}");
        }
    }

    #[test]
    fn a_process_launchd_did_not_start_binds_for_itself() {
        assert!(DaemonListeners::from_launchd().unwrap().is_none());
    }

    #[test]
    fn it_binds_loopback_by_default() {
        let listeners = DaemonListeners::bind(&config(false)).unwrap();

        assert!(!listeners.lan_access);
        for listener in listeners.http.iter().chain(&listeners.https) {
            assert!(listener.local_addr().unwrap().ip().is_loopback());
        }
    }

    #[test]
    fn the_lan_opt_in_binds_every_interface() {
        let listeners = DaemonListeners::bind(&config(true)).unwrap();

        assert!(listeners.lan_access);
        assert!(
            listeners.http[0]
                .local_addr()
                .unwrap()
                .ip()
                .is_unspecified()
        );
    }

    #[test]
    fn a_taken_port_gives_the_message_the_app_looks_for() {
        let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let config = DaemonConfig {
            http_port: taken.local_addr().unwrap().port(),
            ..config(false)
        };

        let error = DaemonListeners::bind(&config).unwrap_err();

        assert!(error.to_string().contains("already in use"));
    }

    #[test]
    fn a_stopped_daemon_closes_what_is_waiting() {
        let listeners = DaemonListeners::bind(&config(false)).unwrap();
        let address = listeners.http[0].local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();

        listeners.close_waiting_connections();

        let mut buffer = [0u8; 1];
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        assert_eq!(client.read(&mut buffer).unwrap_or(0), 0);
    }
}
