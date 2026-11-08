//! Services that `localify dev` started, wherever it runs: a person's terminal,
//! the desktop app's terminal, or the background. `status`, `logs`, `stop` and
//! `restart` act on them from any folder, for a person, the app or an agent.
//!
//! Each service has one small JSON file, rewritten on every start. It outlives
//! the process, so a service that stopped still shows its exit code and output.
//! The `localify dev` process that runs a service, its executor, holds an
//! exclusive lock on the service's `.lock` file for the whole run, and writes
//! the record under it. A run is over once nothing of its session is left and
//! its folder holds `end.json` (see `runs`).

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::branding::BRANDING;
use crate::infrastructure::launch_sessions::unix_now;
use crate::infrastructure::process::{
    self, NativeProcesses, Signal, process_info, process_tree, signal_processes,
};
use crate::infrastructure::runs::finish::{
    Finish, FinishMode, Finisher, Membership, RunSession, membership,
};
use crate::infrastructure::runs::output::{self, plain_lines};
use crate::infrastructure::runs::{RunId, run_dir};

/// Where the app's terminal copies its whole shell, for a service it starts.
/// Set on the shell by the app. Read only for app runs until they get a run PTY.
pub const SERVICE_OUTPUT_ENV: &str = "LOCALIFY_SERVICE_OUTPUT";
/// Who started `localify dev`: `app` or `background`. A terminal sets nothing.
pub const SERVICE_ORIGIN_ENV: &str = "LOCALIFY_SERVICE_ORIGIN";

const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of the end of an output file `tail` reads.
const TAIL_WINDOW: u64 = 1024 * 1024;

/// Which service in which project environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceKey {
    pub project_id: String,
    pub environment: Option<String>,
    pub service_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceOrigin {
    /// The desktop app's terminal panel.
    App,
    /// `localify dev --detach`.
    Background,
    /// A terminal Localify does not own.
    Terminal,
}

impl ServiceOrigin {
    pub fn as_env(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Background => "background",
            Self::Terminal => "terminal",
        }
    }

    pub fn from_env(value: Option<&str>) -> Self {
        match value {
            Some("app") => Self::App,
            Some("background") => Self::Background,
            _ => Self::Terminal,
        }
    }
}

/// Where a run's output is. `None` on a record: the run has no stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunOutput {
    /// A background run: the executor copies the service's stdout and stderr
    /// from pipes into the run's stream.
    Pipe,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceExit {
    /// Unix seconds.
    pub at: u64,
    /// `None` when the service was ended by a signal, or its executor died.
    pub code: Option<i32>,
}

/// Who a run is, in a record written by a `localify dev` with run ids. Start
/// and heartbeat times are in the boot's monotonic time, never the wall clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunIdentity {
    pub run_id: RunId,
    /// The executor's start.
    pub start: u64,
    /// The run's session and process group, both the executor's PID. Recorded
    /// only when the executor leads its own session (a run PTY or a detached
    /// start): in a person's shell they would name the shell's session.
    #[serde(default)]
    pub sid: Option<u32>,
    #[serde(default)]
    pub pgid: Option<u32>,
    pub boot_id: String,
    /// The executor's last heartbeat, refreshed every 5 seconds.
    pub last_alive_at: u64,
    pub output: Option<RunOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningService {
    #[serde(flatten)]
    pub key: ServiceKey,
    /// The executor's PID.
    pub pid: u32,
    /// `None` for a legacy run, recorded by a version without run ids.
    #[serde(flatten)]
    pub run: Option<RunIdentity>,
    /// A legacy run's start time, as `ps` printed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_started_at: Option<String>,
    pub url: String,
    /// Loopback port the service listens on.
    pub port: u16,
    pub origin: ServiceOrigin,
    /// Launch profile, to restart the service on the same variables.
    pub profile: Option<String>,
    /// Unix seconds, for display.
    pub started_at: u64,
    /// The app terminal's whole-shell copy, for an app or legacy run only.
    pub log_file: Option<PathBuf>,
    pub exit: Option<ServiceExit>,
}

impl RunningService {
    pub fn run_id(&self) -> Option<&RunId> {
        self.run.as_ref().map(|run| &run.run_id)
    }

    /// The run's output stream, for a background run.
    pub fn has_stream(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|run| run.output == Some(RunOutput::Pipe))
    }

    fn session(&self) -> Option<RunSession> {
        let run = self.run.as_ref()?;
        Some(RunSession {
            executor_pid: self.pid,
            executor_start: run.start,
            sid: run.sid,
            pgid: run.pgid,
            boot_id: run.boot_id.clone(),
            last_alive_at: run.last_alive_at,
        })
    }

    /// A legacy run is its executor, alive while its PID belongs to a process
    /// that started no later than the record was written: a process that took
    /// the PID over started after the writer ended.
    fn legacy_alive(&self) -> bool {
        process_info(self.pid).is_some_and(|process| process.started_wall <= self.started_at + 1)
    }
}

/// A run about to start, as `register` records it.
pub struct NewRun {
    pub key: ServiceKey,
    pub run_id: RunId,
    pub url: String,
    pub port: u16,
    pub origin: ServiceOrigin,
    pub profile: Option<String>,
    pub log_file: Option<PathBuf>,
    pub output: Option<RunOutput>,
}

/// The executor's exclusive lock on its service, held until it exits. The OS
/// releases it even after a crash.
pub struct RunLock {
    _file: File,
    key: ServiceKey,
}

pub struct RunningServiceStore {
    dir: PathBuf,
}

impl RunningServiceStore {
    pub fn new_default() -> Result<Self> {
        let dir = BRANDING
            .user_state_dir()
            .context("HOME is not set, so Localify cannot track running services.")?;
        Ok(Self::new(dir.join("services")))
    }

    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// A new, empty output file for a service the app starts in its terminal,
    /// never one that another service already writes to.
    #[allow(dead_code)] // Used by the desktop app through the library crate.
    pub fn new_log_file(&self, owner: &str) -> Result<PathBuf> {
        let dir = self.dir.join("logs");
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create '{}'", dir.display()))?;
        let stamp = unix_now();
        for attempt in 0..1000 {
            let path = dir.join(format!("{stamp}-{owner}-{attempt}.log"));
            match File::create_new(&path) {
                Ok(_) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to create '{}'", path.display()));
                }
            }
        }
        bail!("Failed to create a new output file in '{}'", dir.display())
    }

    /// `services/output/<runId>/`.
    pub fn run_dir(&self, run_id: &RunId) -> PathBuf {
        run_dir(&self.dir, run_id)
    }

    /// Where a detached executor writes its own messages, before and during
    /// its run: `services/launches/<launchId>.log`.
    pub fn launch_log(&self, launch_id: &RunId) -> Result<PathBuf> {
        let dir = self.dir.join("launches");
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create '{}'", dir.display()))?;
        Ok(dir.join(format!("{launch_id}.log")))
    }

    /// Takes the service's lock, for the executor, before it runs anything.
    /// Refuses while another executor holds it, or while the previous run is
    /// not finalized: it is finalized here when nothing of it is left alive.
    pub fn lock(&self, key: &ServiceKey) -> Result<RunLock> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Failed to create '{}'", self.dir.display()))?;
        let path = self.dir.join(format!("{}.lock", record_name(key)));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open '{}'", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(self.already_running(key)),
            Err(TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("Failed to lock '{}'", path.display()));
            }
        }
        if let Some(previous) = self.get(key)? {
            self.admit_after(&previous)?;
        }
        Ok(RunLock {
            _file: file,
            key: key.clone(),
        })
    }

    fn already_running(&self, key: &ServiceKey) -> anyhow::Error {
        let running = self.get(key).ok().flatten();
        let detail = running
            .map(|run| format!(" (PID {}) at {}", run.pid, run.url))
            .unwrap_or_default();
        anyhow::anyhow!(
            "'{}' is already running{detail}. Stop it first with: {} stop {}",
            key.service_id,
            BRANDING.cli_command(),
            service_selector(key)
        )
    }

    /// The next launcher's part in ending the previous run: it finalizes a run
    /// with nothing left alive, and refuses otherwise.
    fn admit_after(&self, previous: &RunningService) -> Result<()> {
        let key = &previous.key;
        let (Some(run), Some(session)) = (&previous.run, previous.session()) else {
            if previous.legacy_alive() {
                return Err(self.already_running(key));
            }
            return Ok(());
        };
        let dir = self.run_dir(&run.run_id);
        let finish =
            Finisher::native().finish(&dir, &session, previous.has_stream(), FinishMode::Admit)?;
        match finish {
            Finish::AlreadyFinalized | Finish::Finalized(_) | Finish::Gone => {
                self.mark_ended(previous)?;
                Ok(())
            }
            Finish::ExecutorAlive => Err(self.already_running(key)),
            Finish::LiveMembers(pids) => {
                let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
                bail!(
                    "'{}' still has processes from its previous run (PID {}). Stop them first with: {} stop {}",
                    key.service_id,
                    pids.join(", "),
                    BRANDING.cli_command(),
                    service_selector(key)
                )
            }
            Finish::Busy => bail!(
                "'{}' is still stopping: its previous run is being finalized. Try again in a moment.",
                key.service_id
            ),
        }
    }

    /// Records that the current process runs this service, under its lock, and
    /// makes the run's folder first. A failure is fatal: nothing may start
    /// that `status`, `stop` and the next launch could not find. The previous
    /// run's output is deleted once the new record is in place.
    pub fn register(&self, lock: &RunLock, run: NewRun) -> Result<RunningService> {
        if lock.key != run.key {
            bail!(
                "The run lock is for another service than '{}'",
                run.key.service_id
            );
        }
        let previous = self.get(&run.key)?;
        let run_dir = self.run_dir(&run.run_id);
        output::create_run_dir(&run_dir)?;
        let pid = std::process::id();
        let me =
            process_info(pid).context("Failed to read this process's identity from the kernel")?;
        let boot_id =
            process::boot_id().context("Failed to read this boot's id from the kernel")?;
        let leads_session = me.sid == Some(pid);
        let record = RunningService {
            key: run.key,
            pid,
            run: Some(RunIdentity {
                run_id: run.run_id,
                start: me.start,
                sid: leads_session.then_some(pid),
                pgid: leads_session.then_some(pid),
                boot_id,
                last_alive_at: process::monotonic_now(),
                output: run.output,
            }),
            process_started_at: None,
            url: run.url,
            port: run.port,
            origin: run.origin,
            profile: run.profile,
            started_at: unix_now(),
            log_file: run.log_file,
            exit: None,
        };
        if let Err(error) = self.write(&record) {
            let _ = fs::remove_dir_all(&run_dir);
            return Err(error);
        }
        if let Some(previous) = previous {
            self.remove_output(&previous, Some(&record));
        }
        Ok(record)
    }

    /// Refreshes `lastAliveAt` in the current process's record. Does nothing
    /// once another run has taken the record over.
    pub fn heartbeat(&self, key: &ServiceKey, run_id: &RunId) -> Result<()> {
        let Some(mut record) = self.get(key)? else {
            return Ok(());
        };
        let Some(run) = record.run.as_mut().filter(|run| &run.run_id == run_id) else {
            return Ok(());
        };
        run.last_alive_at = process::monotonic_now();
        self.write(&record)
    }

    /// Records how a run ended. Does nothing when another run has taken the
    /// record over since, or when its end is already recorded.
    pub fn mark_exited(&self, key: &ServiceKey, run_id: &RunId, code: Option<i32>) -> Result<()> {
        let Some(mut record) = self.get(key)? else {
            return Ok(());
        };
        if record.run_id() != Some(run_id) || record.exit.is_some() {
            return Ok(());
        }
        record.exit = Some(ServiceExit {
            at: unix_now(),
            code,
        });
        self.write(&record)
    }

    fn mark_ended(&self, record: &RunningService) -> Result<()> {
        match record.run_id() {
            Some(run_id) => self.mark_exited(&record.key, run_id, None),
            None => Ok(()),
        }
    }

    /// The last run of this service, running or not.
    pub fn get(&self, key: &ServiceKey) -> Result<Option<RunningService>> {
        read_record(&self.record_path(key))
    }

    /// Every service with a record, running or not.
    pub fn list(&self) -> Result<Vec<RunningService>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read '{}'", self.dir.display()));
            }
        };
        let mut services = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && let Some(service) = read_record(&path)?
            {
                services.push(service);
            }
        }
        services.sort_by_key(|service| record_name(&service.key));
        Ok(services)
    }

    /// Whether any record still points at this output file.
    #[allow(dead_code)] // Used by the desktop app through the library crate.
    pub fn log_in_use(&self, log_file: &Path) -> Result<bool> {
        Ok(self
            .list()?
            .iter()
            .any(|service| service.log_file.as_deref() == Some(log_file)))
    }

    /// Whether the run is still going: its executor runs, or it is not
    /// finalized and a process proven to belong to it is alive.
    pub fn is_running(&self, service: &RunningService) -> bool {
        let (Some(run), Some(session)) = (&service.run, service.session()) else {
            return service.legacy_alive();
        };
        if session.executor_alive(&NativeProcesses) {
            return true;
        }
        let finalized = output::read_end(&self.run_dir(&run.run_id))
            .map(|end| end.is_some())
            .unwrap_or(false);
        !finalized
            && matches!(
                membership(&NativeProcesses, &session),
                Membership::Proven(_)
            )
    }

    /// Ends the run: every process of it, then its finalization. A run already
    /// over is still finalized if it was not, so the next launch is admitted.
    /// Returns whether a process was stopped.
    pub fn stop(&self, service: &RunningService) -> Result<bool> {
        let running = self.is_running(service);
        let (Some(run), Some(session)) = (&service.run, service.session()) else {
            if running {
                stop_tree(&service.key.service_id, service.pid)?;
            }
            return Ok(running);
        };
        if running && run.sid.is_none() {
            stop_tree(&service.key.service_id, service.pid)?;
        }
        let dir = self.run_dir(&run.run_id);
        let finisher = Finisher::native();
        let finish = if running && run.sid.is_some() {
            finisher.stop(&dir, &session, service.has_stream())?
        } else {
            finisher.finish(&dir, &session, service.has_stream(), FinishMode::End)?
        };
        if finish == Finish::ExecutorAlive {
            bail!(
                "'{}' did not stop: its executor (PID {}) is still running.",
                service.key.service_id,
                service.pid
            );
        }
        self.mark_ended(service)?;
        Ok(running)
    }

    /// The daemon's part in ending background runs: a run whose executor is
    /// gone is ended (its proven processes signalled) and finalized
    /// `interrupted`, since its reader died with it.
    pub fn finish_background_runs(&self) -> Result<Vec<(ServiceKey, Finish)>> {
        let mut finished = Vec::new();
        for service in self.list()? {
            let (Some(run), Some(session)) = (&service.run, service.session()) else {
                continue;
            };
            let dir = self.run_dir(&run.run_id);
            if !service.has_stream()
                || output::read_end(&dir)?.is_some()
                || session.executor_alive(&NativeProcesses)
            {
                continue;
            }
            let finish = Finisher::native().finish(&dir, &session, true, FinishMode::End)?;
            self.mark_ended(&service)?;
            finished.push((service.key.clone(), finish));
        }
        Ok(finished)
    }

    /// Stops the service if it runs, then forgets it and deletes its output.
    /// Returns whether a process was stopped.
    pub fn stop_and_forget(&self, key: &ServiceKey) -> Result<bool> {
        let Some(service) = self.get(key)? else {
            return Ok(false);
        };
        let stopped = self.stop(&service)?;
        self.remove_output(&service, None);
        let path = self.record_path(key);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove '{}'", path.display()))?;
        }
        Ok(stopped)
    }

    /// Deletes a run's output, unless `next` still uses it.
    fn remove_output(&self, service: &RunningService, next: Option<&RunningService>) {
        if let Some(run_id) = service.run_id()
            && next.and_then(RunningService::run_id) != Some(run_id)
        {
            let _ = fs::remove_dir_all(self.run_dir(run_id));
            if let Ok(log) = self.launch_log(run_id) {
                let _ = fs::remove_file(log);
            }
        }
        if let Some(log_file) = &service.log_file
            && next.and_then(|next| next.log_file.as_ref()) != Some(log_file)
        {
            let _ = fs::remove_file(log_file);
        }
    }

    /// Writes through a synced temporary file renamed over the record, so a
    /// reader never sees half a record.
    fn write(&self, record: &RunningService) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Failed to create '{}'", self.dir.display()))?;
        let path = self.record_path(&record.key);
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let mut file = File::create(&temporary)
            .with_context(|| format!("Failed to write '{}'", temporary.display()))?;
        file.write_all(serde_json::to_string_pretty(record)?.as_bytes())
            .and_then(|()| file.sync_all())
            .with_context(|| format!("Failed to write '{}'", temporary.display()))?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("Failed to write '{}'", path.display()))
    }

    fn record_path(&self, key: &ServiceKey) -> PathBuf {
        self.dir.join(format!("{}.json", record_name(key)))
    }
}

/// The arguments that name a service run exactly, from any folder.
pub fn service_selector(key: &ServiceKey) -> String {
    let mut selector = format!("{} --project {}", key.service_id, key.project_id);
    if let Some(environment) = &key.environment {
        selector.push_str(&format!(" --env {environment}"));
    }
    selector
}

/// A process known by its PID and start time, so a reused PID is never signaled.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Tracked {
    pid: u32,
    start: u64,
}

impl Tracked {
    fn of(pid: u32) -> Option<Self> {
        process_info(pid).map(|process| Self {
            pid,
            start: process.start,
        })
    }

    fn is_alive(&self) -> bool {
        Self::of(self.pid).as_ref() == Some(self)
    }
}

/// Asks a process and everything it started to stop, and kills what is still
/// alive after a grace period. Every process of the tree is watched, not only
/// `localify dev`: a server that ignores SIGTERM outlives its parent. For runs
/// whose session is not their own: a legacy run, or one in a person's shell.
fn stop_tree(service: &str, root: u32) -> Result<()> {
    let mut tracked = track_tree(root, Vec::new());
    signal_processes(&pids(&tracked), Signal::Terminate);

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        tracked.retain(Tracked::is_alive);
        if tracked.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        tracked = track_tree(root, tracked);
    }

    signal_processes(&pids(&tracked), Signal::Kill);
    thread::sleep(Duration::from_millis(300));
    tracked.retain(Tracked::is_alive);
    if !tracked.is_empty() {
        let left = pids(&tracked)
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        bail!(
            "'{service}' still has processes after SIGKILL: {left}. Stop them with: kill -9 {left}"
        );
    }
    Ok(())
}

/// Adds the descendants of every tracked process still alive, including ones
/// started since the last look.
fn track_tree(root: u32, mut tracked: Vec<Tracked>) -> Vec<Tracked> {
    let mut roots = vec![root];
    roots.extend(tracked.iter().map(|process| process.pid));
    for pid in roots.into_iter().flat_map(process_tree) {
        if !tracked.iter().any(|process| process.pid == pid)
            && let Some(process) = Tracked::of(pid)
        {
            tracked.push(process);
        }
    }
    tracked
}

fn pids(tracked: &[Tracked]) -> Vec<u32> {
    tracked.iter().map(|process| process.pid).collect()
}

/// Lines of an app run's output file, and where to read on from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputChunk {
    pub lines: Vec<String>,
    /// Byte offset to pass back to read only what comes next.
    pub cursor: u64,
}

/// The last `count` lines of an app run's output file, as a terminal shows them.
pub fn tail(path: &Path, count: usize) -> Result<OutputChunk> {
    let length = fs::metadata(path)
        .with_context(|| format!("Failed to read '{}'", path.display()))?
        .len();
    let chunk = read_from(path, length.saturating_sub(TAIL_WINDOW), TAIL_WINDOW)?;
    let start = chunk.lines.len().saturating_sub(count);
    Ok(OutputChunk {
        lines: chunk.lines[start..].to_vec(),
        cursor: chunk.cursor,
    })
}

/// The complete lines written at or after `cursor`, at most `max_bytes` of
/// them. A line still being written is left for the next read, so the cursor
/// always ends on a line break.
pub fn read_from(path: &Path, cursor: u64, max_bytes: u64) -> Result<OutputChunk> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to read '{}'", path.display()))?;
    let length = file.metadata()?.len();
    // A cursor past the end means the file was replaced by a new run: start over.
    let start = if cursor > length { 0 } else { cursor };
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let text = String::from_utf8_lossy(&bytes[..complete]);
    Ok(OutputChunk {
        lines: plain_lines(&text),
        cursor: start + complete as u64,
    })
}

fn read_record(path: &Path) -> Result<Option<RunningService>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read '{}'", path.display()));
        }
    };
    // A record from another version is as good as no record.
    Ok(serde_json::from_str(&content).ok())
}

fn record_name(key: &ServiceKey) -> String {
    format!(
        "{}--{}--{}",
        file_safe(&key.project_id),
        file_safe(key.environment.as_deref().unwrap_or("default")),
        file_safe(&key.service_id)
    )
}

fn file_safe(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    fn key(service: &str) -> ServiceKey {
        ServiceKey {
            project_id: "my-app".into(),
            environment: Some("feat-auth".into()),
            service_id: service.into(),
        }
    }

    fn run(service: &str, log_file: Option<PathBuf>) -> NewRun {
        NewRun {
            key: key(service),
            run_id: RunId::generate().unwrap(),
            url: "https://web.localify".into(),
            port: 3000,
            origin: ServiceOrigin::Background,
            profile: None,
            log_file,
            output: None,
        }
    }

    fn legacy_record(pid: u32, started_at: u64, log_file: Option<PathBuf>) -> RunningService {
        RunningService {
            key: key("web"),
            pid,
            run: None,
            process_started_at: Some("Thu Nov  5 10:00:00 2026".into()),
            url: "https://web.localify".into(),
            port: 3000,
            origin: ServiceOrigin::App,
            profile: None,
            started_at,
            log_file,
            exit: None,
        }
    }

    #[test]
    fn a_registered_service_runs_in_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let lock = store.lock(&key("web")).unwrap();

        let record = store.register(&lock, run("web", None)).unwrap();

        let service = store.get(&key("web")).unwrap().unwrap();
        assert_eq!(service.pid, std::process::id());
        assert_eq!(service.run_id(), record.run_id());
        assert!(store.is_running(&service));
        assert!(
            store
                .run_dir(record.run_id().unwrap())
                .join("finalize.lock")
                .exists()
        );
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn the_lock_admits_one_executor_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let lock = store.lock(&key("web")).unwrap();

        let error = store.lock(&key("web")).err().unwrap();

        assert!(error.to_string().contains("already running"), "{error}");
        drop(lock);
        assert!(store.lock(&key("web")).is_ok());
    }

    #[test]
    fn a_legacy_run_that_still_runs_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let mut other = Command::new("sleep").arg("5").spawn().unwrap();
        store
            .write(&legacy_record(other.id(), unix_now() + 1, None))
            .unwrap();

        let error = store.lock(&key("web")).err().unwrap();

        assert!(error.to_string().contains("already running"), "{error}");
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[test]
    fn a_legacy_record_whose_pid_was_reused_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let mut later = Command::new("sleep").arg("5").spawn().unwrap();

        // Written a minute before this process started: another process wrote it.
        let record = legacy_record(later.id(), unix_now() - 60, None);

        assert!(!store.is_running(&record));
        later.kill().unwrap();
        later.wait().unwrap();
    }

    #[test]
    fn a_legacy_record_without_a_run_id_still_reads() {
        let json = r#"{"projectId":"my-app","environment":null,"serviceId":"web","pid":42,
            "processStartedAt":"Thu Nov  5 10:00:00 2026","url":"https://web.localify","port":3000,
            "origin":"app","profile":null,"startedAt":1,"logFile":"/tmp/x.log","exit":null}"#;

        let record: RunningService = serde_json::from_str(json).unwrap();

        assert!(record.run.is_none());
        assert_eq!(record.log_file.as_deref(), Some(Path::new("/tmp/x.log")));
    }

    #[test]
    fn a_new_run_deletes_the_previous_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let first_log = dir.path().join("first.log");
        fs::write(&first_log, "old output").unwrap();
        let lock = store.lock(&key("web")).unwrap();
        let first = store
            .register(&lock, run("web", Some(first_log.clone())))
            .unwrap();
        let first_dir = store.run_dir(first.run_id().unwrap());
        output::publish_end(
            &first_dir,
            &output::RunEnd::now(output::EndReason::Complete, 0),
        )
        .unwrap();

        store.register(&lock, run("web", None)).unwrap();

        assert!(!first_log.exists());
        assert!(!first_dir.exists());
    }

    #[test]
    fn the_exit_code_is_kept_after_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let lock = store.lock(&key("web")).unwrap();
        let record = store.register(&lock, run("web", None)).unwrap();

        store
            .mark_exited(&key("web"), record.run_id().unwrap(), Some(3))
            .unwrap();

        let exit = store.get(&key("web")).unwrap().unwrap().exit.unwrap();
        assert_eq!(exit.code, Some(3));
    }

    #[test]
    fn a_heartbeat_moves_last_alive_at_forward_in_monotonic_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let lock = store.lock(&key("web")).unwrap();
        let record = store.register(&lock, run("web", None)).unwrap();
        let before = record.run.unwrap();
        thread::sleep(Duration::from_millis(30));

        store.heartbeat(&key("web"), &before.run_id).unwrap();

        let after = store.get(&key("web")).unwrap().unwrap().run.unwrap();
        assert!(after.last_alive_at >= before.last_alive_at);
        assert!(after.last_alive_at <= process::monotonic_now());
        assert!(after.start <= after.last_alive_at);
    }

    #[test]
    fn forgetting_a_service_that_ended_removes_its_record_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let log = dir.path().join("web.log");
        fs::write(&log, "output").unwrap();
        store
            .write(&legacy_record(u32::MAX - 1, 0, Some(log.clone())))
            .unwrap();

        assert!(!store.stop_and_forget(&key("web")).unwrap());

        assert!(store.get(&key("web")).unwrap().is_none());
        assert!(!log.exists());
    }

    #[test]
    fn stop_waits_for_a_child_that_ignores_sigterm() {
        let mut parent = Command::new("/bin/sh")
            .args(["-c", "sh -c 'trap \"\" TERM; sleep 30' & wait"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(300));
        let tree = process_tree(parent.id());
        assert!(tree.len() >= 2, "{tree:?}");

        stop_tree("web", parent.id()).unwrap();

        let _ = parent.wait();
        for pid in tree {
            assert!(
                !Tracked::of(pid).is_some_and(|process| process.is_alive()),
                "{pid} survived"
            );
        }
    }

    #[test]
    fn a_cursor_reads_only_complete_new_lines_and_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        fs::write(&path, "one\ntwo\nthr").unwrap();

        let first = read_from(&path, 0, TAIL_WINDOW).unwrap();
        assert_eq!(first.lines, ["one", "two"]);

        fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        let next = read_from(&path, first.cursor, 6).unwrap();
        assert_eq!(next.lines, ["three"]);
        let last = read_from(&path, next.cursor, TAIL_WINDOW).unwrap();
        assert_eq!(last.lines, ["four"]);
        assert_eq!(
            read_from(&path, last.cursor, TAIL_WINDOW)
                .unwrap()
                .lines
                .len(),
            0
        );
    }

    #[test]
    fn each_new_output_file_is_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());

        let first = store.new_log_file("app-1").unwrap();
        let second = store.new_log_file("app-1").unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        assert_eq!(tail(&path, 2).unwrap().lines, ["two", "three"]);
    }
}
