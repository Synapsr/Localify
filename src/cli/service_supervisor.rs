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
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::infrastructure::process::{NativeProcesses, Signal, process_info, signal_process_tree};
use crate::infrastructure::runs::finish::{Finisher, RunSession};
use crate::infrastructure::runs::output::StreamSink;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const STOP_GRACE_PERIOD: Duration = Duration::from_secs(10);
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
    /// Called right after the service starts, then every [`RUN_HEARTBEAT`].
    pub heartbeat: &'a dyn Fn(),
    /// `localify dev` leads the run's session: what the service leaves in it
    /// is ended before the run returns.
    pub leads_session: bool,
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
}

/// How the run ended, once its output is all in.
pub struct LaunchEnd {
    pub outcome: LaunchOutcome,
    /// Every copy of the output reached its end: nothing the service wrote is
    /// left unread.
    pub output_complete: bool,
}

/// Starts the service and waits for it, returning why it stopped.
pub fn run(launch: SupervisedLaunch<'_>) -> Result<LaunchEnd> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let mut child = spawn(&launch)?;
    (launch.heartbeat)();
    watch_stop_signals(child.id(), Arc::clone(&interrupted));
    let output_done = copy_output(&mut child, &launch);

    let outcome = wait(&mut child, &launch, &interrupted);
    if launch.leads_session {
        end_own_session();
    }
    let output_complete = drain_output(output_done);

    let outcome = outcome?;
    if let Some(session) = &launch.session
        && let Err(error) = session.stop(&outcome.reason())
    {
        eprintln!("Warning: could not report the stop to the daemon: {error:#}");
    }
    Ok(LaunchEnd {
        outcome,
        output_complete,
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
fn copy_output(child: &mut Child, launch: &SupervisedLaunch<'_>) -> Option<mpsc::Receiver<()>> {
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return None;
    };
    let patterns = launch.mask_patterns.clone().unwrap_or_default();
    let copies: [JoinHandle<()>; 2] = match &launch.stream {
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
        for copy in copies {
            let _ = copy.join();
        }
        let _ = done_sender.send(());
    });
    Some(done_receiver)
}

/// Waits for the remaining output, but not forever: a process that left the
/// run's session may keep the pipe open. Returns whether it all came in.
fn drain_output(done: Option<mpsc::Receiver<()>>) -> bool {
    match done {
        Some(done) => done.recv_timeout(OUTPUT_DRAIN_TIMEOUT).is_ok(),
        None => true,
    }
}

/// Ends what the service left in the session `localify dev` leads: SIGTERM,
/// then SIGKILL after the grace period. The executor is alive, so its session
/// is the run's without any further proof.
fn end_own_session() {
    let pid = std::process::id();
    let Some(me) = process_info(pid) else {
        return;
    };
    let session = RunSession {
        executor_pid: pid,
        executor_start: me.start,
        sid: Some(pid),
        pgid: Some(pid),
        boot_id: String::new(),
        last_alive_at: me.start,
    };
    let finisher = Finisher {
        table: &NativeProcesses,
        grace: STOP_GRACE_PERIOD,
    };
    let members = session.members(&NativeProcesses);
    if let Err(error) = finisher.end_members(&session, members, STOP_GRACE_PERIOD, Some(pid)) {
        eprintln!("Warning: {error:#}");
    }
}

fn wait(
    child: &mut Child,
    launch: &SupervisedLaunch<'_>,
    interrupted: &AtomicBool,
) -> Result<LaunchOutcome> {
    let started = Instant::now();
    let mut next_session_heartbeat = started + LAUNCH_SESSION_HEARTBEAT;
    let mut next_run_heartbeat = started + RUN_HEARTBEAT;
    let mut stopping_since: Option<Instant> = None;

    loop {
        if let Some(status) = child.try_wait().context("Failed to wait for the service")? {
            return Ok(match (stopping_since, launch.max_duration) {
                (Some(_), Some(duration)) => LaunchOutcome::Expired(duration),
                _ if interrupted.load(Ordering::SeqCst) => LaunchOutcome::Interrupted,
                _ => LaunchOutcome::Exited(status),
            });
        }

        let now = Instant::now();
        if let Some(duration) = launch.max_duration
            && stopping_since.is_none()
            && now.duration_since(started) >= duration
        {
            eprintln!(
                "\n{}: the launch profile reached its max_duration of {}. Stopping the service.",
                BRANDING.app_name,
                humantime::format_duration(duration)
            );
            signal_process_tree(child.id(), Signal::Terminate);
            stopping_since = Some(now);
        }
        if stopping_since.is_some_and(|since| now.duration_since(since) >= STOP_GRACE_PERIOD) {
            signal_process_tree(child.id(), Signal::Kill);
        }

        if now >= next_run_heartbeat {
            (launch.heartbeat)();
            next_run_heartbeat = now + RUN_HEARTBEAT;
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

/// Keeps Localify alive on Ctrl+C so it can finish masking the output and
/// report the stop. The terminal already delivers Ctrl+C to the service;
/// SIGTERM and SIGHUP sent to Localify alone are forwarded to it.
fn watch_stop_signals(child_pid: u32, interrupted: Arc<AtomicBool>) {
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
            loop {
                tokio::select! {
                    _ = interrupt.recv() => interrupted.store(true, Ordering::SeqCst),
                    _ = terminate.recv() => {
                        interrupted.store(true, Ordering::SeqCst);
                        signal_process_tree(child_pid, Signal::Terminate);
                    }
                    _ = hangup.recv() => {
                        interrupted.store(true, Ordering::SeqCst);
                        signal_process_tree(child_pid, Signal::Hangup);
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
            leads_session: false,
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

        assert!(end.output_complete);
        assert!(beats.get() >= 1);
        let read = output::read(dir.path(), 0, 4096).unwrap();
        let mut lines: Vec<String> = read.lines.into_iter().map(|line| line.text).collect();
        lines.sort();
        assert_eq!(lines, ["err-line-***", "out-line"]);
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
