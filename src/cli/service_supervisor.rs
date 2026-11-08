//! Runs every service `localify dev` starts, with or without a launch profile,
//! until it exits.
//!
//! It refreshes the run's heartbeat, copies a background run's output into its
//! stream, masks Proton Pass and Infisical values in the output, stops the
//! service when the profile's `max_duration` elapses, keeps the daemon session
//! alive, and survives Ctrl+C long enough to report why the service stopped.
//! When `localify dev` leads the run's session, it also ends whatever the
//! service left in it before returning.

use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use localify::infrastructure::launch_sessions::{
    LAUNCH_SESSION_HEARTBEAT, LaunchSessionDto, LaunchSessionUpdate,
};
use localify::infrastructure::mgmt_client::MgmtSocketClient;

use crate::branding::BRANDING;
use crate::domain::launch_profile::LaunchEnvironment;
use crate::infrastructure::output_masking::spawn_masked_copy;
use crate::infrastructure::process::{
    Identity, ProcessInfo, ProcessTable, Signal, all_processes, helpers, signal_identities,
    terminal_foreground_group, track_tree,
};
use crate::infrastructure::runs::finish::{Finisher, KILL_WAIT, STOP_GRACE};
use crate::infrastructure::runs::output::StreamSink;

/// How often the service's exit is checked: a cheap wait on the child.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// How often the service's descendants are looked up, scanning every
/// process, while the run is healthy. A descendant that forks and leaves the
/// session between two looks is not known to a later stop.
const SCAN_WHILE_HEALTHY: Duration = Duration::from_secs(2);
/// How often they are looked up during a stop or a session cleanup.
const SCAN_WHILE_STOPPING: Duration = POLL_INTERVAL;
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the run's record says its executor is alive.
pub const RUN_HEARTBEAT: Duration = Duration::from_secs(5);

/// A write to the terminal failed as only a terminal nobody holds any more
/// fails: in a run PTY, the app is gone.
static TERMINAL_LOST: AtomicBool = AtomicBool::new(false);

/// Writes `text` to stderr, never panicking. In a run PTY whose app died,
/// every write fails, and `eprintln!` would abort `localify dev` before it
/// ended its service. A failure that says the terminal is gone is kept, so a
/// run in the app's terminal stops.
pub fn report(text: &str) {
    let mut stderr = io::stderr().lock();
    if let Err(error) = stderr
        .write_all(text.as_bytes())
        .and_then(|()| stderr.flush())
        && matches!(
            error.raw_os_error(),
            Some(libc::EIO | libc::EPIPE | libc::ENXIO)
        )
    {
        TERMINAL_LOST.store(true, Ordering::SeqCst);
    }
}

pub struct SupervisedLaunch<'a> {
    pub command: &'a [String],
    pub workdir: &'a Path,
    pub environment: &'a LaunchEnvironment,
    /// Secret byte patterns; `None` leaves the output as the service wrote it.
    pub mask_patterns: Option<Vec<Vec<u8>>>,
    /// The run's stream for a background run; `None` keeps the output in the
    /// terminal `localify dev` runs in.
    pub stream: Option<StreamSink>,
    pub max_duration: Option<Duration>,
    pub session: Option<SessionReporter>,
    /// Called right after the service starts, then every `heartbeat_every`
    /// ([`RUN_HEARTBEAT`] for a run).
    pub heartbeat: &'a dyn Fn(),
    pub heartbeat_every: Duration,
    /// `localify dev` leads the run's session: what the service leaves in it
    /// is ended before the run returns, by `finisher`.
    pub leads_session: bool,
    /// The terminal is the app's run PTY. Losing it, a SIGHUP or a write that
    /// fails as on a closed terminal, means the app is gone: the run stops,
    /// as for a SIGTERM. In any other terminal a SIGHUP is only forwarded.
    pub app_terminal: bool,
    /// Stop signals already watched, since before the profile resolved
    /// (`localify dev`); `None` starts watching with the service.
    pub supervision: Option<Supervision>,
    pub finisher: &'a Finisher<'a>,
}

/// The watch of the stop signals sent to `localify dev`, from before its
/// profile resolves until it exits. While the profile resolves there is no
/// service to stop: a Ctrl+C, SIGTERM or SIGHUP then ends the helpers the
/// profile runs, such as `infisical`, and what they started, and exits.
#[derive(Clone)]
pub struct Supervision {
    stops: Arc<StopRequests>,
    forwarding: Arc<Mutex<Forwarding>>,
}

impl Supervision {
    /// Watches from now on, in the resolving phase.
    pub fn start(app_terminal: bool) -> Self {
        let supervision = Self {
            stops: Arc::new(StopRequests::default()),
            forwarding: Arc::new(Mutex::new(Forwarding {
                resolving: true,
                ..Forwarding::default()
            })),
        };
        watch_stop_signals(
            Arc::clone(&supervision.forwarding),
            Arc::clone(&supervision.stops),
            app_terminal,
        );
        supervision
    }

    /// The profile resolved: stop signals now wait for the service, and
    /// reach it once it runs. When a stop already began ending the
    /// resolution, nothing may start: this waits for that stop to exit.
    pub fn resolved(&self) {
        let resolved = self
            .forwarding
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .finish_resolution();
        if !resolved {
            loop {
                thread::park();
            }
        }
    }
}

/// A stop while the profile resolves: its helpers are ended, and so is
/// `localify dev`, with the shell's code for that signal.
fn end_resolution(code: i32) -> ! {
    report("\nStopped while the launch profile resolved.\n");
    helpers::end_all(STOP_GRACE, KILL_WAIT);
    std::process::exit(code);
}

pub enum LaunchOutcome {
    Exited(ExitStatus),
    /// The user pressed Ctrl+C or the process was asked to stop.
    Interrupted,
    /// `max_duration` elapsed and Localify stopped the service.
    Expired(Duration),
}

impl LaunchOutcome {
    fn reason(&self) -> String {
        match self {
            Self::Exited(status) => describe_exit(*status),
            Self::Interrupted => "interrupted".into(),
            Self::Expired(duration) => format!(
                "max_duration of {} reached",
                humantime::format_duration(*duration)
            ),
        }
    }

    pub fn code(&self) -> Option<i32> {
        match self {
            Self::Exited(status) => status.code(),
            Self::Interrupted | Self::Expired(_) => None,
        }
    }

    /// What `localify dev` exits with, as a shell reports a job: the service's
    /// own code, 128 + N for signal N, and 130 after Ctrl+C.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Exited(status) => status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
            Self::Interrupted => 130,
            Self::Expired(_) => 1,
        }
    }
}

/// How the run ended, once its output is all in.
pub struct LaunchEnd {
    pub outcome: LaunchOutcome,
    /// How the copy of the output ended. Says nothing about the run's
    /// processes.
    pub capture: Capture,
    /// Why processes of the run's session are still alive. The run must then
    /// stay unfinalized, so its next launch is refused.
    pub cleanup: Result<()>,
}

/// How the copy of a service's output ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// Every copy reached the end of its pipe.
    Complete,
    /// A process that left the run still holds a pipe open.
    Unfinished,
    /// Writing failed (a full disk); what came after was read and dropped.
    Failed(String),
}

/// Requests from outside the service: each Ctrl+C, and a SIGTERM or SIGHUP
/// sent to `localify dev` itself.
#[derive(Default)]
pub struct StopRequests {
    pub interrupts: AtomicUsize,
    /// A SIGTERM was forwarded: a stop, as `localify stop` asks for one.
    pub terminated: AtomicBool,
    /// A SIGHUP was forwarded, and nothing more: a service may trap it to
    /// reload, or ignore a terminal that closed.
    pub hung_up: AtomicBool,
    /// Held by a test to keep supervision from polling.
    #[cfg(test)]
    pub poll_gate: std::sync::Mutex<()>,
}

impl StopRequests {
    fn presses(&self) -> usize {
        self.interrupts.load(Ordering::SeqCst)
    }

    /// Whether a stop was asked for, which the run then stops on.
    fn stopping(&self) -> bool {
        self.presses() > 0 || self.terminated.load(Ordering::SeqCst)
    }

    /// Whether anything from outside ended the service.
    fn any(&self) -> bool {
        self.stopping() || self.hung_up.load(Ordering::SeqCst)
    }

    /// The app's run PTY is gone: a stop, as `localify stop` asks for one,
    /// once.
    fn lose_terminal(&self, stopping: &Stopping<'_>) {
        if !self.terminated.swap(true, Ordering::SeqCst) {
            stopping.signal(&stopping.stop_targets(), Signal::Terminate);
        }
    }
}

/// Starts the service and waits for it, returning why it stopped.
pub fn run(launch: SupervisedLaunch<'_>) -> Result<LaunchEnd> {
    // Watching first: a Ctrl+C right after the service starts must not end
    // `localify dev` before it can stop the service.
    let supervision = match &launch.supervision {
        Some(supervision) => supervision.clone(),
        None => Supervision::start(launch.app_terminal),
    };
    supervision.resolved();
    run_with(launch, supervision.stops, supervision.forwarding)
}

/// `run`, with stop requests a test can make without signalling itself.
fn run_with(
    launch: SupervisedLaunch<'_>,
    stops: Arc<StopRequests>,
    forwarding: Arc<Mutex<Forwarding>>,
) -> Result<LaunchEnd> {
    let mut child = spawn(&launch)?;
    // A service that already exited is a zombie no table shows: nothing to signal.
    let root = launch
        .finisher
        .table
        .process(child.id())
        .map(|process| Identity::of(&process))
        .unwrap_or(Identity {
            pid: child.id(),
            start: 0,
        });
    crate::infrastructure::test_pause::pause_at("before-identity");
    forwarding
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .publish(root);
    (launch.heartbeat)();
    let output_done = copy_output(&mut child, &launch);

    let (outcome, cleanup) = supervise(&mut child, root, &launch, &stops);
    if cleanup.is_err() {
        // Every survivor started before this heartbeat, so the next launcher
        // proves it belongs to the run, and refuses.
        (launch.heartbeat)();
    }
    let capture = drain_output(output_done, launch.stream.as_ref());

    let outcome = outcome?;
    if let Some(session) = &launch.session
        && let Err(error) = session.stop(&outcome.reason())
    {
        report(&format!(
            "Warning: could not report the stop to the daemon: {error:#}\n"
        ));
    }
    Ok(LaunchEnd {
        outcome,
        capture,
        cleanup,
    })
}

fn spawn(launch: &SupervisedLaunch<'_>) -> Result<Child> {
    let program = launch
        .command
        .first()
        .context("The service command is empty")?;
    let mut command = Command::new(program);
    command
        .args(&launch.command[1..])
        .current_dir(launch.workdir)
        .stdin(Stdio::inherit());
    for key in &launch.environment.unset {
        command.env_remove(key);
    }
    command.envs(
        launch
            .environment
            .set
            .iter()
            .map(|(key, value)| (key, value)),
    );

    if launch.mask_patterns.is_some() || launch.stream.is_some() {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    command
        .spawn()
        .with_context(|| format!("Failed to start command '{program}'"))
}

/// Copies piped output into the run's stream, or back to the terminal masked.
/// The receiver gets the first copy's failure, once both copies ended.
fn copy_output(
    child: &mut Child,
    launch: &SupervisedLaunch<'_>,
) -> Option<mpsc::Receiver<Result<(), String>>> {
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return None;
    };
    let patterns = launch.mask_patterns.clone().unwrap_or_default();
    let copies: [JoinHandle<io::Result<()>>; 2] = match &launch.stream {
        Some(stream) => [
            spawn_masked_copy(stdout, stream.clone(), patterns.clone()),
            spawn_masked_copy(stderr, stream.clone(), patterns),
        ],
        None => [
            spawn_masked_copy(stdout, io::stdout(), patterns.clone()),
            spawn_masked_copy(stderr, io::stderr(), patterns),
        ],
    };
    let (done_sender, done_receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut result = Ok(());
        for copy in copies {
            let copied = match copy.join() {
                Ok(copied) => copied.map_err(|error| error.to_string()),
                Err(_) => Err("the output copy stopped unexpectedly".to_string()),
            };
            result = result.and(copied);
        }
        let _ = done_sender.send(result);
    });
    Some(done_receiver)
}

/// Waits for the remaining output, but not forever: a process that left the
/// run's session may keep the pipe open. A write that already failed is
/// reported even then.
fn drain_output(
    done: Option<mpsc::Receiver<Result<(), String>>>,
    stream: Option<&StreamSink>,
) -> Capture {
    let Some(done) = done else {
        return Capture::Complete;
    };
    match done.recv_timeout(OUTPUT_DRAIN_TIMEOUT) {
        Ok(Ok(())) => Capture::Complete,
        Ok(Err(error)) => Capture::Failed(error),
        Err(_) => match stream.and_then(StreamSink::failure) {
            Some(error) => Capture::Failed(error),
            None => Capture::Unfinished,
        },
    }
}

/// Where a service stands in being stopped by `localify dev`, from its start
/// until every process it is after is gone, the service's own exit included.
struct Stopping<'a> {
    table: &'a dyn ProcessTable,
    grace: Duration,
    /// The service, as it started.
    root: Identity,
    /// `localify dev`'s PID, when it leads the run's session.
    leader: Option<u32>,
    /// The service and its descendants, kept by identity all along, so one
    /// that outlives its parent is still known.
    tracked: Vec<Identity>,
    /// The other members of the session `localify dev` leads.
    members: Vec<Identity>,
    expiring_since: Option<Instant>,
    interrupted_since: Option<Instant>,
    /// A SIGTERM sent to `localify dev` was forwarded to the service.
    terminated_since: Option<Instant>,
    /// The service exited by itself, and its session is being emptied.
    cleaning_since: Option<Instant>,
    terminated: bool,
}

impl<'a> Stopping<'a> {
    fn new(launch: &SupervisedLaunch<'a>, root: Identity) -> Self {
        Self {
            table: launch.finisher.table,
            grace: launch.finisher.grace,
            root,
            leader: launch.leads_session.then(std::process::id),
            tracked: Vec::new(),
            members: Vec::new(),
            expiring_since: None,
            interrupted_since: None,
            terminated_since: None,
            cleaning_since: None,
            terminated: false,
        }
    }

    fn track(&mut self) -> Vec<ProcessInfo> {
        let scan = self.table.processes();
        let mut roots = self.tracked.clone();
        if !roots.contains(&self.root) {
            roots.push(self.root);
        }
        self.tracked = track_tree(&roots, &scan);
        if let Some(leader) = self.leader {
            self.members = scan
                .iter()
                .filter(|process| process.sid == Some(leader) && process.pid != leader)
                .map(Identity::of)
                .collect();
        }
        scan
    }

    fn stopping(&self) -> bool {
        self.expiring_since.is_some()
            || self.interrupted_since.is_some()
            || self.terminated_since.is_some()
    }

    /// A stop is after the service's tree and the session; a cleanup after a
    /// normal exit, after the session only.
    fn targets(&self) -> Vec<Identity> {
        let mut targets = Vec::new();
        if self.stopping() {
            targets.extend(self.tracked.iter().copied());
        }
        if self.stopping() || self.cleaning_since.is_some() {
            for member in &self.members {
                if !targets.contains(member) {
                    targets.push(*member);
                }
            }
        }
        targets
    }

    /// What a stop is after: the service's tree and the session's members.
    fn stop_targets(&self) -> Vec<Identity> {
        self.targets_on_interrupt()
    }

    fn signal(&self, targets: &[Identity], signal: Signal) {
        for target in targets {
            let unchanged = self
                .table
                .process(target.pid)
                .is_some_and(|now| now.start == target.start);
            if unchanged {
                self.table.signal(target.pid, signal);
            }
        }
    }

    /// The first Ctrl+C: SIGINT to whoever no terminal reached (see
    /// [`interrupt_targets`]).
    fn interrupt(&mut self, now: Instant, scan: &[ProcessInfo]) {
        let terminal_group = terminal_foreground_group();
        let targets = interrupt_targets(&self.targets_on_interrupt(), scan, terminal_group);
        self.signal(&targets, Signal::Interrupt);
        self.interrupted_since = Some(now);
    }

    fn targets_on_interrupt(&self) -> Vec<Identity> {
        let mut targets = self.tracked.clone();
        targets.extend(
            self.members
                .iter()
                .filter(|member| !self.tracked.contains(member)),
        );
        targets
    }

    /// SIGTERM after the grace period and SIGKILL after twice that, counted
    /// from the first Ctrl+C or from `max_duration`; a second Ctrl+C kills at
    /// once. A cleanup sends SIGTERM at once and SIGKILL after the grace.
    fn escalate(&mut self, now: Instant, presses: usize) {
        let targets = self.targets();
        let kill = match (
            self.interrupted_since,
            self.expiring_since.or(self.terminated_since),
            self.cleaning_since,
        ) {
            (Some(since), _, _) => presses >= 2 || now >= since + self.grace * 2,
            (None, Some(since), _) => now >= since + self.grace,
            (None, None, Some(since)) => now >= since + self.grace,
            (None, None, None) => false,
        };
        let terminate = match (self.interrupted_since, self.cleaning_since) {
            (Some(since), _) => now >= since + self.grace,
            (None, Some(_)) => true,
            (None, None) => false,
        };
        if kill {
            self.signal(&targets, Signal::Kill);
        } else if terminate && !self.terminated {
            self.signal(&targets, Signal::Terminate);
            self.terminated = true;
        }
    }

    /// When SIGKILL is due, for a stop or a cleanup in progress.
    fn kill_due(&self) -> Option<Instant> {
        match (
            self.interrupted_since,
            self.expiring_since.or(self.terminated_since),
            self.cleaning_since,
        ) {
            (Some(since), _, _) => Some(since + self.grace * 2),
            (None, Some(since), _) | (None, None, Some(since)) => Some(since + self.grace),
            (None, None, None) => None,
        }
    }
}

/// Who gets a forwarded SIGINT: every process outside the foreground group
/// of `localify dev`'s controlling terminal, which the terminal already
/// reached with the Ctrl+C, whoever leads the session and wherever a shell
/// moved that group. Without a controlling terminal (a detached run), every
/// process gets one. Where the SIGINT came from is not asked: on macOS a
/// terminal's Ctrl+C names a sender, as a `kill` does. A `kill -INT` of
/// `localify dev` in a terminal therefore reaches the foreground group only
/// through the SIGTERM and SIGKILL that follow.
fn interrupt_targets(
    targets: &[Identity],
    scan: &[ProcessInfo],
    terminal_group: Option<u32>,
) -> Vec<Identity> {
    targets
        .iter()
        .filter(|target| {
            let group = scan
                .iter()
                .find(|process| Identity::of(process) == **target)
                .map(|process| process.pgid);
            terminal_group.is_none() || group != terminal_group
        })
        .copied()
        .collect()
}

/// When the run's and the daemon session's heartbeats are next due.
struct Heartbeats {
    run: Instant,
    session: Instant,
}

impl Heartbeats {
    fn beat(&mut self, now: Instant, launch: &SupervisedLaunch<'_>) {
        if now >= self.run {
            (launch.heartbeat)();
            self.run = now + launch.heartbeat_every;
        }
        if now >= self.session {
            if let Some(session) = &launch.session {
                let _ = session.heartbeat();
            }
            self.session = now + LAUNCH_SESSION_HEARTBEAT;
        }
    }
}

/// Waits for the service, stopping it on Ctrl+C or at `max_duration`, then
/// goes on until every process the stop or the session cleanup is after is
/// gone, on the same clock. Returns why the service stopped, and whether
/// anything it left is still alive.
fn supervise(
    child: &mut Child,
    root: Identity,
    launch: &SupervisedLaunch<'_>,
    stops: &StopRequests,
) -> (Result<LaunchOutcome>, Result<()>) {
    let started = Instant::now();
    let mut beats = Heartbeats {
        run: started + launch.heartbeat_every,
        session: started + LAUNCH_SESSION_HEARTBEAT,
    };
    let mut stopping = Stopping::new(launch, root);
    let mut next_scan = started;
    let mut scan = Vec::new();
    let status = loop {
        #[cfg(test)]
        drop(stops.poll_gate.lock());
        let now = Instant::now();
        let expiring = launch
            .max_duration
            .is_some_and(|duration| now.duration_since(started) >= duration);
        // Every process is scanned only as often as the run's state needs.
        let busy = stopping.stopping() || expiring || stops.stopping();
        if busy || now >= next_scan {
            scan = stopping.track();
            next_scan = now
                + if busy {
                    SCAN_WHILE_STOPPING
                } else {
                    SCAN_WHILE_HEALTHY
                };
        }
        if let Some(duration) = launch.max_duration
            && stopping.expiring_since.is_none()
            && now.duration_since(started) >= duration
        {
            report(&format!(
                "\n{}: the launch profile reached its max_duration of {}. Stopping the service.\n",
                BRANDING.app_name,
                humantime::format_duration(duration)
            ));
            stopping.expiring_since = Some(now);
            stopping.signal(&stopping.targets(), Signal::Terminate);
        }
        if launch.app_terminal && TERMINAL_LOST.load(Ordering::SeqCst) {
            stops.lose_terminal(&stopping);
        }
        // Before the exit: a service a Ctrl+C just ended leaves its stop to run.
        respond_to_stop_requests(&mut stopping, stops, now, &scan);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                return (Err(error).context("Failed to wait for the service"), Ok(()));
            }
        }
        beats.beat(now, launch);
        thread::sleep(POLL_INTERVAL);
    };

    if launch.leads_session && !stopping.stopping() {
        stopping.cleaning_since = Some(Instant::now());
    }
    // The rest is a stop or a cleanup: every SCAN_WHILE_STOPPING.
    let cleanup = loop {
        let scan = stopping.track();
        let now = Instant::now();
        respond_to_stop_requests(&mut stopping, stops, now, &scan);
        let left = stopping.targets();
        if left.is_empty() {
            break Ok(());
        }
        if stopping
            .kill_due()
            .is_some_and(|due| now >= due + KILL_WAIT)
        {
            let left: Vec<String> = left.iter().map(|target| target.pid.to_string()).collect();
            let left = left.join(" ");
            break Err(anyhow::anyhow!(
                "processes of the run are still alive after SIGKILL: {left}. Stop them with: kill -9 {left}"
            ));
        }
        beats.beat(now, launch);
        thread::sleep(SCAN_WHILE_STOPPING);
    };

    let outcome = match (stopping.expiring_since, launch.max_duration) {
        (Some(_), Some(duration)) => LaunchOutcome::Expired(duration),
        _ if stops.any() => LaunchOutcome::Interrupted,
        _ => LaunchOutcome::Exited(status),
    };
    (Ok(outcome), cleanup)
}

/// Turns pending stop requests into a stop: a Ctrl+C, or a SIGTERM already
/// forwarded to the service, whose survivors get SIGKILL after the grace
/// period. A SIGHUP is only forwarded.
fn respond_to_stop_requests(
    stopping: &mut Stopping<'_>,
    stops: &StopRequests,
    now: Instant,
    scan: &[ProcessInfo],
) {
    let presses = stops.presses();
    if presses > 0 && stopping.interrupted_since.is_none() {
        stopping.interrupt(now, scan);
    }
    if stops.terminated.load(Ordering::SeqCst) && stopping.terminated_since.is_none() {
        stopping.terminated_since = Some(now);
    }
    stopping.escalate(now, presses);
}

/// Where SIGTERM and SIGHUP sent to Localify alone go: the service's tree.
/// Until the service is known, they wait, and reach it as soon as it is.
#[derive(Default)]
struct Forwarding {
    root: Option<Identity>,
    pending: Vec<Signal>,
    /// The profile still resolves: a stop ends its helpers and exits.
    resolving: bool,
    /// A stop ends the resolution: the service must never start.
    ending: bool,
}

impl Forwarding {
    fn forward(&mut self, signal: Signal) {
        match self.root {
            Some(root) => signal_identities(&track_tree(&[root], &all_processes()), signal),
            None => self.pending.push(signal),
        }
    }

    /// A stop arrived: whether it ends the resolution. Decided under the
    /// lock `finish_resolution` takes, so exactly one of them wins.
    fn begin_stop(&mut self) -> bool {
        self.ending |= self.resolving;
        self.ending
    }

    /// The profile resolved: whether the service may start, which it may not
    /// once a stop began ending the resolution.
    fn finish_resolution(&mut self) -> bool {
        if self.ending {
            return false;
        }
        self.resolving = false;
        true
    }

    fn publish(&mut self, root: Identity) {
        self.root = Some(root);
        for signal in std::mem::take(&mut self.pending) {
            self.forward(signal);
        }
    }
}

/// Counts each Ctrl+C, keeping Localify alive to stop the service in bounds,
/// and forwards SIGTERM and SIGHUP sent to Localify alone to the service's
/// tree. In the app's run PTY a SIGHUP means the app closed the terminal: it
/// stops the run as a SIGTERM does.
fn watch_stop_signals(
    forwarding: Arc<Mutex<Forwarding>>,
    stops: Arc<StopRequests>,
    app_terminal: bool,
) {
    let (ready_sender, ready_receiver) = mpsc::channel();
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => {
                let _ = ready_sender.send(());
                return;
            }
        };
        runtime.block_on(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut interrupt), Ok(mut terminate), Ok(mut hangup)) = (
                signal(SignalKind::interrupt()),
                signal(SignalKind::terminate()),
                signal(SignalKind::hangup()),
            ) else {
                let _ = ready_sender.send(());
                return;
            };
            let _ = ready_sender.send(());
            let forward = |signal: Signal| {
                forwarding
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .forward(signal);
            };
            let resolving = || {
                forwarding
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .begin_stop()
            };
            loop {
                tokio::select! {
                    _ = interrupt.recv() => {
                        if resolving() {
                            end_resolution(128 + libc::SIGINT);
                        }
                        stops.interrupts.fetch_add(1, Ordering::SeqCst);
                    }
                    _ = terminate.recv() => {
                        if resolving() {
                            end_resolution(128 + libc::SIGTERM);
                        }
                        stops.terminated.store(true, Ordering::SeqCst);
                        forward(Signal::Terminate);
                        crate::infrastructure::test_pause::pause_at("terminate-handled");
                    }
                    _ = hangup.recv() => {
                        if resolving() {
                            end_resolution(128 + libc::SIGHUP);
                        }
                        if app_terminal {
                            stops.terminated.store(true, Ordering::SeqCst);
                            forward(Signal::Terminate);
                        } else {
                            stops.hung_up.store(true, Ordering::SeqCst);
                            forward(Signal::Hangup);
                        }
                    }
                }
            }
        });
    });
    let _ = ready_receiver.recv_timeout(Duration::from_secs(1));
}

fn describe_exit(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("terminated by signal {signal}"),
        (None, None) => "exited".into(),
    }
}

/// Reports a launch session to the daemon, on its management socket.
pub struct SessionReporter {
    daemon: MgmtSocketClient,
    session: LaunchSessionDto,
}

impl SessionReporter {
    /// Registers the session. Fails when the daemon socket does not accept it.
    pub fn start(session: LaunchSessionDto, socket_path: &Path) -> Result<Self> {
        let update = LaunchSessionUpdate::Running {
            session: session.clone(),
        };

        let daemon = MgmtSocketClient::new(socket_path);
        if let Err(error) = daemon.report_launch_session(&update) {
            bail!("the daemon socket is not reachable ({error})");
        }
        Ok(Self { daemon, session })
    }

    fn heartbeat(&self) -> Result<()> {
        self.send(LaunchSessionUpdate::Running {
            session: self.session.clone(),
        })
    }

    fn stop(&self, reason: &str) -> Result<()> {
        self.send(LaunchSessionUpdate::Stopped {
            session: self.session.clone(),
            reason: reason.to_string(),
        })
    }

    fn send(&self, update: LaunchSessionUpdate) -> Result<()> {
        self.daemon
            .report_launch_session(&update)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::infrastructure::runs::output::{self, StreamWriter};

    fn launch<'a>(
        command: &'a [String],
        environment: &'a LaunchEnvironment,
        workdir: &'a Path,
        heartbeat: &'a dyn Fn(),
    ) -> SupervisedLaunch<'a> {
        SupervisedLaunch {
            command,
            workdir,
            environment,
            mask_patterns: None,
            stream: None,
            max_duration: None,
            session: None,
            heartbeat,
            heartbeat_every: RUN_HEARTBEAT,
            leads_session: false,
            app_terminal: false,
            supervision: None,
            finisher: Box::leak(Box::new(Finisher::native())),
        }
    }

    #[test]
    fn a_stop_during_resolution_and_the_end_of_resolution_never_both_win() {
        let mut stopped_first = Forwarding {
            resolving: true,
            ..Forwarding::default()
        };
        assert!(stopped_first.begin_stop());
        assert!(
            !stopped_first.finish_resolution(),
            "the service would start"
        );

        let mut resolved_first = Forwarding {
            resolving: true,
            ..Forwarding::default()
        };
        assert!(resolved_first.finish_resolution());
        assert!(
            !resolved_first.begin_stop(),
            "the stop would exit over a service"
        );
    }

    fn shell(script: &str) -> Vec<String> {
        vec!["sh".into(), "-c".into(), script.into()]
    }

    #[test]
    fn reports_the_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("exit 3");
        let environment = LaunchEnvironment::default();

        let end = run(launch(&command, &environment, dir.path(), &|| {})).unwrap();

        assert!(matches!(end.outcome, LaunchOutcome::Exited(status) if status.code() == Some(3)));
    }

    #[test]
    fn applies_set_and_unset_variables() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("test \"$PROFILE_VALUE\" = from-profile && test -z \"$HOME\"");
        let environment = LaunchEnvironment {
            set: vec![("PROFILE_VALUE".into(), "from-profile".into())],
            unset: vec!["HOME".into()],
            ..LaunchEnvironment::default()
        };

        let end = run(launch(&command, &environment, dir.path(), &|| {})).unwrap();

        assert!(matches!(end.outcome, LaunchOutcome::Exited(status) if status.success()));
    }

    #[test]
    fn stops_the_service_when_max_duration_elapses() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("sleep 30");
        let environment = LaunchEnvironment::default();
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.max_duration = Some(Duration::from_millis(300));

        let started = Instant::now();
        let end = run(request).unwrap();

        assert!(matches!(end.outcome, LaunchOutcome::Expired(_)));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_background_run_writes_stdout_and_stderr_masked_into_its_stream() {
        let dir = tempfile::tempdir().unwrap();
        output::create_run_dir(dir.path()).unwrap();
        let stream = StreamSink::new(StreamWriter::create(dir.path()).unwrap());
        let command = shell("echo out-line; echo err-line-supersecret >&2");
        let environment = LaunchEnvironment::default();
        let beats = Cell::new(0);
        let heartbeat = || beats.set(beats.get() + 1);
        let mut request = launch(&command, &environment, dir.path(), &heartbeat);
        request.stream = Some(stream.clone());
        request.mask_patterns = Some(vec![b"supersecret".to_vec()]);

        let end = run(request).unwrap();

        assert_eq!(end.capture, Capture::Complete);
        assert!(beats.get() >= 1);
        let read = output::read(dir.path(), 0, 4096).unwrap();
        let mut lines: Vec<String> = read.lines.into_iter().map(|line| line.text).collect();
        lines.sort();
        assert_eq!(lines, ["err-line-***", "out-line"]);
    }

    /// The kernel's table, counting full scans.
    #[derive(Default)]
    struct CountingScans(Cell<usize>);

    impl crate::infrastructure::process::ProcessTable for CountingScans {
        fn processes(&self) -> Vec<crate::infrastructure::process::ProcessInfo> {
            self.0.set(self.0.get() + 1);
            crate::infrastructure::process::all_processes()
        }

        fn process(&self, pid: u32) -> Option<crate::infrastructure::process::ProcessInfo> {
            crate::infrastructure::process::process_info(pid)
        }

        fn signal(&self, pid: u32, signal: Signal) {
            crate::infrastructure::process::send_signal(pid, signal);
        }

        fn boot_id(&self) -> Option<String> {
            crate::infrastructure::process::boot_id()
        }

        fn now(&self) -> u64 {
            crate::infrastructure::process::monotonic_now()
        }
    }

    #[test]
    fn a_healthy_run_scans_every_process_only_now_and_then() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("sleep 1.5");
        let environment = LaunchEnvironment::default();
        let table = CountingScans::default();
        let finisher = Finisher {
            table: &table,
            grace: Duration::from_secs(10),
        };
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.finisher = &finisher;

        run(request).unwrap();

        // One as it starts, one as it exits: not one every 200 ms poll.
        assert!(table.0.get() <= 2, "{} full scans", table.0.get());
    }

    #[test]
    fn the_heartbeat_is_refreshed_while_the_service_runs() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("sleep 1");
        let environment = LaunchEnvironment::default();
        let beats = Cell::new(0);
        let heartbeat = || beats.set(beats.get() + 1);
        let mut request = launch(&command, &environment, dir.path(), &heartbeat);
        request.heartbeat_every = Duration::from_millis(100);

        run(request).unwrap();

        // One as it starts; the rest come as often as the supervision polls.
        assert!(beats.get() >= 3, "{} heartbeats in a second", beats.get());
    }

    /// This process as the executor leading its session, with a member that
    /// ignores SIGTERM, and SIGKILL too unless `dies_on_kill`. The member shows
    /// once `visible_from` has passed, and starts when a scan first sees it.
    struct Unkillable {
        me: crate::infrastructure::process::ProcessInfo,
        member: crate::infrastructure::process::ProcessInfo,
        member_start: Cell<Option<u64>>,
        executor_gone: Cell<bool>,
        visible_from: Instant,
        dies_on_kill: bool,
        dead: Cell<bool>,
        signals: std::cell::RefCell<Vec<(u32, Signal)>>,
    }

    impl Unkillable {
        /// Hidden for its first 250 ms: after the run's first heartbeat.
        fn new() -> Self {
            Self::with(Duration::from_millis(250), false)
        }

        fn with(hidden_for: Duration, dies_on_kill: bool) -> Self {
            let pid = std::process::id();
            let me = crate::infrastructure::process::process_info(pid).unwrap();
            let me = crate::infrastructure::process::ProcessInfo {
                sid: Some(pid),
                ..me
            };
            let member = crate::infrastructure::process::ProcessInfo {
                pid: u32::MAX - 7,
                ppid: pid,
                pgid: pid,
                sid: Some(pid),
                start: me.start + 1,
                started_wall: me.started_wall,
            };
            Self {
                me,
                member,
                member_start: Cell::new(None),
                executor_gone: Cell::new(false),
                visible_from: Instant::now() + hidden_for,
                dies_on_kill,
                dead: Cell::new(false),
                signals: Vec::new().into(),
            }
        }
    }

    impl crate::infrastructure::process::ProcessTable for Unkillable {
        fn processes(&self) -> Vec<crate::infrastructure::process::ProcessInfo> {
            let me = (!self.executor_gone.get()).then_some(self.me);
            if self.dead.get() || Instant::now() < self.visible_from {
                return me.into_iter().collect();
            }
            let start = self
                .member_start
                .get()
                .unwrap_or_else(crate::infrastructure::process::monotonic_now);
            self.member_start.set(Some(start));
            let member = crate::infrastructure::process::ProcessInfo {
                start,
                ..self.member
            };
            me.into_iter().chain([member]).collect()
        }

        fn process(&self, pid: u32) -> Option<crate::infrastructure::process::ProcessInfo> {
            self.processes()
                .into_iter()
                .find(|process| process.pid == pid)
        }

        fn signal(&self, pid: u32, signal: Signal) {
            self.signals.borrow_mut().push((pid, signal));
            if pid == self.member.pid && signal == Signal::Kill && self.dies_on_kill {
                self.dead.set(true);
            }
        }

        fn boot_id(&self) -> Option<String> {
            Some("this boot".into())
        }

        fn now(&self) -> u64 {
            crate::infrastructure::process::monotonic_now()
        }
    }

    #[test]
    fn a_member_left_alive_fails_the_cleanup_instead_of_ending_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("exit 0");
        let environment = LaunchEnvironment::default();
        let table = Unkillable::with(Duration::ZERO, false);
        let finisher = Finisher {
            table: &table,
            grace: Duration::from_millis(50),
        };
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.leads_session = true;
        request.finisher = &finisher;

        let end = run(request).unwrap();

        assert_eq!(end.capture, Capture::Complete);
        let error = end.cleanup.unwrap_err().to_string();
        assert!(error.contains(&(u32::MAX - 7).to_string()), "{error}");
    }

    #[test]
    fn after_a_failed_cleanup_the_next_launch_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("sleep 0.3");
        let environment = LaunchEnvironment::default();
        let table = Unkillable::new();
        let finisher = Finisher {
            table: &table,
            grace: Duration::from_millis(50),
        };
        let last_alive = Cell::new(0);
        let heartbeat = || last_alive.set(crate::infrastructure::process::monotonic_now());
        let mut request = launch(&command, &environment, dir.path(), &heartbeat);
        request.leads_session = true;
        request.finisher = &finisher;
        assert!(run(request).unwrap().cleanup.is_err());

        // The executor exits; the next launcher looks at what it recorded.
        table.executor_gone.set(true);
        let pid = std::process::id();
        let recorded = crate::infrastructure::runs::finish::RunSession {
            executor_pid: pid,
            executor_start: table.me.start,
            sid: Some(pid),
            pgid: Some(pid),
            boot_id: "this boot".into(),
            last_alive_at: last_alive.get(),
        };
        crate::infrastructure::runs::output::create_run_dir(dir.path()).unwrap();
        let next = finisher.finish(dir.path(), &recorded, true);

        // The survivor is proven to be the run's, so the next launch tries to
        // end it, cannot, and finalizes nothing.
        let error = next.unwrap_err().to_string();
        assert!(error.contains(&(u32::MAX - 7).to_string()), "{error}");
        assert!(
            crate::infrastructure::runs::output::read_end(dir.path())
                .unwrap()
                .is_none()
        );
    }

    fn interrupted_after(presses: usize, grace: Duration) -> (LaunchEnd, Duration) {
        let dir = tempfile::tempdir().unwrap();
        // Neither Ctrl+C nor SIGTERM stops it.
        let command = shell("trap '' INT TERM; sleep 30");
        let environment = LaunchEnvironment::default();
        let finisher = Finisher {
            table: &crate::infrastructure::process::NativeProcesses,
            grace,
        };
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.finisher = &finisher;
        let stops = Arc::new(StopRequests::default());
        let pressing = Arc::clone(&stops);
        thread::spawn(move || {
            for _ in 0..presses {
                thread::sleep(Duration::from_millis(300));
                pressing.interrupts.fetch_add(1, Ordering::SeqCst);
            }
        });

        let started = Instant::now();
        let end = run_with(request, stops, Arc::default()).unwrap();
        (end, started.elapsed())
    }

    #[test]
    fn ctrl_c_stops_a_service_that_ignores_it_in_bounded_time() {
        let (end, took) = interrupted_after(1, Duration::from_millis(300));

        assert!(matches!(end.outcome, LaunchOutcome::Interrupted));
        assert_eq!(end.outcome.exit_code(), 130);
        assert!(took < Duration::from_secs(10), "took {took:?}");
    }

    #[test]
    fn a_second_ctrl_c_kills_at_once() {
        let (end, took) = interrupted_after(2, Duration::from_secs(10));

        assert_eq!(end.outcome.exit_code(), 130);
        assert!(took < Duration::from_secs(5), "took {took:?}");
    }

    #[test]
    fn a_run_exits_as_a_shell_reports_its_job() {
        let dir = tempfile::tempdir().unwrap();
        let environment = LaunchEnvironment::default();
        let exit = |script: &str| {
            let command = shell(script);
            let end = run(launch(&command, &environment, dir.path(), &|| {})).unwrap();
            end.outcome.exit_code()
        };

        assert_eq!(exit("exit 3"), 3);
        assert_eq!(exit("kill -TERM $$"), 143);
        assert_eq!(LaunchOutcome::Interrupted.exit_code(), 130);
    }

    /// A service whose launcher and worker react to signals differently; the
    /// worker writes its PID to `worker.pid` in `dir`.
    fn launcher_and_worker(dir: &Path, launcher_ignores: &str) -> Vec<String> {
        let pid_file = dir.join("worker.pid");
        shell(&format!(
            "(trap '' INT TERM; exec sleep 30) & echo $! > '{}'; trap '' {launcher_ignores}; exec sleep 30",
            pid_file.display()
        ))
    }

    fn worker_is_gone(dir: &Path) -> bool {
        let pid: u32 = std::fs::read_to_string(dir.join("worker.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        crate::infrastructure::process::process_info(pid).is_none()
    }

    /// Counts a Ctrl+C for `localify dev`, as its handler would, after `after`.
    fn press(stops: &Arc<StopRequests>, after: Duration, times: usize) {
        let stops = Arc::clone(stops);
        thread::spawn(move || {
            thread::sleep(after);
            for _ in 0..times {
                stops.interrupts.fetch_add(1, Ordering::SeqCst);
            }
        });
    }

    #[test]
    fn a_worker_that_ignores_ctrl_c_is_stopped_after_its_launcher_exits() {
        for leads_session in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            // The launcher ends on SIGINT; the worker ignores SIGINT and SIGTERM.
            let command = launcher_and_worker(dir.path(), "TERM");
            let environment = LaunchEnvironment::default();
            let finisher = Finisher {
                table: &crate::infrastructure::process::NativeProcesses,
                grace: Duration::from_millis(300),
            };
            let mut request = launch(&command, &environment, dir.path(), &|| {});
            request.finisher = &finisher;
            request.leads_session = leads_session;
            let stops = Arc::new(StopRequests::default());
            press(&stops, Duration::from_millis(500), 1);

            let end = run_with(request, stops, Arc::default()).unwrap();

            assert!(matches!(end.outcome, LaunchOutcome::Interrupted));
            assert!(
                worker_is_gone(dir.path()),
                "the worker survived its launcher"
            );
        }
    }

    #[test]
    fn a_launcher_that_exits_on_sigterm_leaves_its_worker_to_sigkill_on_time() {
        let dir = tempfile::tempdir().unwrap();
        // The launcher ignores SIGINT and ends on SIGTERM; the worker ignores both.
        let command = launcher_and_worker(dir.path(), "INT");
        let environment = LaunchEnvironment::default();
        let grace = Duration::from_millis(300);
        let finisher = Finisher {
            table: &crate::infrastructure::process::NativeProcesses,
            grace,
        };
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.finisher = &finisher;
        let stops = Arc::new(StopRequests::default());
        press(&stops, Duration::from_millis(500), 1);
        let started = Instant::now();

        let end = run_with(request, stops, Arc::default()).unwrap();

        assert!(end.cleanup.is_ok());
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(500) + grace * 2, "{took:?}");
        assert!(took < Duration::from_secs(10), "{took:?}");
        assert!(
            worker_is_gone(dir.path()),
            "the worker survived its launcher"
        );
    }

    #[test]
    fn a_second_ctrl_c_during_the_session_cleanup_kills_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("exit 0");
        let environment = LaunchEnvironment::default();
        let table = Unkillable::with(Duration::ZERO, true);
        let finisher = Finisher {
            table: &table,
            grace: Duration::from_secs(10),
        };
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.leads_session = true;
        request.finisher = &finisher;
        let stops = Arc::new(StopRequests::default());
        press(&stops, Duration::from_millis(500), 2);
        let started = Instant::now();

        let end = run_with(request, stops, Arc::default()).unwrap();

        assert!(end.cleanup.is_ok());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(
            table
                .signals
                .borrow()
                .contains(&(u32::MAX - 7, Signal::Kill))
        );
    }

    #[test]
    fn a_ctrl_c_that_ends_the_launcher_before_the_next_poll_still_stops_its_worker() {
        let dir = tempfile::tempdir().unwrap();
        let launcher_pid = dir.path().join("launcher.pid");
        // The launcher ends on SIGINT; the worker ignores it.
        let command = shell(&format!(
            "(trap '' INT; exec sleep 30) & echo $! > '{}'; echo $$ > '{}'; exec sleep 30",
            dir.path().join("worker.pid").display(),
            launcher_pid.display()
        ));
        let environment = LaunchEnvironment::default();
        let finisher = Finisher {
            table: &crate::infrastructure::process::NativeProcesses,
            grace: Duration::from_millis(300),
        };
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.finisher = &finisher;
        let stops = Arc::new(StopRequests::default());
        let pressing = Arc::clone(&stops);
        thread::spawn(move || {
            // Once a healthy scan has seen the worker, a terminal's Ctrl+C,
            // with supervision held until the launcher has really exited, so
            // its next poll finds the exit and the Ctrl+C together.
            thread::sleep(SCAN_WHILE_HEALTHY + Duration::from_millis(500));
            let launcher: u32 = std::fs::read_to_string(launcher_pid)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let held = pressing.poll_gate.lock().unwrap();
            crate::infrastructure::process::send_signal(launcher, Signal::Interrupt);
            while crate::infrastructure::process::process_info(launcher).is_some() {
                thread::sleep(Duration::from_millis(10));
            }
            pressing.interrupts.fetch_add(1, Ordering::SeqCst);
            drop(held);
        });

        let end = run_with(request, stops, Arc::default()).unwrap();

        assert_eq!(end.outcome.exit_code(), 130);
        assert!(
            worker_is_gone(dir.path()),
            "the worker outlived its launcher"
        );
    }

    #[test]
    fn a_forwarded_sighup_is_only_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let done = dir.path().join("done");
        // Traps SIGHUP, as a service that reloads does, and ends by itself.
        let command = shell(&format!(
            "trap : HUP; echo $$ > '{}'; i=0; while [ $i -lt 15 ]; do sleep 0.1; i=$((i+1)); done; touch '{}'",
            ready.display(),
            done.display()
        ));
        let environment = LaunchEnvironment::default();
        let finisher = Finisher {
            table: &crate::infrastructure::process::NativeProcesses,
            grace: Duration::from_millis(200),
        };
        let mut request = launch(&command, &environment, dir.path(), &|| {});
        request.finisher = &finisher;
        let stops = Arc::new(StopRequests::default());
        let hanging_up = Arc::clone(&stops);
        thread::spawn(move || {
            let service = loop {
                match std::fs::read_to_string(&ready).map(|pid| pid.trim().parse::<u32>()) {
                    Ok(Ok(pid)) => break pid,
                    _ => thread::sleep(Duration::from_millis(10)),
                }
            };
            // What the watcher does with a SIGHUP sent to `localify dev`.
            hanging_up.hung_up.store(true, Ordering::SeqCst);
            crate::infrastructure::process::send_signal(service, Signal::Hangup);
        });

        let end = run_with(request, stops, Arc::default()).unwrap();

        assert!(end.cleanup.is_ok());
        assert!(done.exists(), "a SIGHUP ended the service");
    }

    #[test]
    fn a_ctrl_c_skips_the_group_the_terminal_reached() {
        let own = |pid, pgid| crate::infrastructure::process::ProcessInfo {
            pid,
            ppid: 1,
            pgid,
            sid: Some(1),
            start: 10,
            started_wall: 0,
        };
        let scan = [own(10, 7), own(11, 8)];
        let targets: Vec<Identity> = scan.iter().map(Identity::of).collect();

        let with_a_terminal = interrupt_targets(&targets, &scan, Some(7));
        let without_one = interrupt_targets(&targets, &scan, None);

        assert_eq!(with_a_terminal, [Identity::of(&scan[1])]);
        assert_eq!(without_one, targets);
    }

    #[test]
    fn a_failed_write_is_reported_even_when_the_copy_never_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        crate::infrastructure::runs::output::create_run_dir(&run).unwrap();
        let writer =
            crate::infrastructure::runs::output::StreamWriter::with_segment_size(&run, 4).unwrap();
        let mut sink = StreamSink::new(writer);
        std::fs::remove_dir_all(&run).unwrap();
        assert!(std::io::Write::write(&mut sink, b"more than one segment").is_err());
        // A copy that never ends, as when a process holds the pipe open.
        let (_still_copying, done) = mpsc::channel();

        let capture = drain_output(Some(done), Some(&sink));

        assert!(matches!(capture, Capture::Failed(_)), "{capture:?}");
    }

    #[test]
    fn describes_exit_reasons_without_values() {
        assert_eq!(
            LaunchOutcome::Expired(Duration::from_secs(4 * 3600)).reason(),
            "max_duration of 4h reached"
        );
        assert_eq!(LaunchOutcome::Interrupted.reason(), "interrupted");
    }
}
