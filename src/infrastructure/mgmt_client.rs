use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::application::ports::{
    DaemonConnection, DaemonConnectionError, DaemonRegistrationInfo, DaemonRuntimeInfo,
};
use crate::domain::{
    DomainPattern, DomainRegistration, PathPrefix, ProxyTarget, Route, RouteTarget,
};
use crate::infrastructure::launch_sessions::{LaunchSessionDto, LaunchSessionUpdate};

/// What the daemon should do after it stops serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownKind {
    /// Exit cleanly. launchd does not bring it back until a connection arrives.
    Stop,
    /// Exit with the restart code, so launchd starts the binary at its path again.
    Restart,
}

/// Concrete infrastructure adapter implementing `DaemonConnection`
/// via a synchronous Unix socket client to the daemon's management socket.
pub struct MgmtSocketClient {
    socket_path: PathBuf,
}

impl MgmtSocketClient {
    pub fn new(socket_path: &Path) -> Self {
        Self {
            socket_path: socket_path.to_path_buf(),
        }
    }

    /// Send a command to the management socket and return the parsed JSON response.
    fn send_command(&self, cmd: &str) -> Result<serde_json::Value, DaemonConnectionError> {
        let mut stream = UnixStream::connect(&self.socket_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                DaemonConnectionError::ConnectionFailed(error.into())
            } else {
                DaemonConnectionError::NotRunning
            }
        })?;

        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| DaemonConnectionError::ConnectionFailed(e.into()))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| DaemonConnectionError::ConnectionFailed(e.into()))?;

        stream
            .write_all(format!("{cmd}\n").as_bytes())
            .map_err(|e| DaemonConnectionError::ConnectionFailed(e.into()))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| DaemonConnectionError::ConnectionFailed(e.into()))?;

        let resp: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| DaemonConnectionError::ProtocolError(e.to_string()))?;

        if !resp["ok"].as_bool().unwrap_or(false) {
            let msg = resp["error"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            return Err(DaemonConnectionError::ProtocolError(msg));
        }

        Ok(resp)
    }

    /// Reports the start, heartbeat or stop of a service launched with a
    /// launch profile, for the proxy header, desktop badge and audit log.
    #[allow(dead_code)] // Used by the CLI and desktop through the library crate.
    pub fn report_launch_session(
        &self,
        update: &LaunchSessionUpdate,
    ) -> Result<(), DaemonConnectionError> {
        let command = serde_json::json!({ "cmd": "launch_session", "update": update });
        self.send_command(&command.to_string()).map(|_| ())
    }

    /// Asks the daemon to exit. It replies before it stops serving.
    pub fn shutdown(&self, kind: ShutdownKind) -> Result<(), DaemonConnectionError> {
        let command = match kind {
            ShutdownKind::Stop => "shutdown",
            ShutdownKind::Restart => "restart",
        };
        self.send_command(command).map(|_| ())
    }

    /// PID of the daemon answering on the socket. Unlike the PID file, this cannot
    /// be stale.
    pub fn running_pid(&self) -> Result<u32, DaemonConnectionError> {
        let response = self.send_command("status")?;
        response["data"]["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| DaemonConnectionError::ProtocolError("status has no pid".into()))
    }

    /// Services currently running with a launch profile.
    #[allow(dead_code)] // Used by the desktop through the library crate.
    pub fn launch_sessions(&self) -> Result<Vec<LaunchSessionDto>, DaemonConnectionError> {
        let response = self.send_command("launch_sessions")?;
        serde_json::from_value(response["data"]["sessions"].clone())
            .map_err(|error| DaemonConnectionError::ProtocolError(error.to_string()))
    }
}

impl DaemonConnection for MgmtSocketClient {
    fn status(&self) -> Result<DaemonRuntimeInfo, DaemonConnectionError> {
        let resp = self.send_command("status")?;
        let data = &resp["data"];

        let pid = data["pid"]
            .as_u64()
            .ok_or_else(|| DaemonConnectionError::ProtocolError("missing pid".into()))?
            as u32;
        let registration_count = data["registration_count"].as_u64().unwrap_or(0) as usize;
        let http_port = data["http_port"].as_u64().unwrap_or(80) as u16;
        let https_port = data["https_port"].as_u64().unwrap_or(443) as u16;
        let dns_port = data["dns_port"].as_u64().unwrap_or(1053) as u16;

        // Status doesn't return full registrations, just a count.
        // Return empty vec — callers should use list_registrations() for full data.
        Ok(DaemonRuntimeInfo {
            pid,
            registrations: Vec::with_capacity(registration_count),
            http_port,
            https_port,
            dns_port,
        })
    }

    fn reload(&self) -> Result<(), DaemonConnectionError> {
        self.send_command("reload")?;
        Ok(())
    }

    fn list_registrations(&self) -> Result<Vec<DomainRegistration>, DaemonConnectionError> {
        self.list_registration_info().map(|items| {
            items
                .into_iter()
                .map(|item| item.registration)
                .collect::<Vec<_>>()
        })
    }

    fn list_registration_info(&self) -> Result<Vec<DaemonRegistrationInfo>, DaemonConnectionError> {
        let resp = self.send_command("list")?;
        let domains = resp["data"]["domains"]
            .as_array()
            .ok_or_else(|| DaemonConnectionError::ProtocolError("missing domains array".into()))?;

        let mut registrations = Vec::with_capacity(domains.len());

        for d in domains {
            let pattern_str = d["pattern"]
                .as_str()
                .ok_or_else(|| DaemonConnectionError::ProtocolError("missing pattern".into()))?;
            let source_str = d["source"].as_str().unwrap_or("config");
            let https = d["https"].as_bool().unwrap_or(false);

            // Parse the pattern string (handles "*.foo.localify" and "foo.localify")
            let is_wildcard = pattern_str.starts_with("*.");
            let base_name = if is_wildcard {
                &pattern_str[2..]
            } else {
                pattern_str
            };
            let pattern = DomainPattern::from_name(base_name, is_wildcard).map_err(|e| {
                DaemonConnectionError::ProtocolError(format!(
                    "invalid pattern '{pattern_str}': {e}"
                ))
            })?;

            // Parse routes (supports both Proxy and StaticFiles targets)
            let routes = d["routes"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|r| {
                            let path_str = r["path"].as_str()?;
                            let target_str = r["target"].as_str()?;
                            let path = PathPrefix::new(path_str).ok()?;
                            let target = if target_str.starts_with('/') {
                                RouteTarget::StaticFiles(std::path::PathBuf::from(target_str))
                            } else {
                                ProxyTarget::parse(target_str)
                                    .map(RouteTarget::Proxy)
                                    .ok()?
                            };
                            Some(Route::new(path, target))
                        })
                        .collect()
                })
                .unwrap_or_default();

            let source = if source_str != "config" {
                crate::domain::RegistrationSource::External
            } else {
                crate::domain::RegistrationSource::Config
            };
            let mut reg = DomainRegistration::with_source(pattern, routes, source);
            if https {
                reg.enable_https();
            }

            registrations.push(DaemonRegistrationInfo {
                registration: reg,
                last_accessed_at: d["last_accessed_at"].as_u64(),
            });
        }

        Ok(registrations)
    }
}
