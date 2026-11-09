use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::lifecycle::ShutdownHandle;
use super::public_tunnel::PublicTunnel;
use super::remote::local::build_snapshot;
use super::remote::{
    AccessEvent, Asking, ControlAsk, ControlEvent, Failure, FollowEvent, Following, PairEvent,
    RemoteLink,
};
use super::router::SharedState;
use crate::application::inventory::Have;
use crate::infrastructure::cloud::credentials::CredentialsStore;
use crate::infrastructure::inventory::LocalInventory;
use crate::infrastructure::launch_sessions::LaunchSessionUpdate;
use crate::infrastructure::remote::store::SettingsChange;
use crate::infrastructure::remote::wire::AccessWants;

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
    /// This machine's sign-in and tunnel (`docs/public-urls.md`, "Tunnel").
    #[serde(rename = "public_status")]
    PublicStatus,
    /// Fetch this machine's public URLs again, in the background.
    #[serde(rename = "public_refresh")]
    PublicRefresh,
    /// The public URLs the daemon last learned from Localify Cloud, and this
    /// machine's id: what `localify dev` trusts without calling the network.
    #[serde(rename = "public_urls")]
    PublicUrls,
    /// This machine's inventory (`docs/remote-machines.md`, "Inventory"), or
    /// `unchanged` when `have` names its current revision.
    #[serde(rename = "inventory")]
    Inventory {
        #[serde(default)]
        have: Option<Have>,
    },
    /// The relay link (`docs/remote-machines.md`, "Daemon").
    #[serde(rename = "remote_status")]
    RemoteStatus,
    /// The roster with states. With `probe`, opens a short session to each
    /// paired machine online first, so the states are current.
    #[serde(rename = "remote_machines")]
    RemoteMachines {
        #[serde(default)]
        probe: bool,
    },
    #[serde(rename = "remote_settings")]
    RemoteSettings {
        #[serde(flatten)]
        change: SettingsChange,
    },
    /// Streaming: the code, then `paired` or `cancelled`.
    #[serde(rename = "remote_pair", rename_all = "camelCase")]
    RemotePair { machine_id: String },
    #[serde(rename = "remote_pair_answer", rename_all = "camelCase")]
    RemotePairAnswer { pairing_id: String, accept: bool },
    #[serde(rename = "remote_unpair", rename_all = "camelCase")]
    RemoteUnpair { machine_id: String },
    /// Streaming: pairing requests, presence, revocations. With `access`,
    /// the client shows the access dialog: it hears access requests, and
    /// they wait for its answer instead of being answered `nobody`.
    #[serde(rename = "remote_events")]
    RemoteEvents {
        #[serde(default)]
        access: bool,
    },
    /// Streaming, as a viewer: asks a target to turn on remote controls,
    /// then its answer. Closing the connection cancels the request.
    #[serde(rename = "remote_access", rename_all = "camelCase")]
    RemoteAccess {
        machine_id: String,
        wants: AccessWants,
    },
    /// Streaming, as a viewer: starts, stops or restarts a service on
    /// another machine, then its progress and result. The client waits up
    /// to 120 s, not the usual 2.
    #[serde(rename = "remote_control", rename_all = "camelCase")]
    RemoteControl {
        machine_id: String,
        #[serde(flatten)]
        ask: ControlAsk,
    },
    /// As a target, from the access dialog.
    #[serde(rename = "remote_access_answer", rename_all = "camelCase")]
    RemoteAccessAnswer {
        request_id: String,
        allow: bool,
        #[serde(default)]
        suppress: bool,
    },
    /// The peers whose access requests are suppressed here.
    #[serde(rename = "remote_access_suppressed")]
    RemoteAccessSuppressed,
    /// Lets a suppressed peer ask again.
    #[serde(rename = "remote_access_restore", rename_all = "camelCase")]
    RemoteAccessRestore { machine_id: String },
    /// Another machine's inventory: one snapshot, every page.
    #[serde(rename = "remote_inventory", rename_all = "camelCase")]
    RemoteInventory { machine_id: String },
    /// Streaming: another machine's snapshots while the client stays.
    #[serde(rename = "remote_watch", rename_all = "camelCase")]
    RemoteWatch { machine_id: String },
    /// One bounded read of a run's output on another machine.
    #[serde(rename = "remote_output_read", rename_all = "camelCase")]
    RemoteOutputRead {
        machine_id: String,
        run_id: String,
        offset: u64,
        max_bytes: u32,
    },
    /// Streaming: a run's output on another machine, from `offset`, or
    /// without one from its last `tail` bytes.
    #[serde(rename = "remote_output_follow", rename_all = "camelCase")]
    RemoteOutputFollow {
        machine_id: String,
        run_id: String,
        #[serde(default)]
        offset: Option<u64>,
        #[serde(default)]
        tail: Option<u64>,
    },
}

/// Without an offset, a follow starts this far before the end: a few
/// thousand lines, not a whole run.
const DEFAULT_TAIL: u64 = 256 * 1024;

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
        "public_status" => Some(MgmtCommand::PublicStatus),
        "public_refresh" => Some(MgmtCommand::PublicRefresh),
        "public_urls" => Some(MgmtCommand::PublicUrls),
        "inventory" => Some(MgmtCommand::Inventory { have: None }),
        "remote_status" => Some(MgmtCommand::RemoteStatus),
        "remote_machines" => Some(MgmtCommand::RemoteMachines { probe: false }),
        "remote_events" => Some(MgmtCommand::RemoteEvents { access: false }),
        "remote_access_suppressed" => Some(MgmtCommand::RemoteAccessSuppressed),
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

/// What the machine-wide commands answer from: the tunnel, the inventory
/// and the relay link.
pub struct MachineServices {
    pub tunnel: PublicTunnel,
    pub inventory: Arc<LocalInventory>,
    pub remote: RemoteLink,
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
    services: MachineServices,
) -> anyhow::Result<()> {
    let MachineServices {
        tunnel,
        inventory,
        remote,
    } = services;
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
        let tunnel = tunnel.clone();
        let inventory = Arc::clone(&inventory);
        let remote = remote.clone();

        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            if let Err(e) = reader.read_line(&mut line).await {
                warn!(error = %e, "Failed to read from management socket");
                return;
            }

            let command = parse_command(&line);
            match command {
                Some(MgmtCommand::RemotePair { machine_id }) => {
                    return stream_pairing(&remote, &machine_id, reader, writer).await;
                }
                Some(MgmtCommand::RemoteEvents { access }) => {
                    return stream_events(&remote, access, reader, writer).await;
                }
                Some(MgmtCommand::RemoteAccess { machine_id, wants }) => {
                    let asking = remote.access(&machine_id, wants).await;
                    return stream_access(asking, reader, writer).await;
                }
                Some(MgmtCommand::RemoteControl { machine_id, ask }) => {
                    let events = remote.control(&machine_id, ask);
                    return stream_control(events, reader, writer).await;
                }
                Some(MgmtCommand::RemoteWatch { machine_id }) => {
                    return stream_watch(&remote, &machine_id, reader, writer).await;
                }
                Some(MgmtCommand::RemoteOutputFollow {
                    machine_id,
                    run_id,
                    offset,
                    tail,
                }) => {
                    let tail = tail.unwrap_or(DEFAULT_TAIL);
                    let follow = remote.follow(&machine_id, &run_id, offset, tail).await;
                    return stream_follow(follow, reader, writer).await;
                }
                _ => {}
            }
            let mut exit_after_reply = None;
            let response = match command {
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
                Some(MgmtCommand::PublicStatus) => {
                    match serde_json::to_value(tunnel.status().await) {
                        Ok(status) => MgmtResponse::success(status),
                        Err(error) => MgmtResponse::error(error.to_string()),
                    }
                }
                // Answers at once: the client waits 2 s and a cloud call may take 20.
                Some(MgmtCommand::PublicRefresh) => {
                    tunnel.refresh();
                    remote.refresh();
                    MgmtResponse::success(serde_json::json!({ "refreshing": true }))
                }
                Some(MgmtCommand::PublicUrls) => {
                    let current = CredentialsStore::in_data_dir(inventory.data_dir())
                        .load()
                        .ok()
                        .flatten();
                    let (urls, machine_id) = tunnel.known_urls_for(current.as_ref());
                    MgmtResponse::success(
                        serde_json::json!({ "urls": urls, "machineId": machine_id }),
                    )
                }
                Some(MgmtCommand::Inventory { have }) => {
                    inventory_answer(&inventory, &tunnel, remote.switches(), have).await
                }
                Some(command) => remote_answer(&remote, command).await,
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

/// The one-shot remote commands.
async fn remote_answer(remote: &RemoteLink, command: MgmtCommand) -> MgmtResponse {
    let answer = match command {
        MgmtCommand::RemoteStatus => {
            serde_json::to_value(remote.status()).map_err(anyhow::Error::from)
        }
        MgmtCommand::RemoteMachines { probe } => remote
            .machines(probe)
            .await
            .and_then(|machines| Ok(serde_json::to_value(machines)?)),
        MgmtCommand::RemoteSettings { change } => remote
            .change_settings(change)
            .await
            .and_then(|settings| Ok(serde_json::to_value(settings)?)),
        MgmtCommand::RemotePairAnswer { pairing_id, accept } => remote
            .answer(&pairing_id, accept)
            .await
            .map(|()| serde_json::json!({ "ok": true })),
        MgmtCommand::RemoteUnpair { machine_id } => remote
            .unpair(&machine_id)
            .map(|removed| serde_json::json!({ "ok": true, "removed": removed })),
        MgmtCommand::RemoteAccessAnswer {
            request_id,
            allow,
            suppress,
        } => remote
            .answer_access(&request_id, allow, suppress)
            .await
            .map(|outcome| serde_json::json!({ "outcome": outcome })),
        MgmtCommand::RemoteAccessSuppressed => remote
            .suppressed()
            .and_then(|suppressed| Ok(serde_json::to_value(suppressed)?)),
        MgmtCommand::RemoteAccessRestore { machine_id } => remote
            .restore_access(&machine_id)
            .map(|removed| serde_json::json!({ "ok": true, "removed": removed })),
        MgmtCommand::RemoteInventory { machine_id } => remote
            .inventory(&machine_id)
            .await
            .map_err(failure_error)
            .and_then(|inventory| Ok(serde_json::to_value(inventory)?)),
        MgmtCommand::RemoteOutputRead {
            machine_id,
            run_id,
            offset,
            max_bytes,
        } => remote
            .read_output(&machine_id, &run_id, offset, max_bytes)
            .await
            .map_err(failure_error)
            .and_then(|data| Ok(serde_json::to_value(data)?)),
        _ => return MgmtResponse::error("Not a remote command"),
    };
    match answer {
        Ok(data) => MgmtResponse::success(data),
        Err(error) => MgmtResponse::error(format!("{error:#}")),
    }
}

async fn write_line(writer: &mut OwnedWriteHalf, response: &MgmtResponse) -> bool {
    let Ok(mut json) = serde_json::to_string(response) else {
        return false;
    };
    json.push('\n');
    writer.write_all(json.as_bytes()).await.is_ok() && writer.flush().await.is_ok()
}

/// Resolves when the client closes its end.
async fn client_gone(reader: &mut BufReader<OwnedReadHalf>) {
    let mut rest = String::new();
    loop {
        match reader.read_line(&mut rest).await {
            Ok(0) | Err(_) => return,
            Ok(_) => rest.clear(),
        }
    }
}

/// `remote_pair`: one line per event until the pairing ends. Closing the
/// connection cancels the pairing.
async fn stream_pairing(
    remote: &RemoteLink,
    machine_id: &str,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
) {
    let mut events = match remote.pair(machine_id).await {
        Ok(events) => events,
        Err(error) => {
            write_line(&mut writer, &MgmtResponse::error(format!("{error:#}"))).await;
            return;
        }
    };
    loop {
        tokio::select! {
            _ = client_gone(&mut reader) => return,
            event = events.recv() => {
                let Some(event) = event else { return };
                let last = !matches!(event, PairEvent::Code { .. });
                let Ok(data) = serde_json::to_value(&event) else { return };
                if !write_line(&mut writer, &MgmtResponse::success(data)).await || last {
                    return;
                }
            }
        }
    }
}

/// `remote_events`: while connected, this client counts as a person who can
/// confirm a pairing here, and with `access`, answer an access request.
async fn stream_events(
    remote: &RemoteLink,
    access: bool,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
) {
    let mut subscription = remote.subscribe(access);
    let ready = MgmtResponse::success(serde_json::json!({ "event": "subscribed" }));
    if !write_line(&mut writer, &ready).await {
        return;
    }
    loop {
        tokio::select! {
            _ = client_gone(&mut reader) => return,
            event = subscription.events.recv() => match event {
                Ok(event) if event.is_access() && !subscription.access => continue,
                Ok(event) => {
                    let Ok(data) = serde_json::to_value(&event) else { return };
                    if !write_line(&mut writer, &MgmtResponse::success(data)).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

fn failure_error(failure: Failure) -> anyhow::Error {
    anyhow::anyhow!("{}", failure.message)
}

/// `remote_access`: `waiting`, then the answer. Closing the connection
/// first sends `access.cancel`.
async fn stream_access(
    asking: Result<Asking, Failure>,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
) {
    let mut asking = match asking {
        Ok(asking) => asking,
        Err(failure) => {
            let lost = AccessEvent::Lost {
                code: failure.code,
                message: failure.message,
            };
            if let Ok(data) = serde_json::to_value(&lost) {
                write_line(&mut writer, &MgmtResponse::success(data)).await;
            }
            return;
        }
    };
    loop {
        tokio::select! {
            _ = client_gone(&mut reader) => return,
            event = asking.next() => {
                let Some(event) = event else { return };
                let last = !matches!(event, AccessEvent::Waiting);
                let Ok(data) = serde_json::to_value(&event) else { return };
                if !write_line(&mut writer, &MgmtResponse::success(data)).await || last {
                    return;
                }
            }
        }
    }
}

/// `remote_watch`: one line per snapshot or loss, until the client closes
/// its end, which ends the watch.
async fn stream_watch(
    remote: &RemoteLink,
    machine_id: &str,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
) {
    let mut events = remote.watch(machine_id);
    loop {
        tokio::select! {
            _ = client_gone(&mut reader) => return,
            event = events.recv() => {
                let Some(event) = event else { return };
                let Ok(data) = serde_json::to_value(&event) else { return };
                if !write_line(&mut writer, &MgmtResponse::success(data)).await {
                    return;
                }
            }
        }
    }
}

/// `remote_output_follow`: one line per `output.data`, acknowledged once
/// written, then `ended` when the follow stops before its run did. Closing
/// the connection unfollows.
async fn stream_follow(
    follow: Result<Following, Failure>,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
) {
    let mut follow = match follow {
        Ok(follow) => follow,
        Err(failure) => {
            let ended = FollowEvent::Ended {
                code: failure.code,
                message: failure.message,
            };
            if let Ok(data) = serde_json::to_value(&ended) {
                write_line(&mut writer, &MgmtResponse::success(data)).await;
            }
            return;
        }
    };
    loop {
        tokio::select! {
            _ = client_gone(&mut reader) => return,
            event = follow.next() => {
                let Some(event) = event else { return };
                let last = match &event {
                    FollowEvent::Data(data) => data.ended,
                    FollowEvent::Ended { .. } => true,
                };
                let next = match &event {
                    FollowEvent::Data(data) => Some(data.next),
                    FollowEvent::Ended { .. } => None,
                };
                let Ok(data) = serde_json::to_value(&event) else { return };
                if !write_line(&mut writer, &MgmtResponse::success(data)).await || last {
                    return;
                }
                if let Some(next) = next {
                    follow.shown(next);
                }
            }
        }
    }
}

/// `remote_control`: one line per event, until the result. A client that
/// closes its end stops listening; the control goes on, on the target.
async fn stream_control(
    mut events: mpsc::Receiver<ControlEvent>,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
) {
    loop {
        tokio::select! {
            _ = client_gone(&mut reader) => return,
            event = events.recv() => {
                let Some(event) = event else { return };
                let Ok(data) = serde_json::to_value(&event) else { return };
                if !write_line(&mut writer, &MgmtResponse::success(data)).await {
                    return;
                }
            }
        }
    }
}

/// A fresh snapshot of this machine.
async fn inventory_answer(
    inventory: &Arc<LocalInventory>,
    tunnel: &PublicTunnel,
    switches: crate::application::inventory::Switches,
    have: Option<Have>,
) -> MgmtResponse {
    let snapshot = match build_snapshot(inventory, tunnel, switches).await {
        Ok(snapshot) => snapshot,
        Err(error) => return MgmtResponse::error(format!("{error:#}")),
    };
    if snapshot.is_held_by(have.as_ref()) {
        return MgmtResponse::success(serde_json::json!({
            "unchanged": true,
            "epoch": snapshot.epoch,
            "rev": snapshot.rev,
        }));
    }
    match serde_json::to_value(&snapshot) {
        Ok(snapshot) => MgmtResponse::success(snapshot),
        Err(error) => MgmtResponse::error(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::remote::local::DaemonSource;
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
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "").unwrap();
        let inventory = Arc::new(
            LocalInventory::new(config, dir.path().join("data"))
                .unwrap()
                .with_services_dir(dir.path().join("services")),
        );
        let data_dir = dir.path().join("data");
        let tunnel = PublicTunnel::new(1);
        let source = DaemonSource::new(
            Arc::clone(&inventory),
            tunnel.clone(),
            crate::daemon::remote::local::AppSeen::default(),
        );
        let test_ports = DaemonPorts {
            http: 80,
            https: 443,
            dns: 1053,
        };
        tokio::spawn(async move {
            let _ = serve(
                sock_clone,
                state,
                reload_tx,
                serve_shutdown,
                test_ports,
                MachineServices {
                    tunnel,
                    inventory,
                    remote: RemoteLink::new(&data_dir, Arc::new(source)),
                },
            )
            .await;
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

    #[tokio::test]
    async fn inventory_answers_a_snapshot_then_unchanged_for_the_same_revision() {
        let (sock, _shutdown, _reload_rx, dir) = start_server(vec![]).await;

        let snapshot = send_command(&sock, "inventory").await;
        assert_eq!(snapshot["ok"], true, "{snapshot}");
        let data = &snapshot["data"];
        assert_eq!(data["schema"], 1);
        assert_eq!(data["rev"], 1);
        assert_eq!(data["items"], serde_json::json!([]));
        assert!(dir.path().join("data/launch-rev.key").exists());

        let have = serde_json::json!({
            "cmd": "inventory",
            "have": {"epoch": data["epoch"], "rev": 1},
        });
        let again = send_command(&sock, &have.to_string()).await;
        assert_eq!(again["data"]["unchanged"], true);
        assert_eq!(again["data"]["rev"], 1);
        assert!(again["data"].get("items").is_none());
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

    /// An app from before the access dialog subscribes without `access`, so
    /// access requests on this machine are answered `nobody`.
    #[test]
    fn parse_command_remote_access_commands() {
        assert!(matches!(
            parse_command(r#"{"cmd":"remote_events"}"#),
            Some(MgmtCommand::RemoteEvents { access: false })
        ));
        assert!(matches!(
            parse_command("remote_events"),
            Some(MgmtCommand::RemoteEvents { access: false })
        ));
        assert!(matches!(
            parse_command(r#"{"cmd":"remote_events","access":true}"#),
            Some(MgmtCommand::RemoteEvents { access: true })
        ));
        assert!(matches!(
            parse_command(r#"{"cmd":"remote_access","machineId":"mB","wants":"backgroundStarts"}"#),
            Some(MgmtCommand::RemoteAccess { machine_id, wants: AccessWants::BackgroundStarts })
                if machine_id == "mB"
        ));
        assert!(matches!(
            parse_command(r#"{"cmd":"remote_access_answer","requestId":"q","allow":true}"#),
            Some(MgmtCommand::RemoteAccessAnswer {
                allow: true,
                suppress: false,
                ..
            })
        ));
        assert!(
            parse_command(r#"{"cmd":"remote_access","machineId":"mB","wants":"share"}"#).is_none()
        );
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
