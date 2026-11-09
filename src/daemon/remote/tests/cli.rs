//! `--machine` end to end: the CLI's remote commands (`cli::remote`) against
//! a real management socket, served by viewer A's link, with target B on the
//! fake relay running the real control executor over its in-memory host.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use serde_json::Value;
use tempfile::TempDir;

use super::{Daemon, pair, two};
use crate::application::inventory::{
    EnvironmentRef, ProfileChecks, ProjectRef, RunFacts, RunOrigin, RunProcess, ServiceFacts,
    ServiceRef,
};
use crate::cli::remote::{
    Console, ControlAction, LogsQuery, Machine, Peer, REFUSED, ServiceQuery, control, finish, logs,
    machine, status,
};
use crate::daemon::lifecycle::ShutdownHandle;
use crate::daemon::mgmt_socket::{DaemonPorts, MachineServices, serve};
use crate::daemon::public_tunnel::PublicTunnel;
use crate::daemon::remote::fake_relay::{FakeCloud, FakeService, FakeSource};
use crate::daemon::remote::local::RunLookup;
use crate::daemon::router::RuntimeState;
use crate::infrastructure::inventory::LocalInventory;
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::remote::store::SettingsChange;
use crate::infrastructure::remote::wire::{AppPresence, PROTO_MAX, PROTO_PAIR_SAVED};
use crate::infrastructure::runs::output::{
    EndReason, RunEnd, StreamWriter, create_run_dir, publish_end,
};

const RUN: &str = "RRRRRRRRRRRRRRRRRRRRRR";
/// The run that replaces [`RUN`] when the service restarts.
const NEXT_RUN: &str = "SSSSSSSSSSSSSSSSSSSSSS";

fn web() -> FakeService {
    let run = RunFacts {
        run_id: Some(RUN.into()),
        origin: RunOrigin::App,
        legacy: false,
        has_stream: true,
        process: RunProcess::Running,
        started_at: "2026-11-09T09:00:00Z".into(),
        profile: None,
        profile_danger: false,
        exit: None,
        legacy_id: None,
    };
    FakeService {
        facts: ServiceFacts {
            project: ProjectRef {
                id: "app".into(),
                name: "App".into(),
            },
            environment: EnvironmentRef {
                slug: None,
                name: "Primary".into(),
                primary: true,
                branch: None,
            },
            service: ServiceRef {
                id: "web".into(),
                name: "web".into(),
                config_rev: "0123456789abcdef".into(),
            },
            local_url: "https://web.app.localify".into(),
            port: 4100,
            reachable: Some(true),
            run: Some(run),
            public_url: None,
            profiles: ProfileChecks {
                start: true,
                restart: true,
            },
        },
        record_names: None,
    }
}

async fn controls(b: &Daemon, on: bool) {
    let change = SettingsChange {
        share: Some(true),
        control: Some(on),
        background_starts: Some(false),
    };
    b.link.change_settings(change).await.unwrap();
}

/// A's management socket, as its daemon serves it.
async fn socket(a: &Daemon) -> (PathBuf, ShutdownHandle, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mgmt.sock");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "").unwrap();
    let inventory = Arc::new(
        LocalInventory::new(config, dir.path().join("data"))
            .unwrap()
            .with_services_dir(dir.path().join("services")),
    );
    let shutdown = ShutdownHandle::default();
    let (reload, _) = tokio::sync::mpsc::channel(1);
    let serving = (socket.clone(), shutdown.clone(), a.link.clone());
    tokio::spawn(async move {
        let (socket, shutdown, remote) = serving;
        let state = Arc::new(ArcSwap::from_pointee(RuntimeState::new(Vec::new())));
        let ports = DaemonPorts {
            http: 80,
            https: 443,
            dns: 1053,
        };
        let services = MachineServices {
            tunnel: PublicTunnel::new(1),
            inventory,
            remote,
            updates: None,
        };
        let _ = serve(socket, state, reload, shutdown, ports, services).await;
    });
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (socket, shutdown, dir)
}

/// Runs a CLI command on a blocking thread, as the `localify` process
/// would, and gives back its exit code and what it printed.
async fn cli(
    socket: &Path,
    json: bool,
    command: impl FnOnce(&MgmtSocketClient, &mut Console<Vec<u8>, Vec<u8>>) -> anyhow::Result<i32>
    + Send
    + 'static,
) -> (i32, String, String) {
    let socket = socket.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let client = MgmtSocketClient::new(&socket);
        let mut console = Console {
            json,
            out: Vec::new(),
            err: Vec::new(),
        };
        let result = command(&client, &mut console);
        let code = finish(&mut console, result).unwrap();
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        (code, text(&console.out), text(&console.err))
    })
    .await
    .unwrap()
}

fn mini() -> Peer {
    Peer {
        id: "mB".into(),
        name: "teo-mini".into(),
    }
}

fn web_query() -> ServiceQuery<'static> {
    ServiceQuery {
        service: "web",
        project: None,
        env: None,
    }
}

#[tokio::test]
async fn the_cli_reads_and_controls_another_machine_through_the_daemon() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    controls(&b, true).await;
    b.source.set_app(AppPresence::Running);
    b.source.host().state().services = vec![web()];
    let run_dir = b.dir.path().join("services/output").join(RUN);
    create_run_dir(&run_dir).unwrap();
    let mut writer = StreamWriter::create(&run_dir).unwrap();
    writer.append(b"one\ntwo\nthree\n").unwrap();
    b.source.set_run(RUN, Ok(run_dir.clone()));
    let (socket, _shutdown, _dir) = socket(&a).await;
    // A and B agree on the latest protocol, 3 (`pair.saved`) or later, in
    // the session every command below goes through.
    let agreed = |daemon: &Daemon| daemon.link.shared.view().protos.get("mB").copied();
    a.link.open("mB", true).await.unwrap();
    assert_eq!(agreed(&a), Some(PROTO_MAX));
    const { assert!(PROTO_MAX >= PROTO_PAIR_SAVED) };

    // `--machine` names B by its name in any case, and A itself locally.
    let resolving = socket.clone();
    let (there, here) = tokio::task::spawn_blocking(move || {
        let client = MgmtSocketClient::new(&resolving);
        (
            machine("TEO-MINI", &client).unwrap(),
            machine("mA", &client).unwrap(),
        )
    })
    .await
    .unwrap();
    assert_eq!((there, here), (Machine::Remote(mini()), Machine::Local));

    let (code, out, _) = cli(&socket, true, |client, console| {
        status(client, &mini(), console)
    })
    .await;
    assert_eq!(code, 0);
    let snapshot: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(snapshot["items"][0]["service"]["id"], "web");
    assert_eq!(snapshot["items"][0]["output"]["runId"], RUN);

    // The last lines, then the cursor to read on from.
    let (code, out, err) = cli(&socket, true, |client, console| {
        let query = LogsQuery {
            service: web_query(),
            lines: 2,
            follow: false,
            cursor: None,
        };
        logs(client, &mini(), &query, console)
    })
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "two\nthree\n");
    assert!(err.ends_with(&format!("Next cursor: {RUN}:14\n")), "{err}");

    writer.append(b"four\n").unwrap();
    let (code, out, err) = cli(&socket, false, |client, console| {
        let cursor = format!("{RUN}:14");
        let query = LogsQuery {
            service: web_query(),
            lines: 50,
            follow: false,
            cursor: Some(&cursor),
        };
        logs(client, &mini(), &query, console)
    })
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "four\n");
    assert!(err.ends_with(&format!("Next cursor: {RUN}:19\n")), "{err}");

    // Following stops once the run ends.
    let follow = cli(&socket, false, |client, console| {
        let query = LogsQuery {
            service: web_query(),
            lines: 1,
            follow: true,
            cursor: None,
        };
        logs(client, &mini(), &query, console)
    });
    let ending = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        writer.append(b"five\n").unwrap();
        publish_end(&run_dir, &RunEnd::now(EndReason::Complete, writer.offset())).unwrap();
    };
    let ((code, out, err), ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(follow, ending)
    })
    .await
    .expect("the follow ended with the run");
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "four\nfive\n");
    assert!(err.contains("The run of 'web' ended."), "{err}");

    // A stop B allows goes through, with its progress first.
    let (code, out, err) = cli(&socket, false, |client, console| {
        control(
            client,
            &mini(),
            ControlAction::Stop,
            &web_query(),
            None,
            console,
        )
    })
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "Stopping 'web' on teo-mini…\n'web' stopped on teo-mini.\n"
    );
    assert_eq!(b.source.host().state().stops.len(), 1);
    assert_eq!(agreed(&a), Some(PROTO_MAX));

    // With controls off on B, nothing is sent and the refusal says why.
    controls(&b, false).await;
    let (code, out, _) = cli(&socket, true, |client, console| {
        control(
            client,
            &mini(),
            ControlAction::Start,
            &web_query(),
            None,
            console,
        )
    })
    .await;
    assert_eq!(code, REFUSED);
    let refused: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(refused["code"], "control_disabled");
    assert_eq!(
        refused["message"],
        "teo-mini doesn't allow remote controls."
    );
    assert!(b.source.host().state().handed.is_empty());
}

/// B with `web` running as [`RUN`], whose output holds `output`, and A's
/// management socket.
async fn web_on_b(
    cloud: &FakeCloud,
    output: &[u8],
) -> (Daemon, Daemon, PathBuf, ShutdownHandle, TempDir) {
    let (a, b) = two(cloud).await;
    pair(&a, &b).await;
    controls(&b, true).await;
    b.source.set_app(AppPresence::Running);
    b.source.host().state().services = vec![web()];
    let run_dir = b.dir.path().join("services/output").join(RUN);
    create_run_dir(&run_dir).unwrap();
    StreamWriter::create(&run_dir)
        .unwrap()
        .append(output)
        .unwrap();
    b.source.set_run(RUN, Ok(run_dir));
    let (socket, shutdown, dir) = socket(&a).await;
    (a, b, socket, shutdown, dir)
}

/// `web` restarts on B, whose state is in `dir`: [`NEXT_RUN`] replaces
/// [`RUN`], with `output`, and ends. `RUN` is gone, with its folder.
fn restart_on(source: &FakeSource, dir: &Path, output: &[u8]) {
    let next_dir = dir.join("services/output").join(NEXT_RUN);
    create_run_dir(&next_dir).unwrap();
    let mut writer = StreamWriter::create(&next_dir).unwrap();
    writer.append(output).unwrap();
    publish_end(
        &next_dir,
        &RunEnd::now(EndReason::Complete, writer.offset()),
    )
    .unwrap();
    source.set_run(NEXT_RUN, Ok(next_dir));
    if let Some(run) = source.host().state().services[0].facts.run.as_mut() {
        run.run_id = Some(NEXT_RUN.into());
    }
    // The next run of a service deletes the last one's folder.
    std::fs::remove_dir_all(dir.join("services/output").join(RUN)).unwrap();
    source.set_run(RUN, Err(RunLookup::Gone));
}

fn follow_web(lines: usize) -> LogsQuery<'static> {
    LogsQuery {
        service: web_query(),
        lines,
        follow: true,
        cursor: None,
    }
}

#[tokio::test]
async fn following_goes_on_with_the_run_that_replaces_a_restarted_one() {
    let cloud = FakeCloud::start().await;
    let (_a, b, socket, _shutdown, _dir) = web_on_b(&cloud, b"one\n").await;

    let follow = cli(&socket, true, |client, console| {
        logs(client, &mini(), &follow_web(5), console)
    });
    let restarting = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        restart_on(&b.source, b.dir.path(), b"new one\nnew two\n");
    };
    let ((code, out, err), ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(follow, restarting)
    })
    .await
    .expect("the follow ended with the new run");

    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "one\nnew one\nnew two\n");
    assert!(
        err.contains("'web' restarted on teo-mini: reading its new run from the start."),
        "{err}"
    );
    assert!(err.contains("The run of 'web' ended."), "{err}");
    assert!(
        err.ends_with(&format!("Next cursor: {NEXT_RUN}:16\n")),
        "{err}"
    );
}

#[tokio::test]
async fn a_restart_between_the_snapshot_and_the_first_read_reads_the_new_run() {
    let cloud = FakeCloud::start().await;
    let (_a, b, socket, _shutdown, _dir) = web_on_b(&cloud, b"one\n").await;
    // The CLI's snapshot still shows RUN; once it is built, web restarts.
    b.source.set_build_time(300);
    let built = b.source.builds();
    let (watcher, dir) = (b.source.clone(), b.dir.path().to_path_buf());
    let restarting = tokio::spawn(async move {
        while watcher.builds() == built {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        restart_on(&watcher, &dir, b"new\nsecond\nthird\n");
    });

    let (code, out, err) = cli(&socket, true, |client, console| {
        // Fewer lines than the new run printed: it is still read from its start.
        logs(client, &mini(), &follow_web(1), console)
    })
    .await;
    restarting.await.unwrap();

    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "new\nsecond\nthird\n");
    assert!(
        err.contains("'web' restarted on teo-mini: reading its new run from the start."),
        "{err}"
    );
    assert!(
        err.ends_with(&format!("Next cursor: {NEXT_RUN}:17\n")),
        "{err}"
    );
}
