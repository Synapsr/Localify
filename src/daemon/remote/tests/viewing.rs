//! Viewing another machine end to end: inventory, output, and the PR 3
//! gate, revocation (`docs/remote-machines.md`, "Gates").

use std::path::PathBuf;
use std::time::Duration;

use tokio::time::Instant;

use super::{Daemon, daemon, fast, pair, two, wait_until};
use crate::application::inventory::{
    AppPresence, Context, EnvironmentRef, InventoryItem, ProfileChecks, ProjectRef, ServiceFacts,
    ServiceRef, TargetSettings, inventory_item, pages,
};
use crate::daemon::remote::fake_relay::FakeCloud;
use crate::daemon::remote::local::RunLookup;
use crate::daemon::remote::viewing::WatchEvent;
use crate::daemon::remote::{FollowEvent, Following};
use crate::infrastructure::remote::store::SettingsChange;
use crate::infrastructure::runs::output::{
    EndReason, RunEnd, StreamWriter, create_run_dir, publish_end,
};

const RUN: &str = "AAAAAAAAAAAAAAAAAAAAAA";

async fn sharing(daemon: &Daemon, share: bool) {
    let change = SettingsChange {
        share: Some(share),
        ..Default::default()
    };
    daemon.link.change_settings(change).await.unwrap();
}

fn item(name: &str) -> InventoryItem {
    let facts = ServiceFacts {
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
            id: name.into(),
            name: name.into(),
            config_rev: "0123456789abcdef".into(),
        },
        local_url: format!("https://{name}.app.localify"),
        port: 3000,
        reachable: Some(false),
        run: None,
        public_url: None,
        profiles: ProfileChecks {
            start: true,
            restart: true,
        },
    };
    let context = Context {
        settings: TargetSettings::default(),
        app: AppPresence::Absent,
        tunnel_connected: false,
    };
    inventory_item(facts, &context)
}

/// Two paired machines, B sharing.
async fn paired_and_shared(cloud: &FakeCloud) -> (Daemon, Daemon) {
    let (a, b) = two(cloud).await;
    pair(&a, &b).await;
    sharing(&b, true).await;
    (a, b)
}

fn run_dir(b: &Daemon) -> PathBuf {
    b.dir.path().join("services/output").join(RUN)
}

/// A run's output on B, with `text` written so far.
fn run_on(b: &Daemon, text: &str) -> StreamWriter {
    let dir = run_dir(b);
    create_run_dir(&dir).unwrap();
    let mut writer = StreamWriter::create(&dir).unwrap();
    writer.append(text.as_bytes()).unwrap();
    b.source.set_run(RUN, Ok(dir));
    writer
}

async fn next_follow(follow: &mut Following) -> FollowEvent {
    tokio::time::timeout(Duration::from_secs(6), follow.next())
        .await
        .expect("a follow event")
        .expect("the follow stayed open")
}

/// Lines until `wanted` many arrived, acknowledged as a client shows them.
async fn lines_until(follow: &mut Following, wanted: usize) -> Vec<String> {
    let mut lines = Vec::new();
    while lines.len() < wanted {
        match next_follow(follow).await {
            FollowEvent::Data(data) => {
                follow.shown(data.next);
                lines.extend(data.lines.into_iter().map(|line| line.text));
            }
            FollowEvent::Ended { code, .. } => panic!("ended with {code}"),
        }
    }
    lines
}

fn ended_code(event: &FollowEvent) -> &str {
    match event {
        FollowEvent::Ended { code, .. } => code,
        FollowEvent::Data(data) => panic!("data, not an end: {data:?}"),
    }
}

#[tokio::test]
async fn a_target_that_does_not_share_shows_nothing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    b.source.set_items(vec![item("web")]);

    let refused = a.link.inventory("mB").await.unwrap_err();
    assert_eq!(refused.code, "not_shared");
    run_on(&b, "secret\n");
    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    assert_eq!(ended_code(&next_follow(&mut follow).await), "not_shared");
    assert_eq!(b.source.builds(), 0, "nothing was computed for it");
}

#[tokio::test]
async fn a_viewer_gets_every_page_then_unchanged() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    // Long names, so the items take several pages.
    let items: Vec<_> = (0..120)
        .map(|index| item(&format!("{index:03}-{}", "x".repeat(400))))
        .collect();
    assert!(pages(&items).unwrap().len() > 1);
    b.source.set_items(items);

    let first = a.link.inventory("mB").await.unwrap();
    assert_eq!(first.items.len(), 120);
    assert_eq!(first.machine.name, "teo-mini");
    assert!(first.machine.settings.share);

    let again = a.link.inventory("mB").await.unwrap();
    assert_eq!(again, first, "an unchanged revision is the one held");
}

#[tokio::test]
async fn a_watch_sends_each_change_and_nothing_once_nobody_watches() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    b.source.set_items(vec![item("web")]);

    let mut watch = a.link.watch("mB");
    let first = next_snapshot(&mut watch).await;
    assert_eq!(first.items.len(), 1);
    b.source.set_items(vec![item("web"), item("api")]);
    let second = next_snapshot(&mut watch).await;
    assert_eq!(second.items.len(), 2);
    assert!(second.rev > first.rev);

    drop(watch);
    wait_until("A's session closes", || a.session("mB").is_none()).await;
    let builds = b.source.builds();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(b.source.builds(), builds, "no snapshot without a watch");
}

async fn next_snapshot(
    watch: &mut tokio::sync::mpsc::Receiver<WatchEvent>,
) -> crate::daemon::remote::RemoteInventory {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(6), watch.recv())
            .await
            .expect("a watch event")
            .expect("the watch stayed open");
        match event {
            WatchEvent::Snapshot(inventory) => return inventory,
            WatchEvent::Lost { code, .. } => panic!("lost: {code}"),
            WatchEvent::Connecting => {}
        }
    }
}

#[tokio::test]
async fn a_follow_streams_new_lines_until_the_run_ends() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    let mut writer = run_on(&b, "one\ntwo\n");

    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    assert_eq!(lines_until(&mut follow, 2).await, ["one", "two"]);
    writer.append(b"\x1b[31mthree\x1b[0m\n").unwrap();
    assert_eq!(lines_until(&mut follow, 1).await, ["three"]);

    writer.append(b"last, unfinished").unwrap();
    let end = RunEnd::now(EndReason::Interrupted, writer.offset());
    publish_end(&run_dir(&b), &end).unwrap();
    let ended = loop {
        let FollowEvent::Data(data) = next_follow(&mut follow).await else {
            panic!("ended before the run did");
        };
        if data.ended {
            break data;
        }
    };
    assert!(ended.interrupted);
    assert_eq!(ended.lines.last().unwrap().text, "last, unfinished");
    assert!(follow.next().await.is_none(), "the follow is over");
}

#[tokio::test]
async fn a_follow_from_the_tail_starts_at_a_whole_line() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    let text: String = (0..200).map(|index| format!("line {index:03}\n")).collect();
    run_on(&b, &text);

    // Lines of 9 bytes: 30 bytes before the end is inside line 196.
    let mut follow = a.link.follow("mB", RUN, None, 30).await.unwrap();
    assert_eq!(
        lines_until(&mut follow, 3).await,
        ["line 197", "line 198", "line 199"]
    );
}

#[tokio::test]
async fn a_restarted_run_is_gone_and_a_terminal_run_unavailable() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    run_on(&b, "one\n");

    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    assert_eq!(lines_until(&mut follow, 1).await, ["one"]);
    // The next run of the service deletes this one's folder.
    std::fs::remove_dir_all(run_dir(&b)).unwrap();
    assert_eq!(ended_code(&next_follow(&mut follow).await), "gone");

    b.source
        .set_run(RUN, Err(RunLookup::Unavailable("terminal")));
    let refused = a.link.read_output("mB", RUN, 0, 1024).await.unwrap_err();
    assert_eq!(refused.code, "output_unavailable");
}

/// Turning sharing off ends the follow before the change is confirmed:
/// output written once it returns never reaches the viewer, even while the
/// follow was busy sending.
#[tokio::test]
async fn nothing_written_after_sharing_is_turned_off_reaches_the_viewer() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    let mut writer = run_on(&b, "one\n");
    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    assert_eq!(lines_until(&mut follow, 1).await, ["one"]);
    // Output keeps coming as the switch turns off.
    for index in 0..200 {
        writer
            .append(format!("before {index} {}\n", "x".repeat(200)).as_bytes())
            .unwrap();
    }

    sharing(&b, false).await;
    writer.append(b"secret after\n").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let code = loop {
        match next_follow(&mut follow).await {
            FollowEvent::Data(data) => {
                assert!(
                    data.lines.iter().all(|line| !line.text.contains("after")),
                    "output written after sharing was turned off reached the viewer"
                );
                follow.shown(data.next);
            }
            FollowEvent::Ended { code, .. } => break code,
        }
    };
    assert_eq!(code, "not_shared");
    assert!(follow.next().await.is_none());
}

/// A viewer asking for many snapshots and reads at once: the target admits
/// a bounded number, answers the rest `too_many` or `busy` without doing
/// the work, keeps what waits to be sent bounded, and serves again after.
#[tokio::test]
async fn a_viewer_flooding_gets_and_reads_keeps_the_target_bounded() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    let items: Vec<_> = (0..240)
        .map(|index| item(&format!("{index:03}-{}", "x".repeat(400))))
        .collect();
    b.source.set_items(items);
    let text: String = (0..4000)
        .map(|index| format!("{index:05} {}\n", "y".repeat(90)))
        .collect();
    run_on(&b, &text);

    let reads = (0..60).map(|index| a.link.read_output("mB", RUN, index * 1000, 16 * 1024));
    let gets = (0..12).map(|_| a.link.inventory("mB"));
    let (reads, gets) = tokio::join!(
        futures_util::future::join_all(reads),
        futures_util::future::join_all(gets)
    );

    let refused = |code: &str| code == "too_many" || code == "busy";
    let read_refusals = reads
        .iter()
        .filter(|read| read.as_ref().is_err_and(|failure| refused(&failure.code)))
        .count();
    let codes: Vec<_> = reads
        .iter()
        .map(|read| {
            read.as_ref()
                .map_or_else(|failure| failure.code.clone(), |_| "ok".into())
        })
        .collect();
    assert!(read_refusals > 0, "some reads were refused: {codes:?}");
    assert!(reads.iter().any(Result::is_ok), "some reads were answered");
    for get in &gets {
        if let Err(failure) = get {
            assert!(refused(&failure.code), "{failure:?}");
        }
    }
    // One session's bound: 1 MiB and 64 KiB.
    let peak = b
        .link
        .shared
        .pending_peak
        .load(std::sync::atomic::Ordering::SeqCst);
    assert!(peak <= 1024 * 1024 + 64 * 1024, "{peak}");
    assert!(peak > 0);

    // Everything admitted drained: the target answers again.
    assert!(a.link.read_output("mB", RUN, 0, 1024).await.is_ok());
    assert_eq!(a.link.inventory("mB").await.unwrap().items.len(), 240);
}

/// The gate through the management socket, as the app follows output:
/// `remote_output_follow` streams lines, and removing the target ends the
/// stream within 2 seconds.
#[tokio::test]
async fn removing_the_target_ends_a_follow_on_the_management_socket() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    run_on(&b, "one\n");
    let socket = serve_management(&a).await;

    let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let command = serde_json::json!({
        "cmd": "remote_output_follow", "machineId": "mB", "runId": RUN, "offset": 0,
    });
    stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .unwrap();
    let mut lines = BufReader::new(stream).lines();
    let mut next_line = async || {
        tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("an answer within 2 s")
            .unwrap()
    };
    let first: serde_json::Value = serde_json::from_str(&next_line().await.unwrap()).unwrap();
    assert_eq!(first["data"]["event"], "data");
    assert_eq!(first["data"]["lines"][0]["text"], "one");

    let started = Instant::now();
    cloud.revoke("mB");
    let last: serde_json::Value = serde_json::from_str(&next_line().await.unwrap()).unwrap();
    assert_eq!(last["data"]["event"], "ended", "{last}");
    assert!(next_line().await.is_none(), "the stream closed");
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// The daemon's management socket, answering with `daemon`'s link.
async fn serve_management(daemon: &Daemon) -> PathBuf {
    use crate::daemon::lifecycle::ShutdownHandle;
    use crate::daemon::mgmt_socket::{DaemonPorts, MachineServices, serve};
    use crate::daemon::public_tunnel::PublicTunnel;
    use crate::daemon::router::RuntimeState;
    use crate::infrastructure::inventory::LocalInventory;

    let dir = daemon.dir.path();
    let socket = dir.join("mgmt.sock");
    let config = dir.join("config.toml");
    std::fs::write(&config, "").unwrap();
    let inventory = LocalInventory::new(config, dir.to_path_buf()).unwrap();
    let state = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(RuntimeState::new(
        Vec::new(),
    )));
    let (reload, _) = tokio::sync::mpsc::channel(1);
    let services = MachineServices {
        tunnel: PublicTunnel::new(1),
        inventory: std::sync::Arc::new(inventory),
        remote: daemon.link.clone(),
    };
    let ports = DaemonPorts {
        http: 80,
        https: 443,
        dns: 1053,
    };
    let path = socket.clone();
    tokio::spawn(async move {
        let _ = serve(
            path,
            state,
            reload,
            ShutdownHandle::default(),
            ports,
            services,
        )
        .await;
    });
    wait_until("the management socket listens", || socket.exists()).await;
    socket
}

#[tokio::test]
async fn a_bounded_read_answers_once() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    run_on(&b, "one\ntwo\nthree\n");

    let data = a.link.read_output("mB", RUN, 4, 1024).await.unwrap();
    let lines: Vec<_> = data.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(lines, ["two", "three"]);
    assert_eq!((data.offset, data.next), (4, 14));
}

/// A megabyte of output, paced: the relay's budgets refuse nothing, and
/// every line arrives once, in order.
#[tokio::test]
async fn a_large_follow_stays_inside_the_relays_budgets() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    let text: String = (0..20_000)
        .map(|index| format!("{index:05} {}\n", "y".repeat(40)))
        .collect();
    run_on(&b, &text);

    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    let lines = lines_until(&mut follow, 20_000).await;
    assert_eq!(lines.len(), 20_000);
    assert!(lines[19_999].starts_with("19999"));
    assert_eq!(cloud.refusals(), 0);
}

// --- The PR 3 gate: revocation end to end -------------------------------------

/// The stream's next event is its end, within `bound`.
async fn ends_within(follow: &mut Following, bound: Duration) {
    let started = Instant::now();
    let event = next_follow(follow).await;
    assert!(matches!(event, FollowEvent::Ended { .. }), "{event:?}");
    assert!(started.elapsed() < bound, "{:?}", started.elapsed());
}

/// `revokeMachine`: B is removed from the account while A follows its
/// output. The stream ends within 2 seconds, and A drops B's pin.
#[tokio::test]
async fn removing_the_target_ends_an_open_output_stream_at_once() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    run_on(&b, "one\n");
    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    assert_eq!(lines_until(&mut follow, 1).await, ["one"]);

    cloud.revoke("mB");
    ends_within(&mut follow, Duration::from_secs(2)).await;
    assert!(!a.pinned("mB"));
}

/// `revokeAccount`: the account lapses, and the relay closes both sockets
/// with `4402`. The stream ends within 2 seconds, and nothing opens one
/// again while leases are refused.
#[tokio::test]
async fn a_lapse_pushed_by_the_relay_ends_an_open_output_stream_at_once() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    run_on(&b, "one\n");
    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    assert_eq!(lines_until(&mut follow, 1).await, ["one"]);

    cloud.set_lease(402, 60);
    cloud.close("mA", 4402, "remote access lapsed");
    cloud.close("mB", 4402, "remote access lapsed");
    ends_within(&mut follow, Duration::from_secs(2)).await;
    wait_until("A says it needs Pro", || {
        a.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "subscription_required")
    })
    .await;
    assert!(a.link.follow("mB", RUN, Some(0), 0).await.is_err());
}

/// A change Localify Cloud does not push (an override that expires): the
/// next lease renewal is refused, and the stream ends with it, though
/// output keeps flowing. With the contract's timings that is within 75
/// seconds (a 45-second renewal and its jitter, the relay's own checks
/// within 60); here, within the shortened renewal.
#[tokio::test]
async fn a_lapse_nobody_pushed_ends_an_open_output_stream_at_the_next_renewal() {
    let cloud = FakeCloud::start().await;
    let (a, b) = paired_and_shared(&cloud).await;
    let mut writer = run_on(&b, "one\n");
    let mut follow = a.link.follow("mB", RUN, Some(0), 0).await.unwrap();
    assert_eq!(lines_until(&mut follow, 1).await, ["one"]);

    let started = Instant::now();
    cloud.set_lease(402, 60);
    loop {
        writer.append(b"tick\n").unwrap();
        match next_follow(&mut follow).await {
            FollowEvent::Data(data) => follow.shown(data.next),
            FollowEvent::Ended { .. } => break,
        }
    }
    let timings = fast();
    let bound = timings.lease_renew + timings.lease_renew_jitter + Duration::from_millis(500);
    assert!(started.elapsed() < bound, "{:?}", started.elapsed());
}

/// After `4402`, lease polling that hears `403 banned` says the account is
/// suspended, not that it needs Pro, and stops.
#[tokio::test]
async fn a_ban_found_while_polling_a_lapse_says_banned() {
    let cloud = FakeCloud::start().await;
    let a = daemon(&cloud, "mA", "laptop").await;
    cloud.set_lease(402, 60);
    cloud.close("mA", 4402, "remote access lapsed");
    let a = &a;
    let blocked_with = |code: &'static str| {
        move || {
            a.link
                .status()
                .blocked
                .is_some_and(|blocked| blocked.code == code)
        }
    };
    wait_until("it needs Pro", blocked_with("subscription_required")).await;

    cloud.set_lease(403, 60);
    wait_until("it is banned", blocked_with("banned")).await;
    let connections = cloud.connections();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(cloud.connections(), connections, "a banned account stops");
}
