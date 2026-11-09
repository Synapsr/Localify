//! What this machine shows its paired machines as a target, and does for
//! them (`docs/remote-machines.md`, "Inventory", "Output" and "Controls"):
//! its inventory snapshot, the output folder of a service's last run, and
//! the control executor. The link asks for them only while a viewer asks,
//! and never sends a path or a record.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::application::inventory::{Have, Snapshot, Switches};
use crate::daemon::public_tunnel::PublicTunnel;
use crate::infrastructure::app_bridge;
use crate::infrastructure::cloud::types::TunnelState;
use crate::infrastructure::inventory::{LocalInventory, PublicFacts};
use crate::infrastructure::remote::control::{
    ControlRunner, ControlTimings, Live, LocalHost, Order, Reader,
};
use crate::infrastructure::remote::requests::{Report, RequestEntries};
use crate::infrastructure::remote::wire::{AppPresence, ControlPhase, MachineSummary};
use crate::infrastructure::running_services::RunningServiceStore;
use crate::infrastructure::runs::RunId;
use crate::infrastructure::shell_env::user_shell;
use localify::desktop::ProjectRootStore;
use localify::desktop::shell_environment::{ShellEnvironment, ShellEnvironmentState};

pub type SnapshotFuture = Pin<Box<dyn Future<Output = Result<Snapshot>> + Send>>;

/// Why a run's output cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLookup {
    /// The run is not its service's last one, or no service has it.
    Gone,
    /// The run has no stream to read: `terminal` or `legacy_run`.
    Unavailable(&'static str),
}

/// The target's facts, as the link reads them, and what it does for a
/// viewer's control.
pub trait LocalSource: Send + Sync {
    /// A fresh snapshot of this machine's inventory, its controls decided by
    /// `switches`.
    fn snapshot(&self, switches: Switches) -> SnapshotFuture;
    /// The output folder of `run_id`, while it is its service's last run
    /// and has a stream. Reads the run records: quick, and blocking.
    fn run_output(&self, run_id: &RunId) -> Result<Result<PathBuf, RunLookup>>;
    /// Whether the desktop app runs, as its bridge's `hello` last said:
    /// never waits for the app.
    fn app(&self) -> AppPresence;
    /// Runs a control from check 4 on (see `ControlRunner::run`). Blocks
    /// for as long as the control takes.
    fn control(
        &self,
        order: &Order,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Report;
    /// `control.status`: what became of a peer's request. Blocks.
    fn control_status(&self, peer: &str, request_id: &str) -> Report;
    /// Why background starts run without the user's shell environment,
    /// when it could not be read.
    fn shell_environment_failure(&self) -> Option<String>;
    /// Reads the shell environment again, for the app's `Reload shell
    /// environment`, and says why it still could not be read. Blocks.
    fn reload_shell_environment(&self) -> Option<String>;
}

/// How often [`AppSeen`] asks the app's bridge.
const APP_PROBE_EVERY: Duration = Duration::from_secs(2);

/// Whether the desktop app runs, as a task that asks its bridge every
/// [`APP_PROBE_EVERY`] last saw it, so that a session's actor reads it
/// without waiting for an app that is slow to answer.
#[derive(Clone, Default)]
pub struct AppSeen(Arc<AtomicBool>);

impl AppSeen {
    /// Watches the app's bridge in `data_dir` until `cancel`.
    pub fn watch(data_dir: &std::path::Path, cancel: CancellationToken) -> Self {
        Self::spawn(app_bridge::socket_path(data_dir), APP_PROBE_EVERY, cancel)
    }

    /// Asks the bridge at `socket` now and every `every`, until `cancel`.
    pub fn spawn(socket: PathBuf, every: Duration, cancel: CancellationToken) -> Self {
        let seen = Self::default();
        let updated = seen.clone();
        tokio::spawn(async move {
            loop {
                let asked = socket.clone();
                let running = tokio::task::spawn_blocking(move || app_bridge::probe(&asked))
                    .await
                    .is_ok_and(|presence| presence == AppPresence::Running);
                updated.0.store(running, Ordering::SeqCst);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(every) => {}
                }
            }
        });
        seen
    }

    pub fn get(&self) -> AppPresence {
        if self.0.load(Ordering::SeqCst) {
            AppPresence::Running
        } else {
            AppPresence::Absent
        }
    }
}

/// The daemon's own inventory, tunnel and run records, and its controls.
pub struct DaemonSource {
    inventory: Arc<LocalInventory>,
    tunnel: PublicTunnel,
    app: AppSeen,
    controls: ControlRunner,
    shell: Arc<ShellEnvironment>,
}

impl DaemonSource {
    /// Resolves this executable once, canonical: a detached start runs it,
    /// never a `localify` found on `PATH`.
    pub fn new(inventory: Arc<LocalInventory>, tunnel: PublicTunnel, app: AppSeen) -> Self {
        let executable = LocalHost::own_executable();
        let shell = Arc::new(ShellEnvironment::new(
            executable.clone().unwrap_or_default(),
            user_shell(),
        ));
        let services_dir = inventory.services_dir();
        let host = LocalHost {
            config_path: inventory.config_path().to_path_buf(),
            data_dir: inventory.data_dir().to_path_buf(),
            services_dir: services_dir.clone(),
            executable,
            environment: {
                let shell = Arc::clone(&shell);
                Arc::new(move || shell.environment())
            },
            installation: LocalHost::own_installation(),
            project_roots: project_roots(),
        };
        let controls = ControlRunner::new(
            Arc::new(host),
            RequestEntries::in_services_dir(&services_dir),
            ControlTimings::default(),
        );
        Self {
            inventory,
            tunnel,
            app,
            controls,
            shell,
        }
    }
}

/// The checkouts the app knows, as `localify dev` reads them.
fn project_roots() -> Reader<std::collections::HashMap<String, PathBuf>> {
    Arc::new(|| {
        ProjectRootStore::new_default()
            .and_then(|store| store.load())
            .unwrap_or_default()
            .into_iter()
            .map(|(project_id, root)| (project_id, PathBuf::from(root)))
            .collect()
    })
}

impl LocalSource for DaemonSource {
    fn snapshot(&self, switches: Switches) -> SnapshotFuture {
        let inventory = Arc::clone(&self.inventory);
        let tunnel = self.tunnel.clone();
        Box::pin(async move { build_snapshot(&inventory, &tunnel, switches).await })
    }

    fn run_output(&self, run_id: &RunId) -> Result<Result<PathBuf, RunLookup>> {
        let store = RunningServiceStore::new_default()?;
        lookup(&store, run_id)
    }

    fn app(&self) -> AppPresence {
        self.app.get()
    }

    fn control(
        &self,
        order: &Order,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Report {
        self.controls.run(order, live, progress)
    }

    fn control_status(&self, peer: &str, request_id: &str) -> Report {
        self.controls.status(peer, request_id)
    }

    fn shell_environment_failure(&self) -> Option<String> {
        shell_failure(&self.shell)
    }

    fn reload_shell_environment(&self) -> Option<String> {
        self.shell.reload();
        shell_failure(&self.shell)
    }
}

/// Why `shell`'s environment could not be read, when it could not.
pub fn shell_failure(shell: &ShellEnvironment) -> Option<String> {
    let status = shell.status();
    (status.state == ShellEnvironmentState::Failed).then(|| {
        status
            .reason
            .unwrap_or_else(|| "Your shell's environment could not be read.".into())
    })
}

/// Numbers what viewers are sent: a snapshot's items and the machine
/// summary they go out with. The snapshot's own revision counts the items
/// only, so a change of the summary alone, such as the app opening or a
/// switch turned, moves this one too. Kept by the link, so a new relay
/// connection never numbers a revision again.
#[derive(Debug, Default)]
pub struct SentRevisions {
    rev: u64,
    last: Option<(String, u64, MachineSummary)>,
}

impl SentRevisions {
    /// The revision `snapshot` goes out as with `summary`.
    pub fn number(&mut self, snapshot: &Snapshot, summary: &MachineSummary) -> Have {
        let sent = (snapshot.epoch.clone(), snapshot.rev, summary.clone());
        if self.last.as_ref() != Some(&sent) {
            self.rev += 1;
            self.last = Some(sent);
        }
        Have {
            epoch: snapshot.epoch.clone(),
            rev: self.rev,
        }
    }
}

/// The run among the services' last runs, and whether it has a stream.
pub fn lookup(store: &RunningServiceStore, run_id: &RunId) -> Result<Result<PathBuf, RunLookup>> {
    let last = store
        .list()?
        .into_iter()
        .find(|service| service.run_id() == Some(run_id));
    let Some(service) = last else {
        return Ok(Err(RunLookup::Gone));
    };
    if service.is_legacy() {
        return Ok(Err(RunLookup::Unavailable("legacy_run")));
    }
    if !service.has_stream() {
        return Ok(Err(RunLookup::Unavailable("terminal")));
    }
    Ok(Ok(store.run_dir(run_id)))
}

/// A fresh snapshot, built off the async threads: it reads the config and
/// the run records, scans processes and connects to every service's port.
pub async fn build_snapshot(
    inventory: &Arc<LocalInventory>,
    tunnel: &PublicTunnel,
    switches: Switches,
) -> Result<Snapshot> {
    let (urls, machine_id) = tunnel.known_urls();
    let public = PublicFacts {
        urls,
        machine_id,
        tunnel_connected: tunnel.status().await.tunnel == TunnelState::Connected,
    };
    let inventory = Arc::clone(inventory);
    tokio::task::spawn_blocking(move || inventory.snapshot(&public, &project_roots()(), switches))
        .await?
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    use super::*;

    /// An app answering `hello` at `socket`, until its socket is removed.
    fn app_at(socket: &std::path::Path) {
        let listener = UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut line);
                let _ =
                    stream.write_all(b"{\"ok\":true,\"data\":{\"version\":\"1\",\"bridge\":1}}\n");
            }
        });
    }

    async fn becomes(seen: &AppSeen, wanted: AppPresence) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while seen.get() != wanted {
            assert!(
                tokio::time::Instant::now() < deadline,
                "still not {wanted:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn a_new_summary_moves_the_revision_and_the_same_one_does_not() {
        use crate::application::inventory::Revisions;
        use crate::infrastructure::remote::store::DEFAULT_SETTINGS;

        let mut items = Revisions::new("epoch".into());
        let snapshot = items.snapshot(Vec::new()).unwrap();
        let mut summary = MachineSummary {
            machine_id: "mB".into(),
            name: "B".into(),
            version: "1".into(),
            os: "macos".into(),
            settings: DEFAULT_SETTINGS,
            app: AppPresence::Absent,
        };
        let mut sent = SentRevisions::default();

        let first = sent.number(&snapshot, &summary);
        assert_eq!(sent.number(&snapshot, &summary), first);
        summary.app = AppPresence::Running;
        let opened = sent.number(&snapshot, &summary);
        assert!(opened.rev > first.rev);
        summary.settings.control = !summary.settings.control;
        assert!(sent.number(&snapshot, &summary).rev > opened.rev);
        assert_eq!(first.epoch, snapshot.epoch);
    }

    #[tokio::test]
    async fn the_app_is_seen_opening_and_closing() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("app-bridge.sock");
        let cancel = CancellationToken::new();
        let seen = AppSeen::spawn(socket.clone(), Duration::from_millis(50), cancel.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(seen.get(), AppPresence::Absent);

        app_at(&socket);
        becomes(&seen, AppPresence::Running).await;
        // The app quit: its socket is gone.
        std::fs::remove_file(&socket).unwrap();
        becomes(&seen, AppPresence::Absent).await;
        cancel.cancel();
    }
}
