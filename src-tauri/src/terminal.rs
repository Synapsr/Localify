use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use localify::desktop::DesktopApi;
use localify::desktop::dto::{
    BootstrapDto, DomainInfoDto, ProjectEnvironmentInfoDto, ProjectInfoDto, ProjectServiceInfoDto,
};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

pub const TERMINAL_OUTPUT_EVENT: &str = "terminal-output";
pub const TERMINAL_EXIT_EVENT: &str = "terminal-exit";

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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLaunchServiceResultDto {
    pub session: TerminalSessionDto,
    pub created: bool,
    pub command: Vec<String>,
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

pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, TerminalProcess>>>,
    next_id: AtomicU64,
}

struct TerminalProcess {
    dto: TerminalSessionDto,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

struct ResolvedTerminalTarget {
    target: TerminalSessionTargetDto,
    working_directory: PathBuf,
    title: String,
    command: Option<Vec<String>>,
    already_reachable: bool,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn list_sessions(&self) -> Vec<TerminalSessionDto> {
        self.session_guard()
            .values()
            .map(|session| session.dto.clone())
            .collect()
    }

    pub fn create_session<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
        api: &DesktopApi,
        request: TerminalCreateSessionRequest,
    ) -> Result<TerminalSessionDto, String> {
        let resolved = resolve_terminal_target(api, &request.target)?;
        let (dto, _) = self.create_resolved_session(app, resolved, request.cols, request.rows)?;

        Ok(dto)
    }

    pub fn launch_service<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
        api: &DesktopApi,
        request: TerminalLaunchServiceRequest,
    ) -> Result<TerminalLaunchServiceResultDto, String> {
        if request.target.kind != TerminalTargetKind::Service {
            return Err("Only service targets can be launched.".into());
        }

        let resolved = resolve_terminal_target(api, &request.target)?;
        let command = resolved
            .command
            .clone()
            .ok_or_else(|| "This terminal target cannot be launched.".to_string())?;

        if resolved.already_reachable {
            return Err(
                "This service is already reachable. Stop it before launching it again.".into(),
            );
        }

        if let Some(session) = self.find_running_session(&resolved.target) {
            self.write_command(&session.id, &command)?;
            return Ok(TerminalLaunchServiceResultDto {
                session,
                created: false,
                command,
            });
        }

        let (session, _) =
            self.create_resolved_session(app, resolved, request.cols, request.rows)?;
        self.write_command(&session.id, &command)?;

        Ok(TerminalLaunchServiceResultDto {
            session,
            created: true,
            command,
        })
    }

    fn create_resolved_session<R: Runtime + 'static>(
        &self,
        app: AppHandle<R>,
        resolved: ResolvedTerminalTarget,
        cols: u16,
        rows: u16,
    ) -> Result<(TerminalSessionDto, bool), String> {
        let size = terminal_size(cols, rows)?;
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|error| format!("Failed to create terminal session: {error}"))?;
        let mut command = CommandBuilder::new_default_prog();
        command.cwd(resolved.working_directory.as_os_str());
        command.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("Failed to start terminal shell: {error}"))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Failed to attach terminal output: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Failed to attach terminal input: {error}"))?;
        let killer = child.clone_killer();

        let id = format!("terminal-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let dto = TerminalSessionDto {
            id: id.clone(),
            title: resolved.title,
            working_directory: resolved.working_directory.display().to_string(),
            target: resolved.target,
            status: TerminalSessionStatus::Running,
        };

        self.session_guard().insert(
            id.clone(),
            TerminalProcess {
                dto: dto.clone(),
                master: pair.master,
                writer,
                killer,
            },
        );

        drop(pair.slave);
        spawn_output_thread(app.clone(), id.clone(), reader);
        spawn_wait_thread(app, Arc::clone(&self.sessions), id, child);

        Ok((dto, true))
    }

    pub fn write(&self, request: TerminalWriteRequest) -> Result<(), String> {
        let mut sessions = self.session_guard();
        let session = sessions
            .get_mut(&request.session_id)
            .ok_or_else(|| format!("Terminal session '{}' was not found.", request.session_id))?;
        session
            .writer
            .write_all(request.data.as_bytes())
            .map_err(|error| format!("Failed to write terminal input: {error}"))?;
        session
            .writer
            .flush()
            .map_err(|error| format!("Failed to flush terminal input: {error}"))
    }

    pub fn resize(&self, request: TerminalResizeRequest) -> Result<(), String> {
        let size = terminal_size(request.cols, request.rows)?;
        let sessions = self.session_guard();
        let session = sessions
            .get(&request.session_id)
            .ok_or_else(|| format!("Terminal session '{}' was not found.", request.session_id))?;
        session
            .master
            .resize(size)
            .map_err(|error| format!("Failed to resize terminal session: {error}"))
    }

    pub fn interrupt(&self, request: TerminalSessionRequest) -> Result<(), String> {
        self.write(TerminalWriteRequest {
            session_id: request.session_id,
            data: "\u{3}".into(),
        })
    }

    pub fn close(&self, request: TerminalSessionRequest) -> Result<(), String> {
        let mut session = self
            .session_guard()
            .remove(&request.session_id)
            .ok_or_else(|| format!("Terminal session '{}' was not found.", request.session_id))?;
        session
            .killer
            .kill()
            .map_err(|error| format!("Failed to close terminal session: {error}"))
    }

    fn session_guard(&self) -> MutexGuard<'_, HashMap<String, TerminalProcess>> {
        match self.sessions.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn find_running_session(
        &self,
        target: &TerminalSessionTargetDto,
    ) -> Option<TerminalSessionDto> {
        self.session_guard()
            .values()
            .find(|session| {
                session.dto.status == TerminalSessionStatus::Running
                    && session_target_matches(&session.dto.target, target)
            })
            .map(|session| session.dto.clone())
    }

    fn write_command(&self, session_id: &str, command: &[String]) -> Result<(), String> {
        self.write(TerminalWriteRequest {
            session_id: session_id.to_string(),
            data: format!("{}\r", shell_command(command)),
        })
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

fn spawn_output_thread<R: Runtime + 'static>(
    app: AppHandle<R>,
    session_id: String,
    mut reader: Box<dyn Read + Send>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return,
                Ok(count) => {
                    let _ = app.emit(
                        TERMINAL_OUTPUT_EVENT,
                        TerminalOutputPayload {
                            session_id: session_id.clone(),
                            bytes: buffer[..count].to_vec(),
                        },
                    );
                }
                Err(_) => return,
            }
        }
    });
}

fn spawn_wait_thread<R: Runtime + 'static>(
    app: AppHandle<R>,
    sessions: Arc<Mutex<HashMap<String, TerminalProcess>>>,
    session_id: String,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    thread::spawn(move || {
        let status = child.wait();
        {
            let mut sessions = match sessions.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(session) = sessions.get_mut(&session_id) {
                session.dto.status = TerminalSessionStatus::Exited;
            }
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

fn terminal_size(cols: u16, rows: u16) -> Result<PtySize, String> {
    if cols == 0 || rows == 0 {
        return Err("Terminal size must include at least one row and column.".into());
    }

    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}

fn resolve_terminal_target(
    api: &DesktopApi,
    request: &TerminalTargetRequest,
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
            already_reachable: false,
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
            already_reachable: false,
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
        already_reachable: service_reachable(&bootstrap, environment, service),
        target: TerminalSessionTargetDto {
            kind: TerminalTargetKind::Service,
            project_id: environment.project_id.clone(),
            project_name: environment.project_name.clone(),
            environment_slug: environment.slug.clone(),
            environment_name: environment.name.clone(),
            service_id: Some(service.id.clone()),
            service_name: Some(service.name.clone()),
        },
        command: Some(api.service_dev_command(
            &environment.project_id,
            &environment.slug,
            &service.id,
        )),
    })
}

fn service_reachable(
    bootstrap: &BootstrapDto,
    environment: &ProjectEnvironmentInfoDto,
    service: &ProjectServiceInfoDto,
) -> bool {
    service_domain_reachable(&bootstrap.domains, environment, service)
}

fn service_domain_reachable(
    domains: &[DomainInfoDto],
    environment: &ProjectEnvironmentInfoDto,
    service: &ProjectServiceInfoDto,
) -> bool {
    let Some(environment_service) = environment
        .services
        .iter()
        .find(|environment_service| environment_service.service_id == service.id)
    else {
        return false;
    };

    domain_reachable(domains, &environment_service.domain)
}

fn domain_reachable(domains: &[DomainInfoDto], pattern: &str) -> bool {
    domains
        .iter()
        .any(|domain| domain.registration.pattern == pattern && domain.target_reachable)
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

fn shell_command(command: &[String]) -> String {
    command
        .iter()
        .map(|part| shell_quote(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }

    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"@%_+=:,./-".contains(&byte))
    {
        return value.into();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::{service_domain_reachable, shell_command, shell_quote};
    use localify::desktop::dto::{
        DomainInfoDto, ProjectEnvironmentInfoDto, ProjectEnvironmentServiceInfoDto,
        ProjectServiceInfoDto, RegistrationDto, RouteDto,
    };

    #[test]
    fn quotes_shell_arguments_with_spaces() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
    }

    #[test]
    fn preserves_simple_shell_arguments() {
        assert_eq!(
            shell_command(&["pnpm".into(), "run".into(), "dev".into()]),
            "pnpm run dev"
        );
    }

    #[test]
    fn escapes_single_quotes() {
        assert_eq!(shell_quote("it's ok"), "'it'\"'\"'s ok'");
    }

    #[test]
    fn service_reachability_uses_environment_domain() {
        let service = project_service("web", "*.store.localify", "127.0.0.1:61092");
        let primary = project_environment("primary", "*.store.localify");
        let worktree = project_environment(
            "fix-login-6a2a8fd4",
            "fix-login-6a2a8fd4.store.localify.test",
        );
        let domains = vec![
            domain("*.store.localify", "127.0.0.1:61092", true),
            domain(
                "fix-login-6a2a8fd4.store.localify.test",
                "127.0.0.1:63612",
                false,
            ),
        ];

        assert!(service_domain_reachable(&domains, &primary, &service));
        assert!(!service_domain_reachable(&domains, &worktree, &service));
    }

    fn project_service(id: &str, primary_domain: &str, target: &str) -> ProjectServiceInfoDto {
        ProjectServiceInfoDto {
            id: id.into(),
            name: id.into(),
            project_id: "store".into(),
            project_name: "store".into(),
            project_enabled: true,
            primary_domain: primary_domain.into(),
            url: format!("https://{primary_domain}"),
            domains: vec![primary_domain.into()],
            path: Some("apps/web".into()),
            command: None,
            target: Some(target.into()),
            inferred: false,
        }
    }

    fn project_environment(slug: &str, domain: &str) -> ProjectEnvironmentInfoDto {
        ProjectEnvironmentInfoDto {
            project_id: "store".into(),
            project_name: "store".into(),
            name: slug.into(),
            slug: slug.into(),
            primary: slug == "primary",
            root_path: Some("/Users/dev/Developer/acme/Store".into()),
            git_common_dir: None,
            git_branch: None,
            services: vec![ProjectEnvironmentServiceInfoDto {
                service_id: "web".into(),
                domain: domain.into(),
            }],
        }
    }

    fn domain(pattern: &str, target: &str, target_reachable: bool) -> DomainInfoDto {
        DomainInfoDto {
            registration: RegistrationDto {
                pattern: pattern.into(),
                domain: pattern.trim_start_matches("*.").into(),
                wildcard: pattern.starts_with("*."),
                https_enabled: true,
                source: "config".into(),
                routes: vec![RouteDto {
                    path: "/".into(),
                    target: target.into(),
                    target_kind: "proxy".into(),
                }],
            },
            has_cert: false,
            cert_trusted: None,
            project_id: Some("store".into()),
            project_name: Some("store".into()),
            project_enabled: true,
            active: target_reachable,
            target_reachable,
            last_accessed_at: None,
        }
    }
}
