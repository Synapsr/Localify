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
//! stream `complete`, and only then reports the run as ended. While it reads,
//! it holds the run's `reader.lock`; if the app dies, the kernel releases it
//! and the next finisher finalizes what was written as `interrupted`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::branding::BRANDING;
use crate::desktop::pty::{self, PtyMaster, WindowSize};
use crate::infrastructure::running_services::{
    RunningService, RunningServiceStore, SERVICE_ORIGIN_ENV, ServiceOrigin,
};
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::output::{
    self, EndReason, FinalizeLock, ReaderLock, RunEnd, StreamSink, StreamWriter,
};

/// How long the app waits, once the run's processes are gone, for the PTY to
/// reach its end. A process that left the run may hold it open: what it
/// writes after that is not the run's.
const EOF_WAIT: Duration = Duration::from_secs(3);
const READ_CHUNK: usize = 64 * 1024;

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

/// The whole environment of `localify dev` in a run PTY: `base`, the user's
/// shell environment, or the app's own when it could not be read, with what
/// selects the app's installation taken from the app, so a run always uses
/// the app's config and state, and what marks the run as the app's.
pub fn run_environment(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    working_directory: &Path,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = base.into_iter().collect();
    let mut set = |key: &str, value: Option<OsString>| {
        environment.retain(|(name, _)| name != key);
        if let Some(value) = value {
            environment.push((key.into(), value));
        }
    };
    set(
        BRANDING.profile_env(),
        std::env::var_os(BRANDING.profile_env()),
    );
    if cfg!(target_os = "linux") {
        // The services' state dir on Linux.
        set("XDG_STATE_HOME", std::env::var_os("XDG_STATE_HOME"));
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
    /// because the service already runs, never did.
    pub registered: bool,
    /// Why the run could not be ended or finalized: it then stays open, and
    /// the service cannot start again until its processes are gone.
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
/// session, as `RunningServiceStore::stop` does.
pub struct RunPty {
    run_id: RunId,
    executor_pid: u32,
    master: PtyMaster,
    writer: Mutex<File>,
    ended: Arc<AtomicBool>,
}

impl RunPty {
    /// Starts `localify dev` in a new run PTY with a new run id. `on_output`
    /// gets every byte of the PTY, after it is in the run's stream; holding it
    /// back holds back the reading. `on_end` is called once the run is
    /// finalized, or `localify dev` refused to start it.
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
        let (reader_lock, sink, master, child) = match started {
            Ok(started) => started,
            Err(error) => {
                let _ = fs::remove_dir_all(&dir);
                return Err(error);
            }
        };
        let executor_pid = child.id();
        let writer = master.try_clone()?;
        let reader = master.try_clone()?;
        let ended = Arc::new(AtomicBool::new(false));
        let (eof_sender, eof) = mpsc::channel();
        let read_sink = sink.clone();
        thread::spawn(move || {
            copy_output(reader, &read_sink, on_output);
            let _ = eof_sender.send(());
        });
        let finisher = Finishing {
            store,
            run_id: run_id.clone(),
            sink,
            reader_lock,
            eof,
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

/// The run's folder ready for its reader, then `localify dev` started in it.
fn start(
    dir: &Path,
    run_id: &RunId,
    launch: RunPtyLaunch,
) -> Result<(ReaderLock, StreamSink, PtyMaster, Child)> {
    let reader_lock = ReaderLock::acquire(dir)?;
    let sink = StreamSink::new(StreamWriter::create(dir)?);
    let mut command = Command::new(&launch.executable);
    command
        .args(&launch.args)
        // In one argument: a base64url id may start with `-`.
        .arg(format!("--launch-id={run_id}"))
        .env_clear()
        .envs(launch.environment)
        .current_dir(&launch.working_directory);
    let (master, child) = pty::spawn(command, launch.size, true)?;
    Ok((reader_lock, sink, master, child))
}

/// Copies every byte of the PTY into the run's stream, then to `on_output`,
/// until the PTY reaches its end. A failed write is kept by the sink and
/// reported in `end.json`; the terminal still shows everything.
fn copy_output(mut reader: File, sink: &StreamSink, mut on_output: impl FnMut(&[u8])) {
    let mut sink = sink.clone();
    let mut buffer = vec![0_u8; READ_CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                let _ = sink.write_all(&buffer[..count]);
                on_output(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            // EIO once every process holding the terminal closed it.
            Err(_) => return,
        }
    }
}

/// The app's part as the finisher of its run.
struct Finishing {
    store: Arc<RunningServiceStore>,
    run_id: RunId,
    sink: StreamSink,
    reader_lock: ReaderLock,
    /// Sent once the PTY reached its end.
    eof: Receiver<()>,
}

impl Finishing {
    /// Waits for `localify dev`, then ends and finalizes the run.
    fn wait(self, mut child: Child) -> RunPtyEnd {
        let status = child.wait();
        let (code, signal) = match &status {
            Ok(status) => exit_of(*status),
            Err(_) => (None, None),
        };
        let record = self.record();
        let registered = record.is_some();
        let error = match record {
            Some(record) => self.finish(&record, code).err(),
            None => {
                self.discard();
                None
            }
        };
        RunPtyEnd {
            code,
            signal,
            at: unix_now(),
            registered,
            error: error.map(|error| format!("{error:#}")),
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
    /// finalizes the stream `complete`. A run whose processes outlived the
    /// cleanup stays open, so its next launch is refused.
    fn finish(self, record: &RunningService, code: Option<i32>) -> Result<()> {
        self.store.end_after_executor(record)?;
        let _ = self.eof.recv_timeout(EOF_WAIT);
        let dir = self.store.run_dir(&self.run_id);
        if let Some(_finalizing) = FinalizeLock::acquire(&dir)? {
            let mut end = RunEnd::now(EndReason::Complete, self.sink.close());
            end.error = self.sink.failure();
            if output::read_end(&dir)?.is_none() {
                output::publish_end(&dir, &end)?;
            }
        }
        drop(self.reader_lock);
        self.store.mark_exited(&record.key, &self.run_id, code)
    }

    /// `localify dev` refused before registering the run: its folder is
    /// nobody's, and what it printed stays in the tab only.
    fn discard(self) {
        let _ = self.eof.recv_timeout(EOF_WAIT);
        self.sink.close();
        let dir = self.store.run_dir(&self.run_id);
        drop(self.reader_lock);
        let _ = fs::remove_dir_all(dir);
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
            error: None,
        }
    }

    fn value<'a>(environment: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsString> {
        environment
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
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

        let environment = run_environment(base, Path::new("/repo/apps/web"));

        assert_eq!(value(&environment, "PATH").unwrap(), "/shell/bin:/usr/bin");
        assert_eq!(
            value(&environment, "LOCALIFY_SERVICE_ORIGIN").unwrap(),
            "app"
        );
        assert_eq!(value(&environment, "TERM").unwrap(), "xterm-256color");
        assert_eq!(value(&environment, "PWD").unwrap(), "/repo/apps/web");
        assert!(value(&environment, "OLDPWD").is_none());
        assert_eq!(
            value(&environment, "LOCALIFY_PROFILE"),
            std::env::var_os("LOCALIFY_PROFILE").as_ref()
        );
        let origins = environment
            .iter()
            .filter(|(name, _)| name == "LOCALIFY_SERVICE_ORIGIN")
            .count();
        assert_eq!(origins, 1);
    }

    #[test]
    fn a_signal_or_a_code_past_128_reads_as_stopped() {
        assert!(!end(Some(0), None).stopped());
        assert!(!end(Some(1), None).stopped());
        assert!(end(Some(130), None).stopped());
        assert!(end(None, Some(9)).stopped());
    }
}
