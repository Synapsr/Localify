//! A run PTY: the pseudo-terminal the desktop app creates for one run of a
//! service, with `localify dev` as its only child (docs/remote-machines.md,
//! "A run PTY, and a stream per run").
//!
//! No shell runs in it and nothing is typed into one, so every byte of the
//! PTY is the run's: the app copies all of them, and only them, into the run's
//! stream. `localify dev` leads the PTY's session, so the run has its own
//! session and process group, and Ctrl-Z is off since no shell could resume a
//! suspended service.
//!
//! The app is the finisher of its run PTYs. Once the executor exits, it ends
//! what is left of the run's session, reads the PTY to its end, finalizes the
//! stream, and only then reports the run as ended. While it reads, it holds
//! the run's `reader.lock`; if the app dies, the kernel releases it, and
//! `localify dev`, or the daemon when it died too, finalizes what was written
//! as `interrupted`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::branding::BRANDING;
use crate::desktop::pty::{self, PtyMaster, WindowSize};
use crate::infrastructure::process::{self, Identity, Signal, track_tree};
use crate::infrastructure::running_services::{
    RunningService, RunningServiceStore, SERVICE_ORIGIN_ENV, ServiceOrigin,
};
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::finish::{Finish, KILL_WAIT, STOP_GRACE};
use crate::infrastructure::runs::launches::{LaunchExit, dispatch_expiry};
use crate::infrastructure::runs::output::{
    self, EndReason, FinalizeLock, ReaderLock, RunEnd, StreamSink, StreamWriter,
};

/// How long the app waits, once the run's processes are gone, for the PTY to
/// reach its end. A process that left the run may hold it open: the stream
/// then ends `interrupted`, since what it still held was never read.
const EOF_WAIT: Duration = Duration::from_secs(3);
const READ_CHUNK: usize = 64 * 1024;
/// How often a reader told to stop looks at it while the PTY is quiet.
const READ_POLL_MS: libc::c_int = 100;

/// What the app runs in a new run PTY.
pub struct RunPtyLaunch {
    /// The pinned `localify` executable, never looked up on `PATH`.
    pub executable: PathBuf,
    /// Its arguments, `--config <path> dev …`; the launch id is added here.
    pub args: Vec<OsString>,
    pub working_directory: PathBuf,
    /// The whole environment `localify dev` gets.
    pub environment: Vec<(OsString, OsString)>,
    pub size: WindowSize,
}

/// What selects the app's own installation: its home, its state dir on
/// Linux, and its install profile. A run always gets the app's, whatever the
/// user's shell sets, so it records itself where the app reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInstallation {
    pub home: Option<OsString>,
    pub xdg_state_home: Option<OsString>,
    pub profile: Option<OsString>,
}

impl AppInstallation {
    /// This process's: the app's.
    pub fn of_this_process() -> Self {
        Self {
            home: std::env::var_os("HOME"),
            xdg_state_home: std::env::var_os("XDG_STATE_HOME"),
            profile: std::env::var_os(BRANDING.profile_env()),
        }
    }
}

/// The whole environment of `localify dev` in a run PTY: `base`, the user's
/// shell environment, or the app's own when it could not be read, with what
/// selects the app's installation taken from `installation`, and what marks
/// the run as the app's.
pub fn run_environment(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    working_directory: &Path,
    installation: &AppInstallation,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = base.into_iter().collect();
    let mut set = |key: &str, value: Option<OsString>| {
        environment.retain(|(name, _)| name != key);
        if let Some(value) = value {
            environment.push((key.into(), value));
        }
    };
    set("HOME", installation.home.clone());
    set(BRANDING.profile_env(), installation.profile.clone());
    if cfg!(target_os = "linux") {
        // The services' state dir on Linux.
        set("XDG_STATE_HOME", installation.xdg_state_home.clone());
    }
    set(SERVICE_ORIGIN_ENV, Some(ServiceOrigin::App.as_env().into()));
    set("TERM", Some("xterm-256color".into()));
    // What the shell that resolved the environment ran in, not the run.
    set("PWD", Some(working_directory.as_os_str().to_os_string()));
    set("OLDPWD", None);
    environment
}

/// How a run PTY's run ended, once it was finalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPtyEnd {
    /// `localify dev`'s exit code: the service's own, 128 + N when signal N
    /// ended it, and 130 after Ctrl-C.
    pub code: Option<i32>,
    /// The signal that killed `localify dev` itself.
    pub signal: Option<i32>,
    /// Unix seconds.
    pub at: u64,
    /// Whether `localify dev` registered the run. One that refused to start,
    /// because the service already runs, or was cancelled first, never did.
    pub registered: bool,
    /// How the stream was finalized, for a registered run.
    pub output: Option<EndReason>,
    /// Why the run could not be ended or finalized, or why its output was
    /// cut off.
    pub error: Option<String>,
}

impl RunPtyEnd {
    /// Ended by a signal rather than by exiting on its own: a stop, Ctrl-C,
    /// or a crash.
    pub fn stopped(&self) -> bool {
        self.signal.is_some() || self.code.is_some_and(|code| code > 128)
    }
}

/// A running run PTY. Dropping it does not end the run: stop it through its
/// session, as `RunningServiceStore::stop` does, or cancel it.
pub struct RunPty {
    run_id: RunId,
    executor_pid: u32,
    /// `localify dev` as it started, read before anything could reap it.
    executor: Option<Identity>,
    master: PtyMaster,
    writer: Mutex<File>,
    ended: Arc<AtomicBool>,
}

impl RunPty {
    /// Starts `localify dev` in a new run PTY with a new run id. `on_output`
    /// gets every byte of the PTY, after it is in the run's stream; holding it
    /// back holds back the reading. `on_end` is called once the run is
    /// finalized, or `localify dev` ended without registering it.
    pub fn spawn(
        store: Arc<RunningServiceStore>,
        launch: RunPtyLaunch,
        on_output: impl FnMut(&[u8]) + Send + 'static,
        on_end: impl FnOnce(RunPtyEnd) + Send + 'static,
    ) -> Result<Self> {
        let run_id = RunId::generate()?;
        let dir = store.run_dir(&run_id);
        output::create_run_dir(&dir)?;
        let started = start(&dir, &run_id, launch);
        let (reader_lock, sink, master, child, writer, reader) = match started {
            Ok(started) => started,
            Err(error) => {
                let _ = fs::remove_dir_all(&dir);
                return Err(error);
            }
        };
        let executor_pid = child.id();
        // Unreaped until the waiter below runs, so the PID is still this one.
        let executor = process::process_info(executor_pid).map(|info| Identity::of(&info));
        let ended = Arc::new(AtomicBool::new(false));
        let stop_reading = Arc::new(AtomicBool::new(false));
        let (end_sender, read_end) = mpsc::channel();
        let read_sink = sink.clone();
        let reader_stop = Arc::clone(&stop_reading);
        thread::spawn(move || {
            let ready = reader.try_clone().ok();
            let end = copy_output(
                reader,
                || ready.as_ref().is_none_or(readable),
                &read_sink,
                on_output,
                &reader_stop,
            );
            let _ = end_sender.send(end);
        });
        let finisher = Finishing {
            store,
            run_id: run_id.clone(),
            sink,
            reader_lock,
            read_end,
            stop_reading,
        };
        let ended_flag = Arc::clone(&ended);
        thread::spawn(move || {
            let end = finisher.wait(child);
            ended_flag.store(true, Ordering::SeqCst);
            on_end(end);
        });
        Ok(Self {
            run_id,
            executor_pid,
            executor,
            master,
            writer: Mutex::new(writer),
            ended,
        })
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// The PID of `localify dev`, the run's session and process group.
    pub fn executor_pid(&self) -> u32 {
        self.executor_pid
    }

    /// Whether the run is over and finalized.
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// Waits until the run is over and finalized, at most `timeout`.
    pub fn wait_ended(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.has_ended() {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
        true
    }

    /// Ends a launch through `localify dev`'s own identity, for a run with no
    /// record yet: while profile sources resolve, say. SIGTERM, then SIGKILL
    /// after the grace period, to `localify dev`, the members of its session
    /// and its descendants, each checked against its start first. Succeeds
    /// only once all of them are gone and the run has ended. A launch that
    /// registered meanwhile stops as for `localify stop`, since `localify dev`
    /// forwards the SIGTERM to its service.
    pub fn cancel(&self) -> Result<()> {
        let Some(executor) = self.executor else {
            // It exited before it could be read: its end is on its way.
            return self.ensure_ended(&[], KILL_WAIT);
        };
        let mut known = run_processes(executor, &[]);
        signal_alive(&known, Signal::Terminate);
        if self.ended_with(&known, STOP_GRACE) {
            return Ok(());
        }
        known = run_processes(executor, &known);
        signal_alive(&known, Signal::Kill);
        self.ensure_ended(&known, KILL_WAIT)
    }

    /// Whether the run ended and none of `known` is left, within `timeout`.
    fn ended_with(&self, known: &[Identity], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let alive = track_tree(known, &process::all_processes());
            if self.has_ended() && alive.is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn ensure_ended(&self, known: &[Identity], timeout: Duration) -> Result<()> {
        if self.ended_with(known, timeout) {
            return Ok(());
        }
        let left: Vec<String> = track_tree(known, &process::all_processes())
            .iter()
            .map(|identity| identity.pid.to_string())
            .collect();
        bail!(
            "The launch did not end: localify dev (PID {}) or what it started ({}) is still there.",
            self.executor_pid,
            left.join(", ")
        )
    }

    /// Input for the service, as typed into its tab. Refused once the run has
    /// ended: the tab then takes no input.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        if self.has_ended() {
            bail!("This service has exited. Restart it, or open a terminal here.");
        }
        let mut writer = lock(&self.writer);
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .context("Failed to write to the service's terminal")
    }

    pub fn resize(&self, size: WindowSize) -> Result<()> {
        self.master.resize(size)
    }

    /// The run PTY's settings, as the service sees them.
    pub fn termios(&self) -> Result<libc::termios> {
        self.master.termios()
    }
}

/// The launch's processes: `known` ones still alive as themselves and their
/// descendants, and, while the executor is alive as itself, the executor, the
/// members of the session it leads, and its descendants, such as a profile
/// CLI in a process group of its own. A session is the run's only while its
/// leader is, so a dead executor's session is never scanned.
fn run_processes(executor: Identity, known: &[Identity]) -> Vec<Identity> {
    let scan = process::all_processes();
    let mut roots = known.to_vec();
    if Identity::current(executor.pid) == Some(executor) {
        roots.push(executor);
        roots.extend(
            scan.iter()
                .filter(|process| process.sid == Some(executor.pid))
                .map(Identity::of),
        );
    }
    track_tree(&roots, &scan)
}

/// Signals each process still alive as the one known.
fn signal_alive(known: &[Identity], signal: Signal) {
    for identity in known {
        if Identity::current(identity.pid) == Some(*identity) {
            process::send_signal(identity.pid, signal);
        }
    }
}

type Started = (ReaderLock, StreamSink, PtyMaster, Child, File, File);

/// The run's folder ready for its reader, then `localify dev` started in it.
fn start(dir: &Path, run_id: &RunId, launch: RunPtyLaunch) -> Result<Started> {
    let reader_lock = ReaderLock::acquire(dir)?;
    let sink = StreamSink::new(StreamWriter::create(dir)?);
    let mut command = Command::new(&launch.executable);
    command
        .args(&launch.args)
        // In one argument: a base64url id may start with `-`. A restart
        // stopped the run it replaces before this point, so that stop never
        // counts against the launch's expiry.
        .arg(format!("--launch-id={run_id}"))
        .arg(format!("--launch-expires={}", dispatch_expiry()))
        .env_clear()
        .envs(launch.environment)
        .current_dir(&launch.working_directory);
    let (master, mut child) = pty::spawn(command, launch.size, true)?;
    let handles = master
        .try_clone()
        .and_then(|writer| Ok((writer, master.try_clone()?)));
    let (writer, reader) = match handles {
        Ok(handles) => handles,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    Ok((reader_lock, sink, master, child, writer, reader))
}

/// How the reading of a run PTY ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReadEnd {
    /// Every process holding the terminal closed it.
    Eof,
    /// Reading failed: what was still in the PTY was never read.
    Error(String),
    /// Told to stop, as the PTY never reached its end.
    Stopped,
}

/// Copies every byte of the PTY into the run's stream, then to `on_output`,
/// until the PTY reaches its end, reading fails, or `stop` is set. `ready`
/// says when there is something to read, waiting a moment at most. A failed
/// write is kept by the sink and reported in `end.json`; the terminal still
/// shows everything.
fn copy_output(
    mut reader: impl Read,
    mut ready: impl FnMut() -> bool,
    sink: &StreamSink,
    mut on_output: impl FnMut(&[u8]),
    stop: &AtomicBool,
) -> ReadEnd {
    let mut sink = sink.clone();
    let mut buffer = vec![0_u8; READ_CHUNK];
    loop {
        if stop.load(Ordering::SeqCst) {
            return ReadEnd::Stopped;
        }
        if !ready() {
            continue;
        }
        match reader.read(&mut buffer) {
            Ok(0) => return ReadEnd::Eof,
            Ok(count) => {
                let _ = sink.write_all(&buffer[..count]);
                on_output(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            // Linux answers EIO once every process holding the terminal closed it.
            Err(error) if error.raw_os_error() == Some(libc::EIO) => return ReadEnd::Eof,
            Err(error) => return ReadEnd::Error(error.to_string()),
        }
    }
}

/// How a run PTY's stream ends, from how its reading ended (`None`: not
/// within the wait) and whether writing the stream failed.
fn stream_end(read: Option<&ReadEnd>, write_failure: Option<String>) -> Option<String> {
    match (read, write_failure) {
        (_, Some(failure)) => Some(format!("Writing the output failed: {failure}")),
        (Some(ReadEnd::Eof), None) => None,
        (Some(ReadEnd::Error(error)), None) => {
            Some(format!("Reading the terminal failed: {error}"))
        }
        (Some(ReadEnd::Stopped) | None, None) => Some(format!(
            "The terminal was still open {} seconds after the run ended: what it held was not read.",
            EOF_WAIT.as_secs()
        )),
    }
}

/// Whether the PTY has something to read, its end included, waiting at most
/// [`READ_POLL_MS`], so a reader told to stop sees it.
fn readable(file: &File) -> bool {
    let mut poll = libc::pollfd {
        fd: file.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for an open descriptor, for the duration of
    // the call.
    let ready = unsafe { libc::poll(&mut poll, 1, READ_POLL_MS) };
    ready != 0
}

/// The app's part as the finisher of its run.
struct Finishing {
    store: Arc<RunningServiceStore>,
    run_id: RunId,
    sink: StreamSink,
    reader_lock: ReaderLock,
    /// How the reading ended, once it did.
    read_end: Receiver<ReadEnd>,
    /// Tells the reader to stop when the PTY never reaches its end.
    stop_reading: Arc<AtomicBool>,
}

impl Finishing {
    /// Waits for `localify dev`, then ends and finalizes the run.
    fn wait(self, mut child: Child) -> RunPtyEnd {
        wait_unreaped(child.id());
        if self.record().is_none() {
            // Exited before registering, a profile CLI of its, in a process
            // group of its own, may be left. Unreaped, `localify dev` still
            // holds its session id, so whatever carries it is the launch's.
            end_session_of_unreaped(child.id());
        }
        let status = child.wait();
        let (code, signal) = match &status {
            Ok(status) => exit_of(*status),
            Err(_) => (None, None),
        };
        let record = self.record();
        let registered = record.is_some();
        let (output, error) = match record {
            Some(record) => match self.finish(&record, code, signal) {
                Ok((reason, cut_off)) => (Some(reason), cut_off),
                Err(error) => (None, Some(format!("{error:#}"))),
            },
            None => {
                self.discard();
                (None, None)
            }
        };
        RunPtyEnd {
            code,
            signal,
            at: unix_now(),
            registered,
            output,
            error,
        }
    }

    /// The service's record, while it still names this run.
    fn record(&self) -> Option<RunningService> {
        self.store
            .list()
            .ok()?
            .into_iter()
            .find(|record| record.run_id() == Some(&self.run_id))
    }

    /// Ends what is left of the run's session, reads the PTY to its end, and
    /// finalizes the stream: `complete` only once its end was read and every
    /// byte written, `interrupted` with the reason otherwise. A run whose
    /// processes outlived the cleanup stays open, so its next launch is
    /// refused.
    fn finish(
        self,
        record: &RunningService,
        code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<(EndReason, Option<String>)> {
        // A process nothing proves to be the run's keeps it open: never
        // finalized under it, never signalled.
        if let Finish::Unconfirmed(pids) = self.store.end_after_executor(record)? {
            bail!("{}", Finish::unconfirmed_message(&pids));
        }
        let read = self.read_end.recv_timeout(EOF_WAIT).ok();
        if read.is_none() {
            self.stop_reading.store(true, Ordering::SeqCst);
        }
        let offset = self.sink.close();
        let cut_off = stream_end(read.as_ref(), self.sink.failure());
        let mut end = match cut_off {
            None => RunEnd::now(EndReason::Complete, offset),
            Some(_) => RunEnd::now(EndReason::Interrupted, offset),
        };
        end.error = cut_off.clone();
        let dir = self.store.run_dir(&self.run_id);
        let mut reason = end.reason;
        if let Some(_finalizing) = FinalizeLock::acquire(&dir)? {
            match output::read_end(&dir)? {
                Some(existing) => reason = existing.reason,
                None => {
                    output::publish_end(&dir, &end)?;
                }
            }
        }
        self.store
            .launches()
            .add_exit(&self.run_id, LaunchExit::now(code, signal, None))?;
        self.store.mark_exited(&record.key, &self.run_id, code)?;
        // Held until the exit is recorded: the daemon completes a run whose
        // reader died before that, and would record it without its code.
        drop(self.reader_lock);
        Ok((reason, cut_off))
    }

    /// `localify dev` ended before registering the run: its folder is
    /// nobody's, and what it printed stays in the tab only.
    fn discard(self) {
        if self.read_end.recv_timeout(EOF_WAIT).is_err() {
            self.stop_reading.store(true, Ordering::SeqCst);
        }
        self.sink.close();
        let dir = self.store.run_dir(&self.run_id);
        drop(self.reader_lock);
        let _ = fs::remove_dir_all(dir);
    }
}

/// Blocks until the child exits, without reaping it.
fn wait_unreaped(pid: u32) {
    loop {
        // SAFETY: siginfo_t is plain data, zeroed before waitid fills it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: the pointer is valid for the call; WNOWAIT leaves the child
        // to be reaped by `Child::wait`.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}

/// Ends the members of the session an exited, unreaped leader still holds:
/// SIGTERM, then SIGKILL after the grace period.
fn end_session_of_unreaped(leader: u32) {
    let members = || -> Vec<Identity> {
        process::all_processes()
            .iter()
            .filter(|process| process.sid == Some(leader) && process.pid != leader)
            .map(Identity::of)
            .collect()
    };
    let started = Instant::now();
    let mut killing = false;
    loop {
        let left = members();
        if left.is_empty() || started.elapsed() >= STOP_GRACE + KILL_WAIT {
            return;
        }
        if !killing && started.elapsed() >= STOP_GRACE {
            killing = true;
        }
        signal_alive(
            &left,
            if killing {
                Signal::Kill
            } else {
                Signal::Terminate
            },
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn exit_of(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    (status.code(), status.signal())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(code: Option<i32>, signal: Option<i32>) -> RunPtyEnd {
        RunPtyEnd {
            code,
            signal,
            at: 0,
            registered: true,
            output: Some(EndReason::Complete),
            error: None,
        }
    }

    fn value<'a>(environment: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsString> {
        environment
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    fn installation() -> AppInstallation {
        AppInstallation {
            home: Some("/Users/app".into()),
            xdg_state_home: Some("/state/app".into()),
            profile: None,
        }
    }

    #[test]
    fn a_run_s_environment_is_the_shell_s_marked_as_the_app_s() {
        let base = [
            ("PATH", "/shell/bin:/usr/bin"),
            ("LOCALIFY_SERVICE_ORIGIN", "terminal"),
            ("OLDPWD", "/elsewhere"),
            ("PWD", "/"),
        ]
        .map(|(key, value)| (OsString::from(key), OsString::from(value)));

        let environment = run_environment(base, Path::new("/repo/apps/web"), &installation());

        assert_eq!(value(&environment, "PATH").unwrap(), "/shell/bin:/usr/bin");
        assert_eq!(
            value(&environment, "LOCALIFY_SERVICE_ORIGIN").unwrap(),
            "app"
        );
        assert_eq!(value(&environment, "TERM").unwrap(), "xterm-256color");
        assert_eq!(value(&environment, "PWD").unwrap(), "/repo/apps/web");
        assert!(value(&environment, "OLDPWD").is_none());
        let origins = environment
            .iter()
            .filter(|(name, _)| name == "LOCALIFY_SERVICE_ORIGIN")
            .count();
        assert_eq!(origins, 1);
    }

    #[test]
    fn what_selects_the_installation_comes_from_the_app_not_the_shell() {
        let base = [
            ("HOME", "/Users/someone-else"),
            ("XDG_STATE_HOME", "/state/elsewhere"),
            ("LOCALIFY_PROFILE", "dev"),
        ]
        .map(|(key, value)| (OsString::from(key), OsString::from(value)));

        let environment = run_environment(base, Path::new("/repo"), &installation());

        assert_eq!(value(&environment, "HOME").unwrap(), "/Users/app");
        assert!(value(&environment, "LOCALIFY_PROFILE").is_none());
        if cfg!(target_os = "linux") {
            assert_eq!(value(&environment, "XDG_STATE_HOME").unwrap(), "/state/app");
        }
    }

    /// A reader that gives `bytes`, then fails with `error`.
    struct Failing {
        bytes: Option<Vec<u8>>,
        error: fn() -> std::io::Error,
    }

    impl Read for Failing {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            match self.bytes.take() {
                Some(bytes) => {
                    buffer[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                None => Err((self.error)()),
            }
        }
    }

    fn read_with(error: fn() -> std::io::Error) -> (ReadEnd, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let sink = StreamSink::new(StreamWriter::create(dir.path()).unwrap());
        let reader = Failing {
            bytes: Some(b"read before the error".to_vec()),
            error,
        };
        let end = copy_output(reader, || true, &sink, |_| {}, &AtomicBool::new(false));
        sink.close();
        (end, fs::read(dir.path().join("0.log")).unwrap())
    }

    #[test]
    fn a_reader_error_is_not_an_end_of_file() {
        let (end, stream) = read_with(|| std::io::Error::other("injected failure"));

        assert_eq!(end, ReadEnd::Error("injected failure".into()));
        assert_eq!(stream, b"read before the error");
        let cut_off = stream_end(Some(&end), None).unwrap();
        assert!(cut_off.contains("injected failure"), "{cut_off}");
    }

    #[test]
    fn eio_once_the_terminal_closed_is_its_end() {
        let (end, _) = read_with(|| std::io::Error::from_raw_os_error(libc::EIO));

        assert_eq!(end, ReadEnd::Eof);
        assert_eq!(stream_end(Some(&end), None), None);
        assert!(
            stream_end(None, None).is_some(),
            "no end within the wait is no end"
        );
        assert!(stream_end(Some(&ReadEnd::Eof), Some("disk full".into())).is_some());
    }

    #[test]
    fn a_signal_or_a_code_past_128_reads_as_stopped() {
        assert!(!end(Some(0), None).stopped());
        assert!(!end(Some(1), None).stopped());
        assert!(end(Some(130), None).stopped());
        assert!(end(None, Some(9)).stopped());
    }
}
