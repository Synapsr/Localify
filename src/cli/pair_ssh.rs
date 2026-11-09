//! Pairing over SSH (`docs/remote-machines.md`, "Pairing over SSH"; ADR
//! 0035): `localify machines pair <B> --over-ssh <destination>` on the
//! initiator, and the helper it starts on B, `localify machines pair
//! --accept --for-ssh`.
//!
//! The initiator runs `ssh` itself, reads both codes, compares them here,
//! and confirms both sides only when they match. The trust comes from the
//! SSH access the caller already has; the helper's refusal of a terminal
//! only keeps a person from using it by mistake.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::branding::BRANDING;
use crate::infrastructure::mgmt_client::{MgmtSocketClient, MgmtStream};
use crate::infrastructure::remote::pairing::{Outcome, PinState};
use crate::infrastructure::remote::ssh_pair::{
    self, ControlLine, HelperLine, HelperLines, ProtocolError,
};

/// 15 s for `ssh` to connect, its own `ConnectTimeout`, then 15 s more for
/// `ready`.
const READY_WAIT: Duration = Duration::from_secs(30);
/// The pairing's 2 minutes, from a valid `ready`.
const PAIRING_WAIT: Duration = Duration::from_secs(120);
/// Once the attempt is decided, how long the other side gets to say how it
/// ended, and `ssh` to exit.
const WIND_DOWN: Duration = Duration::from_secs(10);

/// What reaches the one loop of either side.
enum Input {
    /// A line from the other side: B's stdout on A, A's lines on B.
    Line(Result<Vec<u8>, ProtocolError>),
    /// The other side closed its end.
    LineEnd,
    /// An answer of this machine's daemon.
    Daemon(Value),
    /// The daemon ended its stream, with the error it sent, if any.
    DaemonEnd(Option<String>),
}

/// How a pairing ended, for a person or a program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub outcome: Outcome,
    pub machine_id: Option<String>,
    pub name: Option<String>,
    /// This machine's pin, as its daemon verified it.
    pub local: PinState,
    /// The other machine's pin, as it said.
    pub peer: PinState,
    pub detail: String,
}

impl Report {
    /// Refused or cancelled before anything could be written.
    pub fn refused(target: Option<&Target>, detail: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Cancelled,
            machine_id: target.map(|target| target.machine_id.clone()),
            name: target.map(|target| target.name.clone()),
            local: PinState::NotPinned,
            peer: PinState::NotPinned,
            detail: detail.into(),
        }
    }

    /// 0 paired on both sides, 2 nothing saved, 4 partial.
    pub fn exit_code(&self) -> i32 {
        exit_code(self.outcome)
    }
}

fn exit_code(outcome: Outcome) -> i32 {
    match outcome {
        Outcome::Paired => 0,
        Outcome::Cancelled => 2,
        Outcome::Partial => 4,
    }
}

/// Reads lines from the other side until it closes, as they come. After a
/// line that breaks the protocol, the rest is drained and dropped.
fn read_lines(input: impl Read + Send + 'static, tx: Sender<Input>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(input);
        let mut broken = false;
        loop {
            match ssh_pair::read_line(&mut reader) {
                Ok(Some(line)) if !broken => {
                    broken = line.is_err();
                    if tx.send(Input::Line(line)).is_err() {
                        return;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = tx.send(Input::LineEnd);
                    return;
                }
            }
        }
    });
}

/// Forwards the daemon's answers until it closes the stream.
fn read_daemon(mut stream: MgmtStream, tx: Sender<Input>) {
    thread::spawn(move || {
        loop {
            match stream.next_answer() {
                Ok(Some(answer)) => {
                    if tx.send(Input::Daemon(answer)).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(Input::DaemonEnd(None));
                    return;
                }
                Err(error) => {
                    let _ = tx.send(Input::DaemonEnd(Some(error.to_string())));
                    return;
                }
            }
        }
    });
}

/// `text`, or `fallback` when it is empty.
fn or_else(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

/// The time left until `deadline`.
fn left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// A pin state the daemon sent, `unknown` when it sent none this version
/// reads.
fn pin_state(value: &Value) -> PinState {
    serde_json::from_value(value.clone()).unwrap_or(PinState::Unknown)
}

/// The states and detail of a daemon's last pairing event: `paired`,
/// `cancelled` or `partial`.
fn ended(event: &Value) -> Option<(PinState, PinState, String)> {
    let message = event["message"].as_str().unwrap_or_default().to_string();
    match event["event"].as_str()? {
        "paired" => Some((PinState::Pinned, PinState::Pinned, String::new())),
        "cancelled" => Some((PinState::NotPinned, PinState::NotPinned, message)),
        "partial" => Some((
            pin_state(&event["local"]),
            pin_state(&event["peer"]),
            message,
        )),
        _ => None,
    }
}

// --- The accepting side --------------------------------------------------------

/// What A passed on B's command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperArgs {
    /// A's machine id: only its pairing request may consume the acceptor.
    pub from: String,
    /// The account A is signed in to.
    pub account: String,
}

/// The helper is for a program at the other end of a pipe. A terminal on
/// either end means a person ran it by mistake.
pub fn refuse_terminal(stdin_is_terminal: bool, stdout_is_terminal: bool) -> Result<()> {
    if stdin_is_terminal || stdout_is_terminal {
        bail!(
            "--for-ssh is what `{0} machines pair --over-ssh` starts on this machine, not something to run at a terminal. To pair by hand, run `{0} machines pair --accept`.",
            BRANDING.cli_command()
        );
    }
    Ok(())
}

/// `localify machines pair --accept --for-ssh`: registers a one-shot
/// acceptor with this machine's daemon, then speaks the line protocol on
/// stdin and stdout. A failure before `ready` writes nothing to stdout.
/// Returns the exit code.
pub fn accept_for_ssh(client: &MgmtSocketClient, args: &HelperArgs) -> Result<i32> {
    use std::io::IsTerminal;
    refuse_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;
    run_helper(client, args, io::stdin(), &mut io::stdout())
}

/// The helper once the terminal check passed: `input` is what A writes,
/// `out` carries the protocol's lines only.
pub fn run_helper(
    client: &MgmtSocketClient,
    args: &HelperArgs,
    input: impl Read + Send + 'static,
    out: &mut impl Write,
) -> Result<i32> {
    let command = json!({
        "cmd": "remote_pair_accept", "machineId": args.from, "account": args.account,
    });
    let mut stream = client
        .remote_stream(&command)
        .map_err(super::machines::daemon_error)?;
    let ready = stream
        .next_answer()
        .map_err(super::machines::daemon_error)?
        .ok_or_else(|| anyhow!("The daemon closed the connection before it was ready."))?;
    let ready = ready_line(&ready, args)?;
    let HelperLine::Ready { attempt, .. } = &ready else {
        bail!("The daemon's answer is not ready.");
    };
    let attempt = attempt.clone();
    let closer = stream.closer()?;
    write_line(out, &ready.to_line())?;
    let (tx, rx) = mpsc::channel();
    read_lines(input, tx.clone());
    read_daemon(stream, tx);
    let mut helper = Helper {
        client,
        closer,
        code: None,
        answered: false,
        accepted: false,
    };
    let (outcome, local, peer, detail) = helper.run(&rx, out, &attempt)?;
    let result = HelperLine::Result {
        attempt,
        outcome,
        local,
        peer,
        detail,
    };
    write_line(out, &result.to_line())?;
    Ok(exit_code(outcome))
}

/// The daemon's `ready`, checked against what A expects.
fn ready_line(ready: &Value, args: &HelperArgs) -> Result<HelperLine> {
    let field = |name: &str| ready[name].as_str().unwrap_or_default().to_string();
    if ready["event"] != "ready" {
        bail!("The daemon did not register the acceptor.");
    }
    let (machine_id, account) = (field("machineId"), field("account"));
    if account != args.account {
        bail!(
            "This machine is signed in to another Localify Cloud account than the one asked for."
        );
    }
    if machine_id.is_empty() || machine_id == args.from {
        bail!("This machine's daemon reports no machine id of its own, or the initiator's.");
    }
    Ok(HelperLine::Ready {
        machine_id,
        account,
        attempt: field("attempt"),
    })
}

fn write_line(out: &mut impl Write, line: &str) -> Result<()> {
    out.write_all(line.as_bytes())?;
    out.flush()?;
    Ok(())
}

struct Helper<'a> {
    client: &'a MgmtSocketClient,
    /// Closes the daemon stream: before the code, that cancels the acceptor.
    closer: UnixStream,
    /// The pairing id and the code, once the daemon showed it.
    code: Option<(String, String)>,
    /// A's confirm or cancel was acted on; later lines change nothing.
    answered: bool,
    /// This side sent its acceptance to the daemon.
    accepted: bool,
}

impl Helper<'_> {
    /// Until the daemon says how the pairing ended.
    fn run(
        &mut self,
        rx: &Receiver<Input>,
        out: &mut impl Write,
        attempt: &str,
    ) -> Result<(Outcome, PinState, PinState, String)> {
        // The daemon's 2 minutes decide; this only bounds a stuck daemon.
        let deadline = Instant::now() + PAIRING_WAIT + WIND_DOWN;
        loop {
            match rx.recv_timeout(left(deadline)) {
                Ok(Input::Daemon(event)) if event["event"] == "code" => {
                    let code = event["code"].as_str().unwrap_or_default().to_string();
                    let pairing_id = event["pairingId"].as_str().unwrap_or_default();
                    self.code = Some((pairing_id.to_string(), code.clone()));
                    let line = HelperLine::Code {
                        attempt: attempt.to_string(),
                        code,
                    };
                    write_line(out, &line.to_line())?;
                }
                Ok(Input::Daemon(event)) => {
                    if let Some((local, peer, detail)) = ended(&event) {
                        return Ok((Outcome::of(local, peer), local, peer, detail));
                    }
                }
                Ok(Input::DaemonEnd(error)) => return Ok(self.lost(error)),
                Ok(Input::Line(Ok(line))) => match ssh_pair::parse_control_line(&line) {
                    Ok(ControlLine::Confirm(code)) => self.confirm(&code),
                    Ok(ControlLine::Cancel) => self.cancel("The initiator cancelled."),
                    Err(error) => self.cancel(&format!("The initiator sent {error}.")),
                },
                Ok(Input::Line(Err(error))) => {
                    self.cancel(&format!("The initiator sent {error}."));
                }
                Ok(Input::LineEnd) => {
                    self.cancel("The initiator closed stdin before it confirmed.")
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    self.cancel("The daemon did not say how the pairing ended.");
                    return Ok(self.lost(None));
                }
            }
        }
    }

    /// Confirms only on this side's own code, exactly; anything else
    /// cancels.
    fn confirm(&mut self, code: &str) {
        let pairing_id = match &self.code {
            Some((pairing_id, mine)) if mine == code => pairing_id.clone(),
            Some(_) => return self.cancel("The initiator confirmed another code."),
            None => return self.cancel("The initiator confirmed before the code."),
        };
        if self.answered {
            return;
        }
        self.answered = true;
        // Even without a reply, the daemon may have taken it.
        self.accepted = true;
        if let Err(error) = self.client.remote_pair_answer(&pairing_id, true) {
            eprintln!("Could not confirm the pairing: {error}");
        }
    }

    fn cancel(&mut self, why: &str) {
        if self.answered {
            return;
        }
        self.answered = true;
        eprintln!("{why}");
        match &self.code {
            Some((pairing_id, _)) => {
                if let Err(error) = self.client.remote_pair_answer(pairing_id, false) {
                    eprintln!("Could not cancel the pairing: {error}");
                }
            }
            // Closing the stream cancels the acceptor.
            None => {
                let _ = self.closer.shutdown(Shutdown::Both);
            }
        }
    }

    /// The daemon's stream ended without a last event.
    fn lost(&self, error: Option<String>) -> (Outcome, PinState, PinState, String) {
        let detail = error.unwrap_or_else(|| "The pairing ended before it started.".into());
        if self.accepted {
            (
                Outcome::Partial,
                PinState::Unknown,
                PinState::Unknown,
                detail,
            )
        } else {
            // This side never accepted, so neither side could write a pin.
            (
                Outcome::Cancelled,
                PinState::NotPinned,
                PinState::NotPinned,
                detail,
            )
        }
    }
}

// --- The initiating side -------------------------------------------------------

/// The machine to pair with, and the account both must share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub machine_id: String,
    pub name: String,
    /// This machine's id: the only one B's acceptor takes a request from.
    pub own_id: String,
    /// This machine's account: B's daemon must report the same.
    pub account: String,
}

/// The other end: B's helper through `ssh`, or in tests, a helper run
/// here on pipes.
pub struct Peer {
    stdin: Option<Box<dyn Write + Send>>,
    child: Option<Child>,
    tx: Sender<Input>,
    rx: Receiver<Input>,
}

impl Peer {
    /// Runs `command` directly, without a local shell, with all three
    /// streams piped: `ssh`, or in tests the helper itself.
    pub fn spawn(mut command: Command, name: &str) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("ssh started without its pipes"));
        };
        let mut peer = Self::from_pipes(stdin, stdout, stderr, name);
        peer.child = Some(child);
        Ok(peer)
    }

    /// The other end on these pipes. Its stdout and stderr are drained at
    /// once, so neither can block it.
    pub fn from_pipes(
        stdin: impl Write + Send + 'static,
        stdout: impl Read + Send + 'static,
        stderr: impl Read + Send + 'static,
        name: &str,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        read_lines(stdout, tx.clone());
        log_diagnostics(stderr, name.to_string());
        Self {
            stdin: Some(Box::new(stdin)),
            child: None,
            tx,
            rx,
        }
    }

    fn send(&mut self, line: &ControlLine) -> bool {
        let Some(stdin) = &mut self.stdin else {
            return false;
        };
        stdin.write_all(line.to_line().as_bytes()).is_ok() && stdin.flush().is_ok()
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Waits up to `wait` for the child to exit, then kills it; reaps it
    /// either way.
    fn reap(&mut self, wait: Duration) -> Option<ExitStatus> {
        let mut child = self.child.take()?;
        let deadline = Instant::now() + wait;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    return child.wait().ok();
                }
            }
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.close_stdin();
        self.reap(Duration::ZERO);
    }
}

/// B's stderr goes to this machine's, line by line, without control
/// characters; it is never parsed.
fn log_diagnostics(stderr: impl Read + Send + 'static, name: String) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut chunk = Vec::new();
        loop {
            chunk.clear();
            let read = (&mut reader)
                .take(ssh_pair::MAX_LINE as u64)
                .read_until(b'\n', &mut chunk);
            match read {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let text: String = String::from_utf8_lossy(&chunk)
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\t')
                        .collect();
                    eprintln!("{name}: {text}");
                }
            }
        }
    });
}

/// Runs a pairing with the helper on `peer`, from its `ready` on, through
/// this machine's daemon. `started` is when `ssh` started.
pub fn drive(own: &MgmtSocketClient, target: &Target, peer: Peer, started: Instant) -> Report {
    let mut run = Run {
        own,
        target,
        peer,
        lines: HelperLines::default(),
        ours: None,
        theirs: None,
        decided: None,
        own_closer: None,
        own_end: None,
        own_closed: false,
        peer_result: None,
        peer_done: false,
        failure: None,
    };
    let attempt = match run.await_ready(started + READY_WAIT) {
        Ok(attempt) => attempt,
        Err(why) => {
            run.peer.send(&ControlLine::Cancel);
            run.peer.close_stdin();
            run.peer.reap(Duration::from_secs(2));
            return Report::refused(Some(target), why);
        }
    };
    run.pair(&attempt);
    run.wind_down();
    run.report()
}

struct Run<'a> {
    own: &'a MgmtSocketClient,
    target: &'a Target,
    peer: Peer,
    lines: HelperLines,
    /// This machine's pairing id and code, once its daemon showed them.
    ours: Option<(String, String)>,
    /// B's code.
    theirs: Option<String>,
    /// `Some(true)` once both sides were told to confirm, `Some(false)`
    /// once they were told to cancel.
    decided: Option<bool>,
    own_closer: Option<UnixStream>,
    /// This machine's last pairing event.
    own_end: Option<Value>,
    own_closed: bool,
    /// B's `result`: outcome, its own pin, its view of this machine's.
    peer_result: Option<(PinState, String)>,
    /// B said how it ended, closed its stdout, or broke the protocol.
    peer_done: bool,
    /// Why the attempt ended early, for the report.
    failure: Option<String>,
}

impl Run<'_> {
    /// The first line must be `ready`, for the machine and account
    /// expected. Returns its attempt.
    fn await_ready(&mut self, deadline: Instant) -> Result<String, String> {
        let name = self.target.name.clone();
        loop {
            match self.peer.rx.recv_timeout(left(deadline)) {
                Ok(Input::Line(Ok(line))) => {
                    return match self.lines.take(&line) {
                        Ok(HelperLine::Ready {
                            machine_id,
                            account,
                            attempt,
                        }) => self.check_ready(&machine_id, &account).map(|()| attempt),
                        Ok(_) => Err(format!("{name} did not start with ready.")),
                        Err(error) => Err(format!(
                            "{name} sent a line this version cannot take: {error}."
                        )),
                    };
                }
                Ok(Input::Line(Err(error))) => {
                    return Err(format!(
                        "{name} sent a line this version cannot take: {error}."
                    ));
                }
                Ok(Input::LineEnd) => return Err(self.ended_early()),
                Ok(Input::Daemon(_) | Input::DaemonEnd(_)) => {}
                Err(_) => {
                    return Err(format!(
                        "{name} was not ready within {} s: ssh could not connect, or Localify there did not answer.",
                        READY_WAIT.as_secs()
                    ));
                }
            }
        }
    }

    fn check_ready(&self, machine_id: &str, account: &str) -> Result<(), String> {
        let target = self.target;
        if machine_id != target.machine_id {
            return Err(format!(
                "The machine reached over SSH is {machine_id}, not {} ({}). Check the destination.",
                target.name, target.machine_id
            ));
        }
        if account != target.account {
            return Err(format!(
                "{} is signed in to another Localify Cloud account than this machine.",
                target.name
            ));
        }
        Ok(())
    }

    /// B's stdout closed before `ready`: say why, from its exit status.
    fn ended_early(&mut self) -> String {
        let status = self.peer.reap(Duration::from_secs(2));
        let name = &self.target.name;
        match status.and_then(|status| status.code()) {
            Some(127) => format!(
                "Localify was not found on {name} by the non-interactive shell ssh runs. Pass its absolute path with --remote-localify."
            ),
            Some(255) => format!(
                "ssh could not reach {name}; its message is above. Its host key must already be trusted, and no password prompt is possible."
            ),
            Some(code) => format!(
                "The accepting side on {name} stopped before it was ready (exit {code}); its message is above."
            ),
            None => format!("The accepting side on {name} stopped before it was ready."),
        }
    }

    /// From a valid `ready` to both sides' ends, within 2 minutes.
    fn pair(&mut self, attempt: &str) {
        let deadline = Instant::now() + PAIRING_WAIT;
        let command = json!({
            "cmd": "remote_pair", "machineId": self.target.machine_id, "attempt": attempt,
        });
        match self.own.remote_stream(&command) {
            Ok(stream) => {
                self.own_closer = stream.closer().ok();
                read_daemon(stream, self.peer.tx.clone());
            }
            Err(error) => {
                self.own_closed = true;
                self.abandon(format!(
                    "This machine's daemon could not start the pairing: {error}"
                ));
            }
        }
        while !(self.own_done() && self.peer_done) {
            match self.peer.rx.recv_timeout(left(deadline)) {
                Ok(input) => self.take(input),
                Err(_) => {
                    let name = &self.target.name;
                    self.abandon(format!(
                        "The pairing with {name} did not finish within 2 minutes."
                    ));
                    return;
                }
            }
        }
    }

    fn own_done(&self) -> bool {
        self.own_end.is_some() || self.own_closed
    }

    fn take(&mut self, input: Input) {
        match input {
            Input::Daemon(event) if event["event"] == "code" => {
                let code = event["code"].as_str().unwrap_or_default().to_string();
                let pairing_id = event["pairingId"].as_str().unwrap_or_default().to_string();
                self.ours = Some((pairing_id, code));
                self.decide();
            }
            Input::Daemon(event) => {
                if let Some((_, _, detail)) = ended(&event) {
                    self.own_end = Some(event);
                    self.abandon(or_else(
                        detail,
                        "This machine's side of the pairing ended first.",
                    ));
                }
            }
            Input::DaemonEnd(error) => {
                self.own_closed = true;
                let why =
                    error.unwrap_or_else(|| "This machine's daemon ended the pairing.".into());
                self.abandon(why);
            }
            Input::Line(Ok(line)) => self.take_line(&line),
            Input::Line(Err(error)) => self.broken(&error),
            Input::LineEnd => {
                self.peer_done = true;
                let name = &self.target.name;
                self.abandon(format!(
                    "The accepting side on {name} stopped before it answered."
                ));
            }
        }
    }

    fn take_line(&mut self, line: &[u8]) {
        match self.lines.take(line) {
            Ok(HelperLine::Code { code, .. }) => {
                self.theirs = Some(code);
                self.decide();
            }
            Ok(HelperLine::Result { local, detail, .. }) => {
                let name = &self.target.name;
                let why = or_else(
                    detail.clone(),
                    &format!("{name}'s side of the pairing ended first."),
                );
                self.peer_result = Some((local, detail));
                self.peer_done = true;
                self.abandon(why);
            }
            Ok(HelperLine::Ready { .. }) => {}
            Err(error) => self.broken(&error),
        }
    }

    /// A line that breaks the protocol ends the attempt: nothing more from
    /// B is trusted.
    fn broken(&mut self, error: &ProtocolError) {
        self.peer_done = true;
        let name = &self.target.name;
        self.abandon(format!(
            "{name} sent a line this version cannot take: {error}."
        ));
        self.peer.close_stdin();
    }

    /// Once both codes are in: confirm both sides on a match, cancel both
    /// otherwise.
    fn decide(&mut self) {
        let (Some((pairing_id, ours)), Some(theirs), None) =
            (&self.ours, &self.theirs, self.decided)
        else {
            return;
        };
        let pairing_id = pairing_id.clone();
        if ours != theirs {
            let message = format!(
                "The codes differ: {} shows {theirs}, this machine {ours}. Something between the machines changed the pairing.",
                self.target.name
            );
            return self.abandon(message);
        }
        let code = ours.clone();
        if !self.peer.send(&ControlLine::Confirm(code)) {
            let name = &self.target.name;
            return self.abandon(format!("Could not confirm on {name}: its stdin closed."));
        }
        self.decided = Some(true);
        if let Err(error) = self.own.remote_pair_answer(&pairing_id, true) {
            self.failure
                .get_or_insert(format!("Could not confirm on this machine: {error}"));
        }
    }

    /// Cancels both sides, unless they were already told to confirm or
    /// cancel.
    fn abandon(&mut self, why: String) {
        if self.decided.is_some() {
            return;
        }
        self.decided = Some(false);
        self.failure.get_or_insert(why);
        self.peer.send(&ControlLine::Cancel);
        match &self.ours {
            Some((pairing_id, _)) if !self.own_done() => {
                let _ = self.own.remote_pair_answer(pairing_id, false);
            }
            // Before the code, closing the stream cancels the pairing.
            _ => self.close_own(),
        }
    }

    fn close_own(&mut self) {
        if let Some(closer) = self.own_closer.take() {
            let _ = closer.shutdown(Shutdown::Both);
        }
    }

    /// Both sides get a little time to say how they ended; then B's stdin
    /// closes, and `ssh` is reaped.
    fn wind_down(&mut self) {
        let deadline = Instant::now() + WIND_DOWN;
        while !(self.own_done() && self.peer_done) {
            match self.peer.rx.recv_timeout(left(deadline)) {
                Ok(Input::Daemon(event)) if ended(&event).is_some() => self.own_end = Some(event),
                Ok(Input::DaemonEnd(_)) => self.own_closed = true,
                Ok(Input::Line(Ok(line))) if !self.peer_done => self.take_line(&line),
                Ok(Input::Line(Err(_)) | Input::LineEnd) => self.peer_done = true,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        self.close_own();
        self.peer.close_stdin();
        self.peer.reap(WIND_DOWN);
    }

    /// Both sides' states: this one's from its daemon, B's from its own
    /// `result`, else from what this daemon heard.
    fn report(&self) -> Report {
        let accepted = self.decided == Some(true);
        let (local, heard, own_detail) = match self.own_end.as_ref().and_then(ended) {
            Some(states) => states,
            None if accepted => (PinState::Unknown, PinState::Unknown, String::new()),
            None => (PinState::NotPinned, PinState::NotPinned, String::new()),
        };
        // B accepts only on this machine's confirm line.
        let (peer, peer_detail) = match &self.peer_result {
            Some((state, detail)) => (*state, detail.clone()),
            None if accepted => (heard, String::new()),
            None => (PinState::NotPinned, String::new()),
        };
        let outcome = Outcome::of(local, peer);
        let nonempty = |detail: String| Some(detail).filter(|detail| !detail.is_empty());
        let detail = match outcome {
            Outcome::Paired => None,
            // Why it stopped; the states already say what each side saved.
            Outcome::Partial => self.failure.clone().or_else(|| {
                let reason = self.own_end.as_ref()?["reason"].as_str()?;
                Some(format!("This machine's side ended with {reason}."))
            }),
            Outcome::Cancelled => self
                .failure
                .clone()
                .filter(|_| !accepted)
                .or_else(|| nonempty(own_detail))
                .or_else(|| nonempty(peer_detail)),
        }
        .unwrap_or_default();
        Report {
            outcome,
            machine_id: Some(self.target.machine_id.clone()),
            name: Some(self.target.name.clone()),
            local,
            peer,
            detail,
        }
    }
}
