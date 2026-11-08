//! Runs every service `localify dev` starts, with or without a launch profile,
//! until it exits.
//!
//! It refreshes the run's heartbeat, copies a background run's output into its
//! stream, masks Proton Pass and Infisical values in the output, stops the
//! service when the profile's `max_duration` elapses, keeps the daemon session
//! alive, and survives Ctrl+C long enough to report why the service stopped.
//! When `localify dev` leads the run's session, it also ends whatever the
//! service left in it before returning.

use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
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
    self, Identity, ProcessInfo, ProcessTable, Signal, all_processes, signal_identities, track_tree,
};
use crate::infrastructure::runs::finish::{Finisher, KILL_WAIT};
use crate::infrastructure::runs::output::StreamSink;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the run's record says its executor is alive.
pub const RUN_HEARTBEAT: Duration = Duration::from_secs(5);

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
    pub finisher: &'a Finisher<'a>,
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

/// Stop requests from outside the service: each Ctrl+C, and a SIGTERM or
/// SIGHUP sent to `localify dev` itself.
#[derive(Default)]
pub struct StopRequests {
    pub interrupts: AtomicUsize,
    /// The interrupts a process sent with `kill`, not the terminal.
    pub directed: AtomicUsize,
    pub signalled: AtomicBool,
    /// The process-wide SIGINT counts when this run began to watch them.
    baseline: Option<(usize, usize)>,
}

impl StopRequests {
    fn watching() -> Self {
        Self {
            baseline: Some(process::interrupts()),
            ..Self::default()
        }
    }

    /// Every Ctrl+C so far, and how many of them a process sent.
    fn presses(&self) -> (usize, usize) {
        let mut all = self.interrupts.load(Ordering::SeqCst);
        let mut directed = self.directed.load(Ordering::SeqCst);
        if let Some((base_all, base_directed)) = self.baseline {
            let (now_all, now_directed) = process::interrupts();
            all += now_all.saturating_sub(base_all);
            directed += now_directed.saturating_sub(base_directed);
        }
        (all, directed)
    }

    fn any(&self) -> bool {
        self.presses().0 > 0 || self.signalled.load(Ordering::SeqCst)
    }
}

/// Starts the service and waits for it, returning why it stopped.
pub fn run(launch: SupervisedLaunch<'_>) -> Result<LaunchEnd> {
    if let Err(error) = process::count_interrupts() {
        eprintln!("Warning: Ctrl+C may end Localify before the service: {error}");
    }
    run_with(launch, Arc::new(StopRequests::watching()), true)
}

/// `run`, with stop requests a test can make without signalling itself.
fn run_with(
    launch: SupervisedLaunch<'_>,
    stops: Arc<StopRequests>,
    watch_signals: bool,
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
    (launch.heartbeat)();
    if watch_signals {
        watch_stop_signals(root, Arc::clone(&stops));
    }
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
        eprintln!("Warning: could not report the stop to the daemon: {error:#}");
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
        self.expiring_since.is_some() || self.interrupted_since.is_some()
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

    /// The first Ctrl+C: SIGINT to whoever did not get it already. One from
    /// the terminal reached `localify dev`'s whole process group; one a
    /// process sent reached `localify dev` alone.
    fn interrupt(&mut self, now: Instant, directed: bool, scan: &[ProcessInfo]) {
        let own_group = scan
            .iter()
            .find(|process| process.pid == std::process::id())
            .map(|process| process.pgid);
        let targets = interrupt_targets(&self.targets_on_interrupt(), scan, own_group, directed);
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
            self.expiring_since,
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
            self.expiring_since,
            self.cleaning_since,
        ) {
            (Some(since), _, _) => Some(since + self.grace * 2),
            (None, Some(since), _) | (None, None, Some(since)) => Some(since + self.grace),
            (None, None, None) => None,
        }
    }
}

/// The processes a Ctrl+C has not reached: all of them when a process sent
/// it to `localify dev`, otherwise those outside `localify dev`'s group, which
/// the terminal signalled whole.
fn interrupt_targets(
    targets: &[Identity],
    scan: &[ProcessInfo],
    own_group: Option<u32>,
    directed: bool,
) -> Vec<Identity> {
    targets
        .iter()
        .filter(|target| {
            let group = scan
                .iter()
                .find(|process| Identity::of(process) == **target)
                .map(|process| process.pgid);
            directed || own_group.is_none() || group != own_group
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
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                return (Err(error).context("Failed to wait for the service"), Ok(()));
            }
        }
        let now = Instant::now();
        let scan = stopping.track();
        if let Some(duration) = launch.max_duration
            && stopping.expiring_since.is_none()
            && now.duration_since(started) >= duration
        {
            eprintln!(
                "\n{}: the launch profile reached its max_duration of {}. Stopping the service.",
                BRANDING.app_name,
                humantime::format_duration(duration)
            );
            stopping.expiring_since = Some(now);
            stopping.signal(&stopping.targets(), Signal::Terminate);
        }
        respond_to_interrupts(&mut stopping, stops, now, &scan);
        beats.beat(now, launch);
        thread::sleep(POLL_INTERVAL);
    };

    if launch.leads_session && !stopping.stopping() {
        stopping.cleaning_since = Some(Instant::now());
    }
    let cleanup = loop {
        let scan = stopping.track();
        let left = stopping.targets();
        if left.is_empty() {
            break Ok(());
        }
        let now = Instant::now();
        respond_to_interrupts(&mut stopping, stops, now, &scan);
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
        thread::sleep(POLL_INTERVAL);
    };

    let outcome = match (stopping.expiring_since, launch.max_duration) {
        (Some(_), Some(duration)) => LaunchOutcome::Expired(duration),
        _ if stops.any() => LaunchOutcome::Interrupted,
        _ => LaunchOutcome::Exited(status),
    };
    (Ok(outcome), cleanup)
}

fn respond_to_interrupts(
    stopping: &mut Stopping<'_>,
    stops: &StopRequests,
    now: Instant,
    scan: &[ProcessInfo],
) {
    let (presses, directed) = stops.presses();
    if presses > 0 && stopping.interrupted_since.is_none() {
        stopping.interrupt(now, directed > 0, scan);
    }
    stopping.escalate(now, presses);
}

/// Forwards SIGTERM and SIGHUP sent to Localify alone to the service's tree.
/// Ctrl+C is counted by the handler `process::count_interrupts` installs.
fn watch_stop_signals(root: Identity, stops: Arc<StopRequests>) {
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
            let (Ok(mut terminate), Ok(mut hangup)) = (
                signal(SignalKind::terminate()),
                signal(SignalKind::hangup()),
            ) else {
                let _ = ready_sender.send(());
                return;
            };
            let _ = ready_sender.send(());
            let forward = |signal: Signal| {
                stops.signalled.store(true, Ordering::SeqCst);
                signal_identities(&track_tree(&[root], &all_processes()), signal);
            };
            loop {
                tokio::select! {
                    _ = terminate.recv() => forward(Signal::Terminate),
                    _ = hangup.recv() => forward(Signal::Hangup),
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
            finisher: Box::leak(Box::new(Finisher::native())),
        }
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
        let next = finisher
            .finish(
                dir.path(),
                &recorded,
                true,
                crate::infrastructure::runs::finish::FinishMode::Admit,
            )
            .unwrap();

        assert_eq!(
            next,
            crate::infrastructure::runs::finish::Finish::LiveMembers(vec![u32::MAX - 7])
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
        let end = run_with(request, stops, false).unwrap();
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

    /// Presses Ctrl+C, as a `kill` would, after `after`.
    fn press(stops: &Arc<StopRequests>, after: Duration, times: usize) {
        let stops = Arc::clone(stops);
        thread::spawn(move || {
            thread::sleep(after);
            for _ in 0..times {
                stops.directed.fetch_add(1, Ordering::SeqCst);
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

            let end = run_with(request, stops, false).unwrap();

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

        let end = run_with(request, stops, false).unwrap();

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

        let end = run_with(request, stops, false).unwrap();

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
    fn a_ctrl_c_from_the_terminal_skips_the_group_it_already_reached() {
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

        let from_terminal = interrupt_targets(&targets, &scan, Some(7), false);
        let from_kill = interrupt_targets(&targets, &scan, Some(7), true);

        assert_eq!(from_terminal, [Identity::of(&scan[1])]);
        assert_eq!(from_kill, targets);
        assert!(!process::sent_by_a_process(0));
        assert!(process::sent_by_a_process(4242));
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
