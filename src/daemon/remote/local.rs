//! What this machine shows its paired machines as a target
//! (`docs/remote-machines.md`, "Inventory" and "Output"): its inventory
//! snapshot, and the output folder of a service's last run. The link asks
//! for them only while a viewer asks, and never sends a path or a record.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

use crate::application::inventory::Snapshot;
use crate::daemon::public_tunnel::PublicTunnel;
use crate::infrastructure::cloud::types::TunnelState;
use crate::infrastructure::inventory::{LocalInventory, PublicFacts};
use crate::infrastructure::running_services::RunningServiceStore;
use crate::infrastructure::runs::RunId;
use localify::desktop::ProjectRootStore;

pub type SnapshotFuture = Pin<Box<dyn Future<Output = Result<Snapshot>> + Send>>;

/// Why a run's output cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLookup {
    /// The run is not its service's last one, or no service has it.
    Gone,
    /// The run has no stream to read: `terminal` or `legacy_run`.
    Unavailable(&'static str),
}

/// The target's facts, as the link reads them.
pub trait LocalSource: Send + Sync {
    /// A fresh snapshot of this machine's inventory.
    fn snapshot(&self) -> SnapshotFuture;
    /// The output folder of `run_id`, while it is its service's last run
    /// and has a stream. Reads the run records: quick, and blocking.
    fn run_output(&self, run_id: &RunId) -> Result<Result<PathBuf, RunLookup>>;
}

/// The daemon's own inventory, tunnel and run records.
pub struct DaemonSource {
    inventory: Arc<LocalInventory>,
    tunnel: PublicTunnel,
}

impl DaemonSource {
    pub fn new(inventory: Arc<LocalInventory>, tunnel: PublicTunnel) -> Self {
        Self { inventory, tunnel }
    }
}

impl LocalSource for DaemonSource {
    fn snapshot(&self) -> SnapshotFuture {
        let inventory = Arc::clone(&self.inventory);
        let tunnel = self.tunnel.clone();
        Box::pin(async move { build_snapshot(&inventory, &tunnel).await })
    }

    fn run_output(&self, run_id: &RunId) -> Result<Result<PathBuf, RunLookup>> {
        let store = RunningServiceStore::new_default()?;
        lookup(&store, run_id)
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
) -> Result<Snapshot> {
    let (urls, machine_id) = tunnel.known_urls();
    let public = PublicFacts {
        urls,
        machine_id,
        tunnel_connected: tunnel.status().await.tunnel == TunnelState::Connected,
    };
    let inventory = Arc::clone(inventory);
    tokio::task::spawn_blocking(move || {
        // The checkouts the app knows, as `localify dev` reads them.
        let project_roots = ProjectRootStore::new_default()
            .and_then(|store| store.load())
            .unwrap_or_default()
            .into_iter()
            .map(|(project_id, root)| (project_id, PathBuf::from(root)))
            .collect();
        inventory.snapshot(&public, &project_roots)
    })
    .await?
}
