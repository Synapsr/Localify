//! The convergence gate of `docs/remote-machines.md` ("PR 4" gates) through
//! a real daemon watch: target B serves its real inventory and runs the real
//! control executor over a temporary home, and the service is started from
//! a terminal with the real `localify dev`, stopped with the real
//! `localify stop`, and its config edited on disk. The fake relay only
//! exists in this crate, so this runs here rather than in `tests/`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{Daemon, launch_serving, pair, wait_until};
use crate::application::inventory::Switches;
use crate::branding::BRANDING;
use crate::daemon::remote::fake_relay::{FakeCloud, FakeSource};
use crate::daemon::remote::local::{LocalSource, RunLookup, SnapshotFuture, lookup};
use crate::daemon::remote::viewing::WatchEvent;
use crate::daemon::remote::{ControlAsk, ControlEvent};
use crate::infrastructure::inventory::{LocalInventory, PublicFacts};
use crate::infrastructure::remote::control::{
    ControlRunner, ControlTimings, Live, LocalHost, Order,
};
use crate::infrastructure::remote::requests::{Report, RequestEntries};
use crate::infrastructure::remote::store::SettingsChange;
use crate::infrastructure::remote::wire::{
    AppPresence, ControlAction, ControlExpect, ControlOutcome, ControlPhase, ControlService,
};
use crate::infrastructure::running_services::RunningServiceStore;
use crate::infrastructure::runs::RunId;

/// What the gate allows for each change to show.
const CONVERGE: Duration = Duration::from_secs(12);

/// A temporary home with one project, `app`, whose service `web` runs
/// `script`.
struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    app: PathBuf,
    port: u16,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, app) = (root.join("home"), root.join("app"));
        for folder in [&home, &app] {
            fs::create_dir_all(folder).unwrap();
        }
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let sandbox = Self {
            _dir: dir,
            config: root.join("config.toml"),
            root,
            home,
            app,
            port,
        };
        sandbox.write_config("exec sleep 300");
        sandbox
    }

    fn write_config(&self, script: &str) {
        let config = format!(
            r#"[paths]
data_dir = "{root}/data"
pid_file = "{root}/localify.pid"
log_file = "{root}/logs/localify.log"
certs_dir = "{root}/data/certs"
socket_path = "{root}/localify.sock"

[domains."web.localify"]
pattern = "web.localify"
https_enabled = false
project = "app"

[[domains."web.localify".routes]]
path = "/"
target = "127.0.0.1:{port}"

[projects.app]
name = "app"
enabled = true
root_path = "{app}"

[[projects.app.services]]
name = "web"
primary_domain = "web.localify"
command = ["sh", "-c", "{script}"]
"#,
            root = self.root.display(),
            app = self.app.display(),
            port = self.port,
        );
        fs::write(&self.config, config).unwrap();
    }

    fn services_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home
                .join("Library/Application Support")
                .join(BRANDING.desktop_state_dir_name)
                .join("services")
        } else {
            self.root.join("state").join(BRANDING.slug).join("services")
        }
    }

    fn environment(&self) -> Vec<(OsString, OsString)> {
        vec![
            ("HOME".into(), self.home.clone().into()),
            ("XDG_STATE_HOME".into(), self.root.join("state").into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
    }

    /// The real `localify`, from a person's terminal: no origin set.
    fn localify(&self, args: &[&str]) -> Command {
        let mut command = Command::new(localify_binary());
        command
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .current_dir(&self.home)
            .env_clear()
            .envs(self.environment())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn source(&self) -> Arc<RealSource> {
        let inventory = LocalInventory::new(self.config.clone(), self.root.join("data"))
            .unwrap()
            .with_services_dir(self.services_dir());
        let environment = self.environment();
        let host = LocalHost {
            config_path: self.config.clone(),
            data_dir: self.root.join("data"),
            services_dir: self.services_dir(),
            executable: Ok(localify_binary()),
            environment: Arc::new(move || environment.clone()),
            installation: vec![
                ("HOME".into(), Some(self.home.clone().into())),
                (BRANDING.profile_env().into(), None),
                (
                    "XDG_STATE_HOME".into(),
                    Some(self.root.join("state").into()),
                ),
            ],
            project_roots: Arc::new(Default::default),
        };
        let runner = ControlRunner::new(
            Arc::new(host),
            RequestEntries::in_services_dir(&self.services_dir()),
            ControlTimings::default(),
        );
        Arc::new(RealSource {
            inventory: Arc::new(inventory),
            runner,
            store: RunningServiceStore::new(self.services_dir()),
        })
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let store = RunningServiceStore::new(self.services_dir());
        for record in store.list().unwrap_or_default() {
            let _ = store.stop(&record);
        }
    }
}

/// `localify` as `cargo test` builds it, next to this test binary's folder.
fn localify_binary() -> PathBuf {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|test| Some(test.parent()?.parent()?.join("localify")))
        .filter(|binary| binary.is_file());
    binary.expect("the localify binary is built: run `cargo test`, which builds it")
}

/// Target B's real facts and executor, over the sandbox.
struct RealSource {
    inventory: Arc<LocalInventory>,
    runner: ControlRunner,
    store: RunningServiceStore,
}

impl LocalSource for RealSource {
    fn snapshot(&self, switches: Switches) -> SnapshotFuture {
        let inventory = Arc::clone(&self.inventory);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let public = PublicFacts {
                    urls: Vec::new(),
                    machine_id: None,
                    tunnel_connected: false,
                };
                inventory.snapshot(&public, &HashMap::new(), switches)
            })
            .await?
        })
    }

    fn run_output(&self, run_id: &RunId) -> anyhow::Result<Result<PathBuf, RunLookup>> {
        lookup(&self.store, run_id)
    }

    fn app(&self) -> AppPresence {
        AppPresence::Absent
    }

    fn control(
        &self,
        order: &Order,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Report {
        self.runner.run(order, live, progress)
    }

    fn control_status(&self, peer: &str, request_id: &str) -> Report {
        self.runner.status(peer, request_id)
    }

    fn shell_environment_failure(&self) -> Option<String> {
        None
    }

    fn reload_shell_environment(&self) -> Option<String> {
        None
    }
}

/// Waits for a snapshot whose `web` `wanted` accepts, within [`CONVERGE`].
async fn shows(
    watch: &mut mpsc::Receiver<WatchEvent>,
    what: &str,
    wanted: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + CONVERGE;
    loop {
        let event = tokio::time::timeout_at(deadline, watch.recv())
            .await
            .unwrap_or_else(|_| panic!("{what} did not show within 12 seconds"))
            .unwrap();
        if let WatchEvent::Snapshot(snapshot) = event
            && let Some(web) = snapshot
                .items
                .iter()
                .find(|item| item["service"]["id"] == "web")
            && wanted(web)
        {
            return web.clone();
        }
    }
}

/// `action` built on `seen`, sent over the watch's session; its outcome,
/// with `conflict` as its code when it is one.
async fn as_seen(a: &Daemon, action: ControlAction, seen: &serde_json::Value) -> ControlOutcome {
    let ask = ControlAsk {
        action,
        service: ControlService {
            project_id: "app".into(),
            environment: seen["environment"]["slug"].as_str().map(str::to_string),
            service_id: "web".into(),
        },
        profile: None,
        expect: ControlExpect {
            run_id: seen["run"]["runId"].as_str().map(str::to_string),
            config_rev: seen["service"]["configRev"].as_str().unwrap().to_string(),
            legacy_id: None,
        },
    };
    let mut events = a.link.control("mB", ask);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(ControlEvent::Result {
                outcome,
                code,
                message,
                ..
            })) => {
                assert_eq!(code.as_deref(), Some("conflict"), "{message}");
                return outcome;
            }
            Ok(Some(ControlEvent::Lost { code, message })) => panic!("lost: {code} {message}"),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => panic!("the control ended without a result"),
        }
    }
}

fn wait_for(child: &mut Child) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline, "it never ended");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The gate: while A watches B, `web` is started from a terminal, stopped
/// with `localify stop`, and its config edited. Within 12 seconds A's
/// snapshot shows each, and a control built on the state before gets
/// `conflict`: a start once it runs, a stop of the run once it stopped, a
/// start on the old revision once the config changed.
#[tokio::test]
async fn real_starts_stops_and_config_edits_converge_and_old_controls_conflict() {
    let sandbox = Sandbox::new();
    let cloud = FakeCloud::start().await;
    let a = super::daemon(&cloud, "mA", "laptop").await;
    let token = cloud.add_machine("mB", "teo-mini");
    let b = launch_serving(
        &cloud,
        ("mB", "teo-mini", &token),
        tempfile::tempdir().unwrap(),
        super::fast(),
        sandbox.source(),
        Arc::new(FakeSource::default()),
    );
    let b = super::connected(b).await;
    wait_until("both see each other", || a.sees("mB") && b.sees("mA")).await;
    pair(&a, &b).await;
    b.link
        .change_settings(SettingsChange {
            share: Some(true),
            control: Some(true),
            background_starts: Some(false),
        })
        .await
        .unwrap();
    let mut watch = a.link.watch("mB");
    let idle = shows(&mut watch, "the idle service", |web| web["owner"].is_null()).await;

    // Started from a terminal.
    let mut terminal = sandbox
        .localify(&["dev", "web", "--project", "app"])
        .spawn()
        .unwrap();
    let running = shows(&mut watch, "the terminal run", |web| {
        web["owner"] == "terminal" && web["process"] == "running"
    })
    .await;
    assert_eq!(
        as_seen(&a, ControlAction::Start, &idle).await,
        ControlOutcome::Conflict
    );

    // Stopped with `localify stop`.
    let stopped = sandbox
        .localify(&["stop", "web", "--project", "app"])
        .status()
        .unwrap();
    assert!(stopped.success());
    wait_for(&mut terminal);
    let stopped = shows(&mut watch, "the stop", |web| web["process"] == "stopped").await;
    assert_eq!(
        as_seen(&a, ControlAction::Stop, &running).await,
        ControlOutcome::Conflict
    );

    // Its config edited on disk.
    sandbox.write_config("exec sleep 301");
    let old_rev = stopped["service"]["configRev"].clone();
    shows(&mut watch, "the config edit", |web| {
        web["service"]["configRev"] != old_rev
    })
    .await;
    assert_eq!(
        as_seen(&a, ControlAction::Start, &stopped).await,
        ControlOutcome::Conflict
    );
    let store = RunningServiceStore::new(sandbox.services_dir());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 1);
    assert!(
        !store.is_running(&records[0]),
        "no control started anything"
    );
}
