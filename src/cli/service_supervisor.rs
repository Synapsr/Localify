//! Runs a service launched with a launch profile until it exits.
//!
//! On top of a plain `Command::status()`, it masks Proton Pass and Infisical
//! values in the output, stops the service when the profile's `max_duration`
//! elapses, keeps the daemon session alive, and survives Ctrl+C long enough
//! to report why the service stopped.

use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use localify::infrastructure::launch_sessions::{
    LAUNCH_SESSION_HEARTBEAT, LaunchSessionDto, LaunchSessionUpdate,
};
use localify::infrastructure::mgmt_client::MgmtSocketClient;
use localify::system_helper::{SystemHelperClient, current_uid, helper_socket_path};

use crate::branding::BRANDING;
use crate::domain::launch_profile::LaunchEnvironment;
use crate::infrastructure::output_masking::spawn_masked_copy;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const STOP_GRACE_PERIOD: Duration = Duration::from_secs(10);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

pub struct SupervisedLaunch<'a> {
    pub command: &'a [String],
    pub workdir: &'a Path,
    pub environment: &'a LaunchEnvironment,
    /// Secret byte patterns; `None` inherits stdout and stderr untouched.
    pub mask_patterns: Option<Vec<Vec<u8>>>,
    pub max_duration: Option<Duration>,
    pub session: Option<SessionReporter>,
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
}

/// Starts the service and waits for it, returning why it stopped.
pub fn run(launch: SupervisedLaunch<'_>) -> Result<LaunchOutcome> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let mut child = spawn(&launch)?;
    watch_stop_signals(child.id(), Arc::clone(&interrupted));
    let output_done = pipe_masked_output(&mut child, launch.mask_patterns.clone());

    let outcome = wait(&mut child, &launch, &interrupted);
    drain_output(output_done);

    let outcome = outcome?;
    if let Some(session) = &launch.session
        && let Err(error) = session.stop(&outcome.reason())
    {
        eprintln!("Warning: could not report the stop to the daemon: {error:#}");
    }
    Ok(outcome)
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

    if launch.mask_patterns.is_some() {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    command
        .spawn()
        .with_context(|| format!("Failed to start command '{program}'"))
}

fn pipe_masked_output(
    child: &mut Child,
    patterns: Option<Vec<Vec<u8>>>,
) -> Option<mpsc::Receiver<()>> {
    let patterns = patterns?;
    let (done_sender, done_receiver) = mpsc::channel();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let copies = [
        stdout.map(|out| spawn_masked_copy(out, io::stdout(), patterns.clone())),
        stderr.map(|err| spawn_masked_copy(err, io::stderr(), patterns)),
    ];
    thread::spawn(move || {
        for copy in copies.into_iter().flatten() {
            let _ = copy.join();
        }
        let _ = done_sender.send(());
    });
    Some(done_receiver)
}

/// Waits for the remaining output, but not forever: a background process
/// the service left behind may keep the pipe open.
fn drain_output(done: Option<mpsc::Receiver<()>>) {
    if let Some(done) = done {
        let _ = done.recv_timeout(OUTPUT_DRAIN_TIMEOUT);
    }
}

fn wait(
    child: &mut Child,
    launch: &SupervisedLaunch<'_>,
    interrupted: &AtomicBool,
) -> Result<LaunchOutcome> {
    let started = Instant::now();
    let mut next_heartbeat = started + LAUNCH_SESSION_HEARTBEAT;
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
            signal_process_tree(child.id(), "TERM");
            stopping_since = Some(now);
        }
        if stopping_since.is_some_and(|since| now.duration_since(since) >= STOP_GRACE_PERIOD) {
            signal_process_tree(child.id(), "KILL");
        }

        if now >= next_heartbeat {
            if let Some(session) = &launch.session {
                let _ = session.heartbeat();
            }
            next_heartbeat = now + LAUNCH_SESSION_HEARTBEAT;
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
                        signal_process_tree(child_pid, "TERM");
                    }
                    _ = hangup.recv() => {
                        interrupted.store(true, Ordering::SeqCst);
                        signal_process_tree(child_pid, "HUP");
                    }
                }
            }
        });
    });
    let _ = ready_receiver.recv_timeout(Duration::from_secs(1));
}

/// Signals the service and every process it started (`pnpm` → `next` →
/// workers), without touching the shell or Localify itself.
fn signal_process_tree(root_pid: u32, signal: &str) {
    let mut pids = vec![root_pid];
    let mut index = 0;
    while index < pids.len() {
        pids.extend(child_pids(pids[index]));
        index += 1;
    }
    let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
    let _ = Command::new("kill")
        .arg(format!("-{signal}"))
        .args(&pids)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn child_pids(pid: u32) -> Vec<u32> {
    Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn describe_exit(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("terminated by signal {signal}"),
        (None, None) => "exited".into(),
    }
}

/// Reports a launch session to the daemon: directly on its socket when this
/// process may open it, otherwise through the desktop app's system helper.
pub struct SessionReporter {
    channel: SessionChannel,
    session: LaunchSessionDto,
}

enum SessionChannel {
    Daemon(MgmtSocketClient),
    Helper(SystemHelperClient),
}

impl SessionReporter {
    /// Registers the session. Fails when neither the daemon socket nor the
    /// system helper accepts it.
    pub fn start(session: LaunchSessionDto, socket_path: &Path) -> Result<Self> {
        let update = LaunchSessionUpdate::Running {
            session: session.clone(),
        };

        let daemon = MgmtSocketClient::new(socket_path);
        let daemon_error = match daemon.report_launch_session(&update) {
            Ok(()) => {
                return Ok(Self {
                    channel: SessionChannel::Daemon(daemon),
                    session,
                });
            }
            Err(error) => error,
        };

        let helper = current_uid()
            .ok()
            .map(|uid| SystemHelperClient::new(helper_socket_path(uid)))
            .filter(|helper| helper.ping().is_ok());
        let Some(helper) = helper else {
            bail!(
                "the daemon socket is not reachable ({daemon_error}) and no system helper is running"
            );
        };
        helper.report_launch_session(update)?;
        Ok(Self {
            channel: SessionChannel::Helper(helper),
            session,
        })
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
        match &self.channel {
            SessionChannel::Daemon(daemon) => daemon
                .report_launch_session(&update)
                .map_err(|error| anyhow::anyhow!("{error}")),
            SessionChannel::Helper(helper) => helper.report_launch_session(update),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch<'a>(
        command: &'a [String],
        environment: &'a LaunchEnvironment,
        workdir: &'a Path,
    ) -> SupervisedLaunch<'a> {
        SupervisedLaunch {
            command,
            workdir,
            environment,
            mask_patterns: None,
            max_duration: None,
            session: None,
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

        let outcome = run(launch(&command, &environment, dir.path())).unwrap();

        assert!(matches!(outcome, LaunchOutcome::Exited(status) if status.code() == Some(3)));
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

        let outcome = run(launch(&command, &environment, dir.path())).unwrap();

        assert!(matches!(outcome, LaunchOutcome::Exited(status) if status.success()));
    }

    #[test]
    fn stops_the_service_when_max_duration_elapses() {
        let dir = tempfile::tempdir().unwrap();
        let command = shell("sleep 30");
        let environment = LaunchEnvironment::default();
        let mut request = launch(&command, &environment, dir.path());
        request.max_duration = Some(Duration::from_millis(300));

        let started = Instant::now();
        let outcome = run(request).unwrap();

        assert!(matches!(outcome, LaunchOutcome::Expired(_)));
        assert!(started.elapsed() < Duration::from_secs(10));
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
