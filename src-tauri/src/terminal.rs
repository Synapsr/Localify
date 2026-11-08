//! The terminal panel's sessions. A shell tab is an ordinary interactive
//! shell. A service tab is the run itself: a run PTY with `localify dev` as
//! its only child (docs/remote-machines.md, "A run PTY, and a stream per
//! run"). Nothing is ever typed into a shell to start a service, and a
//! service is stopped through its run's session, never with a Ctrl-C.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use localify::desktop::DesktopApi;
use localify::desktop::dto::{
    BootstrapDto, ProjectEnvironmentInfoDto, ProjectInfoDto, ProjectServiceInfoDto,
    ServiceRunCommand,
};
use localify::desktop::pty::WindowSize;
use localify::desktop::run_pty::{RunPty, RunPtyEnd, RunPtyLaunch, run_environment};
use localify::desktop::shell_environment::{ShellEnvironment, ShellEnvironmentDto};
use localify::domain::launch_profile::LaunchProfileName;
use localify::infrastructure::running_services::RunningServiceStore;
use localify::infrastructure::shell_env;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

pub const TERMINAL_OUTPUT_EVENT: &str = "terminal-output";
pub const TERMINAL_EXIT_EVENT: &str = "terminal-exit";
/// A session changed: a service tab's run ended and was finalized.
pub const TERMINAL_SESSION_EVENT: &str = "terminal-session";
pub const SHELL_ENVIRONMENT_EVENT: &str = "shell-environment";

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
    /// Why the run could not be ended or finalized.
    pub error: Option<String>,
}

impl From<RunPtyEnd> for TerminalRunEndDto {
    fn from(end: RunPtyEnd) -> Self {
        Self {
            code: end.code,
            stopped: end.stopped(),
            at: end.at,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalSessionStatus {
    Running,
    Exited,
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
    pub exit_code: u32,
    pub signal: Option<String>,
}

type Sessions = Arc<Mutex<HashMap<String, TerminalProcess>>>;

/// Shared by every Tauri command; cheap to clone into a blocking task.
#[derive(Clone)]
pub struct TerminalManager {
    sessions: Sessions,
    next_id: Arc<AtomicU64>,
    shell_environment: Arc<ShellEnvironment>,
}

struct TerminalProcess {
    dto: TerminalSessionDto,
    process: SessionProcess,
}

enum SessionProcess {
    Shell {
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        killer: Box<dyn ChildKiller + Send + Sync>,
    },
    Run(Arc<RunPty>),
}

struct ResolvedTerminalTarget {
    target: TerminalSessionTargetDto,
    working_directory: PathBuf,
    title: String,
    command: Option<ServiceRunCommand>,
}

/// A service tab to start: what it runs, and the tab it takes over.
struct RunRequest {
    resolved: ResolvedTerminalTarget,
    profile: Option<String>,
    size: WindowSize,
    replaces: Option<String>,
}

impl TerminalManager {
    /// `executable` is the pinned `localify`, which also prints the shell
    /// environment.
    pub fn new(executable: PathBuf) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            shell_environment: Arc::new(ShellEnvironment::new(executable, shell_env::user_shell())),
        }
    }

    /// Reads the shell environment in the background, so the first launch
    /// does not wait for it, and tells the panel how it went.
    pub fn warm_shell_environment<R: Runtime + 'static>(&self, app: AppHandle<R>) {
        let shell_environment = Arc::clone(&self.shell_environment);
        thread::spawn(move || {
            shell_environment.environment();
            let _ = app.emit(SHELL_ENVIRONMENT_EVENT, shell_environment.status());
        });
    }

    pub fn shell_environment(&self) -> ShellEnvironmentDto {
        self.shell_environment.status()
    }

    /// Reads the shell environment again; later launches use the new one.
    pub fn reload_shell_environment<R: Runtime>(&self, app: &AppHandle<R>) -> ShellEnvironmentDto {
        let _ = app.emit(SHELL_ENVIRONMENT_EVENT, self.shell_environment.status());
        let status = self.shell_environment.reload();
        let _ = app.emit(SHELL_ENVIRONMENT_EVENT, status.clone());
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
    pub fn create_session<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
        api: &DesktopApi,
        request: TerminalCreateSessionRequest,
    ) -> Result<TerminalSessionDto, String> {
        let resolved = resolve_terminal_target(api, &request.target, None)?;
        self.create_shell(app, resolved, request.cols, request.rows)
    }

    /// Starts the service in a new run PTY. An ended service tab of it is
    /// taken over; one still running is returned as it is.
    pub fn launch_service<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
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
        let session = self.start_run(
            app,
            RunRequest {
                resolved,
                profile: request.profile,
                size: window_size(request.cols, request.rows)?,
                replaces: replaces.clone(),
            },
        )?;
        Ok(TerminalLaunchServiceResultDto {
            session,
            replaced: replaces,
        })
    }

    /// Stops the tab's run, waits until it has ended, then starts the service
    /// again on the same profile in a fresh run PTY that takes the tab over.
    pub fn restart_service<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
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
        self.stop_run(&request.session_id)?;
        let resolved =
            resolve_terminal_target(api, &request_target(&session.target), profile.as_deref())?;
        let new_session = self.start_run(
            app,
            RunRequest {
                resolved,
                profile,
                size: window_size(request.cols, request.rows)?,
                replaces: Some(request.session_id.clone()),
            },
        )?;
        Ok(TerminalLaunchServiceResultDto {
            session: new_session,
            replaced: Some(request.session_id),
        })
    }

    /// Stops a service tab's run through its session and process group, as
    /// `localify stop` does, and returns once its output is finalized. The
    /// tab then shows how it ended.
    pub fn stop_service(&self, request: TerminalSessionRequest) -> Result<(), String> {
        self.stop_run(&request.session_id)
    }

    fn stop_run(&self, session_id: &str) -> Result<(), String> {
        let run = match &self
            .session_guard()
            .get(session_id)
            .map(|session| &session.process)
        {
            Some(SessionProcess::Run(run)) => Arc::clone(run),
            Some(SessionProcess::Shell { .. }) => {
                return Err("This tab is a shell, not a service.".into());
            }
            None => return Err(missing_session(session_id)),
        };
        if run.has_ended() {
            return Ok(());
        }
        let store = RunningServiceStore::new_default().map_err(|error| error.to_string())?;
        let record = store
            .list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|record| record.run_id() == Some(run.run_id()));
        // No record: `localify dev` has not registered the run yet, or
        // refused it. Its tab shows the end once its executor exits.
        if let Some(record) = record {
            store.stop(&record).map_err(|error| format!("{error:#}"))?;
        }
        Ok(())
    }

    fn start_run<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
        request: RunRequest,
    ) -> Result<TerminalSessionDto, String> {
        let RunRequest {
            resolved,
            profile,
            size,
            replaces,
        } = request;
        let command = resolved
            .command
            .clone()
            .ok_or_else(|| "This terminal target cannot be launched.".to_string())?;
        let store = RunningServiceStore::new_default().map_err(|error| error.to_string())?;
        let environment = run_environment(
            self.shell_environment.environment(),
            &resolved.working_directory,
        );
        let id = self.new_id();
        let output_app = app.clone();
        let output_id = id.clone();
        let end_app = app;
        let end_id = id.clone();
        let sessions = Arc::clone(&self.sessions);
        let run = RunPty::spawn(
            Arc::new(store),
            RunPtyLaunch {
                executable: command.executable,
                args: command.args,
                working_directory: resolved.working_directory.clone(),
                environment,
                size,
            },
            move |bytes| emit_output(&output_app, &output_id, bytes),
            move |end| record_end(&end_app, &sessions, &end_id, end),
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
        let mut sessions = self.session_guard();
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

    fn create_shell<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
        resolved: ResolvedTerminalTarget,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalSessionDto, String> {
        let size = window_size(cols, rows)?;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Failed to create terminal session: {error}"))?;
        let mut command = CommandBuilder::new_default_prog();
        command.cwd(resolved.working_directory.as_os_str());
        command.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("Failed to start terminal shell: {error}"))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Failed to attach terminal output: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Failed to attach terminal input: {error}"))?;
        let killer = child.clone_killer();

        let id = self.new_id();
        let dto = TerminalSessionDto {
            id: id.clone(),
            title: resolved.title,
            working_directory: resolved.working_directory.display().to_string(),
            target: resolved.target,
            status: TerminalSessionStatus::Running,
            run: None,
        };
        self.session_guard().insert(
            id.clone(),
            TerminalProcess {
                dto: dto.clone(),
                process: SessionProcess::Shell {
                    master: pair.master,
                    writer,
                    killer,
                },
            },
        );
        drop(pair.slave);

        // A shell's output goes to its tab only: it is never written to a
        // stream, so nothing typed in a shell tab is ever exported.
        let output_app = app.clone();
        let output_id = id.clone();
        thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    return;
                }
                emit_output(&output_app, &output_id, &buffer[..count]);
            }
        });
        spawn_shell_wait_thread(app, Arc::clone(&self.sessions), id, child);
        Ok(dto)
    }

    pub fn write(&self, request: TerminalWriteRequest) -> Result<(), String> {
        let mut sessions = self.session_guard();
        let session = sessions
            .get_mut(&request.session_id)
            .ok_or_else(|| missing_session(&request.session_id))?;
        match &mut session.process {
            SessionProcess::Shell { writer, .. } => writer
                .write_all(request.data.as_bytes())
                .and_then(|()| writer.flush())
                .map_err(|error| format!("Failed to write terminal input: {error}")),
            SessionProcess::Run(run) => run
                .write(request.data.as_bytes())
                .map_err(|error| format!("{error:#}")),
        }
    }

    pub fn resize(&self, request: TerminalResizeRequest) -> Result<(), String> {
        let size = window_size(request.cols, request.rows)?;
        let sessions = self.session_guard();
        let session = sessions
            .get(&request.session_id)
            .ok_or_else(|| missing_session(&request.session_id))?;
        match &session.process {
            SessionProcess::Shell { master, .. } => master
                .resize(PtySize {
                    rows: size.rows,
                    cols: size.cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|error| format!("Failed to resize terminal session: {error}")),
            SessionProcess::Run(run) => run.resize(size).map_err(|error| format!("{error:#}")),
        }
    }

    /// Closes a tab. A service tab whose run still goes is stopped first,
    /// through its session.
    pub fn close(&self, request: TerminalSessionRequest) -> Result<(), String> {
        let is_run = matches!(
            self.session_guard()
                .get(&request.session_id)
                .map(|session| &session.process),
            Some(SessionProcess::Run(_))
        );
        if is_run {
            self.stop_run(&request.session_id)?;
        }
        let session = self
            .session_guard()
            .remove(&request.session_id)
            .ok_or_else(|| missing_session(&request.session_id))?;
        match session.process {
            SessionProcess::Shell { mut killer, .. } => killer
                .kill()
                .map_err(|error| format!("Failed to close terminal session: {error}")),
            SessionProcess::Run(_) => Ok(()),
        }
    }

    fn new_id(&self) -> String {
        format!("terminal-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn session_guard(&self) -> MutexGuard<'_, HashMap<String, TerminalProcess>> {
        lock_sessions(&self.sessions)
    }

    fn session_dto(&self, session_id: &str) -> Option<TerminalSessionDto> {
        self.session_guard()
            .get(session_id)
            .map(|session| session.dto.clone())
    }

    /// The service tab of this service, running or ended.
    fn find_service_session(
        &self,
        target: &TerminalSessionTargetDto,
    ) -> Option<TerminalSessionDto> {
        let sessions = self.session_guard();
        let mut tabs = sessions.values().filter(|session| {
            session.dto.run.is_some() && session_target_matches(&session.dto.target, target)
        });
        let first = tabs.next()?;
        let running = std::iter::once(first)
            .chain(tabs)
            .find(|session| session.dto.status == TerminalSessionStatus::Running);
        Some(running.unwrap_or(first).dto.clone())
    }
}

fn emit_output<R: Runtime>(app: &AppHandle<R>, session_id: &str, bytes: &[u8]) {
    let _ = app.emit(
        TERMINAL_OUTPUT_EVENT,
        TerminalOutputPayload {
            session_id: session_id.to_string(),
            bytes: bytes.to_vec(),
        },
    );
}

/// A service tab's run ended and its output is finalized: the tab shows how.
fn record_end<R: Runtime>(
    app: &AppHandle<R>,
    sessions: &Sessions,
    session_id: &str,
    end: RunPtyEnd,
) {
    let dto = {
        let mut sessions = lock_sessions(sessions);
        let Some(session) = sessions.get_mut(session_id) else {
            return;
        };
        session.dto.status = TerminalSessionStatus::Exited;
        if let Some(run) = session.dto.run.as_mut() {
            run.ended = Some(end.into());
        }
        session.dto.clone()
    };
    let _ = app.emit(TERMINAL_SESSION_EVENT, dto);
}

fn lock_sessions(sessions: &Sessions) -> MutexGuard<'_, HashMap<String, TerminalProcess>> {
    match sessions.lock() {
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

fn spawn_shell_wait_thread<R: Runtime + 'static>(
    app: AppHandle<R>,
    sessions: Sessions,
    session_id: String,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    thread::spawn(move || {
        let status = child.wait();
        if let Some(session) = lock_sessions(&sessions).get_mut(&session_id) {
            session.dto.status = TerminalSessionStatus::Exited;
        }

        if let Ok(status) = status {
            let _ = app.emit(
                TERMINAL_EXIT_EVENT,
                TerminalExitPayload {
                    session_id,
                    exit_code: status.exit_code(),
                    signal: status.signal().map(ToOwned::to_owned),
                },
            );
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
        .ok_or_else(|| {
            format!(
                "Service '{}' was not found for project '{}'.",
                service_id, project_id
            )
        })
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
    use super::{
        TerminalRunEndDto, TerminalSessionTargetDto, TerminalTargetKind, session_target_matches,
    };
    use localify::desktop::run_pty::RunPtyEnd;

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
            error: None,
        });

        assert!(end.stopped);
        assert_eq!(end.code, Some(143));
    }
}
