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

use std::collections::HashSet;
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
    self, Identity, NativeProcesses, Signal, process_info, signal_identities, track_tree,
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

    /// A legacy run is its executor, known by the start time `ps` printed. Its
    /// PID is that process only when the native start reads the same, to the
    /// second, unambiguously, and this boot began before the record was
    /// written. Anything less is `Unknown`, and is never signalled.
    fn legacy_state(&self) -> RunState {
        match self.legacy_identity() {
            Ok(_) => RunState::Running,
            Err(state) => state,
        }
    }

    /// The verified process of a legacy run, or the state it shows as.
    fn legacy_identity(&self) -> Result<Identity, RunState> {
        let Some(process) = process_info(self.pid) else {
            return Err(RunState::Stopped);
        };
        let Some(saved) = self.process_started_at.as_deref().map(str::trim) else {
            return Err(RunState::Unknown);
        };
        let reads_as_saved =
            |seconds: u64| process::lstart_label(seconds).as_deref() == Some(saved);
        // A local time a clock change repeats names two starts an hour apart.
        let ambiguous = reads_as_saved(process.started_wall.saturating_sub(3600))
            || reads_as_saved(process.started_wall + 3600);
        let booted_before = process::boot_time().is_some_and(|boot| boot < self.started_at);
        if reads_as_saved(process.started_wall) && !ambiguous && booted_before {
            Ok(Identity::of(&process))
        } else {
            Err(RunState::Unknown)
        }
    }
}

/// Whether a run is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Running,
    Stopped,
    /// Recorded by an older version, and its PID now belongs to a process
    /// Localify cannot prove is the service: it is never signalled.
    Unknown,
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
    /// Runs between reading a record and writing it back, to replay races.
    #[cfg(test)]
    before_write: Option<Box<dyn Fn() + Send + Sync>>,
    /// Runs after a forgotten run was stopped, before its record goes.
    #[cfg(test)]
    before_forget: Option<Box<dyn Fn() + Send + Sync>>,
}

impl RunningServiceStore {
    pub fn new_default() -> Result<Self> {
        let dir = BRANDING
            .user_state_dir()
            .context("HOME is not set, so Localify cannot track running services.")?;
        Ok(Self::new(dir.join("services")))
    }

    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            #[cfg(test)]
            before_write: None,
            #[cfg(test)]
            before_forget: None,
        }
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
            // An unverifiable PID is not proof of a run: the port check still
            // refuses a service that listens.
            if previous.legacy_state() == RunState::Running {
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
        let _record = self.lock_record(&run.key)?;
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
        self.update(key, run_id, |record| {
            if let Some(run) = record.run.as_mut() {
                run.last_alive_at = process::monotonic_now();
            }
            true
        })
    }

    /// Records how a run ended. Does nothing when another run has taken the
    /// record over since, or when its end is already recorded.
    pub fn mark_exited(&self, key: &ServiceKey, run_id: &RunId, code: Option<i32>) -> Result<()> {
        self.update(key, run_id, |record| {
            if record.exit.is_some() {
                return false;
            }
            record.exit = Some(ServiceExit {
                at: unix_now(),
                code,
            });
            true
        })
    }

    /// Changes the record of `run_id`, under the service's record lock, so no
    /// two writers interleave a read and a write. Nothing changes once another
    /// run owns the record, or when `change` says so.
    fn update(
        &self,
        key: &ServiceKey,
        run_id: &RunId,
        change: impl FnOnce(&mut RunningService) -> bool,
    ) -> Result<()> {
        let _record = self.lock_record(key)?;
        let Some(mut record) = self.get(key)? else {
            return Ok(());
        };
        if record.run_id() != Some(run_id) || !change(&mut record) {
            return Ok(());
        }
        #[cfg(test)]
        if let Some(pause) = &self.before_write {
            pause();
        }
        self.write(&record)
    }

    /// An exclusive `flock` held for one read-modify-write of a service's
    /// record. The `@` keeps its name apart from any record or run lock, since
    /// record names never hold one.
    fn lock_record(&self, key: &ServiceKey) -> Result<File> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Failed to create '{}'", self.dir.display()))?;
        let path = self.dir.join(format!("{}@record.lock", record_name(key)));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open '{}'", path.display()))?;
        file.lock()
            .with_context(|| format!("Failed to lock '{}'", path.display()))?;
        Ok(file)
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
    pub fn state(&self, service: &RunningService) -> RunState {
        let (Some(run), Some(session)) = (&service.run, service.session()) else {
            return service.legacy_state();
        };
        if session.executor_alive(&NativeProcesses) {
            return RunState::Running;
        }
        let finalized = output::read_end(&self.run_dir(&run.run_id))
            .map(|end| end.is_some())
            .unwrap_or(false);
        let proven = matches!(
            membership(&NativeProcesses, &session),
            Membership::Proven(_)
        );
        if !finalized && proven {
            RunState::Running
        } else {
            RunState::Stopped
        }
    }

    /// Whether [`RunningServiceStore::state`] is `Running`.
    pub fn is_running(&self, service: &RunningService) -> bool {
        self.state(service) == RunState::Running
    }

    /// Ends the run: every process of it, then its finalization. A run already
    /// over is still finalized if it was not, so the next launch is admitted.
    /// Returns whether a process was stopped. A legacy run Localify cannot
    /// verify is refused, never signalled.
    pub fn stop(&self, service: &RunningService) -> Result<bool> {
        let state = self.state(service);
        let running = state == RunState::Running;
        let (Some(run), Some(session)) = (&service.run, service.session()) else {
            return match service.legacy_identity() {
                Ok(root) => stop_tree(&service.key.service_id, root).map(|()| true),
                Err(RunState::Unknown) => bail!("{}", unverified_legacy_run(service)),
                Err(_) => Ok(false),
            };
        };
        if running && run.sid.is_none() {
            let root = Identity {
                pid: service.pid,
                start: run.start,
            };
            stop_tree(&service.key.service_id, root)?;
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
        // A PID Localify cannot verify is left alone, and only forgotten.
        let stopped = match self.state(&service) {
            RunState::Unknown => false,
            RunState::Running | RunState::Stopped => self.stop(&service)?,
        };
        #[cfg(test)]
        if let Some(pause) = &self.before_forget {
            pause();
        }
        let _record = self.lock_record(key)?;
        // A run that registered meanwhile keeps its record and its output.
        let current = self.get(key)?;
        if !current
            .as_ref()
            .is_some_and(|current| same_run(current, &service))
        {
            return Ok(stopped);
        }
        self.remove_output(&service, None);
        let path = self.record_path(key);
        fs::remove_file(&path).with_context(|| format!("Failed to remove '{}'", path.display()))?;
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

/// Whether two records name the same run: the same run id, or for legacy runs
/// the same executor and start.
fn same_run(left: &RunningService, right: &RunningService) -> bool {
    match (left.run_id(), right.run_id()) {
        (Some(left), Some(right)) => left == right,
        (None, None) => left.pid == right.pid && left.started_at == right.started_at,
        _ => false,
    }
}

/// Why a legacy run is neither shown as running nor stopped.
pub fn unverified_legacy_run(service: &RunningService) -> String {
    format!(
        "'{}' was started by an older version of {}, and PID {} may now be another process. {} does not signal it. If it is still the service, stop it by hand: kill {}",
        service.key.service_id, BRANDING.app_name, service.pid, BRANDING.app_name, service.pid
    )
}

/// The arguments that name a service run exactly, from any folder.
pub fn service_selector(key: &ServiceKey) -> String {
    let mut selector = format!("{} --project {}", key.service_id, key.project_id);
    if let Some(environment) = &key.environment {
        selector.push_str(&format!(" --env {environment}"));
    }
    selector
}

/// Asks a process and everything it started to stop, and kills what is still
/// alive after a grace period. `root` is the verified executor; the tree grows
/// only from processes still alive as themselves, and each one's start is
/// checked again before each signal. Every process of the tree is watched, not
/// only `localify dev`: a server that ignores SIGTERM outlives its parent. For
/// runs whose session is not their own: a legacy run, or one in a person's
/// shell.
fn stop_tree(service: &str, root: Identity) -> Result<()> {
    let mut tracked = track_tree(&[root], &process::all_processes());
    let mut terminated: HashSet<Identity> = HashSet::new();

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        tracked = track_tree(&tracked, &process::all_processes());
        if tracked.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        let new: Vec<Identity> = tracked
            .iter()
            .filter(|identity| terminated.insert(**identity))
            .copied()
            .collect();
        signal_identities(&new, Signal::Terminate);
        thread::sleep(Duration::from_millis(100));
    }

    signal_identities(&tracked, Signal::Kill);
    thread::sleep(Duration::from_millis(300));
    tracked = track_tree(&tracked, &process::all_processes());
    if !tracked.is_empty() {
        let left = tracked
            .iter()
            .map(|identity| identity.pid.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        bail!(
            "'{service}' still has processes after SIGKILL: {left}. Stop them with: kill -9 {left}"
        );
    }
    Ok(())
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

    /// What an older version recorded for `pid`: the start `ps` printed.
    fn recorded_by_ps(pid: u32, started_at: u64) -> RunningService {
        let printed = Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        RunningService {
            process_started_at: Some(String::from_utf8_lossy(&printed.stdout).trim().into()),
            ..legacy_record(pid, started_at, None)
        }
    }

    fn sleeper() -> std::process::Child {
        Command::new("sleep").arg("30").spawn().unwrap()
    }

    #[test]
    fn a_legacy_run_whose_start_matches_runs_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let mut service = sleeper();
        let record = recorded_by_ps(service.id(), unix_now());
        store.write(&record).unwrap();

        assert_eq!(store.state(&record), RunState::Running);
        let error = store.lock(&key("web")).err().unwrap();
        assert!(error.to_string().contains("already running"), "{error}");

        assert!(store.stop(&record).unwrap());
        service.wait().unwrap();
    }

    #[test]
    fn a_legacy_pid_with_another_start_is_unknown_and_never_signalled() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let mut unrelated = sleeper();
        // The clock moved back: the record looks written after this start.
        let record = RunningService {
            process_started_at: Some("Thu Nov  5 10:00:00 2026".into()),
            ..legacy_record(unrelated.id(), unix_now() + 3600, None)
        };

        assert_eq!(store.state(&record), RunState::Unknown);
        let error = store.stop(&record).unwrap_err();
        assert!(error.to_string().contains("stop it by hand"), "{error}");

        thread::sleep(Duration::from_millis(100));
        assert!(unrelated.try_wait().unwrap().is_none(), "it was signalled");
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }

    #[test]
    fn a_legacy_record_without_a_start_or_from_before_this_boot_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let mut unrelated = sleeper();
        let without_start = RunningService {
            process_started_at: None,
            ..legacy_record(unrelated.id(), unix_now(), None)
        };
        // A matching start, but the record predates this boot.
        let before_boot = recorded_by_ps(unrelated.id(), 1);

        assert_eq!(store.state(&without_start), RunState::Unknown);
        assert_eq!(store.state(&before_boot), RunState::Unknown);
        assert!(store.stop(&before_boot).is_err());
        assert!(unrelated.try_wait().unwrap().is_none(), "it was signalled");
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }

    #[test]
    fn a_delayed_finisher_never_overwrites_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let (paused, wait_paused) = std::sync::mpsc::channel();
        let (resume, wait_resume) = std::sync::mpsc::channel::<()>();
        let wait_resume = std::sync::Mutex::new(wait_resume);
        let mut late = RunningServiceStore::new(dir.path().to_path_buf());
        late.before_write = Some(Box::new(move || {
            let _ = paused.send(());
            let _ = wait_resume.lock().unwrap().recv();
        }));
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let lock = store.lock(&key("web")).unwrap();
        let first = store.register(&lock, run("web", None)).unwrap();

        let second = thread::scope(|scope| {
            let finisher =
                scope.spawn(|| late.mark_exited(&key("web"), first.run_id().unwrap(), Some(1)));
            wait_paused.recv().unwrap();
            let registering = scope.spawn(|| store.register(&lock, run("web", None)));
            thread::sleep(Duration::from_millis(300));
            resume.send(()).unwrap();
            finisher.join().unwrap().unwrap();
            registering.join().unwrap().unwrap()
        });

        let current = store.get(&key("web")).unwrap().unwrap();
        assert_eq!(current.run_id(), second.run_id());
        assert!(current.exit.is_none(), "{:?}", current.exit);
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
        let root = Identity::current(parent.id()).unwrap();
        let tree = track_tree(&[root], &process::all_processes());
        assert!(tree.len() >= 2, "{tree:?}");

        stop_tree("web", root).unwrap();

        let _ = parent.wait();
        for identity in tree {
            assert_ne!(Identity::current(identity.pid), Some(identity), "survived");
        }
    }

    #[test]
    fn stop_never_signals_a_root_whose_start_changed() {
        let mut unrelated = Command::new("sleep").arg("30").spawn().unwrap();
        let recorded = Identity {
            pid: unrelated.id(),
            start: Identity::current(unrelated.id()).unwrap().start + 1,
        };

        stop_tree("web", recorded).unwrap();

        assert!(unrelated.try_wait().unwrap().is_none(), "it was signalled");
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }

    #[test]
    fn forgetting_a_run_keeps_the_record_of_one_that_registered_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let mut gone = Command::new("true").spawn().unwrap();
        gone.wait().unwrap();
        let first = RunningService {
            run: Some(RunIdentity {
                run_id: RunId::generate().unwrap(),
                start: 1,
                sid: None,
                pgid: None,
                boot_id: process::boot_id().unwrap(),
                last_alive_at: 1,
                output: None,
            }),
            process_started_at: None,
            ..legacy_record(gone.id(), 0, None)
        };
        let registering = std::sync::Arc::new(RunningServiceStore::new(dir.path().to_path_buf()));
        registering.write(&first).unwrap();
        let lock = std::sync::Arc::new(registering.lock(&key("web")).unwrap());
        let second = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut forgetting = RunningServiceStore::new(dir.path().to_path_buf());
        let (store, held, registered) = (registering.clone(), lock.clone(), second.clone());
        forgetting.before_forget = Some(Box::new(move || {
            let run = store.register(&held, run("web", None)).unwrap();
            *registered.lock().unwrap() = Some(run);
        }));

        forgetting.stop_and_forget(&key("web")).unwrap();

        let second = second
            .lock()
            .unwrap()
            .clone()
            .expect("the hook did not run");
        let current = registering
            .get(&key("web"))
            .unwrap()
            .expect("the new run was deleted");
        assert_eq!(current.run_id(), second.run_id());
        assert!(registering.run_dir(second.run_id().unwrap()).exists());
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
