//! The link end to end, between daemons in this process and the fake relay:
//! the Rust side of the PR 2 gates in `docs/remote-machines.md`, "Gates".

use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::fake_relay::{FakeCloud, Route, is_message, opens};
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
        lapsed_wait: ms(3_600_000),
        clock_check: ms(100),
        retry_after_drop: ms(300),
    }
}

struct Daemon {
    dir: TempDir,
    link: RemoteLink,
    cancel: CancellationToken,
    id: String,
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
    let token = cloud.add_machine(id, name);
    start(cloud, id, name, &token, tempfile::tempdir().unwrap()).await
}

async fn start(cloud: &FakeCloud, id: &str, name: &str, token: &str, dir: TempDir) -> Daemon {
    let daemon = launch(cloud, id, name, token, dir);
    wait_until("the link connects", || {
        daemon.link.status().relay == RelayState::Connected
    })
    .await;
    daemon
}

fn launch(cloud: &FakeCloud, id: &str, name: &str, token: &str, dir: TempDir) -> Daemon {
    CredentialsStore::in_data_dir(dir.path())
        .save(&CloudCredentials {
            api_base: cloud.base.clone(),
            token: token.into(),
            login: "teo".into(),
            machine_id: id.into(),
            machine_name: name.into(),
        })
        .unwrap();
    let link = RemoteLink::with_timings(dir.path(), fast());
    let cancel = CancellationToken::new();
    spawn(link.clone(), cancel.clone());
    Daemon {
        dir,
        link,
        cancel,
        id: id.into(),
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
    let PairEvent::Code {
        pairing_id: id_a,
        code: code_a,
        ..
    } = next_pair_event(&mut events).await
    else {
        panic!("no code");
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
    let mut subscription = b.link.subscribe();
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

// --- Pairing -----------------------------------------------------------------

#[tokio::test]
async fn two_machines_pair_once_both_people_confirm_the_same_code() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;

    let key_a = a.store().load_identity().unwrap().unwrap().public_key;
    let key_b = b.store().load_identity().unwrap().unwrap().public_key;
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
    let mut subscription = b.link.subscribe();

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
    let mut subscription = b.link.subscribe();
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
    let _subscription = b.link.subscribe();
    // The relay repeats every session message B sends: the commitment
    // arrives twice at A.
    cloud.set_hook(Some(Box::new(|from, frame| {
        if from == "mB" && is_message(frame) {
            Route::Twice
        } else {
            Route::Forward
        }
    })));
    let mut events = a.link.pair(&b.id).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));

    // The relay holds B's hello and sends it after the commitment: the
    // commitment comes first.
    let mut held = false;
    cloud.set_hook(Some(Box::new(move |from, frame| {
        if from == "mB" && is_message(frame) && !held {
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
    let _subscription = b.link.subscribe();
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
    let mut subscription = b.link.subscribe();
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
    let key_b = b.store().load_identity().unwrap().unwrap().public_key;
    a.store()
        .update_pins(|pins| {
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
    a.link.machines(false).await.unwrap();
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
        .update_pins(|pins| {
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
    assert!(code == "unauthorized" || code == "refused", "{code}");
    cloud.set_lease(200, 60);
    a.link.open("mB", true).await.unwrap();
}

#[tokio::test]
async fn a_lease_that_runs_out_ends_the_sessions() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    // A lease of 1 s, renewed at 0.75 s; renewals then fail.
    cloud.set_lease(200, 1);
    a.link.open("mB", true).await.unwrap();
    cloud.set_lease(500, 1);
    let started = Instant::now();
    wait_until("the session ends", || a.session("mB").is_none()).await;
    assert!(
        started.elapsed() <= Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
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
async fn a_revoked_machine_loses_its_pin_everywhere() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    pair(&a, &b).await;
    let mut subscription = a.link.subscribe();
    cloud.revoke("mB");
    next_event(&mut subscription.events, |event| {
        matches!(event, Event::Revoked { .. })
    })
    .await;
    assert!(!a.pinned("mB"));
    wait_until("B is signed out", || {
        b.link.status().relay == RelayState::Blocked
    })
    .await;
    assert_eq!(b.link.status().blocked.unwrap().code, "signed_out");
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
