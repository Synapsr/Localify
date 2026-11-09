//! Session protocol 2 and access requests end to end, between daemons in
//! this process and the fake relay: the "Versions" and "Access requests"
//! gates of PR 4 in `docs/remote-machines.md`, as far as the daemon goes
//! (the dialog itself is the app's, PR 4d).

use std::time::Duration;

use tokio::sync::broadcast;

use super::viewing::serve_management;
use super::{Daemon, daemon_with, fast, next_event, pair, wait_until};
use crate::daemon::remote::fake_relay::FakeCloud;
use crate::daemon::remote::machines::SessionState;
use crate::daemon::remote::{AccessEvent, Asking, Command, Event, Subscription, Timings};
use crate::infrastructure::cloud::credentials::sign_in_lock;
use crate::infrastructure::remote::store::SettingsChange;
use crate::infrastructure::remote::wire::{AccessOutcome, AccessWants, ProtoRange, SessionMessage};

/// No per-peer limit: each test that needs one says so.
fn unlimited() -> Timings {
    Timings {
        access_every: Duration::ZERO,
        access_after_refusal: Duration::ZERO,
        ..fast()
    }
}

async fn settings(daemon: &Daemon, change: SettingsChange) {
    daemon.link.change_settings(change).await.unwrap();
}

fn share() -> SettingsChange {
    SettingsChange {
        share: Some(true),
        ..SettingsChange::default()
    }
}

fn control(on: bool) -> SettingsChange {
    SettingsChange {
        control: Some(on),
        ..SettingsChange::default()
    }
}

/// A asks B, paired, with B sharing; both with `timings`.
async fn asker_and_target(cloud: &FakeCloud, timings: Timings) -> (Daemon, Daemon) {
    let a = daemon_with(cloud, "mA", "laptop", timings).await;
    let b = daemon_with(cloud, "mB", "teo-mini", timings).await;
    wait_until("both see each other", || a.sees("mB") && b.sees("mA")).await;
    pair(&a, &b).await;
    settings(&b, share()).await;
    (a, b)
}

/// The app on `daemon`, showing the access dialog.
fn app(daemon: &Daemon) -> Subscription {
    daemon.link.subscribe(true)
}

async fn ask(a: &Daemon, wants: AccessWants) -> Asking {
    let mut asking = a.link.access("mB", wants).await.unwrap();
    assert_eq!(asking.next().await, Some(AccessEvent::Waiting));
    asking
}

async fn answer(asking: &mut Asking) -> AccessEvent {
    tokio::time::timeout(Duration::from_secs(6), asking.next())
        .await
        .expect("an answer")
        .expect("an end")
}

async fn asks(a: &Daemon, wants: AccessWants) -> AccessEvent {
    answer(&mut ask(a, wants).await).await
}

fn result(outcome: AccessOutcome) -> AccessEvent {
    AccessEvent::Result { outcome }
}

fn error_code(event: &AccessEvent) -> &str {
    match event {
        AccessEvent::Error { code, .. } => code,
        other => panic!("not an error: {other:?}"),
    }
}

/// The dialog's request, as the app hears it.
async fn dialog(events: &mut broadcast::Receiver<Event>) -> Event {
    next_event(events, |event| matches!(event, Event::AccessRequest { .. })).await
}

fn request_id(event: &Event) -> String {
    match event {
        Event::AccessRequest { request_id, .. } => request_id.clone(),
        other => panic!("not an access request: {other:?}"),
    }
}

async fn closed(events: &mut broadcast::Receiver<Event>, id: &str) {
    loop {
        let event = next_event(events, |event| matches!(event, Event::AccessClosed { .. })).await;
        if matches!(&event, Event::AccessClosed { request_id } if request_id == id) {
            return;
        }
    }
}

/// Whether an access request reaches the app within a short while.
async fn shows_nothing(events: &mut broadcast::Receiver<Event>) -> bool {
    let shown = tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            match events.recv().await {
                Ok(Event::AccessRequest { .. }) => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
            }
        }
    })
    .await;
    shown.is_err()
}

fn proto(daemon: &Daemon, peer: &str) -> Option<u32> {
    daemon.link.shared.view().protos.get(peer).copied()
}

fn switches(daemon: &Daemon) -> (bool, bool, bool) {
    let settings = daemon.store().settings().unwrap();
    (settings.share, settings.control, settings.background_starts)
}

// --- Versions ----------------------------------------------------------------

/// A relay that alters `presence` to claim version 1 for both changes
/// nothing: the `hello`s decide, and access requests go through.
#[tokio::test]
async fn presence_claiming_version_1_does_not_downgrade_two_version_2_peers() {
    let cloud = FakeCloud::start().await;
    cloud.claim_proto(1, 1);
    let (a, b) = asker_and_target(&cloud, fast()).await;
    let presence = a.link.shared.view().presence["mB"].proto;
    assert_eq!(presence, ProtoRange { min: 1, max: 1 });

    a.link.open("mB", true).await.unwrap();
    assert_eq!(proto(&a, "mB"), Some(2));
    let listed = a.link.machines(false).await.unwrap();
    let listed = listed.iter().find(|machine| machine.id == "mB").unwrap();
    assert_eq!(listed.proto, Some(2));

    let mut events = app(&b);
    let _asking = ask(&a, AccessWants::Control).await;
    let Event::AccessRequest { machine_id, .. } = dialog(&mut events.events).await else {
        unreachable!()
    };
    assert_eq!(machine_id, "mA");
}

/// A target of version 1: the session agrees on 1, the viewer answers
/// `unsupported` itself, and sends no protocol 2 message (one would end the
/// session there as an unknown type).
#[tokio::test]
async fn a_version_1_target_agrees_on_1_and_gets_no_protocol_2_message() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, fast()).await;
    b.link.speak(ProtoRange { min: 1, max: 1 });
    let mut events = app(&b);

    a.link.open("mB", true).await.unwrap();
    assert_eq!(proto(&a, "mB"), Some(1));
    let event = asks(&a, AccessWants::Control).await;
    assert_eq!(error_code(&event), "unsupported", "{event:?}");

    assert!(shows_nothing(&mut events.events).await);
    assert!(
        matches!(a.session("mB"), Some(SessionState::Open { .. })),
        "the session went on"
    );
    assert_eq!(switches(&b), (true, false, false));
}

/// A viewer of version 1, or one that breaks the rule: a protocol 2 message
/// in a version 1 session is an unknown type, and ends the session.
#[tokio::test]
async fn a_protocol_2_message_in_a_version_1_session_is_an_unknown_type() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, fast()).await;
    a.link.speak(ProtoRange { min: 1, max: 1 });
    let mut events = app(&b);

    a.link.open("mB", true).await.unwrap();
    assert_eq!(proto(&a, "mB"), Some(1));
    let command = Command::Raw {
        peer: "mB".into(),
        message: SessionMessage::AccessRequest {
            id: "r1".into(),
            wants: AccessWants::Control,
        },
    };
    a.link
        .shared
        .conn()
        .unwrap()
        .commands
        .send(command)
        .unwrap();

    wait_until("the target ends the session", || a.session("mB").is_none()).await;
    assert!(shows_nothing(&mut events.events).await);
    assert_eq!(proto(&a, "mB"), None, "no session, no version");
}

/// Ranges that do not overlap end the session with `unsupported`.
#[tokio::test]
async fn hello_ranges_that_do_not_overlap_end_the_session() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, fast()).await;
    b.link.speak(ProtoRange { min: 3, max: 3 });

    let failure = a.link.open("mB", true).await.unwrap_err();
    assert_eq!(failure.code, "unsupported", "{failure:?}");
}

// --- Access requests ---------------------------------------------------------

/// `Allow` turns on exactly what was asked, never `share`; the dialog names
/// the asker from the local pin.
#[tokio::test]
async fn allow_turns_on_exactly_the_switches_asked_for() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, unlimited()).await;
    let mut events = app(&b);

    let mut asking = ask(&a, AccessWants::Control).await;
    let shown = dialog(&mut events.events).await;
    let Event::AccessRequest {
        name,
        fingerprint,
        wants,
        ..
    } = &shown
    else {
        unreachable!()
    };
    assert_eq!(name, &b.store().pins().unwrap()["mA"].name);
    assert_eq!(fingerprint.len(), 19, "{fingerprint}");
    assert_eq!(*wants, AccessWants::Control);
    let id = request_id(&shown);
    let outcome = b.link.answer_access(&id, true, false).await.unwrap();

    assert_eq!(outcome, AccessOutcome::Granted);
    assert_eq!(answer(&mut asking).await, result(AccessOutcome::Granted));
    closed(&mut events.events, &id).await;
    assert_eq!(switches(&b), (true, true, false));

    // Background starts ask for control too, and get both.
    settings(&b, control(false)).await;
    let mut asking = ask(&a, AccessWants::BackgroundStarts).await;
    let id = request_id(&dialog(&mut events.events).await);
    b.link.answer_access(&id, true, false).await.unwrap();
    assert_eq!(answer(&mut asking).await, result(AccessOutcome::Granted));
    assert_eq!(switches(&b), (true, true, true));

    // Already on: `granted` at once, with no dialog.
    assert_eq!(
        asks(&a, AccessWants::Control).await,
        result(AccessOutcome::Granted)
    );
    assert!(shows_nothing(&mut events.events).await);
}

/// Access never turns `share` on.
#[tokio::test]
async fn a_target_that_does_not_share_refuses_with_not_shared() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, fast()).await;
    settings(
        &b,
        SettingsChange {
            share: Some(false),
            ..SettingsChange::default()
        },
    )
    .await;
    let mut events = app(&b);

    let event = asks(&a, AccessWants::Control).await;
    assert_eq!(error_code(&event), "not_shared");
    assert!(shows_nothing(&mut events.events).await);
    assert_eq!(switches(&b), (false, false, false));
}

/// `Don't allow` and 2 minutes without an answer change nothing; a late
/// `Allow` after the expiry neither.
#[tokio::test]
async fn dont_allow_and_no_answer_change_nothing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, unlimited()).await;
    let mut events = app(&b);

    let mut asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    let outcome = b.link.answer_access(&id, false, false).await.unwrap();
    assert_eq!(outcome, AccessOutcome::Declined);
    assert_eq!(answer(&mut asking).await, result(AccessOutcome::Declined));
    // A late Allow of the same dialog changes nothing.
    let late = b.link.answer_access(&id, true, false).await.unwrap();
    assert_eq!(late, AccessOutcome::Declined);
    assert_eq!(switches(&b), (true, false, false));

    let mut asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    assert_eq!(answer(&mut asking).await, result(AccessOutcome::Expired));
    closed(&mut events.events, &id).await;
    let late = b.link.answer_access(&id, true, false).await.unwrap();
    assert_eq!(late, AccessOutcome::Expired);
    assert_eq!(switches(&b), (true, false, false));
}

/// With no app that shows the dialog (none, or one from before it), the
/// answer is `nobody` at once.
#[tokio::test]
async fn with_the_app_absent_the_answer_is_nobody() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, fast()).await;
    let mut pairing_only = b.link.subscribe(false);

    assert_eq!(
        asks(&a, AccessWants::Control).await,
        result(AccessOutcome::Nobody)
    );
    assert!(shows_nothing(&mut pairing_only.events).await);
    assert_eq!(switches(&b), (true, false, false));
}

/// A late `Allow` changes nothing after the viewer cancelled, after the
/// switch it needs was turned off meanwhile, or after a write that did not
/// go through the daemon; a late answer with "Don't ask again", allowing
/// or not, neither suppresses the asker nor counts as a refusal.
#[tokio::test]
async fn a_late_allow_after_a_cancel_or_a_settings_change_changes_nothing() {
    let cloud = FakeCloud::start().await;
    let timings = Timings {
        access_after_refusal: Duration::from_secs(60),
        ..unlimited()
    };
    let (a, b) = asker_and_target(&cloud, timings).await;
    let mut events = app(&b);

    // Cancelled: the viewer stopped waiting.
    let asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    drop(asking);
    closed(&mut events.events, &id).await;
    let late = b.link.answer_access(&id, true, false).await.unwrap();
    assert_eq!(late, AccessOutcome::Cancelled);
    assert_eq!(switches(&b), (true, false, false));

    // `control` turned off while asking for background starts.
    settings(&b, control(true)).await;
    let mut asking = ask(&a, AccessWants::BackgroundStarts).await;
    let id = request_id(&dialog(&mut events.events).await);
    settings(&b, control(false)).await;
    assert_eq!(answer(&mut asking).await, result(AccessOutcome::Cancelled));
    closed(&mut events.events, &id).await;
    let late = b.link.answer_access(&id, true, false).await.unwrap();
    assert_eq!(late, AccessOutcome::Cancelled);
    assert_eq!(switches(&b), (true, false, false));

    // A write the daemon was not told about: the generation decides, for
    // an `Allow` with "Don't ask again", then for a `Don't allow` with it.
    for allow in [true, false] {
        let mut asking = ask(&a, AccessWants::Control).await;
        let id = request_id(&dialog(&mut events.events).await);
        b.store()
            .update_settings("mB", SettingsChange::default())
            .unwrap();
        let late = b.link.answer_access(&id, allow, true).await.unwrap();
        assert_eq!(late, AccessOutcome::Cancelled, "allow: {allow}");
        assert_eq!(answer(&mut asking).await, result(AccessOutcome::Cancelled));
        assert!(b.link.suppressed().unwrap().is_empty(), "allow: {allow}");
        assert_eq!(switches(&b), (true, false, false));
    }
    // Nor did the stale refusal count: A asks again at once (the fifth
    // dialog this hour, the last one allowed).
    let _asking = ask(&a, AccessWants::Control).await;
    dialog(&mut events.events).await;
}

/// Answers a dialog on another task, held before the settings lock as
/// another writer holding it would hold it; returns once it waits there.
async fn answer_held(
    b: &Daemon,
    id: String,
) -> (
    tokio::task::JoinHandle<anyhow::Result<AccessOutcome>>,
    std::sync::mpsc::Sender<()>,
) {
    let (reached, release) = b.link.pause_next_answer();
    let link = b.link.clone();
    let answering = tokio::spawn(async move { link.answer_access(&id, true, false).await });
    tokio::time::timeout(Duration::from_secs(6), reached)
        .await
        .expect("the answer reaches the lock")
        .unwrap();
    (answering, release)
}

/// An unpair that lands while an `Allow` waits for the settings lock, its
/// session's end still queued, wins: no switch turns on. The unpair's write
/// happens under the lock the answer is blocked on, after the answer did
/// everything it does before that lock, so an `Allow` that read the pin
/// before taking the lock would grant here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unpair_while_an_allow_waits_for_the_lock_wins() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, unlimited()).await;
    let mut events = app(&b);
    let _asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);

    let (answering, release) = answer_held(&b, id).await;
    // Another writer takes the sign-in lock first.
    let (held, holding) = tokio::sync::oneshot::channel();
    let (unpair, unpairing) = std::sync::mpsc::channel::<()>();
    let dir = b.dir.path().to_path_buf();
    let writer = std::thread::spawn(move || {
        let lock = sign_in_lock(&dir).unwrap();
        held.send(()).unwrap();
        // Once the answer waits on the lock, unpair A under it, as
        // `RemoteLink::unpair` writes the pins.
        unpairing.recv().unwrap();
        let peers = dir.join("remote").join("peers.json");
        let mut pins: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&peers).unwrap()).unwrap();
        assert!(pins.remove("mA").is_some());
        std::fs::write(&peers, serde_json::to_vec_pretty(&pins).unwrap()).unwrap();
        drop(lock);
    });
    holding.await.unwrap();
    release.send(()).unwrap();
    // Time for the answer to do whatever it does before the lock, and block.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!answering.is_finished(), "the answer waits for the lock");
    unpair.send(()).unwrap();

    let outcome = answering.await.unwrap().unwrap();
    writer.join().unwrap();
    assert_eq!(outcome, AccessOutcome::Cancelled);
    assert_eq!(switches(&b), (true, false, false));
}

/// An `Allow` that gets the settings lock only after the request expired,
/// because another writer held it, changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_allow_that_gets_the_lock_after_the_expiry_changes_nothing() {
    let cloud = FakeCloud::start().await;
    let timings = unlimited();
    let (a, b) = asker_and_target(&cloud, timings).await;
    let mut events = app(&b);
    let _asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    // The request was recorded before its dialog reached the app.
    let expired = std::time::Instant::now() + timings.access_expiry;

    let (answering, release) = answer_held(&b, id).await;
    let (held, holding) = tokio::sync::oneshot::channel();
    let dir = b.dir.path().to_path_buf();
    let writer = std::thread::spawn(move || {
        let lock = sign_in_lock(&dir).unwrap();
        held.send(()).unwrap();
        std::thread::sleep(
            expired.saturating_duration_since(std::time::Instant::now())
                + Duration::from_millis(100),
        );
        drop(lock);
    });
    holding.await.unwrap();
    release.send(()).unwrap();

    let outcome = answering.await.unwrap().unwrap();
    assert_eq!(outcome, AccessOutcome::Expired);
    writer.join().unwrap();
    assert_eq!(switches(&b), (true, false, false));
}

/// A peer asks once every 10 minutes, and once an hour after a refusal:
/// sooner gets `too_many` with `retryAfter`, and shows nothing.
#[tokio::test]
async fn a_peer_asking_too_soon_shows_nothing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, fast()).await;
    let mut events = app(&b);

    let asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    drop(asking);
    closed(&mut events.events, &id).await;

    let event = asks(&a, AccessWants::Control).await;
    assert_eq!(error_code(&event), "too_many");
    let AccessEvent::Error { retry_after, .. } = event else {
        unreachable!()
    };
    assert!(retry_after.is_some_and(|seconds| (1..=60).contains(&seconds)));
    assert!(shows_nothing(&mut events.events).await);

    // Across sessions and relay connections: the link keeps the count.
    let before = cloud.connections();
    cloud.close("mB", 1012, "restart");
    wait_until("B reconnects", || {
        cloud.connections() > before
            && b.link.status().relay == super::RelayState::Connected
            && a.sees("mB")
    })
    .await;
    let event = asks(&a, AccessWants::Control).await;
    assert_eq!(error_code(&event), "too_many");
    assert!(shows_nothing(&mut events.events).await);
}

/// A refusal holds a peer back longer than an ask does.
#[tokio::test]
async fn a_refused_peer_waits_longer() {
    let cloud = FakeCloud::start().await;
    let timings = Timings {
        access_every: Duration::ZERO,
        access_after_refusal: Duration::from_secs(60),
        ..fast()
    };
    let (a, b) = asker_and_target(&cloud, timings).await;
    let mut events = app(&b);

    let mut asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    b.link.answer_access(&id, false, false).await.unwrap();
    assert_eq!(answer(&mut asking).await, result(AccessOutcome::Declined));

    let event = asks(&a, AccessWants::Control).await;
    assert_eq!(error_code(&event), "too_many");
    assert!(shows_nothing(&mut events.events).await);
}

/// A sixth dialog in an hour is refused and shows nothing, whoever asks.
#[tokio::test]
async fn a_target_shows_at_most_five_dialogs_an_hour() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, unlimited()).await;
    let mut events = app(&b);

    for _ in 0..5 {
        let asking = ask(&a, AccessWants::Control).await;
        let id = request_id(&dialog(&mut events.events).await);
        drop(asking);
        closed(&mut events.events, &id).await;
    }
    let event = asks(&a, AccessWants::Control).await;
    assert_eq!(error_code(&event), "too_many");
    assert!(shows_nothing(&mut events.events).await);
}

/// "Don't ask again from A": its requests get `declined` at once and show
/// nothing, until a person here lets it ask again.
#[tokio::test]
async fn a_suppressed_peer_is_declined_and_shows_nothing_until_restored() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, unlimited()).await;
    let mut events = app(&b);

    let mut asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    b.link.answer_access(&id, false, true).await.unwrap();
    assert_eq!(answer(&mut asking).await, result(AccessOutcome::Declined));
    let suppressed = b.link.suppressed().unwrap();
    assert_eq!(suppressed.len(), 1);
    assert_eq!(suppressed[0].machine_id, "mA");
    assert_eq!(suppressed[0].name, b.store().pins().unwrap()["mA"].name);

    assert_eq!(
        asks(&a, AccessWants::Control).await,
        result(AccessOutcome::Declined)
    );
    assert!(shows_nothing(&mut events.events).await);
    assert_eq!(switches(&b), (true, false, false));

    assert!(b.link.restore_access("mA").unwrap());
    assert!(b.link.suppressed().unwrap().is_empty());
    let _asking = ask(&a, AccessWants::Control).await;
    dialog(&mut events.events).await;
}

/// One request waits per target: a second viewer's gets `busy`.
#[tokio::test]
async fn a_second_viewer_while_one_waits_gets_busy() {
    let cloud = FakeCloud::start().await;
    // All three before pairing, so each lease covers the others.
    let a = daemon_with(&cloud, "mA", "laptop", unlimited()).await;
    let b = daemon_with(&cloud, "mB", "teo-mini", unlimited()).await;
    let c = daemon_with(&cloud, "mC", "desk", unlimited()).await;
    wait_until("all see each other", || {
        a.sees("mB") && c.sees("mB") && b.sees("mA") && b.sees("mC")
    })
    .await;
    pair(&a, &b).await;
    pair(&c, &b).await;
    settings(&b, share()).await;
    let mut events = app(&b);

    let _waiting = ask(&a, AccessWants::Control).await;
    dialog(&mut events.events).await;
    let event = asks(&c, AccessWants::Control).await;
    assert_eq!(error_code(&event), "busy");
    assert!(shows_nothing(&mut events.events).await);
}

/// A session that ends closes the dialog, and the viewer stops waiting.
#[tokio::test]
async fn a_session_that_ends_closes_the_dialog() {
    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, unlimited()).await;
    let mut events = app(&b);

    let mut asking = ask(&a, AccessWants::Control).await;
    let id = request_id(&dialog(&mut events.events).await);
    cloud.close("mA", 1012, "restart");

    closed(&mut events.events, &id).await;
    assert!(matches!(
        answer(&mut asking).await,
        AccessEvent::Lost { .. }
    ));
    let late = b.link.answer_access(&id, true, false).await.unwrap();
    assert_eq!(late, AccessOutcome::Cancelled);
    assert_eq!(switches(&b), (true, false, false));
}

/// Through the management socket, as the apps will: the target's app
/// subscribes with `access`, the viewer streams `remote_access`, and the
/// answer comes back on both.
#[tokio::test]
async fn the_management_socket_carries_a_request_and_its_answer() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
    use tokio::net::UnixStream;
    use tokio::net::unix::OwnedReadHalf;

    async fn send(socket: &std::path::Path, command: serde_json::Value) -> UnixStream {
        let mut stream = UnixStream::connect(socket).await.unwrap();
        stream
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        stream
    }
    async fn next(lines: &mut Lines<BufReader<OwnedReadHalf>>) -> serde_json::Value {
        let line = tokio::time::timeout(Duration::from_secs(6), lines.next_line())
            .await
            .expect("a line")
            .unwrap()
            .expect("the stream stayed open");
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["data"].take()
    }

    let cloud = FakeCloud::start().await;
    let (a, b) = asker_and_target(&cloud, unlimited()).await;
    let (on_a, on_b) = (serve_management(&a).await, serve_management(&b).await);

    let (events, _events_out) = send(
        &on_b,
        serde_json::json!({"cmd": "remote_events", "access": true}),
    )
    .await
    .into_split();
    let mut events = BufReader::new(events).lines();
    assert_eq!(next(&mut events).await["event"], "subscribed");
    let asking = send(
        &on_a,
        serde_json::json!({"cmd": "remote_access", "machineId": "mB", "wants": "control"}),
    )
    .await;
    let (asking_in, asking_out) = asking.into_split();
    let mut asking_in = BufReader::new(asking_in).lines();
    assert_eq!(next(&mut asking_in).await["event"], "waiting");
    let shown = loop {
        let event = next(&mut events).await;
        if event["event"] == "access_request" {
            break event;
        }
    };
    assert_eq!(shown["machineId"], "mA");
    assert_eq!(shown["wants"], "control");
    assert!(shown["expiresAt"].is_string());

    // Closing the viewer's stream cancels it, and closes the dialog.
    drop(asking_out);
    drop(asking_in);
    let ended = loop {
        let event = next(&mut events).await;
        if event["event"] == "access_closed" {
            break event;
        }
    };
    assert_eq!(ended["requestId"], shown["requestId"]);

    let answered = send(
        &on_b,
        serde_json::json!({"cmd": "remote_access_answer", "requestId": shown["requestId"],
                           "allow": true, "suppress": false}),
    )
    .await;
    let (answered, _out) = answered.into_split();
    let answer = next(&mut BufReader::new(answered).lines()).await;
    assert_eq!(answer["outcome"], "cancelled");
    assert_eq!(switches(&b), (true, false, false));

    let suppressed = send(
        &on_b,
        serde_json::json!({"cmd": "remote_access_suppressed"}),
    )
    .await;
    let (suppressed, _out) = suppressed.into_split();
    let list = next(&mut BufReader::new(suppressed).lines()).await;
    assert_eq!(list, serde_json::json!([]));
}
