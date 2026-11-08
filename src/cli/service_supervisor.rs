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
    self, Identity, ProcessInfo, Signal, all_processes, signal_identities, track_tree,
};
use crate::infrastructure::runs::finish::{Finisher, RunSession};
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
    pub signalled: AtomicBool,
}

impl StopRequests {
    fn any(&self) -> bool {
        self.interrupts.load(Ordering::SeqCst) > 0 || self.signalled.load(Ordering::SeqCst)
    }
}

/// Starts the service and waits for it, returning why it stopped.
pub fn run(launch: SupervisedLaunch<'_>) -> Result<LaunchEnd> {
    run_with(launch, Arc::new(StopRequests::default()), true)
}

/// `run`, with stop requests a test can make without signalling itself.
fn run_with(
    launch: SupervisedLaunch<'_>,
    stops: Arc<StopRequests>,
    watch_signals: bool,
) -> Result<LaunchEnd> {
    let mut child = spawn(&launch)?;
    // A service that already exited is a zombie no table shows: nothing to signal.
    let root = Identity::current(child.id()).unwrap_or(Identity {
        pid: child.id(),
        start: 0,
    });
    (launch.heartbeat)();
    if watch_signals {
        watch_stop_signals(root, Arc::clone(&stops));
    }
    let output_done = copy_output(&mut child, &launch);

    let outcome = wait(&mut child, root, &launch, &stops);
    let cleanup = match launch.leads_session {
        true => end_own_session(launch.finisher),
        false => Ok(()),
    };
    if cleanup.is_err() {
        // Every survivor started before this heartbeat, so the next launcher
        // proves it belongs to the run, and refuses.
        (launch.heartbeat)();
    }
    let capture = drain_output(output_done);

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
/// run's session may keep the pipe open.
fn drain_output(done: Option<mpsc::Receiver<Result<(), String>>>) -> Capture {
    let Some(done) = done else {
        return Capture::Complete;
    };
    match done.recv_timeout(OUTPUT_DRAIN_TIMEOUT) {
        Ok(Ok(())) => Capture::Complete,
        Ok(Err(error)) => Capture::Failed(error),
        Err(_) => Capture::Unfinished,
    }
}

/// Ends what the service left in the session `localify dev` leads: SIGTERM,
/// then SIGKILL after the grace period. The executor is alive, so its session
/// is the run's without any further proof. Fails while a member is left.
fn end_own_session(finisher: &Finisher<'_>) -> Result<()> {
    let pid = std::process::id();
    let me = finisher
        .table
        .process(pid)
        .context("Failed to read this process's identity from the kernel")?;
    let session = RunSession {
        executor_pid: pid,
        executor_start: me.start,
        sid: Some(pid),
        pgid: Some(pid),
        boot_id: String::new(),
        last_alive_at: me.start,
    };
    let members = session.members(finisher.table);
    finisher.end_members(&session, members, Vec::new(), finisher.grace, Some(pid))
}

/// Where a service stands in being stopped by `localify dev` itself.
#[derive(Default)]
struct Stopping {
    /// The service and its descendants, by identity.
    tracked: Vec<Identity>,
    expiring_since: Option<Instant>,
    interrupted_since: Option<Instant>,
    terminated: bool,
}

impl Stopping {
    fn refresh(&mut self, root: Identity) -> Vec<ProcessInfo> {
        if self.tracked.is_empty() {
            self.tracked.push(root);
        }
        let scan = all_processes();
        self.tracked = track_tree(&self.tracked, &scan);
        scan
    }

    fn signal(&mut self, root: Identity, signal: Signal) {
        self.refresh(root);
        signal_identities(&self.tracked, signal);
    }

    /// Ctrl+C: the first one is forwarded as SIGINT to whoever did not get it
    /// from the terminal; after the grace period comes SIGTERM, after twice
    /// that SIGKILL. A second Ctrl+C kills at once.
    fn on_interrupts(&mut self, root: Identity, presses: usize, grace: Duration, now: Instant) {
        let Some(since) = self.interrupted_since else {
            let scan = self.refresh(root);
            signal_identities(&interrupt_targets(&self.tracked, &scan), Signal::Interrupt);
            self.interrupted_since = Some(now);
            return;
        };
        if presses >= 2 || now.duration_since(since) >= grace * 2 {
            self.signal(root, Signal::Kill);
        } else if now.duration_since(since) >= grace && !self.terminated {
            self.signal(root, Signal::Terminate);
            self.terminated = true;
        }
    }
}

/// The processes a Ctrl+C has not reached: all of them when `localify dev`
/// is not the terminal's foreground job, otherwise those outside its group.
fn interrupt_targets(tracked: &[Identity], scan: &[ProcessInfo]) -> Vec<Identity> {
    let own_group = process::foreground_group();
    tracked
        .iter()
        .filter(|identity| {
            let group = scan
                .iter()
                .find(|process| Identity::of(process) == **identity)
                .map(|process| process.pgid);
            own_group.is_none() || group != own_group
        })
        .copied()
        .collect()
}

fn wait(
    child: &mut Child,
    root: Identity,
    launch: &SupervisedLaunch<'_>,
    stops: &StopRequests,
) -> Result<LaunchOutcome> {
    let started = Instant::now();
    let grace = launch.finisher.grace;
    let mut next_session_heartbeat = started + LAUNCH_SESSION_HEARTBEAT;
    let mut next_run_heartbeat = started + launch.heartbeat_every;
    let mut stopping = Stopping::default();

    loop {
        if let Some(status) = child.try_wait().context("Failed to wait for the service")? {
            return Ok(match (stopping.expiring_since, launch.max_duration) {
                (Some(_), Some(duration)) => LaunchOutcome::Expired(duration),
                _ if stops.any() => LaunchOutcome::Interrupted,
                _ => LaunchOutcome::Exited(status),
            });
        }

        let now = Instant::now();
        if let Some(duration) = launch.max_duration
            && stopping.expiring_since.is_none()
            && now.duration_since(started) >= duration
        {
            eprintln!(
                "\n{}: the launch profile reached its max_duration of {}. Stopping the service.",
                BRANDING.app_name,
                humantime::format_duration(duration)
            );
            stopping.signal(root, Signal::Terminate);
            stopping.expiring_since = Some(now);
        }
        if stopping
            .expiring_since
            .is_some_and(|since| now.duration_since(since) >= grace)
        {
            stopping.signal(root, Signal::Kill);
        }
        let presses = stops.interrupts.load(Ordering::SeqCst);
        if presses > 0 {
            stopping.on_interrupts(root, presses, grace, now);
        }

        if now >= next_run_heartbeat {
            (launch.heartbeat)();
            next_run_heartbeat = now + launch.heartbeat_every;
        }
        if now >= next_session_heartbeat {
            if let Some(session) = &launch.session {
                let _ = session.heartbeat();
            }
            next_session_heartbeat = now + LAUNCH_SESSION_HEARTBEAT;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Keeps Localify alive on Ctrl+C so it can stop the service in bounds and
/// finish its output. SIGTERM and SIGHUP sent to Localify alone are forwarded
/// to the service's tree.
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
                stops.signalled.store(true, Ordering::SeqCst);
                signal_identities(&track_tree(&[root], &all_processes()), signal);
            };
            loop {
                tokio::select! {
                    _ = interrupt.recv() => {
                        stops.interrupts.fetch_add(1, Ordering::SeqCst);
                    }
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

        assert!(beats.get() >= 5, "{} heartbeats in a second", beats.get());
    }

    /// This process as the executor leading its session, with a member that no
    /// signal ever reaches. The member starts when a scan first sees it, after
    /// the service exited, so after the run's first heartbeat.
    struct Unkillable {
        me: crate::infrastructure::process::ProcessInfo,
        member: crate::infrastructure::process::ProcessInfo,
        member_start: Cell<Option<u64>>,
        executor_gone: Cell<bool>,
    }

    impl Unkillable {
        fn new() -> Self {
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
            }
        }
    }

    impl crate::infrastructure::process::ProcessTable for Unkillable {
        fn processes(&self) -> Vec<crate::infrastructure::process::ProcessInfo> {
            let start = self
                .member_start
                .get()
                .unwrap_or_else(crate::infrastructure::process::monotonic_now);
            self.member_start.set(Some(start));
            let member = crate::infrastructure::process::ProcessInfo {
                start,
                ..self.member
            };
            match self.executor_gone.get() {
                true => vec![member],
                false => vec![self.me, member],
            }
        }

        fn process(&self, pid: u32) -> Option<crate::infrastructure::process::ProcessInfo> {
            self.processes()
                .into_iter()
                .find(|process| process.pid == pid)
        }

        fn signal(&self, _pid: u32, _signal: Signal) {}

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
        let table = Unkillable::new();
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
        let recorded = RunSession {
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

    #[test]
    fn describes_exit_reasons_without_values() {
        assert_eq!(
            LaunchOutcome::Expired(Duration::from_secs(4 * 3600)).reason(),
            "max_duration of 4h reached"
        );
        assert_eq!(LaunchOutcome::Interrupted.reason(), "interrupted");
    }
}
