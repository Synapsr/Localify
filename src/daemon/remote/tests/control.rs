//! Remote controls end to end, between daemons in this process and the fake
//! relay: the PR 4 gates of `docs/remote-machines.md` for the reconnect, the
//! dropped frame, request entries, restarts, convergence and the legacy
//! relaunch. The target runs the real control executor and keeps its
//! request entries on disk; only its services are in memory (`FakeHost`).
//! The same gates against real processes are in `tests/remote_controls.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{Daemon, connected, launch, pair, two, wait_until};
use crate::application::inventory::{
    EnvironmentRef, ProfileChecks, ProjectRef, RunFacts, RunOrigin, RunProcess, ServiceFacts,
    ServiceRef,
};
use crate::daemon::remote::fake_relay::{Bridge, FakeCloud, FakeService, Route, is_message};
use crate::daemon::remote::{ControlAsk, ControlEvent};
use crate::infrastructure::remote::control::Expected;
use crate::infrastructure::remote::store::SettingsChange;
use crate::infrastructure::remote::wire::{
    AppPresence, ControlAction, ControlExpect, ControlOutcome, ControlPhase, ControlService,
    ProtoRange,
};
use crate::infrastructure::runs::RunId;
use localify::desktop::shell_environment::ShellEnvironment;

const REV: &str = "0123456789abcdef";
const RUN: &str = "RRRRRRRRRRRRRRRRRRRRRR";
const LEGACY: &str = "boot.4242.77";

fn facts(run: Option<RunFacts>) -> ServiceFacts {
    let running = run
        .as_ref()
        .is_some_and(|run| run.process == RunProcess::Running);
    ServiceFacts {
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
            config_rev: REV.into(),
        },
        local_url: "https://web.localify".into(),
        port: 4100,
        reachable: Some(running),
        run,
        public_url: None,
        profiles: ProfileChecks {
            start: true,
            restart: true,
        },
    }
}

fn run(origin: RunOrigin) -> RunFacts {
    RunFacts {
        run_id: Some(RUN.into()),
        origin,
        legacy: false,
        has_stream: true,
        process: RunProcess::Running,
        started_at: "2026-11-09T09:00:00Z".into(),
        profile: None,
        profile_danger: false,
        exit: None,
        legacy_id: None,
    }
}

/// A legacy run of `origin`; proven when `proven`.
fn legacy(origin: RunOrigin, proven: bool) -> RunFacts {
    RunFacts {
        run_id: None,
        legacy: true,
        has_stream: false,
        legacy_id: proven.then(|| LEGACY.to_string()),
        process: if proven {
            RunProcess::Running
        } else {
            RunProcess::Unproven
        },
        ..run(origin)
    }
}

fn service(run: Option<RunFacts>) -> FakeService {
    let record_names = run
        .as_ref()
        .filter(|run| run.legacy)
        .map(|_| LEGACY.to_string());
    FakeService {
        facts: facts(run),
        record_names,
    }
}

fn ask(action: ControlAction, run_id: Option<&str>) -> ControlAsk {
    ControlAsk {
        action,
        service: ControlService {
            project_id: "app".into(),
            environment: None,
            service_id: "web".into(),
        },
        profile: None,
        expect: ControlExpect {
            run_id: run_id.map(str::to_string),
            config_rev: REV.into(),
            legacy_id: None,
        },
    }
}

fn relaunch() -> ControlAsk {
    let mut ask = ask(ControlAction::Restart, None);
    ask.expect.legacy_id = Some(LEGACY.into());
    ask
}

fn request_id(n: u8) -> String {
    format!("{}{n:02}", "q".repeat(20))
}

async fn switches(daemon: &Daemon, background_starts: bool) {
    daemon
        .link
        .change_settings(SettingsChange {
            share: Some(true),
            control: Some(true),
            background_starts: Some(background_starts),
        })
        .await
        .unwrap();
}

/// A and B paired, with B sharing and allowing controls, its app running,
/// and `web` in the state `run` says; no session yet.
async fn set_up(cloud: &FakeCloud, run: Option<RunFacts>) -> (Daemon, Daemon) {
    let (a, b) = two(cloud).await;
    pair(&a, &b).await;
    switches(&b, false).await;
    b.source.set_app(AppPresence::Running);
    b.source.host().state().services = vec![service(run)];
    (a, b)
}

/// [`set_up`], with A's session to B open, as the app's view of B holds
/// it: a control goes only over an open session.
async fn controlled(cloud: &FakeCloud, run: Option<RunFacts>) -> (Daemon, Daemon) {
    let (a, b) = set_up(cloud, run).await;
    a.link.open("mB", true).await.unwrap();
    (a, b)
}

/// Every event of a control, up to its last.
async fn events(mut events: mpsc::Receiver<ControlEvent>) -> Vec<ControlEvent> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("the control ended in time");
        let Some(event) = event else { return seen };
        let last = matches!(
            event,
            ControlEvent::Result { .. } | ControlEvent::Lost { .. }
        );
        seen.push(event);
        if last {
            return seen;
        }
    }
}

/// The outcome and code a control ended with.
fn ended(events: &[ControlEvent]) -> (ControlOutcome, Option<String>) {
    match events.last() {
        Some(ControlEvent::Result { outcome, code, .. }) => (*outcome, code.clone()),
        other => panic!("no result: {events:?} ({other:?})"),
    }
}

async fn control(a: &Daemon, ask: ControlAsk) -> Vec<ControlEvent> {
    events(a.link.control("mB", ask)).await
}

fn handed(b: &Daemon) -> usize {
    b.source.host().state().handed.len()
}

fn stops(b: &Daemon) -> Vec<Expected> {
    b.source.host().state().stops.clone()
}

/// Holds B's next launch once it reached the app: dropping or sending on
/// the result lets it go on.
fn hold(b: &Daemon) -> std::sync::mpsc::Sender<()> {
    let (release, held) = std::sync::mpsc::channel();
    b.source.host().state().hold = Some(held);
    release
}

// --- Who acts ------------------------------------------------------------------

#[tokio::test]
async fn a_restart_of_an_app_run_goes_through_the_app_and_takes_its_place() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(run(RunOrigin::App))).await;

    let seen = control(&a, ask(ControlAction::Restart, Some(RUN))).await;

    assert!(matches!(seen[0], ControlEvent::Sent { .. }), "{seen:?}");
    let phases: Vec<ControlPhase> = seen
        .iter()
        .filter_map(|event| match event {
            ControlEvent::Progress { phase } => Some(*phase),
            _ => None,
        })
        .collect();
    assert_eq!(phases, [ControlPhase::Dispatched, ControlPhase::Started]);
    assert_eq!(ended(&seen), (ControlOutcome::Done, None));
    let host = b.source.host();
    let state = host.state();
    assert_eq!(state.stops, [Expected::Run(RunId::parse(RUN).unwrap())]);
    assert_eq!(state.handed.len(), 1);
    assert_eq!(
        state.handed[0].replaces.as_ref().map(RunId::as_str),
        Some(RUN)
    );
    let requester = state.handed[0].requested_by.clone().unwrap();
    assert_eq!(
        (requester.machine_id.as_str(), requester.name.as_str()),
        ("mA", "laptop")
    );
    assert!(state.detached.is_empty(), "never a background start");
}

#[tokio::test]
async fn a_start_with_the_app_closed_runs_detached_only_with_background_starts() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, None).await;
    b.source.set_app(AppPresence::Absent);

    let refused = control(&a, ask(ControlAction::Start, None)).await;
    assert_eq!(
        ended(&refused),
        (ControlOutcome::Refused, Some("background_disabled".into()))
    );

    switches(&b, true).await;
    let started = control(&a, ask(ControlAction::Start, None)).await;
    assert_eq!(ended(&started), (ControlOutcome::Done, None));
    let host = b.source.host();
    let state = host.state();
    assert_eq!((state.handed.len(), state.detached.len()), (0, 1));
    assert_eq!(state.detached[0].request_id.len(), 22);
}

/// An app that closed before the launch reached it, or refused it, never
/// gets a background start in its place.
#[tokio::test]
async fn an_app_that_closed_or_refused_a_start_gets_nothing_in_its_place() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, None).await;
    switches(&b, true).await;

    b.source.host().state().bridge = Bridge::Absent;
    let closed = control(&a, ask(ControlAction::Start, None)).await;
    let Some(ControlEvent::Result {
        outcome, message, ..
    }) = closed.last()
    else {
        panic!("{closed:?}")
    };
    assert_eq!(*outcome, ControlOutcome::Failed);
    assert!(message.contains("Nothing was started"), "{message}");

    b.source.host().state().bridge = Bridge::Refuse("unknown_service");
    let refused = control(&a, ask(ControlAction::Start, None)).await;
    assert_eq!(
        ended(&refused),
        (ControlOutcome::Refused, Some("unknown_service".into()))
    );
    assert!(b.source.host().state().detached.is_empty());
}

/// The gate: a restart of a background run with `backgroundStarts` off is
/// refused, and the run keeps running.
#[tokio::test]
async fn a_background_restart_without_background_starts_is_refused_and_keeps_running() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(run(RunOrigin::Background))).await;

    let seen = control(&a, ask(ControlAction::Restart, Some(RUN))).await;

    assert_eq!(
        ended(&seen),
        (ControlOutcome::Refused, Some("background_disabled".into()))
    );
    assert!(stops(&b).is_empty(), "nothing was stopped");
    let still = b.source.host().service("web").run.unwrap();
    assert_eq!(still.process, RunProcess::Running);
    assert_eq!(still.run_id.as_deref(), Some(RUN));
}

#[tokio::test]
async fn controls_are_refused_while_the_switches_or_the_lease_say_no() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, None).await;
    b.link
        .change_settings(SettingsChange {
            control: Some(false),
            ..SettingsChange::default()
        })
        .await
        .unwrap();
    let off = control(&a, ask(ControlAction::Start, None)).await;
    assert_eq!(
        ended(&off),
        (ControlOutcome::Refused, Some("control_disabled".into()))
    );

    switches(&b, false).await;
    // The target asks for a fresh lease before each control, and fails
    // closed.
    cloud.set_lease(402, 60);
    let lapsed = control(&a, ask(ControlAction::Start, None)).await;
    assert_eq!(
        ended(&lapsed),
        (ControlOutcome::Refused, Some("unauthorized".into()))
    );
    assert_eq!(handed(&b), 0);
}

/// One control per service at a time on the target: another one waits for
/// nothing and is refused `busy`.
#[tokio::test]
async fn a_second_control_of_a_service_in_flight_is_busy() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(run(RunOrigin::App))).await;
    let release = hold(&b);

    let first = a.link.control("mB", ask(ControlAction::Restart, Some(RUN)));
    wait_until("the restart reached the app", || handed(&b) == 1).await;
    let second = control(&a, ask(ControlAction::Restart, Some(RUN))).await;
    release.send(()).unwrap();

    assert_eq!(
        ended(&second),
        (ControlOutcome::Refused, Some("busy".into()))
    );
    assert_eq!(ended(&events(first).await), (ControlOutcome::Done, None));
    assert_eq!(handed(&b), 1);
}

// --- Reconnects, dropped frames, request entries ------------------------------

/// The gate: the viewer's socket is cut right after it sends a restart,
/// and the same control is sent again on a new session, with the same and
/// with a new `requestId`. The service restarted once; the same request
/// gets the report of the first restart, and the new one `conflict`.
#[tokio::test]
async fn a_reconnect_cannot_repeat_a_restart() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(run(RunOrigin::App))).await;
    let release = hold(&b);

    let first = a
        .link
        .control_as("mB", ask(ControlAction::Restart, Some(RUN)), request_id(1));
    wait_until("the restart reached the app", || handed(&b) == 1).await;
    cloud.close("mA", 1012, "restart");
    release.send(()).unwrap();
    let seen = events(first).await;

    // Lost with the socket, then reconciled on a new session.
    assert!(
        seen.iter()
            .any(|event| matches!(event, ControlEvent::Unknown { .. })),
        "{seen:?}"
    );
    assert_eq!(ended(&seen), (ControlOutcome::Done, None));
    let ControlEvent::Result {
        run_id: first_run, ..
    } = seen.last().unwrap()
    else {
        unreachable!()
    };

    a.link.open("mB", true).await.unwrap();
    let same = events(a.link.control_as(
        "mB",
        ask(ControlAction::Restart, Some(RUN)),
        request_id(1),
    ))
    .await;
    assert_eq!(ended(&same), (ControlOutcome::Done, None));
    let ControlEvent::Result {
        run_id: same_run, ..
    } = same.last().unwrap()
    else {
        unreachable!()
    };
    assert_eq!(same_run, first_run, "the report of the first restart");

    let new = events(a.link.control_as(
        "mB",
        ask(ControlAction::Restart, Some(RUN)),
        request_id(2),
    ))
    .await;
    assert_eq!(
        ended(&new),
        (ControlOutcome::Conflict, Some("conflict".into()))
    );
    assert_eq!(handed(&b), 1, "restarted once");
    assert_eq!(stops(&b).len(), 1);
}

/// The gate: the relay drops the frame carrying a control's answer; the
/// viewer reconciles with `control.status` on a new session, gets the exact
/// outcome, and never sends the control again.
#[tokio::test]
async fn a_dropped_frame_during_a_control_is_reconciled_and_never_resent() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(run(RunOrigin::App))).await;
    let release = hold(&b);
    let armed = Arc::new(AtomicBool::new(false));
    let dropping = Arc::clone(&armed);
    cloud.set_hook(Some(Box::new(move |from, frame| {
        if from == "mB" && is_message(frame) && dropping.swap(false, Ordering::SeqCst) {
            Route::Drop("unavailable")
        } else {
            Route::Forward
        }
    })));

    let pending = a.link.control("mB", ask(ControlAction::Restart, Some(RUN)));
    wait_until("the restart reached the app", || handed(&b) == 1).await;
    // The next frame of B's carries what follows the dispatch.
    armed.store(true, Ordering::SeqCst);
    release.send(()).unwrap();
    let seen = events(pending).await;

    assert!(
        seen.iter()
            .any(|event| matches!(event, ControlEvent::Unknown { .. })),
        "{seen:?}"
    );
    assert_eq!(ended(&seen), (ControlOutcome::Done, None));
    assert_eq!(handed(&b), 1, "never sent again");
    assert_eq!(stops(&b).len(), 1);
}

/// The gate: a stop whose answer was lost is still correlated by
/// `control.status` after the target's daemon restarts, and the same
/// request id with another action gets `request_mismatch` and runs nothing.
#[tokio::test]
async fn request_entries_survive_a_daemon_restart_and_a_mismatch_runs_nothing() {
    let cloud = FakeCloud::start().await;
    let a = super::daemon(&cloud, "mA", "laptop").await;
    let token = cloud.add_machine("mB", "teo-mini");
    let b = connected(launch(
        &cloud,
        "mB",
        "teo-mini",
        &token,
        tempfile::tempdir().unwrap(),
    ))
    .await;
    wait_until("both see each other", || a.sees("mB") && b.sees("mA")).await;
    pair(&a, &b).await;
    switches(&b, false).await;
    b.source.set_app(AppPresence::Running);
    b.source.host().state().services = vec![service(Some(run(RunOrigin::App)))];
    a.link.open("mB", true).await.unwrap();

    let stopped = events(a.link.control_as(
        "mB",
        ask(ControlAction::Stop, Some(RUN)),
        request_id(7),
    ))
    .await;
    assert_eq!(ended(&stopped), (ControlOutcome::Done, None));

    // B's daemon starts again on the same folder.
    let copy = tempfile::tempdir().unwrap();
    copy_dir(b.dir.path(), copy.path());
    drop(b);
    let b = connected(launch(&cloud, "mB", "teo-mini", &token, copy)).await;
    wait_until("A sees B again", || a.sees("mB")).await;

    let status = a.link.control_status("mB", &request_id(7)).await;
    assert!(
        matches!(
            status,
            ControlEvent::Result {
                outcome: ControlOutcome::Done,
                ..
            }
        ),
        "{status:?}"
    );
    a.link.open("mB", true).await.unwrap();
    let mismatch = events(
        a.link
            .control_as("mB", ask(ControlAction::Start, None), request_id(7)),
    )
    .await;
    assert_eq!(
        ended(&mismatch),
        (ControlOutcome::Refused, Some("request_mismatch".into()))
    );
    let unknown = a.link.control_status("mB", &request_id(8)).await;
    assert!(
        matches!(&unknown, ControlEvent::Result { code: Some(code), .. } if code == "unknown_request"),
        "{unknown:?}"
    );
    assert!(stops(&b).is_empty() && handed(&b) == 0, "nothing ran");
}

/// The gate: launch A starts and exits, a local launch B then replaces the
/// service's record, and the bridge's answer about A was lost. The target
/// still reports A as admitted and exited, with its code.
#[tokio::test]
async fn a_lost_acknowledgement_is_reported_exactly() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, None).await;
    {
        let host = b.source.host();
        let mut state = host.state();
        state.bridge = Bridge::Unanswered;
        state.exit_code = Some(3);
    }
    let pending = a.link.control("mB", ask(ControlAction::Start, None));
    wait_until("the start reached the app", || handed(&b) == 1).await;
    // A local launch takes the service over meanwhile.
    b.source.host().change("web", |web| {
        web.facts.run = Some(RunFacts {
            run_id: Some("LLLLLLLLLLLLLLLLLLLLLL".into()),
            ..run(RunOrigin::Terminal)
        });
    });
    let seen = events(pending).await;

    let Some(ControlEvent::Result {
        outcome,
        message,
        run_id,
        ..
    }) = seen.last()
    else {
        panic!("no result: {seen:?}")
    };
    assert_eq!(*outcome, ControlOutcome::Failed);
    assert!(message.contains("exited with code 3"), "{message}");
    let launched = b.source.host().state().handed[0].launch_id.to_string();
    assert_eq!(run_id.as_deref(), Some(launched.as_str()), "launch A");
}

// --- Convergence ---------------------------------------------------------------

/// The gate: while A watches, the service is started from a terminal,
/// stopped, and its config edited. Each shows within 12 seconds, and a
/// control built on the old state gets `conflict`.
#[tokio::test]
async fn external_starts_and_config_changes_converge_and_an_old_control_conflicts() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, None).await;
    let mut watch = a.link.watch("mB");
    shows(&mut watch, |item| item["owner"].is_null()).await;

    // Started from a terminal.
    b.source.host().change("web", |web| {
        web.facts.run = Some(run(RunOrigin::Terminal));
        web.facts.reachable = Some(true);
    });
    shows(&mut watch, |item| item["owner"] == "terminal").await;
    let old_start = control(&a, ask(ControlAction::Start, None)).await;
    assert_eq!(ended(&old_start).0, ControlOutcome::Conflict);

    // Stopped with `localify stop`.
    b.source.host().change("web", |web| {
        if let Some(run) = web.facts.run.as_mut() {
            run.process = RunProcess::Stopped;
        }
        web.facts.reachable = Some(false);
    });
    shows(&mut watch, |item| item["process"] == "stopped").await;

    // Its config edited.
    b.source.host().change("web", |web| {
        web.facts.service.config_rev = "fedcba9876543210".into();
    });
    shows(&mut watch, |item| {
        item["service"]["configRev"] == "fedcba9876543210"
    })
    .await;
    let old_rev = control(&a, ask(ControlAction::Start, None)).await;
    assert_eq!(ended(&old_rev).0, ControlOutcome::Conflict);
    assert_eq!(handed(&b), 0);
}

/// Waits for a snapshot whose first item `wanted` accepts, 12 s at most.
async fn shows(
    watch: &mut mpsc::Receiver<crate::daemon::remote::viewing::WatchEvent>,
    wanted: fn(&serde_json::Value) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let event = tokio::time::timeout_at(deadline, watch.recv())
            .await
            .expect("it converged within 12 seconds")
            .unwrap();
        if let crate::daemon::remote::viewing::WatchEvent::Snapshot(snapshot) = event
            && wanted(&snapshot.items[0])
        {
            return;
        }
    }
}

// --- Relaunching a legacy run -------------------------------------------------

#[tokio::test]
async fn a_proven_legacy_run_is_relaunched_through_its_owner() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(legacy(RunOrigin::App, true))).await;

    let seen = control(&a, relaunch()).await;

    assert_eq!(ended(&seen), (ControlOutcome::Done, None));
    let host = b.source.host();
    let state = host.state();
    assert_eq!(state.stops, [Expected::Legacy(LEGACY.into())]);
    assert_eq!(state.handed[0].replaces_legacy.as_deref(), Some(LEGACY));
    assert_eq!(state.handed[0].replaces, None, "a new tab");
    let web = state.services[0].facts.run.clone().unwrap();
    assert!(!web.legacy && web.run_id.is_some() && web.has_stream);
}

/// The gate's refusals: each before anything stops.
#[tokio::test]
async fn a_legacy_relaunch_is_refused_before_anything_stops() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(legacy(RunOrigin::App, false))).await;

    // Its PID is not proven.
    let unproven = control(&a, relaunch()).await;
    assert_eq!(ended(&unproven).1.as_deref(), Some("legacy_run"));

    // The record names another run.
    b.source.host().state().services = vec![FakeService {
        record_names: Some("boot.1.1".into()),
        ..service(Some(legacy(RunOrigin::App, true)))
    }];
    let renamed = control(&a, relaunch()).await;
    assert_eq!(ended(&renamed).0, ControlOutcome::Conflict);

    // The app is gone for an app run; background starts are off for a
    // background one.
    b.source.host().state().services = vec![service(Some(legacy(RunOrigin::App, true)))];
    b.source.set_app(AppPresence::Absent);
    let gone = control(&a, relaunch()).await;
    assert_eq!(ended(&gone).1.as_deref(), Some("owner_gone"));
    b.source.host().state().services = vec![service(Some(legacy(RunOrigin::Background, true)))];
    let background = control(&a, relaunch()).await;
    assert_eq!(ended(&background).1.as_deref(), Some("background_disabled"));

    // A control on it that is not a relaunch.
    let stop = control(&a, ask(ControlAction::Stop, None)).await;
    assert_eq!(ended(&stop).1.as_deref(), Some("legacy_run"));

    assert!(stops(&b).is_empty(), "nothing was signalled");
}

/// The gate: a local launch that replaces the record between the stop and
/// the dispatch wins, and the relaunch gets `conflict`.
#[tokio::test]
async fn a_local_launch_between_the_stop_and_the_relaunch_wins() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(legacy(RunOrigin::App, true))).await;
    let release = hold(&b);

    let pending = a.link.control("mB", relaunch());
    wait_until("the relaunch reached the app", || handed(&b) == 1).await;
    b.source.host().change("web", |web| {
        web.record_names = None;
        web.facts.run = Some(run(RunOrigin::Terminal));
    });
    release.send(()).unwrap();
    let seen = events(pending).await;

    assert_eq!(
        ended(&seen),
        (ControlOutcome::Conflict, Some("conflict".into()))
    );
    let web = b.source.host().service("web").run.unwrap();
    assert_eq!(web.origin, RunOrigin::Terminal, "the local launch stays");
}

/// A target of protocol 1 knows no `legacyId`: the viewer never sends one.
#[tokio::test]
async fn a_relaunch_is_never_sent_to_a_target_of_protocol_1() {
    let cloud = FakeCloud::start().await;
    let (a, b) = set_up(&cloud, Some(legacy(RunOrigin::App, true))).await;
    b.link.speak(ProtoRange { min: 1, max: 1 });
    a.link.open("mB", true).await.unwrap();

    let seen = control(&a, relaunch()).await;

    assert_eq!(ended(&seen).1.as_deref(), Some("unsupported"));
    assert!(stops(&b).is_empty() && handed(&b) == 0);
}

// --- Bounds: sessions, sharing, pairing, admission ---------------------------

/// A control goes only over a session already open: without one it is
/// refused at once, with the connection state, and never queued. A status
/// query may still open one.
#[tokio::test]
async fn a_control_without_an_open_session_is_refused_and_never_queued() {
    let cloud = FakeCloud::start().await;
    let (a, b) = set_up(&cloud, None).await;

    let seen = control(&a, ask(ControlAction::Start, None)).await;

    assert_eq!(seen.len(), 1, "never announced as sent: {seen:?}");
    assert_eq!(
        ended(&seen),
        (ControlOutcome::Refused, Some("not_connected".into()))
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(a.session("mB").is_none(), "no session was opened for it");
    assert_eq!(handed(&b), 0);
    let status = a.link.control_status("mB", &request_id(1)).await;
    assert!(
        matches!(&status, ControlEvent::Result { code: Some(code), .. } if code == "unknown_request"),
        "{status:?}"
    );
}

/// Turning `share` off drops every answer still to come: a launch that
/// fails afterwards sends none of its output, and `control.status` reads
/// nothing while B doesn't share.
#[tokio::test]
async fn turning_share_off_drops_a_control_s_answer_and_its_output() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, None).await;
    b.source.host().state().exit_code = Some(3);
    let release = hold(&b);
    let pending = a.link.control("mB", ask(ControlAction::Start, None));
    wait_until("the start reached the app", || handed(&b) == 1).await;

    b.link
        .change_settings(SettingsChange {
            share: Some(false),
            ..SettingsChange::default()
        })
        .await
        .unwrap();
    release.send(()).unwrap();
    let seen = events(pending).await;

    assert!(
        matches!(seen.last(), Some(ControlEvent::Lost { code, .. }) if code == "not_shared"),
        "{seen:?}"
    );
    assert!(!format!("{seen:?}").contains("the last line"), "{seen:?}");
    let Some(ControlEvent::Sent { request_id }) = seen.first() else {
        panic!("{seen:?}")
    };
    let status = a.link.control_status("mB", request_id).await;
    assert!(
        matches!(&status, ControlEvent::Result { code: Some(code), .. } if code == "not_shared"),
        "{status:?}"
    );
    assert!(!format!("{status:?}").contains("the last line"));
}

/// A control still waiting for its fresh lease when B unpairs A never
/// runs, though the account still lists A.
#[tokio::test]
async fn a_control_not_dispatched_yet_never_runs_after_an_unpairing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, None).await;
    cloud.set_lease_delay(Duration::from_secs(2));
    let mut pending = a.link.control("mB", ask(ControlAction::Start, None));
    let sent = pending.recv().await;
    assert!(matches!(sent, Some(ControlEvent::Sent { .. })), "{sent:?}");
    // B has it, and waits for its lease.
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert!(b.link.unpair("mA").unwrap());
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert_eq!(handed(&b), 0, "nothing reached the app");
    assert!(b.source.host().state().detached.is_empty());
}

/// A viewer's controls and status queries are bounded on the target before
/// any work starts for them: past the bound, `busy` at once.
#[tokio::test]
async fn controls_past_the_bound_are_busy_before_any_work_starts() {
    let cloud = FakeCloud::start().await;
    let (a, _b) = controlled(&cloud, None).await;
    cloud.set_lease_delay(Duration::from_secs(5));
    let mut waiting = Vec::new();
    for _ in 0..4 {
        let mut pending = a.link.control("mB", ask(ControlAction::Start, None));
        let sent = pending.recv().await;
        assert!(matches!(sent, Some(ControlEvent::Sent { .. })), "{sent:?}");
        waiting.push(pending);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let asked = Instant::now();
    let fifth = control(&a, ask(ControlAction::Start, None)).await;
    let status = a.link.control_status("mB", &request_id(9)).await;

    assert_eq!(
        ended(&fifth),
        (ControlOutcome::Refused, Some("busy".into()))
    );
    assert!(
        matches!(&status, ControlEvent::Result { code: Some(code), .. } if code == "busy"),
        "{status:?}"
    );
    assert!(asked.elapsed() < Duration::from_secs(2), "refused at once");
}

/// `unknown` is never the end: while B still works on a stop whose answer
/// was lost, A keeps asking `control.status`, never sending the control
/// again, and then fetches B's fresh state.
#[tokio::test]
async fn an_unknown_outcome_is_asked_again_until_it_comes() {
    let cloud = FakeCloud::start().await;
    let (a, b) = controlled(&cloud, Some(run(RunOrigin::App))).await;
    let (release, held) = std::sync::mpsc::channel();
    b.source.host().state().stop_hold = Some(held);
    let pending = a.link.control("mB", ask(ControlAction::Stop, Some(RUN)));
    wait_until("the stop is held", || {
        b.source.host().state().stop_hold.is_none()
    })
    .await;

    cloud.close("mA", 1012, "restart");
    // B says it is still working on it, each time A asks meanwhile.
    tokio::time::sleep(Duration::from_secs(5)).await;
    release.send(()).unwrap();
    let seen = events(pending).await;

    assert!(
        seen.iter()
            .any(|event| matches!(event, ControlEvent::Unknown { .. })),
        "{seen:?}"
    );
    assert_eq!(ended(&seen), (ControlOutcome::Done, None));
    assert_eq!(stops(&b).len(), 1, "never sent again");
    wait_until("A fetched B's fresh state", || {
        a.link
            .shared
            .view()
            .inventories
            .get("mB")
            .is_some_and(|inventory| inventory.items[0]["process"] == "stopped")
    })
    .await;
}

// --- The shell environment of background starts ------------------------------

/// Background starts read the user's shell environment once. When that
/// failed, the app's `Reload shell environment` has the daemon read it
/// again, and `remote_status` no longer says it failed.
#[tokio::test]
async fn reloading_the_shell_environment_recovers_from_a_failure() {
    use std::os::unix::fs::PermissionsExt;

    let cloud = FakeCloud::start().await;
    let b = super::daemon(&cloud, "mB", "teo-mini").await;
    let dir = tempfile::tempdir().unwrap();
    let printer = dir.path().join("localify");
    let shell = b
        .source
        .set_shell(ShellEnvironment::new(printer.clone(), "/bin/sh".into()));
    // Nothing prints the environment yet: the first read fails.
    let first = Arc::clone(&shell);
    tokio::task::spawn_blocking(move || first.environment())
        .await
        .unwrap();
    assert!(b.link.status().shell_environment_error.is_some());

    std::fs::write(
        &printer,
        "#!/bin/sh\nprintf '%s\\0PATH=/usr/bin:/bin\\0LOCALIFY_TEST_MARK=read\\0%s' \"$2\" \"$2\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&printer, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(b.link.reload_shell_environment().await, None);
    assert_eq!(b.link.status().shell_environment_error, None);
    let environment = tokio::task::spawn_blocking(move || shell.environment())
        .await
        .unwrap();
    assert!(
        environment
            .iter()
            .any(|(key, value)| key == "LOCALIFY_TEST_MARK" && value == "read"),
        "{environment:?}"
    );
}

/// Copies a daemon's folder, as it is left on disk.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}
