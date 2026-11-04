use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::{PermissionsExt, chown};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::branding::BRANDING;
use crate::desktop::DesktopApi;
use crate::desktop::dto::{
    AddProjectServiceRequest, AddProjectServiceResultDto, AddRouteRequest, AppConfigDto,
    AssignDomainProjectRequest, AttachProjectWorktreeRequest, AttachProjectWorktreeResultDto,
    AvailablePortDto, BootstrapDto, CertificateRefreshDto, CliAccessDto, ConfigUpdateRequest,
    CreateProjectRequest, CreateProjectWithDomainsRequest, DaemonLaunchDto, DaemonStatusDto,
    DomainInfoDto, DomainPatternRequest, EnsureProjectEnvironmentRequest,
    EnsureProjectEnvironmentResultDto, InitProjectRequest, InitProjectResultDto, InstallResultDto,
    KillPortConflictRequest, LogsDto, LogsRequest, MaterializeProjectServicesRequest,
    MaterializeProjectServicesResultDto, PortConflictDto, ProjectInfoDto, RegisterDomainRequest,
    RegisterDomainResultDto, RegistrationDto, RegistrationMutationDto,
    RemoveProjectEnvironmentRequest, RemoveProjectEnvironmentResultDto, RemoveProjectRequest,
    RemoveProjectResultDto, RemoveRouteRequest, SetProjectServiceDomainRequest,
    SetProjectServiceDomainResultDto, StartDaemonRequest, UninstallPreviewDto, UninstallResultDto,
    UnregisterDomainResultDto, UpdateDomainRequest, UpdateProjectRequest,
};
use crate::infrastructure::binary_identity::DaemonBinaryCheck;
use crate::infrastructure::launch_sessions::LaunchSessionUpdate;
use crate::infrastructure::privilege::current_real_uid;

const HELPER_PING_TIMEOUT: Duration = Duration::from_secs(1);
const HELPER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const HELPER_MUTATION_TIMEOUT: Duration = Duration::from_secs(45);
const HELPER_INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const HELPER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const HELPER_RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase")]
pub enum HelperCommand {
    Ping,
    /// Ask the helper to exit after replying so its supervisor (launchd
    /// `KeepAlive`) respawns it, which recovers a helper that stopped answering.
    RelaunchHelper,
    /// Ask the helper to remove its own service and root-owned binary, so
    /// `localify uninstall` works without sudo.
    RemoveService,
    GetBootstrap,
    GetConfig,
    UpdateConfig {
        request: ConfigUpdateRequest,
    },
    GetDaemonStatus,
    ListDomains,
    ListProjects,
    InstallLocalify,
    RefreshDomainCertificates,
    GetUninstallPreview,
    UninstallLocalify,
    CreateProject {
        request: CreateProjectRequest,
    },
    CreateProjectWithDomains {
        request: CreateProjectWithDomainsRequest,
    },
    InitProject {
        request: InitProjectRequest,
    },
    AddProjectService {
        request: AddProjectServiceRequest,
    },
    SetProjectServiceDomain {
        request: SetProjectServiceDomainRequest,
    },
    MaterializeProjectServices {
        request: MaterializeProjectServicesRequest,
    },
    EnsureProjectEnvironment {
        request: EnsureProjectEnvironmentRequest,
    },
    UpdateProject {
        request: UpdateProjectRequest,
    },
    RemoveProject {
        request: RemoveProjectRequest,
    },
    RemoveProjectEnvironment {
        request: RemoveProjectEnvironmentRequest,
    },
    AttachProjectWorktree {
        request: AttachProjectWorktreeRequest,
    },
    SetupCliAccess,
    ListPortConflicts,
    KillPortConflict {
        request: KillPortConflictRequest,
    },
    AssignDomainProject {
        request: AssignDomainProjectRequest,
    },
    RegisterDomain {
        request: RegisterDomainRequest,
    },
    UpdateDomain {
        request: UpdateDomainRequest,
    },
    UnregisterDomain {
        request: DomainPatternRequest,
    },
    AddRoute {
        request: AddRouteRequest,
    },
    RemoveRoute {
        request: RemoveRouteRequest,
    },
    ListRoutes {
        request: DomainPatternRequest,
    },
    StartDaemon {
        request: StartDaemonRequest,
    },
    StopDaemon,
    RestartDaemon {
        request: StartDaemonRequest,
    },
    ReloadDaemon {
        request: StartDaemonRequest,
    },
    ReadLogs {
        request: LogsRequest,
    },
    GetAvailablePort,
    /// Relays a `localify dev` launch session to the root-owned daemon socket.
    ReportLaunchSession {
        update: LaunchSessionUpdate,
    },
}

impl HelperCommand {
    fn response_timeout(&self) -> Duration {
        match self {
            Self::Ping | Self::RelaunchHelper | Self::RemoveService => HELPER_PING_TIMEOUT,
            Self::GetBootstrap
            | Self::GetConfig
            | Self::GetDaemonStatus
            | Self::ListDomains
            | Self::ListProjects
            | Self::GetUninstallPreview
            | Self::ListPortConflicts
            | Self::ListRoutes { .. }
            | Self::ReadLogs { .. }
            | Self::GetAvailablePort
            | Self::ReportLaunchSession { .. } => HELPER_READ_TIMEOUT,
            Self::InstallLocalify | Self::UninstallLocalify => HELPER_INSTALL_TIMEOUT,
            Self::UpdateConfig { .. }
            | Self::RefreshDomainCertificates
            | Self::CreateProject { .. }
            | Self::CreateProjectWithDomains { .. }
            | Self::InitProject { .. }
            | Self::AddProjectService { .. }
            | Self::SetProjectServiceDomain { .. }
            | Self::MaterializeProjectServices { .. }
            | Self::EnsureProjectEnvironment { .. }
            | Self::UpdateProject { .. }
            | Self::RemoveProject { .. }
            | Self::RemoveProjectEnvironment { .. }
            | Self::AttachProjectWorktree { .. }
            | Self::SetupCliAccess
            | Self::KillPortConflict { .. }
            | Self::AssignDomainProject { .. }
            | Self::RegisterDomain { .. }
            | Self::UpdateDomain { .. }
            | Self::UnregisterDomain { .. }
            | Self::AddRoute { .. }
            | Self::RemoveRoute { .. }
            | Self::StartDaemon { .. }
            | Self::StopDaemon
            | Self::RestartDaemon { .. }
            | Self::ReloadDaemon { .. } => HELPER_MUTATION_TIMEOUT,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::RelaunchHelper => "relaunch the system helper",
            Self::RemoveService => "remove the system helper",
            Self::GetBootstrap => "read desktop configuration",
            Self::GetConfig => "read config",
            Self::GetDaemonStatus => "read daemon status",
            Self::ListDomains => "list domains",
            Self::ListProjects => "list projects",
            Self::InstallLocalify => "install Localify",
            Self::GetUninstallPreview => "read uninstall preview",
            Self::UninstallLocalify => "uninstall Localify",
            Self::ListPortConflicts => "list port conflicts",
            Self::ListRoutes { .. } => "list routes",
            Self::ReadLogs { .. } => "read logs",
            Self::GetAvailablePort => "find available port",
            Self::ReportLaunchSession { .. } => "report a launch profile session",
            _ => "run a system helper command",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct HelperResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

pub struct SystemHelperClient {
    socket_path: PathBuf,
}

impl SystemHelperClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub fn ping(&self) -> Result<()> {
        let _: bool = self.send(HelperCommand::Ping)?;
        Ok(())
    }

    /// Asks the running helper to exit so its supervisor respawns it. The helper
    /// acknowledges before exiting.
    pub fn relaunch_helper(&self) -> Result<()> {
        let _: bool = self.send(HelperCommand::RelaunchHelper)?;
        Ok(())
    }

    /// Asks the running helper to remove its service. The helper acknowledges
    /// before it stops, so a reply does not mean the removal finished.
    pub fn remove_service(&self) -> Result<()> {
        let _: bool = self.send(HelperCommand::RemoveService)?;
        Ok(())
    }

    pub fn get_bootstrap(&self) -> Result<BootstrapDto> {
        self.send(HelperCommand::GetBootstrap)
    }

    pub fn get_config(&self) -> Result<AppConfigDto> {
        self.send(HelperCommand::GetConfig)
    }

    pub fn update_config(&self, request: ConfigUpdateRequest) -> Result<AppConfigDto> {
        self.send(HelperCommand::UpdateConfig { request })
    }

    pub fn get_daemon_status(&self) -> Result<DaemonStatusDto> {
        self.send(HelperCommand::GetDaemonStatus)
    }

    pub fn list_domains(&self) -> Result<Vec<DomainInfoDto>> {
        self.send(HelperCommand::ListDomains)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectInfoDto>> {
        self.send(HelperCommand::ListProjects)
    }

    pub fn install_localify(&self) -> Result<InstallResultDto> {
        self.send(HelperCommand::InstallLocalify)
    }

    pub fn refresh_domain_certificates(&self) -> Result<CertificateRefreshDto> {
        self.send(HelperCommand::RefreshDomainCertificates)
    }

    pub fn get_uninstall_preview(&self) -> Result<UninstallPreviewDto> {
        self.send(HelperCommand::GetUninstallPreview)
    }

    pub fn uninstall_localify(&self) -> Result<UninstallResultDto> {
        self.send(HelperCommand::UninstallLocalify)
    }

    pub fn create_project(&self, request: CreateProjectRequest) -> Result<ProjectInfoDto> {
        self.send(HelperCommand::CreateProject { request })
    }

    pub fn create_project_with_domains(
        &self,
        request: CreateProjectWithDomainsRequest,
    ) -> Result<ProjectInfoDto> {
        self.send(HelperCommand::CreateProjectWithDomains { request })
    }

    pub fn init_project(&self, request: InitProjectRequest) -> Result<InitProjectResultDto> {
        self.send(HelperCommand::InitProject { request })
    }

    pub fn add_project_service(
        &self,
        request: AddProjectServiceRequest,
    ) -> Result<AddProjectServiceResultDto> {
        self.send(HelperCommand::AddProjectService { request })
    }

    pub fn set_project_service_domain(
        &self,
        request: SetProjectServiceDomainRequest,
    ) -> Result<SetProjectServiceDomainResultDto> {
        self.send(HelperCommand::SetProjectServiceDomain { request })
    }

    pub fn materialize_project_services(
        &self,
        request: MaterializeProjectServicesRequest,
    ) -> Result<MaterializeProjectServicesResultDto> {
        self.send(HelperCommand::MaterializeProjectServices { request })
    }

    pub fn ensure_project_environment(
        &self,
        request: EnsureProjectEnvironmentRequest,
    ) -> Result<EnsureProjectEnvironmentResultDto> {
        self.send(HelperCommand::EnsureProjectEnvironment { request })
    }

    pub fn update_project(&self, request: UpdateProjectRequest) -> Result<ProjectInfoDto> {
        self.send(HelperCommand::UpdateProject { request })
    }

    pub fn remove_project(&self, request: RemoveProjectRequest) -> Result<RemoveProjectResultDto> {
        self.send(HelperCommand::RemoveProject { request })
    }

    pub fn remove_project_environment(
        &self,
        request: RemoveProjectEnvironmentRequest,
    ) -> Result<RemoveProjectEnvironmentResultDto> {
        self.send(HelperCommand::RemoveProjectEnvironment { request })
    }

    pub fn attach_project_worktree(
        &self,
        request: AttachProjectWorktreeRequest,
    ) -> Result<AttachProjectWorktreeResultDto> {
        self.send(HelperCommand::AttachProjectWorktree { request })
    }

    pub fn setup_cli_access(&self) -> Result<CliAccessDto> {
        self.send(HelperCommand::SetupCliAccess)
    }

    pub fn list_port_conflicts(&self) -> Result<Vec<PortConflictDto>> {
        self.send(HelperCommand::ListPortConflicts)
    }

    pub fn kill_port_conflict(&self, request: KillPortConflictRequest) -> Result<PortConflictDto> {
        self.send(HelperCommand::KillPortConflict { request })
    }

    pub fn assign_domain_project(
        &self,
        request: AssignDomainProjectRequest,
    ) -> Result<DomainInfoDto> {
        self.send(HelperCommand::AssignDomainProject { request })
    }

    pub fn register_domain(
        &self,
        request: RegisterDomainRequest,
    ) -> Result<RegisterDomainResultDto> {
        self.send(HelperCommand::RegisterDomain { request })
    }

    pub fn update_domain(&self, request: UpdateDomainRequest) -> Result<RegisterDomainResultDto> {
        self.send(HelperCommand::UpdateDomain { request })
    }

    pub fn unregister_domain(
        &self,
        request: DomainPatternRequest,
    ) -> Result<UnregisterDomainResultDto> {
        self.send(HelperCommand::UnregisterDomain { request })
    }

    pub fn add_route(&self, request: AddRouteRequest) -> Result<RegistrationMutationDto> {
        self.send(HelperCommand::AddRoute { request })
    }

    pub fn remove_route(&self, request: RemoveRouteRequest) -> Result<RegistrationMutationDto> {
        self.send(HelperCommand::RemoveRoute { request })
    }

    pub fn list_routes(&self, request: DomainPatternRequest) -> Result<RegistrationDto> {
        self.send(HelperCommand::ListRoutes { request })
    }

    pub fn start_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        self.send(HelperCommand::StartDaemon { request })
    }

    pub fn stop_daemon(&self) -> Result<DaemonStatusDto> {
        self.send(HelperCommand::StopDaemon)
    }

    pub fn restart_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        self.send(HelperCommand::RestartDaemon { request })
    }

    pub fn reload_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        self.send(HelperCommand::ReloadDaemon { request })
    }

    pub fn read_logs(&self, request: LogsRequest) -> Result<LogsDto> {
        self.send(HelperCommand::ReadLogs { request })
    }

    pub fn get_available_port(&self) -> Result<AvailablePortDto> {
        self.send(HelperCommand::GetAvailablePort)
    }

    pub fn report_launch_session(&self, update: LaunchSessionUpdate) -> Result<()> {
        self.send::<serde_json::Value>(HelperCommand::ReportLaunchSession { update })
            .map(|_| ())
    }

    fn send<T>(&self, command: HelperCommand) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "Failed to connect to the {} system helper at {}",
                BRANDING.app_name,
                self.socket_path.display()
            )
        })?;
        let timeout = command.response_timeout();
        let command_label = command.label();

        stream
            .set_read_timeout(Some(timeout))
            .context("Failed to configure helper response timeout")?;
        stream
            .set_write_timeout(Some(timeout))
            .context("Failed to configure helper request timeout")?;

        let payload = serde_json::to_string(&command)?;
        write_helper_payload(&mut stream, payload.as_bytes(), command_label, timeout)?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(_) => {}
            Err(error) if is_io_timeout(&error) => {
                bail!(helper_timeout_message(command_label, timeout));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Failed to read the {command_label} response from the system helper")
                });
            }
        }
        if line.trim().is_empty() {
            bail!(
                "{} system helper closed the connection without replying. \
                 It is likely running an older build.\n\
                 Restart the helper service and try again:\n\
                 sudo {} helper uninstall-service --uid {}",
                BRANDING.app_name,
                BRANDING.cli_command(),
                current_uid()?
            );
        }

        let response: HelperResponse = serde_json::from_str(&line)?;
        if !response.ok {
            bail!(
                "{}",
                response.error.unwrap_or_else(|| {
                    format!(
                        "{} system helper failed without an error message.",
                        BRANDING.app_name
                    )
                })
            );
        }

        let data = response.data.unwrap_or(serde_json::Value::Null);
        serde_json::from_value(data).map_err(Into::into)
    }
}

fn write_helper_payload(
    stream: &mut UnixStream,
    payload: &[u8],
    command_label: &str,
    timeout: Duration,
) -> Result<()> {
    stream
        .write_all(payload)
        .and_then(|_| stream.write_all(b"\n"))
        .and_then(|_| stream.flush())
        .map_err(|error| {
            if is_io_timeout(&error) {
                anyhow::anyhow!(helper_timeout_message(command_label, timeout))
            } else {
                anyhow::anyhow!(error)
            }
        })
        .with_context(|| format!("Failed to send the {command_label} request to the system helper"))
}

fn helper_timeout_message(command_label: &str, timeout: Duration) -> String {
    format!(
        "{} system helper did not reply while trying to {} within {} seconds.\n\
         Restart the helper from Settings, or run: sudo {} helper uninstall-service --uid {}",
        BRANDING.app_name,
        command_label,
        timeout.as_secs(),
        BRANDING.cli_command(),
        current_uid()
            .map(|uid| uid.to_string())
            .unwrap_or_else(|_| "$(id -u)".into())
    )
}

fn is_io_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

pub fn helper_socket_path(uid: u32) -> PathBuf {
    BRANDING.helper_socket_path(uid)
}

pub fn current_uid() -> Result<u32> {
    Ok(current_real_uid())
}

pub fn serve(
    socket_path: &Path,
    allowed_uid: u32,
    config_path: &Path,
    api: &DesktopApi,
) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let _ = fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    set_socket_owner(socket_path, allowed_uid)?;
    spawn_outdated_daemon_check(api.clone());

    loop {
        let (stream, _) = listener.accept()?;
        let api = api.clone();
        let config_path = config_path.to_path_buf();
        thread::spawn(move || {
            if let Err(error) = handle_connection(stream, &api, allowed_uid, &config_path) {
                eprintln!("{} helper connection failed: {error}", BRANDING.app_name);
            }
        });
    }
}

/// Serializes daemon start, stop and restart inside the helper, so the
/// startup check below and a desktop request never restart the daemon at the
/// same time.
static DAEMON_LIFECYCLE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn daemon_lifecycle_lock() -> std::sync::MutexGuard<'static, ()> {
    match DAEMON_LIFECYCLE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A new helper usually means a new build (an administrator approved copying it
/// into place). Restart a daemon still running the previous build, or one that
/// runs from a file other users could change.
fn spawn_outdated_daemon_check(api: DesktopApi) {
    thread::spawn(move || {
        // Let the desktop app finish its own post-update restart first.
        thread::sleep(Duration::from_secs(3));
        let _lifecycle = daemon_lifecycle_lock();
        match api.restart_daemon_if_outdated() {
            Ok(DaemonBinaryCheck::Outdated) => eprintln!(
                "{} helper restarted the daemon on the updated build.",
                BRANDING.app_name
            ),
            Ok(_) => {}
            Err(error) => eprintln!(
                "{} helper could not check the daemon build: {error:#}",
                BRANDING.app_name
            ),
        }
    });
}

fn handle_connection(
    stream: UnixStream,
    api: &DesktopApi,
    allowed_uid: u32,
    config_path: &Path,
) -> Result<()> {
    stream
        .set_read_timeout(Some(HELPER_REQUEST_TIMEOUT))
        .context("Failed to configure helper request read timeout")?;
    stream
        .set_write_timeout(Some(HELPER_RESPONSE_WRITE_TIMEOUT))
        .context("Failed to configure helper response write timeout")?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(_) => {}
        Err(error) if is_io_timeout(&error) => {
            bail!(
                "{} helper connection did not send a full request within {} seconds.",
                BRANDING.app_name,
                HELPER_REQUEST_TIMEOUT.as_secs()
            );
        }
        Err(error) => return Err(error).context("Failed to read helper request"),
    }

    let command = serde_json::from_str::<HelperCommand>(&line).context(format!(
        "Failed to parse {} helper command",
        BRANDING.app_name
    ))?;

    if matches!(
        command,
        HelperCommand::RelaunchHelper | HelperCommand::RemoveService
    ) {
        let response = HelperResponse {
            ok: true,
            error: None,
            data: Some(serde_json::Value::Bool(true)),
        };
        let mut writer = stream;
        writer.write_all(serde_json::to_string(&response)?.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        if matches!(command, HelperCommand::RemoveService) {
            spawn_service_removal(config_path, allowed_uid)?;
            // launchd stops this process during the removal, and exiting first
            // would only make KeepAlive respawn it.
            if cfg!(target_os = "macos") {
                return Ok(());
            }
        }
        // The supervisor (launchd KeepAlive) respawns the helper.
        std::process::exit(0);
    }

    let response = match dispatch_command(api, command) {
        Ok(data) => HelperResponse {
            ok: true,
            error: None,
            data: Some(data),
        },
        Err(error) => HelperResponse {
            ok: false,
            error: Some(error.to_string()),
            data: None,
        },
    };

    let mut writer = stream;
    writer.write_all(serde_json::to_string(&response)?.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// Runs the removal in its own process group, because launchd kills the
/// helper's group when it stops the service.
fn spawn_service_removal(config_path: &Path, allowed_uid: u32) -> Result<()> {
    let executable = std::env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug()
    ))?;
    let _child = Command::new(executable)
        .arg("--config")
        .arg(config_path)
        .args(["helper", "uninstall-service", "--uid"])
        .arg(allowed_uid.to_string())
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to start the system helper removal")?;
    Ok(())
}

fn dispatch_command(api: &DesktopApi, command: HelperCommand) -> Result<serde_json::Value> {
    match command {
        // The last two are handled in handle_connection so the reply is flushed
        // before the helper stops.
        HelperCommand::Ping | HelperCommand::RelaunchHelper | HelperCommand::RemoveService => {
            serde_json::to_value(true).map_err(Into::into)
        }
        HelperCommand::GetBootstrap => serde_json::to_value(api.bootstrap()?).map_err(Into::into),
        HelperCommand::GetConfig => serde_json::to_value(api.read_config()?).map_err(Into::into),
        HelperCommand::UpdateConfig { request } => {
            serde_json::to_value(api.update_config(request)?).map_err(Into::into)
        }
        HelperCommand::GetDaemonStatus => {
            serde_json::to_value(api.daemon_status()?).map_err(Into::into)
        }
        HelperCommand::ListDomains => serde_json::to_value(api.list_domains()?).map_err(Into::into),
        HelperCommand::ListProjects => {
            serde_json::to_value(api.list_projects()?).map_err(Into::into)
        }
        HelperCommand::InstallLocalify => serde_json::to_value(api.install()?).map_err(Into::into),
        HelperCommand::RefreshDomainCertificates => {
            serde_json::to_value(api.refresh_domain_certificates()?).map_err(Into::into)
        }
        HelperCommand::GetUninstallPreview => {
            serde_json::to_value(api.uninstall_preview()?).map_err(Into::into)
        }
        HelperCommand::UninstallLocalify => {
            serde_json::to_value(api.uninstall()?).map_err(Into::into)
        }
        HelperCommand::CreateProject { request } => {
            serde_json::to_value(api.create_project(request)?).map_err(Into::into)
        }
        HelperCommand::CreateProjectWithDomains { request } => {
            serde_json::to_value(api.create_project_with_domains(request)?).map_err(Into::into)
        }
        HelperCommand::InitProject { request } => {
            serde_json::to_value(api.init_project(request)?).map_err(Into::into)
        }
        HelperCommand::AddProjectService { request } => {
            serde_json::to_value(api.add_project_service(request)?).map_err(Into::into)
        }
        HelperCommand::SetProjectServiceDomain { request } => {
            serde_json::to_value(api.set_project_service_domain(request)?).map_err(Into::into)
        }
        HelperCommand::MaterializeProjectServices { request } => {
            serde_json::to_value(api.apply_detected_project_services(request)?).map_err(Into::into)
        }
        HelperCommand::EnsureProjectEnvironment { request } => {
            serde_json::to_value(api.ensure_project_environment(request)?).map_err(Into::into)
        }
        HelperCommand::UpdateProject { request } => {
            serde_json::to_value(api.update_project(request)?).map_err(Into::into)
        }
        HelperCommand::RemoveProject { request } => {
            serde_json::to_value(api.remove_project(request)?).map_err(Into::into)
        }
        HelperCommand::RemoveProjectEnvironment { request } => {
            serde_json::to_value(api.remove_project_environment(request)?).map_err(Into::into)
        }
        HelperCommand::AttachProjectWorktree { request } => {
            serde_json::to_value(api.attach_project_worktree_direct(request)?).map_err(Into::into)
        }
        HelperCommand::SetupCliAccess => {
            serde_json::to_value(api.setup_cli_access()?).map_err(Into::into)
        }
        HelperCommand::ListPortConflicts => {
            serde_json::to_value(api.list_port_conflicts()?).map_err(Into::into)
        }
        HelperCommand::KillPortConflict { request } => {
            serde_json::to_value(api.kill_port_conflict(request)?).map_err(Into::into)
        }
        HelperCommand::AssignDomainProject { request } => {
            serde_json::to_value(api.assign_domain_project(request)?).map_err(Into::into)
        }
        HelperCommand::RegisterDomain { request } => {
            serde_json::to_value(api.register_domain(request)?).map_err(Into::into)
        }
        HelperCommand::UpdateDomain { request } => {
            serde_json::to_value(api.update_domain(request)?).map_err(Into::into)
        }
        HelperCommand::UnregisterDomain { request } => {
            serde_json::to_value(api.unregister_domain(request)?).map_err(Into::into)
        }
        HelperCommand::AddRoute { request } => {
            serde_json::to_value(api.add_route(request)?).map_err(Into::into)
        }
        HelperCommand::RemoveRoute { request } => {
            serde_json::to_value(api.remove_route(request)?).map_err(Into::into)
        }
        HelperCommand::ListRoutes { request } => {
            serde_json::to_value(api.list_routes(request)?).map_err(Into::into)
        }
        HelperCommand::StartDaemon { request } => {
            let _lifecycle = daemon_lifecycle_lock();
            serde_json::to_value(api.start_daemon(request)?).map_err(Into::into)
        }
        HelperCommand::StopDaemon => {
            let _lifecycle = daemon_lifecycle_lock();
            serde_json::to_value(api.stop_daemon()?).map_err(Into::into)
        }
        HelperCommand::RestartDaemon { request } => {
            let _lifecycle = daemon_lifecycle_lock();
            serde_json::to_value(api.restart_daemon(request)?).map_err(Into::into)
        }
        HelperCommand::ReloadDaemon { request } => {
            let _lifecycle = daemon_lifecycle_lock();
            serde_json::to_value(api.reload_daemon(request)?).map_err(Into::into)
        }
        HelperCommand::ReadLogs { request } => {
            serde_json::to_value(api.read_logs(request)?).map_err(Into::into)
        }
        HelperCommand::GetAvailablePort => {
            serde_json::to_value(api.available_port()?).map_err(Into::into)
        }
        HelperCommand::ReportLaunchSession { update } => {
            api.report_launch_session(&update)?;
            Ok(serde_json::json!({ "recorded": true }))
        }
    }
}

fn set_socket_owner(socket_path: &Path, allowed_uid: u32) -> Result<()> {
    chown(socket_path, Some(allowed_uid), None)
        .context("Failed to hand the helper socket to the desktop user")?;

    let mut permissions = fs::metadata(socket_path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(socket_path, permissions)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These took a file path and made root read or write it, for any caller
    /// holding the socket. The app does this work in its own process instead.
    #[test]
    fn the_socket_does_not_accept_root_file_transfer_commands() {
        for command in [
            r#"{"cmd":"exportSettings","request":{"filePath":"/etc/zshenv","mode":"all"}}"#,
            r#"{"cmd":"previewSettingsImport","request":{"filePath":"/etc/master.passwd"}}"#,
            r#"{"cmd":"importSettings","request":{"filePath":"/tmp/x","mode":"replaceAll"}}"#,
        ] {
            assert!(serde_json::from_str::<HelperCommand>(command).is_err());
        }
    }

    #[test]
    fn the_socket_accepts_the_remove_service_command() {
        let command = serde_json::from_str::<HelperCommand>(r#"{"cmd":"removeService"}"#).unwrap();

        assert!(matches!(command, HelperCommand::RemoveService));
    }
}
