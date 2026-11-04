use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::lifecycle::ShutdownHandle;
use super::router::SharedState;
use crate::infrastructure::launch_sessions::LaunchSessionUpdate;

/// Management commands sent over the Unix socket.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd")]
enum MgmtCommand {
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "reload")]
    Reload,
    #[serde(rename = "list")]
    List,
    /// A `localify dev` launched with a profile reports start, heartbeat or stop.
    #[serde(rename = "launch_session")]
    LaunchSession { update: LaunchSessionUpdate },
    #[serde(rename = "launch_sessions")]
    LaunchSessions,
    /// Exit cleanly.
    #[serde(rename = "shutdown")]
    Shutdown,
    /// Exit so the supervisor starts the binary at its path again.
    #[serde(rename = "restart")]
    Restart,
}

/// Response sent back to the management client.
#[derive(Debug, Serialize)]
struct MgmtResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl MgmtResponse {
    fn success(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(data),
        }
    }

    fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            data: None,
        }
    }
}

/// Parse a management command from a JSON line or a simple string.
fn parse_command(line: &str) -> Option<MgmtCommand> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    // Try JSON first
    if let Ok(cmd) = serde_json::from_str::<MgmtCommand>(line) {
        return Some(cmd);
    }

    // Fall back to plain text commands
    match line {
        "status" => Some(MgmtCommand::Status),
        "reload" => Some(MgmtCommand::Reload),
        "list" => Some(MgmtCommand::List),
        "launch_sessions" => Some(MgmtCommand::LaunchSessions),
        "shutdown" => Some(MgmtCommand::Shutdown),
        "restart" => Some(MgmtCommand::Restart),
        _ => None,
    }
}

/// Daemon port configuration passed to the management socket
/// so status responses include actual configured ports.
#[derive(Debug, Clone, Copy)]
pub struct DaemonPorts {
    pub http: u16,
    pub https: u16,
    pub dns: u16,
}

/// Serve management commands on a Unix domain socket.
///
/// The socket accepts one command per connection (JSON-over-lines protocol).
/// Socket file is cleaned up when the daemon shuts down. Only its owner can open
/// it, so stopping the daemon needs no signal and no PID.
pub async fn serve(
    socket_path: PathBuf,
    state: SharedState,
    reload_tx: mpsc::Sender<()>,
    shutdown: ShutdownHandle,
    ports: DaemonPorts,
) -> anyhow::Result<()> {
    let cancel = shutdown.token();

    // Remove stale socket file if it exists
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    crate::infrastructure::file_security::restrict_key_permissions(&socket_path)
        .map_err(|e| anyhow::anyhow!("Failed to set socket permissions: {e}"))?;
    info!(path = %socket_path.display(), "Management socket listening");

    loop {
        let (stream, _) = tokio::select! {
            _ = cancel.cancelled() => break,
            result = listener.accept() => match result {
                Ok(conn) => conn,
                Err(e) => {
                    error!(error = %e, "Failed to accept management connection");
                    continue;
                }
            },
        };

        let state = state.clone();
        let reload_tx = reload_tx.clone();
        let shutdown = shutdown.clone();

        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            if let Err(e) = reader.read_line(&mut line).await {
                warn!(error = %e, "Failed to read from management socket");
                return;
            }

            let mut exit_after_reply = None;
            let response = match parse_command(&line) {
                Some(MgmtCommand::Status) => {
                    let snapshot = state.load();
                    let count = snapshot.registrations().len();
                    let pid = std::process::id();
                    MgmtResponse::success(serde_json::json!({
                        "pid": pid,
                        "registration_count": count,
                        "http_port": ports.http,
                        "https_port": ports.https,
                        "dns_port": ports.dns,
                    }))
                }
                Some(MgmtCommand::List) => {
                    let snapshot = state.load();
                    let domains: Vec<serde_json::Value> = snapshot
                        .registrations()
                        .iter()
                        .map(|r| {
                            let routes: Vec<serde_json::Value> = r
                                .routes()
                                .iter()
                                .map(|route| {
                                    serde_json::json!({
                                        "path": route.path().as_str(),
                                        "target": route.target().to_string(),
                                    })
                                })
                                .collect();
                            serde_json::json!({
                                "pattern": r.display_pattern(),
                                "source": r.source().to_string(),
                                "https": r.is_https_enabled(),
                                "last_accessed_at": snapshot.last_accessed_at_for_registration(r),
                                "routes": routes,
                            })
                        })
                        .collect();
                    MgmtResponse::success(serde_json::json!({
                        "domains": domains,
                    }))
                }
                Some(MgmtCommand::LaunchSession { update }) => {
                    state.load().launch_sessions().apply(update);
                    MgmtResponse::success(serde_json::json!({ "recorded": true }))
                }
                Some(MgmtCommand::LaunchSessions) => {
                    let sessions = state.load().launch_sessions().active();
                    MgmtResponse::success(serde_json::json!({ "sessions": sessions }))
                }
                Some(MgmtCommand::Reload) => {
                    if reload_tx.send(()).await.is_ok() {
                        MgmtResponse::success(serde_json::json!({
                            "reloaded": true,
                        }))
                    } else {
                        MgmtResponse::error("Reload channel closed")
                    }
                }
                Some(MgmtCommand::Shutdown) => {
                    exit_after_reply = Some(false);
                    MgmtResponse::success(serde_json::json!({ "stopping": true }))
                }
                Some(MgmtCommand::Restart) => {
                    exit_after_reply = Some(true);
                    MgmtResponse::success(serde_json::json!({ "restarting": true }))
                }
                None => MgmtResponse::error(format!("Unknown command: {}", line.trim())),
            };

            if let Ok(json) = serde_json::to_string(&response) {
                let _ = writer.write_all(json.as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
                let _ = writer.flush().await;
            }

            match exit_after_reply {
                Some(true) => shutdown.restart(),
                Some(false) => shutdown.stop(),
                None => {}
            }
        });
    }

    // Clean up socket file on shutdown
    let _ = std::fs::remove_file(&socket_path);
    info!("Management socket closed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::router::RuntimeState;
    use crate::domain::{DomainName, DomainPattern, DomainRegistration, Route};
    use arc_swap::ArcSwap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::UnixStream;

    fn test_reg(domain: &str) -> DomainRegistration {
        let name = DomainName::new(domain).unwrap();
        let pattern = DomainPattern::Exact(name);
        let routes = vec![Route::parse("/=3000").unwrap()];
        DomainRegistration::new(pattern, routes)
    }

    /// Helper: start the management socket and return what tests need to interact with it.
    /// Returns the TempDir to keep it alive for the socket path.
    async fn start_server(
        registrations: Vec<DomainRegistration>,
    ) -> (
        PathBuf,
        ShutdownHandle,
        mpsc::Receiver<()>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("test.sock");

        let state: SharedState = Arc::new(ArcSwap::from_pointee(RuntimeState::new(registrations)));
        let (reload_tx, reload_rx) = mpsc::channel::<()>(4);
        let shutdown = ShutdownHandle::default();

        let serve_shutdown = shutdown.clone();
        let sock_clone = sock.clone();
        let test_ports = DaemonPorts {
            http: 80,
            https: 443,
            dns: 1053,
        };
        tokio::spawn(async move {
            let _ = serve(sock_clone, state, reload_tx, serve_shutdown, test_ports).await;
        });

        // Give the listener time to bind
        tokio::time::sleep(Duration::from_millis(20)).await;

        (sock, shutdown, reload_rx, dir)
    }

    /// Send a command to the socket, return the parsed JSON response.
    async fn send_command(sock: &PathBuf, cmd: &str) -> serde_json::Value {
        let mut stream = UnixStream::connect(sock).await.unwrap();
        stream.write_all(cmd.as_bytes()).await.unwrap();
        stream.write_all(b"\n").await.unwrap();

        let mut buf = String::new();
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut buf).await.unwrap();
        serde_json::from_str(&buf).unwrap()
    }

    #[tokio::test]
    async fn shutdown_replies_then_stops_with_a_clean_exit_code() {
        let (sock, shutdown, _reload_rx, _dir) = start_server(vec![]).await;

        let resp = send_command(&sock, "shutdown").await;

        assert_eq!(resp["ok"], true);
        tokio::time::timeout(Duration::from_secs(2), shutdown.token().cancelled())
            .await
            .unwrap();
        assert_eq!(shutdown.exit_code(), 0);
    }

    #[tokio::test]
    async fn restart_replies_then_exits_with_the_code_launchd_restarts_on() {
        let (sock, shutdown, _reload_rx, _dir) = start_server(vec![]).await;

        let resp = send_command(&sock, r#"{"cmd":"restart"}"#).await;

        assert_eq!(resp["ok"], true);
        tokio::time::timeout(Duration::from_secs(2), shutdown.token().cancelled())
            .await
            .unwrap();
        assert_eq!(
            shutdown.exit_code(),
            crate::infrastructure::macos_system::RESTART_EXIT_CODE
        );
    }

    // --- parse_command unit tests ---

    #[test]
    fn parse_command_status() {
        let cmd = parse_command(r#"{"cmd":"status"}"#);
        assert!(matches!(cmd, Some(MgmtCommand::Status)));
    }

    #[test]
    fn parse_command_reload() {
        let cmd = parse_command(r#"{"cmd":"reload"}"#);
        assert!(matches!(cmd, Some(MgmtCommand::Reload)));
    }

    #[test]
    fn parse_command_list() {
        let cmd = parse_command(r#"{"cmd":"list"}"#);
        assert!(matches!(cmd, Some(MgmtCommand::List)));
    }

    #[test]
    fn parse_command_simple_strings() {
        assert!(matches!(parse_command("status"), Some(MgmtCommand::Status)));
        assert!(matches!(parse_command("reload"), Some(MgmtCommand::Reload)));
        assert!(matches!(parse_command("list"), Some(MgmtCommand::List)));
        // With whitespace
        assert!(matches!(
            parse_command("  status  "),
            Some(MgmtCommand::Status)
        ));
    }

    #[test]
    fn parse_command_unknown() {
        assert!(parse_command("foobar").is_none());
        assert!(parse_command("").is_none());
        assert!(parse_command(r#"{"cmd":"unknown"}"#).is_none());
    }

    // --- serve() integration tests ---

    #[tokio::test]
    async fn serve_status_returns_pid_and_count() {
        let (sock, cancel, _rx, _dir) =
            start_server(vec![test_reg("app.localify"), test_reg("api.localify")]).await;

        let resp = send_command(&sock, r#"{"cmd":"status"}"#).await;
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"]["registration_count"], 2);
        assert!(resp["data"]["pid"].as_u64().unwrap() > 0);

        cancel.stop();
    }

    #[tokio::test]
    async fn serve_list_returns_domain_details() {
        let (sock, cancel, _rx, _dir) = start_server(vec![test_reg("app.localify")]).await;

        let resp = send_command(&sock, r#"{"cmd":"list"}"#).await;
        assert_eq!(resp["ok"], true);
        let domains = resp["data"]["domains"].as_array().unwrap();
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0]["pattern"], "app.localify");
        assert_eq!(domains[0]["source"], "config");
        assert_eq!(domains[0]["https"], false);
        assert_eq!(domains[0]["last_accessed_at"], serde_json::Value::Null);
        let routes = domains[0]["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["path"], "/");

        cancel.stop();
    }

    #[tokio::test]
    async fn serve_reload_pushes_to_channel() {
        let (sock, cancel, mut rx, _dir) = start_server(vec![test_reg("old.localify")]).await;

        let resp = send_command(&sock, r#"{"cmd":"reload"}"#).await;
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"]["reloaded"], true);

        // Verify the reload channel received a nudge
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        cancel.stop();
    }

    #[tokio::test]
    async fn serve_unknown_command_returns_error() {
        let (sock, cancel, _rx, _dir) = start_server(vec![]).await;

        let resp = send_command(&sock, "foobar").await;
        assert_eq!(resp["ok"], false);
        assert!(resp["error"].as_str().unwrap().contains("Unknown command"));

        cancel.stop();
    }

    #[tokio::test]
    async fn serve_plain_text_status() {
        let (sock, cancel, _rx, _dir) = start_server(vec![test_reg("app.localify")]).await;

        let resp = send_command(&sock, "status").await;
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"]["registration_count"], 1);

        cancel.stop();
    }

    #[tokio::test]
    async fn serve_records_and_lists_launch_sessions() {
        let (sock, cancel, _rx, _dir) = start_server(vec![]).await;
        let session = serde_json::json!({
            "id": "42-1",
            "projectId": "acme",
            "environmentSlug": "main",
            "serviceId": "web",
            "profile": "prod",
            "danger": true,
            "targetPort": 3000,
            "startedAt": 1,
            "sources": ["Proton Pass note 'Acme/apps/web/.env.production' (vault Env): 12 keys"],
        });
        let running = serde_json::json!({
            "cmd": "launch_session",
            "update": { "state": "running", "session": session },
        });

        let resp = send_command(&sock, &running.to_string()).await;
        assert_eq!(resp["ok"], true);

        let resp = send_command(&sock, "launch_sessions").await;
        assert_eq!(resp["data"]["sessions"][0]["profile"], "prod");
        assert_eq!(resp["data"]["sessions"][0]["targetPort"], 3000);

        let stopped = serde_json::json!({
            "cmd": "launch_session",
            "update": { "state": "stopped", "session": session, "reason": "exited" },
        });
        send_command(&sock, &stopped.to_string()).await;
        let resp = send_command(&sock, "launch_sessions").await;
        assert_eq!(resp["data"]["sessions"], serde_json::json!([]));

        cancel.stop();
    }

    #[tokio::test]
    async fn serve_cleans_up_socket_on_cancel() {
        let (sock, cancel, _rx, _dir) = start_server(vec![]).await;
        assert!(sock.exists());

        cancel.stop();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!sock.exists());
    }
}
