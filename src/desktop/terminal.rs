//! The terminal panel's sessions. A shell tab is an ordinary interactive
//! shell. A service tab is the run itself: a run PTY with `localify dev` as
//! its only child (docs/remote-machines.md, "A run PTY, and a stream per
//! run"). Nothing is ever typed into a shell to start a service, and a
//! service is stopped through its run's session, never with a Ctrl-C.
//!
//! The desktop app gives the manager a [`TerminalEvents`] that emits Tauri
//! events; tests give one that records them.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::desktop::DesktopApi;
use crate::desktop::dto::{
    BootstrapDto, ProjectEnvironmentInfoDto, ProjectInfoDto, ProjectServiceInfoDto,
    ServiceRunCommand,
};
use crate::desktop::pty::{self, PtyMaster, WindowSize};
use crate::desktop::run_pty::{
    AppInstallation, RunLaunch, RunPty, RunPtyEnd, RunPtyLaunch, run_environment,
};
use crate::desktop::shell_environment::{ShellEnvironment, ShellEnvironmentDto};
use crate::domain::launch_profile::LaunchProfileName;
use crate::infrastructure::process::{self, Identity, Signal};
use crate::infrastructure::running_services::RunningServiceStore;
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::finish::{KILL_WAIT, STOP_GRACE};

/// How long a stop or a cancellation waits for the run's end to be
/// finalized, after its processes were asked to stop: the grace period, the
/// kill, and the reading of what they left in the PTY.
const END_WAIT: Duration = Duration::from_secs(STOP_GRACE.as_secs() + KILL_WAIT.as_secs() + 10);
/// The size a tab opened for a dispatched launch starts with, the panel's
/// own; the panel resizes it once it shows it.
const DISPATCHED_COLS: u16 = 120;
const DISPATCHED_ROWS: u16 = 32;

/// Where the manager's sessions report what happens to them.
pub trait TerminalEvents: Send + Sync + 'static {
    /// Bytes a session's terminal showed.
    fn output(&self, payload: TerminalOutputPayload);
    /// A shell tab's shell exited.
    fn exit(&self, payload: TerminalExitPayload);
    /// A session changed: a service tab's run ended and was finalized.
    fn session(&self, session: TerminalSessionDto);
    /// A service tab opened for a launch handed to the app, not from the
    /// window: the window adds it without taking focus.
    fn opened(&self, opened: TerminalOpenedPayload);
    fn shell_environment(&self, status: ShellEnvironmentDto);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalTargetKind {
    Project,
    ProjectEnvironment,
    Service,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTargetRequest {
    pub kind: TerminalTargetKind,
    pub project_id: String,
    pub environment_slug: String,
    #[serde(default)]
    pub service_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionTargetDto {
    pub kind: TerminalTargetKind,
    pub project_id: String,
    pub project_name: String,
    pub environment_slug: String,
    pub environment_name: String,
    #[serde(default)]
    pub service_id: Option<String>,
    #[serde(default)]
    pub service_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateSessionRequest {
    pub target: TerminalTargetRequest,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLaunchServiceRequest {
    pub target: TerminalTargetRequest,
    pub cols: u16,
    pub rows: u16,
    /// Launch profile from the project's `localify.toml`; `None` launches
    /// with the `default` profile, if any.
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRestartServiceRequest {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWriteRequest {
    pub session_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResizeRequest {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionDto {
    pub id: String,
    pub title: String,
    pub working_directory: String,
    pub target: TerminalSessionTargetDto,
    pub status: TerminalSessionStatus,
    /// Set for a service tab: the run it shows.
    pub run: Option<TerminalRunDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRunDto {
    pub run_id: String,
    pub profile: Option<String>,
    /// Set once the run ended and its output was finalized.
    pub ended: Option<TerminalRunEndDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRunEndDto {
    /// `localify dev`'s exit code.
    pub code: Option<i32>,
    /// Ended by a signal (a stop, Ctrl-C) rather than on its own.
    pub stopped: bool,
    /// Unix seconds.
    pub at: u64,
    /// Whether `localify dev` registered the run.
    pub registered: bool,
    /// Why the run could not be ended or finalized, or its output was cut off.
    pub error: Option<String>,
}

impl From<RunPtyEnd> for TerminalRunEndDto {
    fn from(end: RunPtyEnd) -> Self {
        Self {
            code: end.code,
            stopped: end.stopped(),
            at: end.at,
            registered: end.registered,
            error: end.error,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLaunchServiceResultDto {
    pub session: TerminalSessionDto,
    /// The ended service tab this run took over, if any.
    pub replaced: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalSessionStatus {
    Running,
    Exited,
}

/// A service tab the app opened for a dispatched launch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenedPayload {
    pub session: TerminalSessionDto,
    /// The tab it took over, if any.
    pub replaced: Option<String>,
    /// The paired machine that asked, shown on the tab for a few seconds.
    pub started_by: Option<String>,
}

/// A launch handed to the app over its bridge: its id and expiry come from
/// its dispatcher.
#[derive(Debug, Clone)]
pub struct DispatchedLaunch {
    pub target: TerminalTargetRequest,
    pub profile: Option<String>,
    pub launch: RunLaunch,
    /// The name of the paired machine that asked.
    pub started_by: Option<String>,
}

/// Why a dispatched launch was not started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchFailure {
    /// The ids name no service of this machine's configuration.
    UnknownService(String),
    /// Its run PTY could not be started.
    Failed(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputPayload {
    pub session_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExitPayload {
    pub session_id: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

type Sessions = Arc<Mutex<HashMap<String, TerminalProcess>>>;

/// Shared by every Tauri command; cheap to clone into a blocking task.
#[derive(Clone)]
pub struct TerminalManager {
    sessions: Sessions,
    next_id: Arc<AtomicU64>,
    shell_environment: Arc<ShellEnvironment>,
    store: Arc<RunningServiceStore>,
    installation: Arc<AppInstallation>,
    events: Arc<dyn TerminalEvents>,
}

struct TerminalProcess {
    dto: TerminalSessionDto,
    process: SessionProcess,
}

enum SessionProcess {
    Shell(Arc<ShellTab>),
    Run(Arc<RunPty>),
}

/// An ordinary shell in a PTY of its own. Its output goes to its tab only:
/// it is never written to a stream, so nothing typed in a shell tab is ever
/// exported.
struct ShellTab {
    master: PtyMaster,
    writer: Mutex<File>,
    shell: Option<Identity>,
}

struct ResolvedTerminalTarget {
    target: TerminalSessionTargetDto,
    working_directory: PathBuf,
    title: String,
    command: Option<ServiceRunCommand>,
}

/// A service tab to start: what it runs, the tab it takes over, and how its
/// launch is fenced.
struct RunRequest {
    resolved: ResolvedTerminalTarget,
    profile: Option<String>,
    size: WindowSize,
    replaces: Option<String>,
    fence: Fence,
}

/// Where a run PTY's launch id and expiry come from.
enum Fence {
    /// The app's own launch, made right before its run PTY starts, with the
    /// run it replaces once that run has stopped.
    Own { replaces: Option<RunId> },
    /// A launch handed to the app, as its dispatcher made it.
    Dispatched(RunLaunch),
}

impl Fence {
    fn launch(self) -> Result<RunLaunch, String> {
        match self {
            Self::Own { replaces } => {
                RunLaunch::own(replaces).map_err(|error| format!("{error:#}"))
            }
            Self::Dispatched(launch) => Ok(launch),
        }
    }
}

impl TerminalManager {
    /// `executable` is the pinned `localify`, which also prints the shell
    /// environment; `shell` the user's login shell; `store` where runs are
    /// recorded; `installation` what every run gets of the app's own.
    pub fn new(
        executable: PathBuf,
        shell: PathBuf,
        store: RunningServiceStore,
        installation: AppInstallation,
        events: Arc<dyn TerminalEvents>,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            shell_environment: Arc::new(ShellEnvironment::new(executable, shell)),
            store: Arc::new(store),
            installation: Arc::new(installation),
            events,
        }
    }

    /// Reads the shell environment in the background, so the first launch
    /// does not wait for it, and tells the panel how it went.
    pub fn warm_shell_environment(&self) {
        let shell_environment = Arc::clone(&self.shell_environment);
        let events = Arc::clone(&self.events);
        thread::spawn(move || {
            shell_environment.environment();
            events.shell_environment(shell_environment.status());
        });
    }

    pub fn shell_environment(&self) -> ShellEnvironmentDto {
        self.shell_environment.status()
    }

    /// Reads the shell environment again; later launches use the new one.
    pub fn reload_shell_environment(&self) -> ShellEnvironmentDto {
        let status = self.shell_environment.reload();
        self.events.shell_environment(status.clone());
        status
    }

    pub fn list_sessions(&self) -> Vec<TerminalSessionDto> {
        self.session_guard()
            .values()
            .map(|session| session.dto.clone())
            .collect()
    }

    /// Opens an ordinary shell tab. For a service target, in the service's
    /// folder: what `Open a terminal here` opens next to a service tab.
    pub fn create_session(
        &self,
        api: &DesktopApi,
        request: TerminalCreateSessionRequest,
    ) -> Result<TerminalSessionDto, String> {
        let resolved = resolve_terminal_target(api, &request.target, None)?;
        self.create_shell(resolved, window_size(request.cols, request.rows)?)
    }

    /// Starts the service in a new run PTY. An ended service tab of it is
    /// taken over; one still running is returned as it is.
    pub fn launch_service(
        &self,
        api: &DesktopApi,
        request: TerminalLaunchServiceRequest,
    ) -> Result<TerminalLaunchServiceResultDto, String> {
        if request.target.kind != TerminalTargetKind::Service {
            return Err("Only service targets can be launched.".into());
        }
        if let Some(profile) = request.profile.as_deref() {
            LaunchProfileName::new(profile).map_err(|error| error.to_string())?;
        }
        let resolved = resolve_terminal_target(api, &request.target, request.profile.as_deref())?;
        let existing = self.find_service_session(&resolved.target);
        if let Some(session) = existing
            .as_ref()
            .filter(|session| session.status == TerminalSessionStatus::Running)
        {
            return Ok(TerminalLaunchServiceResultDto {
                session: session.clone(),
                replaced: None,
            });
        }
        let replaces = existing.map(|session| session.id);
        let session = self.start_run(RunRequest {
            resolved,
            profile: request.profile,
            size: window_size(request.cols, request.rows)?,
            replaces: replaces.clone(),
            // An ended tab is taken over; no run is replaced.
            fence: Fence::Own { replaces: None },
        })?;
        Ok(TerminalLaunchServiceResultDto {
            session,
            replaced: replaces,
        })
    }

    /// Stops the tab's run, waits until it has ended, then starts the service
    /// again on the same profile in a fresh run PTY that takes the tab over.
    pub fn restart_service(
        &self,
        api: &DesktopApi,
        request: TerminalRestartServiceRequest,
    ) -> Result<TerminalLaunchServiceResultDto, String> {
        let session = self
            .session_dto(&request.session_id)
            .ok_or_else(|| missing_session(&request.session_id))?;
        let Some(run) = &session.run else {
            return Err("Only a service tab can be restarted.".into());
        };
        let profile = run.profile.clone();
        let stopped = self.stop_run(&request.session_id)?;
        let resolved =
            resolve_terminal_target(api, &request_target(&session.target), profile.as_deref())?;
        let new_session = self.start_run(RunRequest {
            resolved,
            profile,
            size: window_size(request.cols, request.rows)?,
            replaces: Some(request.session_id.clone()),
            fence: Fence::Own { replaces: stopped },
        })?;
        Ok(TerminalLaunchServiceResultDto {
            session: new_session,
            replaced: Some(request.session_id),
        })
    }

    /// Stops a service tab's run through its session and process group, as
    /// `localify stop` does, and returns once it has ended and its output is
    /// finalized. The tab then shows how it ended.
    pub fn stop_service(&self, request: TerminalSessionRequest) -> Result<(), String> {
        self.stop_run(&request.session_id).map(|_| ())
    }

    /// Starts a launch handed to the app over its bridge in a new run PTY,
    /// fenced by the launch's own id and expiry, and returns once it is
    /// spawned: `localify dev` decides, under the service's lock, whether it
    /// starts. The tab of the run it replaces is taken over; otherwise it
    /// opens a new one. The window adds it without taking focus.
    pub fn launch_dispatched(
        &self,
        api: &DesktopApi,
        request: DispatchedLaunch,
    ) -> Result<TerminalSessionDto, DispatchFailure> {
        if let Some(profile) = request.profile.as_deref() {
            LaunchProfileName::new(profile)
                .map_err(|error| DispatchFailure::Failed(error.to_string()))?;
        }
        let resolved = resolve_terminal_target(api, &request.target, request.profile.as_deref())
            .map_err(DispatchFailure::UnknownService)?;
        let replaced = request
            .launch
            .replaces
            .as_ref()
            .and_then(|run_id| self.find_run_session(run_id));
        let session = self
            .start_run(RunRequest {
                resolved,
                profile: request.profile,
                size: window_size(DISPATCHED_COLS, DISPATCHED_ROWS)
                    .map_err(DispatchFailure::Failed)?,
                replaces: replaced.clone(),
                fence: Fence::Dispatched(request.launch),
            })
            .map_err(DispatchFailure::Failed)?;
        self.events.opened(TerminalOpenedPayload {
            session: session.clone(),
            replaced,
            started_by: request.started_by,
        });
        Ok(session)
    }

    /// Ends a tab's run and waits for its end. A run with no record is a
    /// launch still preparing, such as resolving its profile's sources: it is
    /// cancelled through `localify dev`'s own identity, never left to start
    /// unseen. Returns the run it stopped once it had registered, which a
    /// restart then replaces; `None` when it had ended already or never
    /// registered.
    fn stop_run(&self, session_id: &str) -> Result<Option<RunId>, String> {
        let run = match self
            .session_guard()
            .get(session_id)
            .map(|session| &session.process)
        {
            Some(SessionProcess::Run(run)) => Arc::clone(run),
            Some(SessionProcess::Shell(_)) => {
                return Err("This tab is a shell, not a service.".into());
            }
            None => return Err(missing_session(session_id)),
        };
        if run.has_ended() {
            return Ok(None);
        }
        let record = self
            .store
            .list()
            .map_err(|error| format!("{error:#}"))?
            .into_iter()
            .find(|record| record.run_id() == Some(run.run_id()));
        let stopped = match record {
            Some(record) => {
                self.store
                    .stop(&record)
                    .map_err(|error| format!("{error:#}"))?;
                Some(run.run_id().clone())
            }
            None => {
                run.cancel().map_err(|error| format!("{error:#}"))?;
                None
            }
        };
        if run.wait_ended(END_WAIT) {
            Ok(stopped)
        } else {
            Err("The service stopped, but its tab has not finished reading its output.".into())
        }
    }

    fn start_run(&self, request: RunRequest) -> Result<TerminalSessionDto, String> {
        let RunRequest {
            resolved,
            profile,
            size,
            replaces,
            fence,
        } = request;
        let command = resolved
            .command
            .clone()
            .ok_or_else(|| "This terminal target cannot be launched.".to_string())?;
        // Before taking the sessions: the first read of the shell can take
        // seconds.
        let environment = run_environment(
            self.shell_environment.environment(),
            &resolved.working_directory,
            &self.installation,
        );
        // After the shell's read, so it never counts against the expiry.
        let launch = fence.launch()?;
        let id = self.new_id();
        let (output_events, output_id) = (Arc::clone(&self.events), id.clone());
        let (end_events, end_sessions, end_id) = (
            Arc::clone(&self.events),
            Arc::clone(&self.sessions),
            id.clone(),
        );
        // Held until the tab is in: a run that ends at once reports its end
        // to a tab that exists.
        let mut sessions = self.session_guard();
        let run = RunPty::spawn(
            Arc::clone(&self.store),
            RunPtyLaunch {
                executable: command.executable,
                args: command.args,
                working_directory: resolved.working_directory.clone(),
                environment,
                size,
                launch,
            },
            move |bytes| {
                output_events.output(TerminalOutputPayload {
                    session_id: output_id.clone(),
                    bytes: bytes.to_vec(),
                });
            },
            move |end| record_end(end_events.as_ref(), &end_sessions, &end_id, end),
        )
        .map_err(|error| format!("Failed to start the service: {error:#}"))?;
        let dto = TerminalSessionDto {
            id: id.clone(),
            title: resolved.title,
            working_directory: resolved.working_directory.display().to_string(),
            target: resolved.target,
            status: TerminalSessionStatus::Running,
            run: Some(TerminalRunDto {
                run_id: run.run_id().to_string(),
                profile,
                ended: None,
            }),
        };
        if let Some(old) = replaces.as_deref() {
            sessions.remove(old);
        }
        sessions.insert(
            id,
            TerminalProcess {
                dto: dto.clone(),
                process: SessionProcess::Run(Arc::new(run)),
            },
        );
        Ok(dto)
    }

    fn create_shell(
        &self,
        resolved: ResolvedTerminalTarget,
        size: WindowSize,
    ) -> Result<TerminalSessionDto, String> {
        let shell = crate::infrastructure::shell_env::user_shell();
        let mut command = Command::new(&shell);
        // A login shell, as a terminal emulator starts one: `-zsh`.
        let name = shell
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sh".into());
        command
            .arg0(format!("-{name}"))
            .current_dir(&resolved.working_directory)
            .env("TERM", "xterm-256color");
        let (master, child) = pty::spawn(command, size, false)
            .map_err(|error| format!("Failed to start terminal shell: {error:#}"))?;
        let shell_identity = process::process_info(child.id()).map(|info| Identity::of(&info));
        let handles = master
            .try_clone()
            .and_then(|writer| Ok((writer, master.try_clone()?)));
        let (writer, mut reader) =
            handles.map_err(|error| format!("Failed to attach the terminal: {error:#}"))?;

        let id = self.new_id();
        let dto = TerminalSessionDto {
            id: id.clone(),
            title: resolved.title,
            working_directory: resolved.working_directory.display().to_string(),
            target: resolved.target,
            status: TerminalSessionStatus::Running,
            run: None,
        };
        // Held until the tab is in, as for a run.
        let mut sessions = self.session_guard();
        sessions.insert(
            id.clone(),
            TerminalProcess {
                dto: dto.clone(),
                process: SessionProcess::Shell(Arc::new(ShellTab {
                    master,
                    writer: Mutex::new(writer),
                    shell: shell_identity,
                })),
            },
        );
        let (output_events, output_id) = (Arc::clone(&self.events), id.clone());
        thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    return;
                }
                output_events.output(TerminalOutputPayload {
                    session_id: output_id.clone(),
                    bytes: buffer[..count].to_vec(),
                });
            }
        });
        spawn_shell_wait_thread(
            Arc::clone(&self.events),
            Arc::clone(&self.sessions),
            id,
            child,
        );
        Ok(dto)
    }

    pub fn write(&self, request: TerminalWriteRequest) -> Result<(), String> {
        let process = self.process(&request.session_id)?;
        match process {
            SessionProcess::Shell(shell) => {
                let mut writer = lock(&shell.writer);
                writer
                    .write_all(request.data.as_bytes())
                    .and_then(|()| writer.flush())
                    .map_err(|error| format!("Failed to write terminal input: {error}"))
            }
            SessionProcess::Run(run) => run
                .write(request.data.as_bytes())
                .map_err(|error| format!("{error:#}")),
        }
    }

    pub fn resize(&self, request: TerminalResizeRequest) -> Result<(), String> {
        let size = window_size(request.cols, request.rows)?;
        let resized = match self.process(&request.session_id)? {
            SessionProcess::Shell(shell) => shell.master.resize(size),
            SessionProcess::Run(run) => run.resize(size),
        };
        resized.map_err(|error| format!("Failed to resize terminal session: {error:#}"))
    }

    /// Closes a tab. A service tab whose run still goes, or is still
    /// preparing, is stopped first, and the tab goes once it has ended.
    pub fn close(&self, request: TerminalSessionRequest) -> Result<(), String> {
        if let SessionProcess::Run(_) = self.process(&request.session_id)? {
            self.stop_run(&request.session_id)?;
        }
        let session = self
            .session_guard()
            .remove(&request.session_id)
            .ok_or_else(|| missing_session(&request.session_id))?;
        if let SessionProcess::Shell(shell) = session.process
            && let Some(identity) = shell.shell
            && Identity::current(identity.pid) == Some(identity)
        {
            // As a terminal emulator closing its window does.
            process::send_signal(identity.pid, Signal::Hangup);
        }
        Ok(())
    }

    fn process(&self, session_id: &str) -> Result<SessionProcess, String> {
        match self
            .session_guard()
            .get(session_id)
            .map(|session| &session.process)
        {
            Some(SessionProcess::Shell(shell)) => Ok(SessionProcess::Shell(Arc::clone(shell))),
            Some(SessionProcess::Run(run)) => Ok(SessionProcess::Run(Arc::clone(run))),
            None => Err(missing_session(session_id)),
        }
    }

    fn new_id(&self) -> String {
        format!("terminal-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn session_guard(&self) -> MutexGuard<'_, HashMap<String, TerminalProcess>> {
        lock(&self.sessions)
    }

    fn session_dto(&self, session_id: &str) -> Option<TerminalSessionDto> {
        self.session_guard()
            .get(session_id)
            .map(|session| session.dto.clone())
    }

    /// The service tab that shows run `run_id`.
    fn find_run_session(&self, run_id: &RunId) -> Option<String> {
        self.session_guard()
            .values()
            .find(|session| {
                session
                    .dto
                    .run
                    .as_ref()
                    .is_some_and(|run| run.run_id == run_id.as_str())
            })
            .map(|session| session.dto.id.clone())
    }

    /// The service tab of this service: the running one, else an ended one.
    fn find_service_session(
        &self,
        target: &TerminalSessionTargetDto,
    ) -> Option<TerminalSessionDto> {
        let sessions = self.session_guard();
        let tabs: Vec<&TerminalProcess> = sessions
            .values()
            .filter(|session| {
                session.dto.run.is_some() && session_target_matches(&session.dto.target, target)
            })
            .collect();
        tabs.iter()
            .find(|session| session.dto.status == TerminalSessionStatus::Running)
            .or(tabs.first())
            .map(|session| session.dto.clone())
    }
}

/// A service tab's run ended and its output is finalized: the tab keeps how,
/// and shows it.
fn record_end(events: &dyn TerminalEvents, sessions: &Sessions, session_id: &str, end: RunPtyEnd) {
    let dto = {
        let mut sessions = lock(sessions);
        let Some(session) = sessions.get_mut(session_id) else {
            return;
        };
        session.dto.status = TerminalSessionStatus::Exited;
        if let Some(run) = session.dto.run.as_mut() {
            run.ended = Some(end.into());
        }
        session.dto.clone()
    };
    events.session(dto);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn missing_session(session_id: &str) -> String {
    format!("Terminal session '{session_id}' was not found.")
}

fn request_target(target: &TerminalSessionTargetDto) -> TerminalTargetRequest {
    TerminalTargetRequest {
        kind: target.kind.clone(),
        project_id: target.project_id.clone(),
        environment_slug: target.environment_slug.clone(),
        service_id: target.service_id.clone(),
    }
}

fn session_target_matches(
    left: &TerminalSessionTargetDto,
    right: &TerminalSessionTargetDto,
) -> bool {
    left.kind == right.kind
        && left.project_id == right.project_id
        && left.environment_slug == right.environment_slug
        && left.service_id == right.service_id
}

fn spawn_shell_wait_thread(
    events: Arc<dyn TerminalEvents>,
    sessions: Sessions,
    session_id: String,
    mut child: Child,
) {
    thread::spawn(move || {
        let status = child.wait();
        if let Some(session) = lock(&sessions).get_mut(&session_id) {
            session.dto.status = TerminalSessionStatus::Exited;
        }
        if let Ok(status) = status {
            events.exit(TerminalExitPayload {
                session_id,
                exit_code: status.code(),
                signal: status.signal(),
            });
        }
    });
}

fn window_size(cols: u16, rows: u16) -> Result<WindowSize, String> {
    WindowSize::new(cols, rows).map_err(|error| error.to_string())
}

fn resolve_terminal_target(
    api: &DesktopApi,
    request: &TerminalTargetRequest,
    launch_profile: Option<&str>,
) -> Result<ResolvedTerminalTarget, String> {
    let bootstrap = api
        .bootstrap()
        .map_err(|error| format!("Failed to read Localify project data: {error}"))?;
    let environment = find_environment(&bootstrap, request)?;

    if request.kind == TerminalTargetKind::Project {
        let project = find_project(&bootstrap, &request.project_id)?;
        let root_path = project
            .root_path
            .as_deref()
            .ok_or_else(|| format!("Project '{}' has no local folder linked.", project.name))?;
        let root_path = project_working_directory(root_path)?;

        return Ok(ResolvedTerminalTarget {
            title: project.name.clone(),
            working_directory: root_path,
            target: TerminalSessionTargetDto {
                kind: TerminalTargetKind::Project,
                project_id: project.id.clone(),
                project_name: project.name.clone(),
                environment_slug: environment.slug.clone(),
                environment_name: environment.name.clone(),
                service_id: None,
                service_name: None,
            },
            command: None,
        });
    }

    let root_path = environment.root_path.as_deref().ok_or_else(|| {
        format!(
            "Project environment '{}' has no local folder linked.",
            environment.slug
        )
    })?;
    let root_path = existing_directory(root_path)?;

    if request.kind == TerminalTargetKind::ProjectEnvironment {
        return Ok(ResolvedTerminalTarget {
            title: environment.slug.clone(),
            working_directory: root_path,
            target: TerminalSessionTargetDto {
                kind: TerminalTargetKind::ProjectEnvironment,
                project_id: environment.project_id.clone(),
                project_name: environment.project_name.clone(),
                environment_slug: environment.slug.clone(),
                environment_name: environment.name.clone(),
                service_id: None,
                service_name: None,
            },
            command: None,
        });
    }

    let service_id = request
        .service_id
        .as_deref()
        .ok_or_else(|| "A service terminal target must include a service id.".to_string())?;
    ensure_environment_service(environment, service_id)?;
    let service = find_service(&bootstrap, &environment.project_id, service_id)?;
    let working_directory = service_working_directory(&root_path, service)?;

    Ok(ResolvedTerminalTarget {
        title: service.name.clone(),
        working_directory,
        target: TerminalSessionTargetDto {
            kind: TerminalTargetKind::Service,
            project_id: environment.project_id.clone(),
            project_name: environment.project_name.clone(),
            environment_slug: environment.slug.clone(),
            environment_name: environment.name.clone(),
            service_id: Some(service.id.clone()),
            service_name: Some(service.name.clone()),
        },
        command: Some(api.service_run_command(
            &environment.project_id,
            &environment.slug,
            &service.id,
            launch_profile,
        )),
    })
}

fn find_project<'a>(
    bootstrap: &'a BootstrapDto,
    project_id: &str,
) -> Result<&'a ProjectInfoDto, String> {
    bootstrap
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| format!("Project '{project_id}' was not found."))
}

fn find_environment<'a>(
    bootstrap: &'a BootstrapDto,
    request: &TerminalTargetRequest,
) -> Result<&'a ProjectEnvironmentInfoDto, String> {
    bootstrap
        .project_environments
        .iter()
        .find(|environment| {
            environment.project_id == request.project_id
                && environment.slug == request.environment_slug
        })
        .ok_or_else(|| {
            format!(
                "Project environment '{}' was not found for project '{}'.",
                request.environment_slug, request.project_id
            )
        })
}

fn find_service<'a>(
    bootstrap: &'a BootstrapDto,
    project_id: &str,
    service_id: &str,
) -> Result<&'a ProjectServiceInfoDto, String> {
    bootstrap
        .services
        .iter()
        .find(|service| service.project_id == project_id && service.id == service_id)
        .ok_or_else(|| format!("Service '{service_id}' was not found for project '{project_id}'."))
}

fn ensure_environment_service(
    environment: &ProjectEnvironmentInfoDto,
    service_id: &str,
) -> Result<(), String> {
    if environment
        .services
        .iter()
        .any(|service| service.service_id == service_id)
    {
        return Ok(());
    }

    Err(format!(
        "Service '{}' is unavailable in environment '{}'.",
        service_id, environment.slug
    ))
}

fn service_working_directory(
    environment_root: &Path,
    service: &ProjectServiceInfoDto,
) -> Result<PathBuf, String> {
    let Some(path) = service
        .path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    else {
        return Ok(environment_root.to_path_buf());
    };

    let path = Path::new(path);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        environment_root.join(path)
    };
    existing_directory(candidate)
}

fn project_working_directory(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    let directory = existing_directory(path)?;
    Ok(git_worktree_root(&directory).unwrap_or(directory))
}

fn git_worktree_root(directory: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(directory)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let root = String::from_utf8(output.stdout).ok()?;
    let root = root.trim();
    if root.is_empty() {
        return None;
    }

    let root = PathBuf::from(root);
    root.is_dir().then_some(root)
}

fn existing_directory(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    let path = path.as_ref();
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }

    Err(format!(
        "Terminal working directory '{}' does not exist.",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(kind: TerminalTargetKind, service: Option<&str>) -> TerminalSessionTargetDto {
        TerminalSessionTargetDto {
            kind,
            project_id: "store".into(),
            project_name: "store".into(),
            environment_slug: "primary".into(),
            environment_name: "primary".into(),
            service_id: service.map(str::to_string),
            service_name: service.map(str::to_string),
        }
    }

    #[test]
    fn a_service_target_matches_only_its_own_service() {
        let web = target(TerminalTargetKind::Service, Some("web"));

        assert!(session_target_matches(
            &web,
            &target(TerminalTargetKind::Service, Some("web"))
        ));
        assert!(!session_target_matches(
            &web,
            &target(TerminalTargetKind::Service, Some("api"))
        ));
        assert!(!session_target_matches(
            &web,
            &target(TerminalTargetKind::Project, None)
        ));
    }

    #[test]
    fn a_run_stopped_by_a_signal_shows_as_stopped() {
        let end = TerminalRunEndDto::from(RunPtyEnd {
            code: Some(143),
            signal: None,
            at: 1,
            registered: true,
            output: None,
            error: None,
        });

        assert!(end.stopped);
        assert_eq!(end.code, Some(143));
    }
}
