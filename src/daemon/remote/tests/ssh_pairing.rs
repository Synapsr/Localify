//! Pairing over SSH (`docs/remote-machines.md`, "Pairing over SSH"; ADR
//! 0035): the one-shot acceptor in the daemon, the `--for-ssh` helper on
//! its line protocol, and the whole pairing driven from A. The SSH hop is
//! replaced by a direct pipe to the helper: the `localify` binary run here,
//! against B's management socket, in a temporary home.

use std::io::{BufRead, BufReader, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc as sync_mpsc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use super::convergence::localify_binary;
use super::viewing::serve_management;
use super::{Daemon, FakeCloud, cut_after_bs_confirm, daemon, next_pair_event, two, wait_until};
use crate::cli::pair_ssh::{self, HelperArgs, Peer, Report, Target};
use crate::daemon::remote::PairEvent;
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::remote::pairing::{Outcome, PinState};
use crate::infrastructure::remote::ssh_pair::{self, ControlLine, HelperLine, ProtocolError};

fn account(daemon: &Daemon) -> String {
    daemon.link.status().account.expect("connected")
}

/// The helper run in a thread on OS pipes: what A would write to its stdin,
/// and the lines of its stdout.
struct Helper {
    stdin: Option<PipeWriter>,
    lines: sync_mpsc::Receiver<String>,
    done: std::thread::JoinHandle<anyhow::Result<i32>>,
}

impl Helper {
    fn start(socket: &Path, from: &str, account: &str) -> Self {
        let (stdin_reader, stdin) = std::io::pipe().unwrap();
        let (out_reader, mut out) = std::io::pipe().unwrap();
        let client = MgmtSocketClient::new(socket);
        let args = HelperArgs {
            from: from.into(),
            account: account.into(),
        };
        let done = std::thread::spawn(move || {
            pair_ssh::run_helper(&client, &args, stdin_reader, &mut out)
        });
        let (tx, lines) = sync_mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out_reader).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            stdin: Some(stdin),
            lines,
            done,
        }
    }

    /// The next stdout line, without blocking the daemons' runtime.
    async fn next(&self) -> Value {
        for _ in 0..300 {
            if let Ok(line) = self.lines.try_recv() {
                assert_eq!(
                    serde_json::from_str::<Value>(&line).unwrap()["v"],
                    1,
                    "{line}"
                );
                return serde_json::from_str(&line).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the helper wrote no line");
    }

    fn write(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }

    async fn exit_code(self) -> anyhow::Result<i32> {
        drop(self.stdin);
        tokio::task::spawn_blocking(move || self.done.join().unwrap())
            .await
            .unwrap()
    }
}

/// The first line A wrote to B's stdin.
fn first_line(stdin: std::io::PipeReader) -> Result<ControlLine, ProtocolError> {
    let mut line = String::new();
    BufReader::new(stdin).read_line(&mut line).unwrap();
    ssh_pair::parse_control_line(line.trim_end().as_bytes())
}

async fn code_of(events: &mut mpsc::Receiver<PairEvent>) -> String {
    let event = next_pair_event(events).await;
    let PairEvent::Code { code, .. } = event else {
        panic!("no code: {event:?}");
    };
    code
}

// --- The acceptor --------------------------------------------------------------

#[tokio::test]
async fn an_acceptor_is_consumed_only_by_its_machine_and_only_once() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let c = daemon(&cloud, "mC", "desk").await;
    wait_until("B and C see each other", || b.sees("mC") && c.sees("mB")).await;
    let (ready, mut accepted) = b.link.accept("mA", &account(&b)).await.unwrap();
    assert_eq!(
        (ready.machine_id.as_str(), ready.account.as_str()),
        ("mB", "u1")
    );
    assert!(!ready.attempt.is_empty());

    // Another machine's request cannot consume it: B is busy with it.
    let mut from_c = c.link.pair("mB").await.unwrap();
    let event = next_pair_event(&mut from_c).await;
    assert!(
        matches!(&event, PairEvent::Cancelled { reason, .. } if reason == "busy"),
        "{event:?}"
    );

    let mut from_a = a.link.pair("mB").await.unwrap();
    let PairEvent::Code {
        pairing_id: id_a,
        code: code_a,
        ..
    } = next_pair_event(&mut from_a).await
    else {
        panic!("no code on A");
    };
    let PairEvent::Code {
        pairing_id: id_b,
        code: code_b,
        ..
    } = next_pair_event(&mut accepted).await
    else {
        panic!("no code on B");
    };
    assert_eq!(code_a, code_b);
    a.link.answer(&id_a, true).await.unwrap();
    b.link.answer(&id_b, true).await.unwrap();
    assert!(matches!(
        next_pair_event(&mut from_a).await,
        PairEvent::Paired { .. }
    ));
    assert!(matches!(
        next_pair_event(&mut accepted).await,
        PairEvent::Paired { .. }
    ));
    assert!(a.pinned("mB") && b.pinned("mA"));

    // Consumed: with nobody else to confirm on B, the next one is declined.
    let mut again = a.link.pair("mB").await.unwrap();
    let event = next_pair_event(&mut again).await;
    assert!(
        matches!(&event, PairEvent::Cancelled { reason, .. } if reason == "nobody_to_confirm"),
        "{event:?}"
    );
}

#[tokio::test]
async fn an_acceptor_takes_one_pairing_at_a_time_and_counts_when_registered() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let held = b.link.accept("mA", &account(&b)).await.unwrap();
    let busy = b.link.accept("mA", &account(&b)).await.unwrap_err();
    assert!(busy.to_string().contains("Another pairing"), "{busy}");
    drop(held);
    // Four more, each given up at once: five in the hour.
    for _ in 0..4 {
        drop(b.link.accept("mA", &account(&b)).await.unwrap());
    }
    let full = b.link.accept("mA", &account(&b)).await.unwrap_err();
    assert!(full.to_string().contains("5 pairing attempts"), "{full}");
    // The count is the target's, whoever asks.
    let _subscription = b.link.subscribe(false);
    let mut events = a.link.pair("mB").await.unwrap();
    let event = next_pair_event(&mut events).await;
    assert!(
        matches!(&event, PairEvent::Cancelled { reason, .. } if reason == "busy"),
        "{event:?}"
    );
}

#[tokio::test]
async fn an_acceptor_refuses_another_account_and_its_own_machine() {
    let cloud = FakeCloud::start().await;
    let (_a, b) = two(&cloud).await;
    let error = b.link.accept("mA", "someone-else").await.unwrap_err();
    assert!(
        error.to_string().contains("another Localify Cloud account"),
        "{error}"
    );
    assert!(b.link.accept("mB", &account(&b)).await.is_err());
    // Neither counted: five still fit.
    for _ in 0..5 {
        drop(b.link.accept("mA", &account(&b)).await.unwrap());
    }
}

#[tokio::test]
async fn an_acceptors_two_minutes_start_when_it_is_registered() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let (_, mut accepted) = b.link.accept("mA", &account(&b)).await.unwrap();
    // The test timings give a pairing 2.5 s.
    let event = next_pair_event(&mut accepted).await;
    assert!(
        matches!(&event, PairEvent::Cancelled { reason, .. } if reason == "timeout"),
        "{event:?}"
    );
    let mut events = a.link.pair("mB").await.unwrap();
    let event = next_pair_event(&mut events).await;
    assert!(
        matches!(&event, PairEvent::Cancelled { reason, .. } if reason == "nobody_to_confirm"),
        "{event:?}"
    );
}

// --- The helper ----------------------------------------------------------------

#[test]
fn the_helper_refuses_a_terminal_on_either_side() {
    assert!(pair_ssh::refuse_terminal(true, false).is_err());
    assert!(pair_ssh::refuse_terminal(false, true).is_err());
    assert!(pair_ssh::refuse_terminal(true, true).is_err());
    assert!(pair_ssh::refuse_terminal(false, false).is_ok());
}

#[tokio::test]
async fn the_helper_refuses_another_account_or_machine_and_writes_nothing() {
    let cloud = FakeCloud::start().await;
    let (_a, b) = two(&cloud).await;
    let on_b = serve_management(&b).await;
    for (from, asked) in [("mA", "someone-else".to_string()), ("mB", account(&b))] {
        let mut helper = Helper::start(&on_b, from, &asked);
        let lines = std::mem::replace(&mut helper.lines, sync_mpsc::channel().1);
        let result = helper.exit_code().await;
        assert!(result.is_err(), "{from} {asked}: {result:?}");
        // Its stdout closed with nothing on it.
        assert!(
            lines.recv_timeout(Duration::from_secs(2)).is_err(),
            "{from} {asked}"
        );
    }
}

#[tokio::test]
async fn the_helper_confirms_only_on_its_exact_code() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let on_b = serve_management(&b).await;
    let mut helper = Helper::start(&on_b, "mA", &account(&b));
    let ready = helper.next().await;
    assert_eq!(ready["event"], "ready");
    assert_eq!(ready["machineId"], "mB");
    let attempt = ready["attempt"].clone();

    let mut events = a.link.pair("mB").await.unwrap();
    let code = code_of(&mut events).await;
    let shown = helper.next().await;
    assert_eq!(shown["event"], "code");
    assert_eq!(shown["attempt"], attempt);
    assert_eq!(shown["code"], code.as_str());

    // The same digits without their space are not the code.
    helper.write(&format!(
        r#"{{"v": 1, "confirm": "{}"}}"#,
        code.replace(' ', "")
    ));
    let result = helper.next().await;
    assert_eq!(result["event"], "result");
    assert_eq!(result["attempt"], attempt);
    assert_eq!(result["outcome"], "cancelled");
    assert_eq!(
        (result["local"].as_str(), result["peer"].as_str()),
        (Some("not_pinned"), Some("not_pinned"))
    );
    assert_eq!(helper.exit_code().await.unwrap(), 2);
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

/// A management socket that takes connections and never answers.
fn silent_socket(dir: &Path) -> PathBuf {
    let path = dir.join("silent.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            held.push(stream);
        }
    });
    path
}

#[test]
fn the_helper_never_waits_forever_for_its_daemon_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let client = MgmtSocketClient::new(&silent_socket(dir.path()));
    let args = HelperArgs {
        from: "mA".into(),
        account: "u1".into(),
    };
    // The daemon never answers: the wait bounds it.
    let (input, _open) = std::io::pipe().unwrap();
    let mut out = Vec::new();
    let started = std::time::Instant::now();
    let wait = Duration::from_millis(300);
    let error = pair_ssh::run_helper_within(&client, &args, input, &mut out, wait).unwrap_err();
    assert!(error.to_string().contains("did not answer"), "{error}");
    assert!(out.is_empty());
    assert!(started.elapsed() < Duration::from_secs(5));

    // A gives up and closes stdin while it waits: it stops at once.
    let (input, closed) = std::io::pipe().unwrap();
    drop(closed);
    let started = std::time::Instant::now();
    let wait = Duration::from_secs(60);
    let error = pair_ssh::run_helper_within(&client, &args, input, &mut out, wait).unwrap_err();
    assert!(error.to_string().contains("gave up"), "{error}");
    assert!(out.is_empty());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn a_closed_stdin_cancels_the_helper() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let on_b = serve_management(&b).await;

    // Before any code: `ready`, then `result`.
    let mut helper = Helper::start(&on_b, "mA", &account(&b));
    assert_eq!(helper.next().await["event"], "ready");
    helper.stdin = None;
    let result = helper.next().await;
    assert_eq!(
        (result["event"].as_str(), result["outcome"].as_str()),
        (Some("result"), Some("cancelled"))
    );
    assert_eq!(helper.exit_code().await.unwrap(), 2);

    // After the code, before a confirm.
    let mut helper = Helper::start(&on_b, "mA", &account(&b));
    assert_eq!(helper.next().await["event"], "ready");
    let mut events = a.link.pair("mB").await.unwrap();
    code_of(&mut events).await;
    assert_eq!(helper.next().await["event"], "code");
    helper.stdin = None;
    let result = helper.next().await;
    assert_eq!(result["outcome"], "cancelled");
    assert_eq!(helper.exit_code().await.unwrap(), 2);
    assert!(matches!(
        next_pair_event(&mut events).await,
        PairEvent::Cancelled { .. }
    ));
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

// --- End to end ----------------------------------------------------------------

/// A config that points the `localify` binary at B's management socket,
/// in a temporary home.
fn helper_config(home: &Path, socket: &Path) -> PathBuf {
    let config = home.join("config.toml");
    let root = home.display();
    std::fs::write(
        &config,
        format!(
            "[paths]\ndata_dir = \"{root}/data\"\npid_file = \"{root}/localify.pid\"\nlog_file = \"{root}/localify.log\"\ncerts_dir = \"{root}/data/certs\"\nsocket_path = \"{}\"\n",
            socket.display()
        ),
    )
    .unwrap();
    config
}

/// The command `ssh` would run on B, run here instead.
fn helper_command(home: &Path, socket: &Path, account: &str) -> Command {
    helper_command_from(home, socket, "mA", account)
}

fn helper_command_from(home: &Path, socket: &Path, from: &str, account: &str) -> Command {
    let mut command = Command::new(localify_binary());
    command
        .arg("--config")
        .arg(helper_config(home, socket))
        .args([
            "machines",
            "pair",
            "--accept",
            "--for-ssh",
            "--from",
            from,
            "--account",
            account,
        ])
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("PATH", "/usr/bin:/bin");
    command
}

/// Runs `localify machines pair mB --over-ssh` on A's daemon, against the
/// helper on `peer`.
async fn pair_over_pipes(a: &Daemon, on_a: &Path, peer: Peer) -> Report {
    let target = Target {
        machine_id: "mB".into(),
        name: "teo-mini".into(),
        own_id: "mA".into(),
        account: account(a),
    };
    drive_from(on_a, target, peer).await
}

async fn drive_from(on: &Path, target: Target, peer: Peer) -> Report {
    let client = MgmtSocketClient::new(on);
    tokio::task::spawn_blocking(move || {
        pair_ssh::drive(&client, &target, peer, std::time::Instant::now())
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn pairing_over_ssh_pins_both_sides_when_the_codes_match() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let (on_a, on_b) = (serve_management(&a).await, serve_management(&b).await);
    let home = tempfile::tempdir().unwrap();
    let peer = Peer::spawn(helper_command(home.path(), &on_b, &account(&b)), "teo-mini").unwrap();
    let report = pair_over_pipes(&a, &on_a, peer).await;
    assert_eq!(report.outcome, Outcome::Paired, "{report:?}");
    assert_eq!(
        (report.local, report.peer),
        (PinState::Pinned, PinState::Pinned)
    );
    assert_eq!(report.exit_code(), 0);
    assert!(a.pinned("mB") && b.pinned("mA"));
}

#[tokio::test]
async fn either_machine_can_initiate_a_pairing_over_ssh() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let (on_a, on_b) = (serve_management(&a).await, serve_management(&b).await);
    let home = tempfile::tempdir().unwrap();
    // B runs `localify machines pair laptop --over-ssh`; A accepts.
    let command = helper_command_from(home.path(), &on_a, "mB", &account(&a));
    let peer = Peer::spawn(command, "laptop").unwrap();
    let target = Target {
        machine_id: "mA".into(),
        name: "laptop".into(),
        own_id: "mB".into(),
        account: account(&b),
    };
    let report = drive_from(&on_b, target, peer).await;
    assert_eq!(report.outcome, Outcome::Paired, "{report:?}");
    assert!(a.pinned("mB") && b.pinned("mA"));
}

#[tokio::test]
async fn pairing_over_ssh_pins_nothing_when_the_codes_differ() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let (on_a, on_b) = (serve_management(&a).await, serve_management(&b).await);
    let home = tempfile::tempdir().unwrap();
    // Something between the machines shows A another code than B's.
    let mut child = helper_command(home.path(), &on_b, &account(&b))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (stdin, stdout, stderr) = (
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
        child.stderr.take().unwrap(),
    );
    let (altered, mut into_altered) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            let mut value: Value = serde_json::from_str(&line).unwrap();
            if value["event"] == "code" {
                let other = if value["code"] == "000 000" {
                    "000 001"
                } else {
                    "000 000"
                };
                value["code"] = other.into();
            }
            if writeln!(into_altered, "{value}").is_err() {
                return;
            }
        }
    });
    let peer = Peer::from_pipes(stdin, altered, stderr, "teo-mini");
    let report = pair_over_pipes(&a, &on_a, peer).await;
    assert_eq!(report.outcome, Outcome::Cancelled, "{report:?}");
    assert!(report.detail.contains("codes differ"), "{}", report.detail);
    assert_eq!(report.exit_code(), 2);
    let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();
    assert_eq!(status.code(), Some(2));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

#[tokio::test]
async fn pairing_over_ssh_cut_after_the_first_confirm_is_partial_then_completes() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let (on_a, on_b) = (serve_management(&a).await, serve_management(&b).await);
    let home = tempfile::tempdir().unwrap();
    cut_after_bs_confirm(&cloud);
    let peer = Peer::spawn(helper_command(home.path(), &on_b, &account(&b)), "teo-mini").unwrap();
    let report = pair_over_pipes(&a, &on_a, peer).await;
    // B saved, as its own result says; A never heard B's confirm.
    assert_eq!(report.outcome, Outcome::Partial, "{report:?}");
    assert_eq!(
        (report.local, report.peer),
        (PinState::NotPinned, PinState::Pinned)
    );
    assert_eq!(report.exit_code(), 4);
    assert!(!a.pinned("mB") && b.pinned("mA"));

    // Pairing the same machines again completes it.
    cloud.set_hook(None);
    let peer = Peer::spawn(helper_command(home.path(), &on_b, &account(&b)), "teo-mini").unwrap();
    let report = pair_over_pipes(&a, &on_a, peer).await;
    assert_eq!(report.outcome, Outcome::Paired, "{report:?}");
    assert!(a.pinned("mB") && b.pinned("mA"));
}

#[tokio::test]
async fn a_ready_for_another_machine_or_account_pins_nothing() {
    for (machine, account) in [("mX", "u1"), ("mB", "u2")] {
        let (stdin, stdin_writer) = std::io::pipe().unwrap();
        let (stdout, mut stdout_writer) = std::io::pipe().unwrap();
        let (stderr, _stderr_writer) = std::io::pipe().unwrap();
        writeln!(
            stdout_writer,
            r#"{{"v": 1, "event": "ready", "machineId": "{machine}", "account": "{account}", "attempt": "at1"}}"#
        )
        .unwrap();
        let peer = Peer::from_pipes(stdin_writer, stdout, stderr, "teo-mini");
        let target = Target {
            machine_id: "mB".into(),
            name: "teo-mini".into(),
            own_id: "mA".into(),
            account: "u1".into(),
        };
        // This machine's daemon is never asked.
        let client = MgmtSocketClient::new(Path::new("/nonexistent/localify.sock"));
        let report = pair_ssh::drive(&client, &target, peer, std::time::Instant::now());
        assert_eq!(report.outcome, Outcome::Cancelled, "{report:?}");
        assert_eq!(report.exit_code(), 2);
        // B was told to cancel.
        assert_eq!(first_line(stdin), Ok(ControlLine::Cancel));
    }
}

#[tokio::test]
async fn after_a_broken_line_nothing_from_b_is_trusted() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let on_a = serve_management(&a).await;
    // B's daemon holds the acceptor; its lines are written here.
    let (ready, mut accepted) = b.link.accept("mA", &account(&b)).await.unwrap();
    let (stdin, stdin_writer) = std::io::pipe().unwrap();
    let (stdout, mut lines) = std::io::pipe().unwrap();
    let (stderr, _stderr_writer) = std::io::pipe().unwrap();
    let attempt = ready.attempt.clone();
    let ready = HelperLine::Ready {
        machine_id: ready.machine_id,
        account: ready.account,
        attempt: attempt.clone(),
    };
    lines.write_all(ready.to_line().as_bytes()).unwrap();
    let peer = Peer::from_pipes(stdin_writer, stdout, stderr, "teo-mini");
    let target = Target {
        machine_id: "mB".into(),
        name: "teo-mini".into(),
        own_id: "mA".into(),
        account: account(&a),
    };
    let driving = tokio::spawn(async move { drive_from(&on_a, target, peer).await });

    let code = code_of(&mut accepted).await;
    let line = HelperLine::Code {
        attempt: attempt.clone(),
        code: code.clone(),
    };
    lines.write_all(line.to_line().as_bytes()).unwrap();
    // The codes match: A confirms its side and B's.
    let confirm = tokio::task::spawn_blocking(move || first_line(stdin))
        .await
        .unwrap();
    assert_eq!(confirm, Ok(ControlLine::Confirm(code)));

    // Then B's stdout breaks the protocol, and claims success after that,
    // while A's daemon still waits for B's.
    let result = HelperLine::Result {
        attempt,
        outcome: Outcome::Paired,
        local: PinState::Pinned,
        peer: PinState::Pinned,
        detail: String::new(),
    };
    lines.write_all(b"not json\n").unwrap();
    lines.write_all(result.to_line().as_bytes()).unwrap();
    drop(lines);

    // B's daemon never answers: its 2 minutes (2.5 s here) decide.
    let report = driving.await.unwrap();
    assert_eq!(report.outcome, Outcome::Partial, "{report:?}");
    assert_eq!(
        (report.local, report.peer),
        (PinState::NotPinned, PinState::Unknown)
    );
    assert_eq!(report.exit_code(), 4);
    assert!(report.detail.contains("cannot take"), "{}", report.detail);
    assert!(!a.pinned("mB") && !b.pinned("mA"));
}

#[tokio::test]
async fn a_line_after_b_s_result_undoes_that_result() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let on_a = serve_management(&a).await;
    let (ready, mut accepted) = b.link.accept("mA", &account(&b)).await.unwrap();
    let (stdin, stdin_writer) = std::io::pipe().unwrap();
    let (stdout, mut lines) = std::io::pipe().unwrap();
    let (stderr, _stderr_writer) = std::io::pipe().unwrap();
    let attempt = ready.attempt.clone();
    let ready = HelperLine::Ready {
        machine_id: ready.machine_id,
        account: ready.account,
        attempt: attempt.clone(),
    };
    lines.write_all(ready.to_line().as_bytes()).unwrap();
    let peer = Peer::from_pipes(stdin_writer, stdout, stderr, "teo-mini");
    let target = Target {
        machine_id: "mB".into(),
        name: "teo-mini".into(),
        own_id: "mA".into(),
        account: account(&a),
    };
    let driving = tokio::spawn(async move { drive_from(&on_a, target, peer).await });

    let PairEvent::Code {
        pairing_id: id_b,
        code,
        ..
    } = next_pair_event(&mut accepted).await
    else {
        panic!("no code on B");
    };
    let line = HelperLine::Code {
        attempt: attempt.clone(),
        code: code.clone(),
    };
    lines.write_all(line.to_line().as_bytes()).unwrap();
    let confirm = tokio::task::spawn_blocking(move || first_line(stdin))
        .await
        .unwrap();
    assert_eq!(confirm, Ok(ControlLine::Confirm(code)));
    // B's daemon confirms too, so A's side finishes before B's `result`.
    b.link.answer(&id_b, true).await.unwrap();
    wait_until("A pinned B", || a.pinned("mB")).await;

    // B claims success, then breaks the protocol before its stdout closes.
    let result = HelperLine::Result {
        attempt,
        outcome: Outcome::Paired,
        local: PinState::Pinned,
        peer: PinState::Pinned,
        detail: String::new(),
    };
    lines.write_all(result.to_line().as_bytes()).unwrap();
    lines.write_all(b"not json\n").unwrap();
    drop(lines);

    let report = driving.await.unwrap();
    assert_ne!(report.outcome, Outcome::Paired, "{report:?}");
    assert_ne!(report.exit_code(), 0);
    assert!(report.detail.contains("cannot take"), "{}", report.detail);
}

#[tokio::test]
async fn a_malformed_or_foreign_line_from_b_ends_the_attempt() {
    let cloud = FakeCloud::start().await;
    let (a, b) = two(&cloud).await;
    let on_a = serve_management(&a).await;
    let _ = b;
    for bad in [
        r#"{"v": 1, "event": "code", "attempt": "other", "code": "123 456"}"#,
        r#"{"v": 2, "event": "code", "attempt": "at1", "code": "123 456"}"#,
        "not json",
    ] {
        let (stdin, stdin_writer) = std::io::pipe().unwrap();
        let (stdout, mut stdout_writer) = std::io::pipe().unwrap();
        let (stderr, _stderr_writer) = std::io::pipe().unwrap();
        writeln!(
            stdout_writer,
            r#"{{"v": 1, "event": "ready", "machineId": "mB", "account": "u1", "attempt": "at1"}}"#
        )
        .unwrap();
        writeln!(stdout_writer, "{bad}").unwrap();
        drop(stdout_writer);
        let peer = Peer::from_pipes(stdin_writer, stdout, stderr, "teo-mini");
        let report = pair_over_pipes(&a, &on_a, peer).await;
        assert_eq!(report.outcome, Outcome::Cancelled, "{bad}: {report:?}");
        assert_eq!(first_line(stdin), Ok(ControlLine::Cancel), "{bad}");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!a.pinned("mB"));
}
