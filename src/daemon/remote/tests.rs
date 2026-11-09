//! The link end to end, between daemons in this process and the fake relay:
//! the Rust side of the PR 2 gates in `docs/remote-machines.md`, "Gates".
//! Viewing, and the PR 3 gate, are in `viewing`.

mod access;
mod viewing;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::fake_relay::{FakeCloud, FakeSource, Route, is_message, opens};
use super::local::LocalSource;
use super::machines::{MachineState, RelayState, SessionState};
use super::{Event, PairEvent, RemoteLink, Timings, spawn};
use crate::infrastructure::cloud::credentials::{CloudCredentials, CredentialsStore};
use crate::infrastructure::remote::noise::{SessionId, StaticKeys, encode_key};
use crate::infrastructure::remote::store::RemoteStore;
use crate::infrastructure::remote::wire::{FrameKind, Purpose};

fn fast() -> Timings {
    let ms = Duration::from_millis;
    Timings {
        handshake_expiry: ms(1500),
        keep_every: ms(300),
        not_responding_after: ms(400),
        viewer_gives_up_after: ms(2500),
        target_gives_up_after: ms(4000),
        pairing_timeout: ms(2500),
        idle_after: ms(30_000),
        lifetime: ms(3_600_000),
        tick: ms(40),
        ping_every: ms(1000),
        relay_silence: ms(10_000),
        backoff_unit: ms(50),
        backoff_cap: ms(1000),
        deploy_jitter: ms(1000),
        wake_jitter: ms(100),
        stable_after: ms(60_000),
        lease_renew: ms(800),
        lease_renew_jitter: ms(100),
        lease_retry: ms(200),
        lapsed_first: ms(200),
        lapsed_spread: ms(100),
        lapsed_recheck: ms(500),
        clock_check: ms(100),
        retry_after_drop: ms(300),
        watch_every: ms(100),
        watch_expiry: ms(30_000),
        follow_poll: ms(50),
        // Long: the tests change the switches through the link, which must
        // act on them by itself.
        settings_recheck: ms(60_000),
        access_expiry: ms(1500),
        access_every: ms(60_000),
        access_after_refusal: ms(60_000),
    }
}

/// A link in `dir`, sharing what `source` holds.
fn link_with(dir: &std::path::Path, source: &Arc<FakeSource>) -> RemoteLink {
    let source: Arc<dyn LocalSource> = source.clone();
    RemoteLink::with_timings(dir, source, fast())
}

fn link(dir: &std::path::Path) -> RemoteLink {
    link_with(dir, &Arc::new(FakeSource::default()))
}

struct Daemon {
    dir: TempDir,
    link: RemoteLink,
    cancel: CancellationToken,
    id: String,
    source: Arc<FakeSource>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Daemon {
    fn store(&self) -> RemoteStore {
        RemoteStore::in_data_dir(self.dir.path())
    }

    fn pinned(&self, peer: &str) -> bool {
        self.store().pins().unwrap().contains_key(peer)
    }

    fn sees(&self, peer: &str) -> bool {
        self.link.shared.view().presence.contains_key(peer)
    }

    fn session(&self, peer: &str) -> Option<SessionState> {
        self.link.shared.view().sessions.get(peer).copied()
    }
}

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..300 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting until {what}");
}

async fn daemon(cloud: &FakeCloud, id: &str, name: &str) -> Daemon {
    daemon_with(cloud, id, name, fast()).await
}

async fn daemon_with(cloud: &FakeCloud, id: &str, name: &str, timings: Timings) -> Daemon {
    let token = cloud.add_machine(id, name);
    let daemon = launch_with(
        cloud,
        id,
        name,
        &token,
        tempfile::tempdir().unwrap(),
        timings,
    );
    connected(daemon).await
}

async fn connected(daemon: Daemon) -> Daemon {
    wait_until("the link connects", || {
        daemon.link.status().relay == RelayState::Connected
    })
    .await;
    daemon
}

fn launch(cloud: &FakeCloud, id: &str, name: &str, token: &str, dir: TempDir) -> Daemon {
    launch_with(cloud, id, name, token, dir, fast())
}

fn launch_with(
    cloud: &FakeCloud,
    id: &str,
    name: &str,
    token: &str,
    dir: TempDir,
    timings: Timings,
) -> Daemon {
    CredentialsStore::in_data_dir(dir.path())
        .save(&CloudCredentials {
            api_base: cloud.base.clone(),
            token: token.into(),
            login: "teo".into(),
            machine_id: id.into(),
            machine_name: name.into(),
        })
        .unwrap();
    let source = Arc::new(FakeSource::default());
    let shared: Arc<dyn LocalSource> = source.clone();
    let link = RemoteLink::with_timings(dir.path(), shared, timings);
    // As `LOCALIFY_CLOUD_API` makes a `wrangler dev` trusted.
    link.trust_api_base(&cloud.base);
    let cancel = CancellationToken::new();
    spawn(link.clone(), cancel.clone());
    Daemon {
        dir,
        link,
        cancel,
        id: id.into(),
        source,
    }
}

async fn two(cloud: &FakeCloud) -> (Daemon, Daemon) {
    let a = daemon(cloud, "mA", "laptop").await;
    let b = daemon(cloud, "mB", "teo-mini").await;
    wait_until("both see each other", || a.sees("mB") && b.sees("mA")).await;
    (a, b)
}

async fn next_pair_event(events: &mut mpsc::Receiver<PairEvent>) -> PairEvent {
    tokio::time::timeout(Duration::from_secs(6), events.recv())
        .await
        .expect("a pairing event")
        .expect("the pairing stream stayed open")
}

async fn next_event(events: &mut broadcast::Receiver<Event>, wanted: fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("an event")
            .expect("the event stream stayed open");
        if wanted(&event) {
            return event;
        }
    }
}

/// Runs a pairing to both codes; returns them with both pairing ids.
async fn to_codes(
    a: &Daemon,
    b: &Daemon,
    events_b: &mut broadcast::Receiver<Event>,
) -> (mpsc::Receiver<PairEvent>, String, String, String, String) {
    let mut events = a.link.pair(&b.id).await.unwrap();
    let first = next_pair_event(&mut events).await;
    let PairEvent::Code {
        pairing_id: id_a,
        code: code_a,
        ..
    } = first
    else {
        panic!("no code: {first:?}");
    };
    let Event::PairRequest {
        pairing_id: id_b,
        code: code_b,
        ..
    } = next_event(events_b, |event| matches!(event, Event::PairRequest { .. })).await
    else {
        unreachable!()
    };
    (events, id_a, code_a, id_b, code_b)
}

async fn pair(a: &Daemon, b: &Daemon) {
    let mut subscription = b.link.subscribe(false);
    let (mut events, id_a, code_a, id_b, code_b) = to_codes(a, b, &mut subscription.events).await;
    assert_eq!(code_a, code_b);
    a.link.answer(&id_a, true).await.unwrap();
    b.link.answer(&id_b, true).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Paired { .. }
    ));
    wait_until("both pin each other", || a.pinned(&b.id) && b.pinned(&a.id)).await;
}

// --- Pacing against the relay's budgets --------------------------------------

/// relay.ts as a sink, on the test's paused clock: frames are handled in
/// order; every `check_every` the next one waits `check_takes` for D1, and
/// the frames that arrive meanwhile wait behind it.
struct RelayModel {
    budget: super::fake_relay::RelayBudget,
    check_every: Duration,
    check_takes: Duration,
    next_check: Instant,
    /// When each frame still in the relay is done, and its size.
    handling: std::collections::VecDeque<(Instant, usize)>,
    refusals: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    handled: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl futures_util::Sink<tokio_tungstenite::tungstenite::Message> for RelayModel {
    type Error = std::io::Error;
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        message: tokio_tungstenite::tungstenite::Message,
    ) -> Result<(), Self::Error> {
        use super::fake_relay::Admit;
        use std::sync::atomic::Ordering;
        let now = Instant::now();
        let size = message.len();
        self.handling.retain(|(done, _)| *done > now);
        // As relay.ts counts it: every frame not done yet, the one being
        // handled included.
        let waiting = self.handling.len();
        let waiting_bytes: usize = self.handling.iter().map(|(_, size)| size).sum();
        match self.budget.admit(size, waiting, waiting_bytes, now) {
            Admit::Forward => {
                let start = self
                    .handling
                    .back()
                    .map_or(now, |(done, _)| (*done).max(now));
                let mut done = start;
                if start >= self.next_check {
                    done += self.check_takes;
                    self.next_check = start + self.check_every;
                }
                self.handling.push_back((done, size));
                self.handled.fetch_add(1, Ordering::SeqCst);
            }
            Admit::Refuse | Admit::Close => {
                self.refusals.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(())
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// Sends `frames` of `size` bytes as fast as the pacer takes them, against
/// a relay whose checks take `check_takes`; returns the refusals.
async fn paced_against_the_relay(frames: usize, size: usize, check_takes: Duration) -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let refusals = std::sync::Arc::new(AtomicUsize::new(0));
    let handled = std::sync::Arc::new(AtomicUsize::new(0));
    let model = RelayModel {
        budget: super::fake_relay::RelayBudget::new(Instant::now()),
        check_every: Duration::from_secs(60),
        check_takes,
        next_check: Instant::now(),
        handling: std::collections::VecDeque::new(),
        refusals: std::sync::Arc::clone(&refusals),
        handled: std::sync::Arc::clone(&handled),
    };
    let (outbox, queue) = super::pacer::queue();
    let writer = tokio::spawn(super::pacer::run(
        model,
        queue,
        super::pacer::Bucket::paced(Instant::now()),
    ));
    let mut sent = 0;
    while sent < frames {
        if outbox.push("x".repeat(size)) {
            sent += 1;
        } else {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    drop(outbox);
    writer.await.unwrap();
    assert_eq!(
        handled.load(Ordering::SeqCst) + refusals.load(Ordering::SeqCst),
        frames
    );
    refusals.load(Ordering::SeqCst)
}

/// The pacer never trips relay.ts's budgets, nor its bound on frames
/// waiting behind a D1 check, for checks up to 600 ms (one a minute, as
/// `authorizedUntil` asks), with small frames and with 60 KiB ones.
#[tokio::test(start_paused = true)]
async fn the_pacer_stays_inside_the_relays_budgets_through_slow_checks() {
    for check in [
        Duration::ZERO,
        Duration::from_millis(300),
        Duration::from_millis(600),
    ] {
        assert_eq!(
            paced_against_the_relay(5000, 1024, check).await,
            0,
            "1 KiB, check {check:?}"
        );
        assert_eq!(
            paced_against_the_relay(300, 60 * 1024, check).await,
            0,
            "60 KiB, check {check:?}"
        );
    }
}

/// The model does refuse what the relay refuses: a check far slower than
/// the bound lets frames pile up past 32.
#[tokio::test(start_paused = true)]
async fn the_relay_model_refuses_frames_past_its_bounds() {
    assert!(paced_against_the_relay(500, 1024, Duration::from_secs(3)).await > 0);
}

/// A relay socket that stops taking frames, while the relay keeps sending:
/// the writer gives up after its timeout, and the connection ends, so the
/// link backs off and reconnects instead of waiting forever.
#[tokio::test(start_paused = true)]
async fn a_socket_that_stops_taking_frames_ends_the_connection() {
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::Role;

    let (client, server) = tokio::io::duplex(64);
    let relay = tokio::spawn(async move {
        let mut socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let welcome =
            json!({"t": "welcome", "v": 1, "connId": "c", "machineId": "mA", "peers": []});
        let _ = futures_util::SinkExt::send(&mut socket, Message::text(welcome.to_string())).await;
        // Sends, never reads: the daemon's frames pile up in the pipe.
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if futures_util::SinkExt::send(&mut socket, Message::text("pong"))
                .await
                .is_err()
            {
                return;
            }
        }
    });
    let stream: Box<dyn super::socket::Io> = Box::new(client);
    let socket = WebSocketStream::from_raw_socket(stream, Role::Client, None).await;
    let dir = tempfile::tempdir().unwrap();
    let link = link(dir.path());
    let http =
        crate::infrastructure::cloud::http::CloudHttp::new("http://127.0.0.1:9", None).unwrap();
    let cloud = std::sync::Arc::new(crate::infrastructure::cloud::api::CloudClient::new(
        http.clone(),
    ));
    let context = super::actor::Context {
        self_id: "mA".into(),
        name: "laptop".into(),
        source: Arc::new(FakeSource::default()),
        token: "lfy_t".into(),
        account: "u1".into(),
        keys: StaticKeys::generate().unwrap(),
        lease: std::sync::Arc::new(super::lease::LeaseKeeper::new(
            crate::infrastructure::cloud::api::CloudClient::new(http),
            fast(),
            super::Revocation { trusted: false },
        )),
        cloud,
    };
    let (commands, receiver) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let ending = tokio::time::timeout(
        Duration::from_secs(120),
        super::actor::run(
            std::sync::Arc::clone(&link.shared),
            context,
            socket,
            receiver,
            commands,
            &cancel,
        ),
    )
    .await
    .expect("the connection ends");
    assert_eq!(ending, super::backoff::Ending::Lost);
    // Pings every second fill the pipe within seconds; the write then times
    // out after 10 s. The relay never went silent.
    assert!(
        started.elapsed() <= Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    relay.abort();
}

// --- Pairing -----------------------------------------------------------------

#[tokio::test]
async fn two_machines_pair_once_both_people_confirm_the_same_code() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;

    let key_a = a
        .store()
        .load_identity()
        .unwrap()
        .unwrap()
        .public_key
        .clone();
    let key_b = b
        .store()
        .load_identity()
        .unwrap()
        .unwrap()
        .public_key
        .clone();
    assert_eq!(a.store().pins().unwrap()["mB"].public_key, key_b);
    assert_eq!(b.store().pins().unwrap()["mA"].public_key, key_a);
    assert_eq!(a.store().pins().unwrap()["mB"].name, "teo-mini");
    assert_eq!(b.store().pins().unwrap()["mA"].name, "laptop");

    // A session now opens, and B's switches are all off.
    let hello = a.link.open("mB", false).await.unwrap();
    assert!(!hello.settings.unwrap().share);
    let machines = a.link.machines(false).await.unwrap();
    let state = |id: &str| machines.iter().find(|m| m.id == id).unwrap().state;
    assert_eq!(state("mA"), MachineState::Connected);
    assert_eq!(state("mB"), MachineState::NotSharing);
}

#[tokio::test]
async fn a_pairing_with_nobody_to_confirm_on_the_target_is_declined() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let mut events = a.link.pair(&b.id).await.unwrap();
    let PairEvent::Cancelled { reason, message } = next_pair_event(&mut events).await else {
        panic!("not declined");
    };
    assert_eq!(reason, "nobody_to_confirm");
    assert!(message.contains("--accept"), "{message}");
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

#[tokio::test]
async fn a_cancel_on_either_side_pins_nothing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let mut subscription = b.link.subscribe(false);

    let (mut events, id_a, _, id_b, _) = to_codes(&a, &b, &mut subscription.events).await;
    a.link.answer(&id_a, true).await.unwrap();
    b.link.answer(&id_b, false).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));

    let (mut events, id_a, _, _, _) = to_codes(&a, &b, &mut subscription.events).await;
    a.link.answer(&id_a, false).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));
    next_event(&mut subscription.events, |event| {
        matches!(event, Event::PairCancelled { .. })
    })
    .await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

#[tokio::test]
async fn a_pairing_nobody_answers_times_out_and_pins_nothing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let mut subscription = b.link.subscribe(false);
    let (mut events, id_a, _, _, _) = to_codes(&a, &b, &mut subscription.events).await;
    // A accepts; B's person never answers.
    a.link.answer(&id_a, true).await.unwrap();
    let PairEvent::Cancelled { reason, .. } = next_pair_event(&mut events).await else {
        panic!("not cancelled");
    };
    assert!(reason == "timeout" || reason == "done", "{reason}");
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

#[tokio::test]
async fn pairing_messages_out_of_order_or_twice_abandon_the_pairing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let _subscription = b.link.subscribe(false);
    // The relay repeats B's commitment, its second message after hello.
    // The copy fails the transport counter before pairing ever sees it
    // (the pairing's own refusal of a second commitment is a unit test).
    let mut sent = 0;
    cloud.set_hook(Some(Box::new(move |from, frame| {
        if from == "mB" && is_message(frame) {
            sent += 1;
            if sent == 2 {
                return Route::Twice;
            }
        }
        Route::Forward
    })));
    let mut events = a.link.pair(&b.id).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));

    // The relay holds B's hello and sends it after the commitment: the
    // commitment comes first. Frames of the first attempt still on their
    // way are left alone.
    let earlier: std::collections::HashSet<String> = cloud
        .frames()
        .into_iter()
        .map(|(_, frame)| frame.sid)
        .collect();
    let mut held = false;
    cloud.set_hook(Some(Box::new(move |from, frame| {
        if from == "mB" && is_message(frame) && !held && !earlier.contains(&frame.sid) {
            held = true;
            Route::Hold
        } else {
            Route::Forward
        }
    })));
    let mut events = a.link.pair(&b.id).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

#[tokio::test]
async fn a_relay_that_alters_the_handshake_pins_nothing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let _subscription = b.link.subscribe(false);
    // A key substituted in B's handshake message.
    cloud.set_hook(Some(Box::new(|from, frame| {
        if from == "mB" && frame.kind == FrameKind::Hs {
            let data = frame.data.take().unwrap();
            let mut bytes =
                base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, data)
                    .unwrap();
            bytes[40] ^= 0xff;
            frame.data = Some(base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                bytes,
            ));
        }
        Route::Forward
    })));
    let mut events = a.link.pair(&b.id).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

#[tokio::test]
async fn a_target_takes_one_pairing_at_a_time_and_five_an_hour() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let mut subscription = b.link.subscribe(false);
    for _ in 0..5 {
        let (mut events, id_a, _, _, _) = to_codes(&a, &b, &mut subscription.events).await;
        a.link.answer(&id_a, false).await.unwrap();
        next_pair_event(&mut events).await;
    }
    let mut events = a.link.pair(&b.id).await.unwrap();
    let PairEvent::Cancelled { reason, .. } = next_pair_event(&mut events).await else {
        panic!("not busy");
    };
    assert_eq!(reason, "busy");
}

// --- Identity ----------------------------------------------------------------

#[tokio::test]
async fn an_unpaired_machine_cannot_open_a_session() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let failure = a.link.open(&b.id, false).await.unwrap_err();
    assert_eq!(failure.code, "not_paired");

    // Even when A thinks it is paired, B is not: the target refuses.
    let key_b = b
        .store()
        .load_identity()
        .unwrap()
        .unwrap()
        .public_key
        .clone();
    a.store()
        .update_pins("mA", |pins| {
            pins.insert(
                "mB".into(),
                crate::infrastructure::remote::store::Pin {
                    public_key: key_b,
                    name: "teo-mini".into(),
                    paired_at: "2026-11-08T10:12:00Z".into(),
                },
            );
        })
        .unwrap();
    let failure = a.link.open(&b.id, false).await.unwrap_err();
    assert_eq!(failure.code, "not_paired");
}

#[tokio::test]
async fn a_key_other_than_the_pin_never_opens_a_session() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;

    // The cloud's roster swaps B's key: A refuses before it dials.
    let other = encode_key(&StaticKeys::generate().unwrap().public);
    cloud.set_roster_key("mB", &other);
    // A probe reads the roster again, as `localify machines` does.
    a.link.machines(true).await.unwrap();
    let failure = a.link.open(&b.id, false).await.unwrap_err();
    assert_eq!(failure.code, "key_mismatch");
    let machines = a.link.machines(false).await.unwrap();
    assert_eq!(
        machines.iter().find(|m| m.id == "mB").unwrap().state,
        MachineState::KeyMismatch
    );

    // Roster and pin agree on a key B does not hold: only the handshake can
    // tell, and A stops before it sends its own key.
    a.store()
        .update_pins("mA", |pins| {
            if let Some(pin) = pins.get_mut("mB") {
                pin.public_key = other.clone();
            }
        })
        .unwrap();
    a.link.shared.view().failures.clear();
    let failure = a.link.open(&b.id, false).await.unwrap_err();
    assert_eq!(failure.code, "key_mismatch");
    let third_messages = cloud
        .frames()
        .iter()
        .rev()
        .take_while(|(from, frame)| !(from == "mA" && opens(frame, Purpose::Session)))
        .filter(|(from, frame)| from == "mA" && frame.kind == FrameKind::Hs)
        .count();
    assert_eq!(third_messages, 0, "A never sent its static key");
}

/// A relay in the middle of a session: it answers A's handshake as B, and
/// opens one to B as A, each with its own static key. A sees a key other
/// than its pin after the second message and stops before sending its own;
/// B completes its handshake with the relay, then refuses the relay's key.
#[tokio::test]
async fn a_relay_that_swaps_static_keys_is_refused_by_both_pins() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    let before = cloud.frames().len();
    cloud.set_mitm("mA", "mB");
    let failure = a.link.open("mB", false).await.unwrap_err();
    assert_eq!(failure.code, "key_mismatch");
    wait_until("B completes the relay's handshake", || {
        cloud.mitm_completed() == 1
    })
    .await;
    wait_until("B refuses the relay's key", || {
        cloud
            .frames()
            .iter()
            .any(|(from, frame)| from == "mB" && frame.kind == FrameKind::Close)
    })
    .await;
    assert_eq!(
        b.link.shared.view().failures.get("mA").map(String::as_str),
        Some("key_mismatch")
    );
    // A never sent its static key: its only handshake frame was the first.
    let from_a = cloud.frames()[before..]
        .iter()
        .filter(|(from, frame)| {
            from == "mA" && frame.kind == FrameKind::Hs && frame.purpose.is_none()
        })
        .count();
    assert_eq!(from_a, 0);
}

#[tokio::test]
async fn a_relay_that_rewrites_from_or_purpose_cannot_open_a_session() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let c = daemon(&cloud, "mC", "desk").await;
    wait_until("A sees C", || a.sees("mC")).await;
    pair(&a, &b).await;
    pair(&c, &b).await;

    // Frames from C delivered to B as if A sent them.
    cloud.set_hook(Some(Box::new(|from, _| {
        if from == "mC" {
            Route::From("mA".into())
        } else {
            Route::Forward
        }
    })));
    assert!(c.link.open("mB", false).await.is_err());

    // The purpose rewritten in the envelope.
    cloud.set_hook(Some(Box::new(|_, frame| {
        if opens(frame, Purpose::Session) {
            frame.purpose = Some(Purpose::Pair);
        }
        Route::Forward
    })));
    assert!(a.link.open("mB", false).await.is_err());

    cloud.set_hook(None);
    a.link.open("mB", false).await.unwrap();
}

#[tokio::test]
async fn a_same_name_sign_in_inherits_no_trust() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    drop(b);
    // B signs in again: a new machine id with the same name.
    let b2 = daemon(&cloud, "mB2", "teo-mini").await;
    wait_until("A sees the new B", || a.sees("mB2")).await;
    assert_eq!(
        a.link.open("mB2", false).await.unwrap_err().code,
        "not_paired"
    );
    assert!(!b2.pinned("mA"));
}

// --- Replay and dropped frames -----------------------------------------------

#[tokio::test]
async fn frames_of_an_earlier_session_do_not_decrypt_in_a_new_one() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", false).await.unwrap();
    let recorded: Vec<_> = cloud
        .frames()
        .into_iter()
        .filter(|(from, frame)| from == "mA" && is_message(frame))
        .collect();
    assert!(!recorded.is_empty());

    a.link.open("mB", true).await.unwrap();
    let sid = cloud
        .frames()
        .into_iter()
        .rev()
        .find(|(from, frame)| from == "mA" && opens(frame, Purpose::Session))
        .map(|(_, frame)| frame.sid)
        .unwrap();
    assert!(a.session("mB").is_some());
    let (_, old) = &recorded[0];
    cloud.inject(
        "mB",
        json!({"t": "recv", "from": "mA", "sid": sid, "kind": "msg", "data": old.data}),
    );
    wait_until("the session ends", || a.session("mB").is_none()).await;
}

#[tokio::test]
async fn a_repeated_or_reordered_message_ends_the_session() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;

    for route in ["twice", "hold"] {
        a.link.open("mB", true).await.unwrap();
        let mut done = false;
        cloud.set_hook(Some(Box::new(move |from, frame| {
            // The first keep: repeated, or held behind the next frame.
            if from == "mA" && is_message(frame) && !done {
                done = true;
                if route == "twice" {
                    Route::Twice
                } else {
                    Route::Hold
                }
            } else {
                Route::Forward
            }
        })));
        wait_until("the session ends", || a.session("mB").is_none()).await;
        cloud.set_hook(None);
    }
}

#[tokio::test]
async fn a_dropped_frame_ends_the_session_and_nothing_is_resent() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    let sessions_on_b = || b.link.shared.view().sessions.len();
    assert_eq!(sessions_on_b(), 0, "B is the target, not a viewer");

    let mut dropped = None;
    let record = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen = std::sync::Arc::clone(&record);
    cloud.set_hook(Some(Box::new(move |from, frame| {
        if from == "mA" && is_message(frame) && dropped.is_none() {
            dropped = frame.data.clone();
            *seen.lock().unwrap() = dropped.clone();
            Route::Drop("unavailable")
        } else {
            Route::Forward
        }
    })));
    wait_until("A ends the session", || a.session("mB").is_none()).await;
    cloud.set_hook(None);
    // The next session waits for the retry delay, then opens fresh.
    assert_eq!(
        a.link.open("mB", true).await.unwrap_err().code,
        "unavailable"
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    a.link.open("mB", true).await.unwrap();
    let dropped = record.lock().unwrap().clone().unwrap();
    let resent = cloud
        .frames()
        .iter()
        .filter(|(_, frame)| frame.data.as_deref() == Some(dropped.as_str()))
        .count();
    assert_eq!(resent, 1, "only the original carried it");
    // B ended its side too: the close reached it.
    let closes = cloud
        .frames()
        .iter()
        .filter(|(from, frame)| from == "mA" && frame.kind == FrameKind::Close)
        .count();
    assert!(closes >= 1);
}

// --- Handshakes and heartbeats -----------------------------------------------

#[tokio::test]
async fn an_incomplete_handshake_is_dropped_and_a_fifth_at_once_is_refused() {
    let cloud = FakeCloud::start().await;
    let (_a, _b) = two(&cloud).await;
    let first = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        [9_u8; 32],
    );
    let mut sids = Vec::new();
    for _ in 0..5 {
        let sid = SessionId::random().unwrap().encode();
        cloud.inject("mB", json!({"t": "recv", "from": "mA", "sid": sid, "kind": "hs", "purpose": "session", "data": first}));
        sids.push(sid);
    }
    let from_b = |sid: &str, kind: FrameKind| {
        cloud
            .frames()
            .iter()
            .any(|(from, frame)| from == "mB" && frame.sid == sid && frame.kind == kind)
    };
    wait_until("B answers four", || {
        sids[..4].iter().all(|sid| from_b(sid, FrameKind::Hs))
    })
    .await;
    wait_until("B refuses the fifth", || from_b(&sids[4], FrameKind::Close)).await;
    assert!(!from_b(&sids[4], FrameKind::Hs));
    let started = Instant::now();
    wait_until("B drops the four after the expiry", || {
        sids[..4].iter().all(|sid| from_b(sid, FrameKind::Close))
    })
    .await;
    assert!(
        started.elapsed() >= Duration::from_millis(1000),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_target_that_stops_answering_is_not_responding_then_gone() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    assert_eq!(
        a.session("mB"),
        Some(SessionState::Open {
            not_responding: false
        })
    );
    // B's daemon hangs: the relay still holds its socket, but nothing it
    // would send arrives.
    cloud.set_hook(Some(Box::new(|from, _| {
        if from == "mB" {
            Route::Swallow
        } else {
            Route::Forward
        }
    })));
    let started = Instant::now();
    wait_until("not responding", || {
        a.session("mB")
            == Some(SessionState::Open {
                not_responding: true,
            })
    })
    .await;
    // Within `not_responding_after` of a keep, keeps going every 300 ms.
    assert!(
        started.elapsed() < Duration::from_millis(1200),
        "{:?}",
        started.elapsed()
    );
    let machines = a.link.machines(false).await.unwrap();
    assert_eq!(
        machines.iter().find(|m| m.id == "mB").unwrap().state,
        MachineState::NotResponding
    );
    wait_until("the session ends", || a.session("mB").is_none()).await;
    assert!(started.elapsed() >= Duration::from_millis(1800));
}

#[tokio::test]
async fn keeps_are_answered_and_hold_a_session_open() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert_eq!(
        a.session("mB"),
        Some(SessionState::Open {
            not_responding: false
        })
    );
    let keeps = cloud
        .frames()
        .iter()
        .filter(|(from, frame)| from == "mA" && is_message(frame))
        .count();
    assert!(keeps >= 8, "{keeps}");
}

// --- Leases ------------------------------------------------------------------

#[tokio::test]
async fn a_refused_renewal_ends_the_sessions() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    cloud.set_lease(402, 60);
    wait_until("the session ends", || a.session("mB").is_none()).await;
    // Whichever side asks for a lease first is refused.
    let code = a.link.open("mB", true).await.unwrap_err().code;
    // A's own lease says the plan lapsed; B's refusal reaches A as a close.
    assert!(
        code == "subscription_required" || code == "refused",
        "{code}"
    );
    cloud.set_lease(200, 60);
    a.link.open("mB", true).await.unwrap();
}

#[tokio::test]
async fn a_lease_that_runs_out_ends_the_sessions() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    // The next renewal grants 1 s; the ones after it fail with a 500,
    // which is no refusal: the lease runs to its end, then the session.
    cloud.set_lease(200, 1);
    let requests = cloud.lease_requests();
    wait_until("a 1-s lease", || cloud.lease_requests() > requests).await;
    cloud.set_lease(500, 1);
    let started = Instant::now();
    wait_until("the session ends", || a.session("mB").is_none()).await;
    assert!(
        started.elapsed() <= Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
}

/// A renewal that fails waits before the next one, with backoff, while the
/// lease keeps its own expiry: no request every tick.
#[tokio::test]
async fn a_failing_renewal_backs_off_and_keeps_the_lease() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    cloud.set_lease(503, 60);
    let requests = cloud.lease_requests();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    // Renewals of A and B, each due at 0.7 s, then after 0.1-0.2, 0.2-0.4,
    // 0.4-0.8 s...: a handful, where retrying every 40 ms tick would be ~90.
    let made = cloud.lease_requests() - requests;
    assert!(made <= 14, "{made} renewals");
    assert_eq!(
        a.session("mB"),
        Some(SessionState::Open {
            not_responding: false
        })
    );
}

#[tokio::test]
async fn after_a_wake_sessions_end_until_a_new_lease() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    a.link
        .shared
        .wake
        .store(true, std::sync::atomic::Ordering::SeqCst);
    wait_until("the session ends", || a.session("mB").is_none()).await;
    let bye = cloud
        .frames()
        .into_iter()
        .any(|(from, frame)| from == "mA" && is_message(&frame));
    assert!(bye);
    // A renewal succeeds: sessions open again.
    a.link.open("mB", true).await.unwrap();
}

#[test]
fn a_wall_clock_moved_back_by_hours_is_not_a_wake() {
    use crate::daemon::public_tunnel::slept_between as slept;
    let hour = Duration::from_secs(3600);
    // Back by hours: the wall clock elapsed less than the monotonic one.
    assert!(!slept(Duration::ZERO, hour));
    // Forward by hours looks like a sleep, which only expires the lease.
    assert!(slept(5 * hour, Duration::from_secs(20)));
    assert!(!slept(Duration::from_secs(20), Duration::from_secs(20)));
}

// --- Revocation and unpairing ------------------------------------------------

#[tokio::test]
async fn a_revoked_machine_loses_its_pin_everywhere_and_signs_out() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    b.link
        .change_settings(crate::infrastructure::remote::store::SettingsChange {
            share: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut subscription = a.link.subscribe(false);
    cloud.revoke("mB");
    next_event(&mut subscription.events, |event| {
        matches!(event, Event::Revoked { .. })
    })
    .await;
    assert!(!a.pinned("mB"));
    // 4401 on B's socket: B signs out here, as `logout` does.
    wait_until("B is signed out", || {
        b.link.status().relay == RelayState::Blocked
    })
    .await;
    assert_eq!(b.link.status().blocked.unwrap().code, "signed_out");
    assert_signed_out(&b);
}

/// `cloud.json`, the key, the pins and the settings are all gone.
fn assert_signed_out(daemon: &Daemon) {
    let data = daemon.dir.path();
    for file in [
        "cloud.json",
        "remote/identity.json",
        "remote/peers.json",
        "remote/settings.json",
    ] {
        assert!(!data.join(file).exists(), "{file} is left");
    }
    assert!(!data.join("remote").exists());
}

/// A `401` on a lease: the token is revoked, and the machine signs out.
#[tokio::test]
async fn a_revoked_token_on_a_lease_signs_the_machine_out() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    // Both ends renew while the session is open, and the first renewal gets
    // 401. That machine signs out; the other one's session ends with it, so
    // it has nothing left to renew.
    cloud.set_lease(401, 60);
    let signed_out = |daemon: &Daemon| {
        daemon
            .link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "signed_out")
    };
    wait_until("a machine is signed out", || {
        signed_out(&a) || signed_out(&b)
    })
    .await;
    let revoked = if signed_out(&a) { &a } else { &b };
    assert_signed_out(revoked);
    wait_until("its socket closes", || !cloud.connected(&revoked.id)).await;
}

/// `logout` while connected: the socket closes, and a pairing in progress
/// writes nothing, even when the other side confirms afterwards.
#[tokio::test]
async fn signing_out_while_connected_closes_the_link_and_pins_nothing() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let mut subscription = b.link.subscribe(false);
    let (mut events, id_a, _, id_b, _) = to_codes(&a, &b, &mut subscription.events).await;
    a.link.answer(&id_a, true).await.unwrap();

    CredentialsStore::in_data_dir(a.dir.path())
        .delete()
        .unwrap();
    a.link.refresh();
    wait_until("A's socket closes", || !cloud.connected("mA")).await;
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));
    let _ = b.link.answer(&id_b, true).await;

    wait_until("A is off", || a.link.status().relay == RelayState::Off).await;
    assert_signed_out(&a);
    let connections = cloud.connections();
    a.link.refresh();
    wait_until("A stays off", || a.link.status().relay == RelayState::Off).await;
    assert_eq!(cloud.connections(), connections);
}

/// Logout, then a new login as another machine id, while the old
/// connection still runs (no refresh reached it): the old pairing's late
/// confirmation writes nothing into the new sign-in.
#[tokio::test]
async fn a_stale_pairing_writes_nothing_into_a_newer_sign_in() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let mut subscription = b.link.subscribe(false);
    let (mut events, id_a, _, id_b, _) = to_codes(&a, &b, &mut subscription.events).await;
    a.link.answer(&id_a, true).await.unwrap();

    let credentials = CredentialsStore::in_data_dir(a.dir.path());
    credentials.delete().unwrap();
    let token = cloud.add_machine("mA2", "laptop");
    credentials
        .save(&CloudCredentials {
            api_base: cloud.base.clone(),
            token,
            login: "teo".into(),
            machine_id: "mA2".into(),
            machine_name: "laptop".into(),
        })
        .unwrap();
    // B confirms: A's old actor gets the confirmation and tries to pin.
    b.link.answer(&id_b, true).await.unwrap();
    let PairEvent::Cancelled { message, .. } = next_pair_event(&mut events).await else {
        panic!("A's pairing should not complete");
    };
    assert!(message.contains("pin could not be saved"), "{message}");
    let store = a.store();
    assert!(store.pins().unwrap().is_empty());
    assert!(!a.dir.path().join("remote/peers.json").exists());
    assert_eq!(credentials.load().unwrap().unwrap().machine_id, "mA2");
}

/// The bodies of a `401` that does not come from the cloud's own check: a
/// proxy's page, nothing, a JSON of something else, or of another code.
const NOT_REVOCATIONS: [&str; 4] = [
    "<html><body>401 Authorization Required</body></html>",
    "",
    r#"{"status": 401}"#,
    r#"{"error": "token_expired", "message": "Log in again."}"#,
];

/// `settings.json` with share on, so a wipe would show.
async fn share_on(daemon: &Daemon) {
    daemon
        .link
        .change_settings(crate::infrastructure::remote::store::SettingsChange {
            share: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
}

fn kept(daemon: &Daemon) {
    let data = daemon.dir.path();
    assert!(data.join("cloud.json").exists(), "cloud.json is gone");
    assert!(
        daemon.store().settings().unwrap().share,
        "the settings are gone"
    );
    assert_ne!(
        daemon.link.status().blocked.map(|blocked| blocked.code),
        Some("signed_out".to_string())
    );
}

/// `/v1/me` and the key upload answering a `401` that is not the cloud's:
/// retried, "can't reach Localify Cloud", and nothing deleted.
#[tokio::test]
async fn a_401_that_is_not_a_revocation_keeps_the_sign_in_on_me_and_key() {
    let cloud = FakeCloud::start().await;
    for route in ["me", "key"] {
        for body in NOT_REVOCATIONS {
            let id = format!("m-{route}-{}", body.len());
            let token = cloud.add_machine(&id, "laptop");
            cloud.set_unauthorized(route, Some(body));
            let daemon = launch(&cloud, &id, "laptop", &token, tempfile::tempdir().unwrap());
            share_on(&daemon).await;
            wait_until("it says it can't reach the cloud", || {
                daemon
                    .link
                    .status()
                    .blocked
                    .is_some_and(|blocked| blocked.code == "unreachable")
            })
            .await;
            assert_eq!(daemon.link.status().relay, RelayState::Reconnecting);
            kept(&daemon);
            cloud.set_unauthorized(route, None);
            daemon.link.refresh();
            wait_until("it connects", || {
                daemon.link.status().relay == RelayState::Connected
            })
            .await;
            kept(&daemon);
        }
    }
}

#[tokio::test]
async fn a_401_that_is_not_a_revocation_keeps_the_sign_in_on_the_upgrade() {
    let cloud = FakeCloud::start().await;
    for body in NOT_REVOCATIONS {
        let id = format!("m-connect-{}", body.len());
        let token = cloud.add_machine(&id, "laptop");
        cloud.set_unauthorized("connect", Some(body));
        let daemon = launch(&cloud, &id, "laptop", &token, tempfile::tempdir().unwrap());
        share_on(&daemon).await;
        let upgrades = cloud.upgrades().len();
        wait_until("it says it can't reach the cloud", || {
            daemon
                .link
                .status()
                .blocked
                .is_some_and(|blocked| blocked.code == "unreachable")
        })
        .await;
        kept(&daemon);
        assert!(
            daemon.store().load_identity().unwrap().is_some(),
            "the key is gone"
        );
        cloud.set_unauthorized("connect", None);
        wait_until("it connects", || {
            daemon.link.status().relay == RelayState::Connected
        })
        .await;
        assert!(cloud.upgrades().len() > upgrades);
        kept(&daemon);
    }
}

/// A lease answered by a `401` that is not the cloud's, on every path that
/// asks for one: the renewal of an open session, a viewer's `open`, and
/// the polling after `4402`. The sign-in stays.
#[tokio::test]
async fn a_401_that_is_not_a_revocation_keeps_the_sign_in_on_leases() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    share_on(&a).await;
    a.link.open("mB", true).await.unwrap();
    for body in NOT_REVOCATIONS {
        cloud.set_unauthorized("lease", Some(body));
        let requests = cloud.lease_requests();
        wait_until("a renewal is refused", || {
            cloud.lease_requests() > requests + 1
        })
        .await;
        kept(&a);
    }
    // A viewer's open, with no lease left after a wake.
    a.link
        .shared
        .wake
        .store(true, std::sync::atomic::Ordering::SeqCst);
    wait_until("the session ends", || a.session("mB").is_none()).await;
    // Not "remote access lapsed", nor a sign-in problem: unreachable.
    let failure = a.link.open("mB", true).await.unwrap_err();
    assert_eq!(failure.code, "unreachable");
    assert!(
        failure.message.contains("Can't reach Localify Cloud"),
        "{}",
        failure.message
    );
    kept(&a);
    // The polling after a lapse says unreachable too, not a lapse, and asks
    // again sooner than the 15-minute (here 500 ms) recheck.
    let requests = cloud.lease_requests();
    cloud.close("mA", 4402, "remote access lapsed");
    wait_until("it asked for a lease", || cloud.lease_requests() > requests).await;
    wait_until("it says unreachable", || {
        a.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "unreachable")
    })
    .await;
    wait_until("it asked again", || cloud.lease_requests() > requests + 1).await;
    kept(&a);
    // A 402 is the plan's: the status says so.
    cloud.set_unauthorized("lease", None);
    cloud.set_lease(402, 60);
    wait_until("it says the plan lapsed", || {
        a.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "subscription_required")
    })
    .await;
    // A 5xx is no lapse either.
    cloud.set_lease(503, 60);
    wait_until("it says unreachable again", || {
        a.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "unreachable")
    })
    .await;
    kept(&a);
    // Granted again: connected, everything kept.
    cloud.set_lease(200, 60);
    wait_until("connected again", || {
        a.link.status().relay == RelayState::Connected
    })
    .await;
    kept(&a);
}

/// The cloud's own revocation on a lease a viewer asks for, and on the
/// polling after a lapse, signs the machine out as a renewal's does.
#[tokio::test]
async fn a_revoked_lease_signs_out_from_open_and_from_the_lapse_polling() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    cloud.set_lease(401, 60);
    a.link
        .shared
        .wake
        .store(true, std::sync::atomic::Ordering::SeqCst);
    wait_until("A's lease expired", || {
        !a.link
            .shared
            .conn()
            .is_some_and(|conn| conn.lease.is_valid(Instant::now()))
    })
    .await;
    assert_eq!(
        a.link.open("mB", true).await.unwrap_err().code,
        "signed_out"
    );
    wait_until("A is signed out", || {
        a.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "signed_out")
    })
    .await;
    assert_signed_out(&a);

    cloud.set_lease(402, 60);
    cloud.close("mB", 4402, "remote access lapsed");
    wait_until("B is lapsed", || {
        b.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "subscription_required")
    })
    .await;
    cloud.set_lease(401, 60);
    wait_until("B is signed out", || {
        b.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "signed_out")
    })
    .await;
    assert_signed_out(&b);
}

/// The cloud's revocation code from an API base that is neither
/// `https://api.localify.dev` nor the explicit override: not trusted.
#[tokio::test]
async fn a_revocation_from_an_untrusted_api_base_keeps_the_sign_in() {
    let cloud = FakeCloud::start().await;
    let token = cloud.add_machine("mU", "laptop");
    let dir = tempfile::tempdir().unwrap();
    CredentialsStore::in_data_dir(dir.path())
        .save(&CloudCredentials {
            api_base: cloud.base.clone(),
            token,
            login: "teo".into(),
            machine_id: "mU".into(),
            machine_name: "laptop".into(),
        })
        .unwrap();
    let link = link(dir.path());
    let cancel = CancellationToken::new();
    spawn(link.clone(), cancel.clone());
    let daemon = Daemon {
        dir,
        link,
        cancel,
        id: "mU".into(),
        source: Arc::new(FakeSource::default()),
    };
    share_on(&daemon).await;
    cloud.set_unauthorized(
        "me",
        Some(r#"{"error": "unauthorized", "message": "Signed out."}"#),
    );
    wait_until("it says it can't reach the cloud", || {
        daemon
            .link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "unreachable")
    })
    .await;
    kept(&daemon);
}

/// A daemon updated from before `owner.json`, restarted on its old folder:
/// it keeps its key, so the cloud's set-once key check still passes, and
/// its pins and settings with it.
#[tokio::test]
async fn an_update_from_before_owner_json_keeps_the_key_pins_and_settings() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    share_on(&a).await;
    let key = a
        .store()
        .load_identity()
        .unwrap()
        .unwrap()
        .public_key
        .clone();
    a.cancel.cancel();
    wait_until("A's socket closes", || !cloud.connected("mA")).await;
    std::fs::remove_file(a.dir.path().join("remote/owner.json")).unwrap();

    let link = link(a.dir.path());
    link.trust_api_base(&cloud.base);
    let cancel = CancellationToken::new();
    spawn(link.clone(), cancel.clone());
    wait_until("the updated daemon connects", || {
        link.status().relay == RelayState::Connected
    })
    .await;
    cancel.cancel();

    assert_eq!(a.store().load_identity().unwrap().unwrap().public_key, key);
    assert!(a.pinned("mB"));
    assert!(a.store().settings().unwrap().share);
    assert!(a.dir.path().join("remote/owner.json").exists());
}

/// The pairing limit lives in the link: a new connection keeps the count.
#[tokio::test]
async fn reconnecting_does_not_reset_the_pairing_limit() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let mut subscription = b.link.subscribe(false);
    for _ in 0..5 {
        let (mut events, id_a, _, _, _) = to_codes(&a, &b, &mut subscription.events).await;
        a.link.answer(&id_a, false).await.unwrap();
        next_pair_event(&mut events).await;
    }
    let connections = cloud.connections();
    cloud.close("mB", 1012, "");
    wait_until("B reconnects", || {
        cloud.connections() > connections && cloud.connected("mB")
    })
    .await;
    wait_until("A sees B again", || a.sees("mB")).await;
    let mut events = a.link.pair(&b.id).await.unwrap();
    let PairEvent::Cancelled { reason, .. } = next_pair_event(&mut events).await else {
        panic!("not busy");
    };
    assert_eq!(reason, "busy");
}

#[tokio::test]
async fn unpairing_with_a_session_open_unpairs_both() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    a.link.open("mB", true).await.unwrap();
    assert!(a.link.unpair("mB").unwrap());
    wait_until("B forgets A", || !b.pinned("mA")).await;
    assert!(!a.pinned("mB"));
}

// --- Reconnects --------------------------------------------------------------

/// The storm end to end, scaled down to 20 real daemons on one account:
/// each holds a socket, a lease keeper and an actor, and 200 of them in one
/// test process would make the suite slow without testing anything more on
/// the daemon's side. The contract's 200 daemons across 20 accounts run in
/// `backoff::tests`, on a simulated clock with replayed randomness.
#[tokio::test]
async fn a_reconnect_storm_spreads_out_and_replays_nothing() {
    let cloud = FakeCloud::start().await;
    let mut daemons = Vec::new();
    for index in 0..20 {
        daemons.push(daemon(&cloud, &format!("m{index}"), &format!("machine-{index}")).await);
    }
    let messages_before = cloud
        .frames()
        .iter()
        .filter(|(_, frame)| is_message(frame))
        .count();
    let upgrades_before = cloud.upgrades().len();
    let closed_at = Instant::now();
    cloud.close_all(1012);
    wait_until("every daemon is back", || {
        cloud.upgrades().len() >= upgrades_before + 20
    })
    .await;
    for daemon in &daemons {
        wait_until("connected again", || {
            daemon.link.status().relay == RelayState::Connected
        })
        .await;
    }
    let mut delays: Vec<Duration> = cloud.upgrades()[upgrades_before..]
        .iter()
        .map(|(_, at)| at.duration_since(closed_at))
        .collect();
    delays.sort();
    // Spread over the deploy jitter (1 s here, 10 s in production), and
    // exactly one attempt per daemon.
    assert!(
        delays[19] - delays[0] >= Duration::from_millis(300),
        "{delays:?}"
    );
    assert!(delays[19] <= Duration::from_millis(1500), "{delays:?}");
    assert_eq!(cloud.upgrades().len(), upgrades_before + 20);
    let messages_after = cloud
        .frames()
        .iter()
        .filter(|(_, frame)| is_message(frame))
        .count();
    assert_eq!(messages_before, messages_after, "nothing was resent");
    assert_eq!(cloud.refusals(), 0, "the relay's budgets refused nothing");
}

#[tokio::test]
async fn final_closes_and_refusals_stop_the_link() {
    let cloud = FakeCloud::start().await;
    let a = daemon(&cloud, "mA", "laptop").await;
    cloud.close("mA", 4403, "");
    wait_until("blocked", || a.link.status().relay == RelayState::Blocked).await;
    assert_eq!(a.link.status().blocked.unwrap().code, "banned");

    cloud.set_remote_access(false);
    a.link.refresh();
    wait_until("needs Pro", || {
        a.link
            .status()
            .blocked
            .is_some_and(|blocked| blocked.code == "subscription_required")
    })
    .await;
    let attempts = cloud.connections();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        cloud.connections(),
        attempts,
        "a lapsed account does not retry on its own"
    );

    cloud.set_remote_access(true);
    a.link.refresh();
    wait_until("connected again", || {
        a.link.status().relay == RelayState::Connected
    })
    .await;
}

/// `4402`: the link waits `random(30, 60)` s (here 200 to 300 ms), asks for
/// a lease, asks again every 15 minutes (here 500 ms) while it is refused,
/// and connects again on its own once one is granted.
#[tokio::test]
async fn a_lapsed_account_reconnects_once_a_lease_is_granted() {
    let cloud = FakeCloud::start().await;
    let a = daemon(&cloud, "mA", "laptop").await;
    cloud.set_lease(402, 60);
    cloud.close("mA", 4402, "remote access lapsed");
    wait_until("blocked", || a.link.status().relay == RelayState::Blocked).await;
    assert_eq!(
        a.link.status().blocked.unwrap().code,
        "subscription_required"
    );
    let connections = cloud.connections();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        cloud.connections(),
        connections,
        "no upgrade while leases are refused"
    );

    cloud.set_lease(200, 60);
    wait_until("connected again", || {
        a.link.status().relay == RelayState::Connected
    })
    .await;
}

#[tokio::test]
async fn the_key_is_uploaded_and_shown_as_a_fingerprint() {
    let cloud = FakeCloud::start().await;
    let a = daemon(&cloud, "mA", "laptop").await;
    let status = a.link.status();
    let identity = a.store().load_identity().unwrap().unwrap();
    let key = crate::infrastructure::remote::noise::decode_key(&identity.public_key).unwrap();
    assert_eq!(
        status.fingerprint,
        Some(crate::infrastructure::remote::noise::fingerprint(&key))
    );
    let machines = a.link.machines(false).await.unwrap();
    assert!(machines[0].is_self);
    assert_eq!(machines[0].fingerprint, status.fingerprint);
}

/// A private key that does not decode: the status says which file, and
/// nothing of what it holds reaches `remote_status`, which `localify
/// machines` prints.
#[tokio::test]
async fn an_invalid_private_key_is_never_shown() {
    let cloud = FakeCloud::start().await;
    let token = cloud.add_machine("mA", "laptop");
    let dir = tempfile::tempdir().unwrap();
    let secret = "c2VjcmV0LXByaXZhdGUta2V5LXRoYXQtaXMtYnJva2VuIQ";
    std::fs::create_dir_all(dir.path().join("remote")).unwrap();
    std::fs::write(
        dir.path().join("remote/owner.json"),
        r#"{"machineId": "mA"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("remote/identity.json"),
        json!({"machineId": "mA", "publicKey": encode_key(&StaticKeys::generate().unwrap().public),
               "privateKey": secret, "createdAt": "2026-11-08T10:00:00Z"})
        .to_string(),
    )
    .unwrap();
    let a = launch(&cloud, "mA", "laptop", &token, dir);
    wait_until("blocked", || a.link.status().relay == RelayState::Blocked).await;
    let status = serde_json::to_string(&a.link.status()).unwrap();
    assert!(status.contains("remote_key_unreadable"), "{status}");
    assert!(
        status.contains("private key in remote/identity.json is invalid"),
        "{status}"
    );
    for window in secret.as_bytes().windows(6) {
        let part = std::str::from_utf8(window).unwrap();
        assert!(!status.contains(part), "{part} in {status}");
    }
}

#[tokio::test]
async fn a_lost_key_on_a_signed_in_machine_says_how_to_recover() {
    let cloud = FakeCloud::start().await;
    let token = cloud.add_machine("mA", "laptop");
    cloud.set_roster_key("mA", &encode_key(&StaticKeys::generate().unwrap().public));
    let a = launch(&cloud, "mA", "laptop", &token, tempfile::tempdir().unwrap());
    wait_until("blocked", || a.link.status().relay == RelayState::Blocked).await;
    let blocked = a.link.status().blocked.unwrap();
    assert_eq!(blocked.code, "remote_key_missing");
    assert!(blocked.message.contains("Sign out and sign in again"));
}
