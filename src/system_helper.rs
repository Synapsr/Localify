use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::branding::BRANDING;
use crate::desktop::DesktopApi;
use crate::desktop::dto::{
    ActivateLicenseRequest, AddRouteRequest, AppConfigDto, AssignDomainProjectRequest,
    AttachProjectWorktreeRequest, AttachProjectWorktreeResultDto, AvailablePortDto, BootstrapDto,
    CertificateRefreshDto, CliAccessDto, ConfigUpdateRequest, CreateProjectRequest,
    CreateProjectWithDomainsRequest, DaemonLaunchDto, DaemonStatusDto, DomainInfoDto,
    DomainPatternRequest, ImportOfflineLicenseRequest, InitProjectRequest, InitProjectResultDto,
    InstallResultDto, KeygenSettingsUpdateRequest, KillPortConflictRequest, LicensingDto, LogsDto,
    LogsRequest, PortConflictDto, ProjectInfoDto, RegisterDomainRequest, RegisterDomainResultDto,
    RegistrationDto, RegistrationMutationDto, RemoveProjectEnvironmentRequest,
    RemoveProjectEnvironmentResultDto, RemoveProjectRequest, RemoveProjectResultDto,
    RemoveRouteRequest, SettingsExportRequest, SettingsExportResultDto, SettingsImportPreviewDto,
    SettingsImportPreviewRequest, SettingsImportRequest, SettingsImportResultDto,
    StartDaemonRequest, UninstallPreviewDto, UninstallResultDto, UnregisterDomainResultDto,
    UpdateDomainRequest, UpdateProjectRequest,
};

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
    GetBootstrap,
    GetConfig,
    UpdateConfig {
        request: ConfigUpdateRequest,
    },
    GetDaemonStatus,
    ListDomains,
    ListProjects,
    InstallRoxy,
    RefreshDomainCertificates,
    GetUninstallPreview,
    UninstallRoxy,
    CreateProject {
        request: CreateProjectRequest,
    },
    CreateProjectWithDomains {
        request: CreateProjectWithDomainsRequest,
    },
    ExportSettings {
        request: SettingsExportRequest,
    },
    PreviewSettingsImport {
        request: SettingsImportPreviewRequest,
    },
    ImportSettings {
        request: SettingsImportRequest,
    },
    GetLicensingStatus,
    UpdateKeygenSettings {
        request: KeygenSettingsUpdateRequest,
    },
    ActivateLicense {
        request: ActivateLicenseRequest,
    },
    ImportOfflineLicense {
        request: ImportOfflineLicenseRequest,
    },
    ClearLicense,
    RefreshLicense,
    InitProject {
        request: InitProjectRequest,
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
}

impl HelperCommand {
    fn response_timeout(&self) -> Duration {
        match self {
            Self::Ping => HELPER_PING_TIMEOUT,
            Self::GetBootstrap
            | Self::GetConfig
            | Self::GetDaemonStatus
            | Self::ListDomains
            | Self::ListProjects
            | Self::GetLicensingStatus
            | Self::GetUninstallPreview
            | Self::ListPortConflicts
            | Self::ListRoutes { .. }
            | Self::ReadLogs { .. }
            | Self::GetAvailablePort => HELPER_READ_TIMEOUT,
            Self::InstallRoxy | Self::UninstallRoxy | Self::ImportSettings { .. } => {
                HELPER_INSTALL_TIMEOUT
            }
            Self::UpdateConfig { .. }
            | Self::RefreshDomainCertificates
            | Self::CreateProject { .. }
            | Self::CreateProjectWithDomains { .. }
            | Self::ExportSettings { .. }
            | Self::PreviewSettingsImport { .. }
            | Self::UpdateKeygenSettings { .. }
            | Self::ActivateLicense { .. }
            | Self::ImportOfflineLicense { .. }
            | Self::ClearLicense
            | Self::RefreshLicense
            | Self::InitProject { .. }
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
            Self::GetBootstrap => "read desktop configuration",
            Self::GetConfig => "read config",
            Self::GetDaemonStatus => "read daemon status",
            Self::ListDomains => "list domains",
            Self::ListProjects => "list projects",
            Self::InstallRoxy => "install Localify",
            Self::GetUninstallPreview => "read uninstall preview",
            Self::UninstallRoxy => "uninstall Localify",
            Self::ImportSettings { .. } => "import settings",
            Self::GetLicensingStatus => "read licensing status",
            Self::ListPortConflicts => "list port conflicts",
            Self::ListRoutes { .. } => "list routes",
            Self::ReadLogs { .. } => "read logs",
            Self::GetAvailablePort => "find available port",
            _ => "run a system helper command",
        }
    }

    fn may_write_config(&self) -> bool {
        matches!(
            self,
            Self::InstallRoxy
                | Self::UpdateConfig { .. }
                | Self::CreateProject { .. }
                | Self::CreateProjectWithDomains { .. }
                | Self::ImportSettings { .. }
                | Self::UpdateKeygenSettings { .. }
                | Self::ActivateLicense { .. }
                | Self::ImportOfflineLicense { .. }
                | Self::ClearLicense
                | Self::RefreshLicense
                | Self::InitProject { .. }
                | Self::UpdateProject { .. }
                | Self::RemoveProject { .. }
                | Self::RemoveProjectEnvironment { .. }
                | Self::AttachProjectWorktree { .. }
                | Self::AssignDomainProject { .. }
                | Self::RegisterDomain { .. }
                | Self::UpdateDomain { .. }
                | Self::UnregisterDomain { .. }
                | Self::AddRoute { .. }
                | Self::RemoveRoute { .. }
        )
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

    pub fn install_roxy(&self) -> Result<InstallResultDto> {
        self.send(HelperCommand::InstallRoxy)
    }

    pub fn refresh_domain_certificates(&self) -> Result<CertificateRefreshDto> {
        self.send(HelperCommand::RefreshDomainCertificates)
    }

    pub fn get_uninstall_preview(&self) -> Result<UninstallPreviewDto> {
        self.send(HelperCommand::GetUninstallPreview)
    }

    pub fn uninstall_roxy(&self) -> Result<UninstallResultDto> {
        self.send(HelperCommand::UninstallRoxy)
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

    pub fn export_settings(
        &self,
        request: SettingsExportRequest,
    ) -> Result<SettingsExportResultDto> {
        self.send(HelperCommand::ExportSettings { request })
    }

    pub fn preview_settings_import(
        &self,
        request: SettingsImportPreviewRequest,
    ) -> Result<SettingsImportPreviewDto> {
        self.send(HelperCommand::PreviewSettingsImport { request })
    }

    pub fn import_settings(
        &self,
        request: SettingsImportRequest,
    ) -> Result<SettingsImportResultDto> {
        self.send(HelperCommand::ImportSettings { request })
    }

    pub fn get_licensing_status(&self) -> Result<LicensingDto> {
        self.send(HelperCommand::GetLicensingStatus)
    }

    pub fn update_keygen_settings(
        &self,
        request: KeygenSettingsUpdateRequest,
    ) -> Result<LicensingDto> {
        self.send(HelperCommand::UpdateKeygenSettings { request })
    }

    pub fn activate_license(&self, request: ActivateLicenseRequest) -> Result<LicensingDto> {
        self.send(HelperCommand::ActivateLicense { request })
    }

    pub fn import_offline_license(
        &self,
        request: ImportOfflineLicenseRequest,
    ) -> Result<LicensingDto> {
        self.send(HelperCommand::ImportOfflineLicense { request })
    }

    pub fn clear_license(&self) -> Result<LicensingDto> {
        self.send(HelperCommand::ClearLicense)
    }

    pub fn refresh_license(&self) -> Result<LicensingDto> {
        self.send(HelperCommand::RefreshLicense)
    }

    pub fn init_project(&self, request: InitProjectRequest) -> Result<InitProjectResultDto> {
        self.send(HelperCommand::InitProject { request })
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
    if let Ok(uid) = std::env::var("UID") {
        return uid
            .trim()
            .parse::<u32>()
            .context("Failed to parse UID from environment");
    }

    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("Failed to determine the current user ID")?;

    if !output.status.success() {
        bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .to_string()
                .if_empty("Failed to determine the current user ID.")
        );
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .context("Failed to parse the current user ID")
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
    let may_write_config = command.may_write_config();
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
    if may_write_config && let Err(error) = restore_user_config_ownership(config_path, allowed_uid)
    {
        eprintln!(
            "{} helper could not restore config ownership for {}: {error}",
            BRANDING.app_name,
            config_path.display()
        );
    }

    let mut writer = stream;
    writer.write_all(serde_json::to_string(&response)?.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn restore_user_config_ownership(config_path: &Path, allowed_uid: u32) -> Result<()> {
    if config_path.starts_with("/etc") {
        return Ok(());
    }

    if let Some(parent) = config_path.parent()
        && parent.exists()
        && should_restore_config_parent_ownership(config_path)
    {
        chown_path_to_uid(parent, allowed_uid).with_context(|| {
            format!(
                "Failed to restore config directory ownership for '{}'",
                parent.display()
            )
        })?;
    }

    if !config_path.exists() {
        return Ok(());
    }

    chown_path_to_uid(config_path, allowed_uid).with_context(|| {
        format!(
            "Failed to restore config file ownership for '{}'",
            config_path.display()
        )
    })?;

    let mut permissions = fs::metadata(config_path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o600);
    fs::set_permissions(config_path, permissions)?;
    Ok(())
}

fn should_restore_config_parent_ownership(config_path: &Path) -> bool {
    let Some(parent) = config_path.parent() else {
        return false;
    };

    let state_dir_name = parent.file_name().and_then(|name| name.to_str());
    let is_localify_state_dir = state_dir_name == Some(BRANDING.desktop_state_dir_name)
        || state_dir_name == Some(BRANDING.dev_desktop_state_dir_name);
    if !is_localify_state_dir {
        return false;
    }

    let Some(application_support_dir) = parent.parent() else {
        return false;
    };
    if application_support_dir
        .file_name()
        .and_then(|name| name.to_str())
        != Some("Application Support")
    {
        return false;
    }

    application_support_dir
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        == Some("Library")
}

fn chown_path_to_uid(path: &Path, uid: u32) -> Result<()> {
    let output = Command::new("chown")
        .arg(uid.to_string())
        .arg(path)
        .output()
        .with_context(|| format!("Failed to run chown for '{}'", path.display()))?;

    if output.status.success() {
        return Ok(());
    }

    bail!(
        "{}",
        String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_string()
            .if_empty("Failed to change ownership.")
    );
}

fn dispatch_command(api: &DesktopApi, command: HelperCommand) -> Result<serde_json::Value> {
    match command {
        HelperCommand::Ping => serde_json::to_value(true).map_err(Into::into),
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
        HelperCommand::InstallRoxy => serde_json::to_value(api.install()?).map_err(Into::into),
        HelperCommand::RefreshDomainCertificates => {
            serde_json::to_value(api.refresh_domain_certificates()?).map_err(Into::into)
        }
        HelperCommand::GetUninstallPreview => {
            serde_json::to_value(api.uninstall_preview()?).map_err(Into::into)
        }
        HelperCommand::UninstallRoxy => serde_json::to_value(api.uninstall()?).map_err(Into::into),
        HelperCommand::CreateProject { request } => {
            serde_json::to_value(api.create_project(request)?).map_err(Into::into)
        }
        HelperCommand::CreateProjectWithDomains { request } => {
            serde_json::to_value(api.create_project_with_domains(request)?).map_err(Into::into)
        }
        HelperCommand::ExportSettings { request } => {
            serde_json::to_value(api.export_settings(request)?).map_err(Into::into)
        }
        HelperCommand::PreviewSettingsImport { request } => {
            serde_json::to_value(api.preview_settings_import(request)?).map_err(Into::into)
        }
        HelperCommand::ImportSettings { request } => {
            serde_json::to_value(api.import_settings(request)?).map_err(Into::into)
        }
        HelperCommand::GetLicensingStatus => {
            serde_json::to_value(api.licensing_status()?).map_err(Into::into)
        }
        HelperCommand::UpdateKeygenSettings { request } => {
            serde_json::to_value(api.update_keygen_settings(request)?).map_err(Into::into)
        }
        HelperCommand::ActivateLicense { request } => {
            serde_json::to_value(api.activate_license(request)?).map_err(Into::into)
        }
        HelperCommand::ImportOfflineLicense { request } => {
            serde_json::to_value(api.import_offline_license(request)?).map_err(Into::into)
        }
        HelperCommand::ClearLicense => {
            serde_json::to_value(api.clear_license()?).map_err(Into::into)
        }
        HelperCommand::RefreshLicense => {
            serde_json::to_value(api.refresh_license()?).map_err(Into::into)
        }
        HelperCommand::InitProject { request } => {
            serde_json::to_value(api.init_project(request)?).map_err(Into::into)
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
            serde_json::to_value(api.start_daemon(request)?).map_err(Into::into)
        }
        HelperCommand::StopDaemon => serde_json::to_value(api.stop_daemon()?).map_err(Into::into),
        HelperCommand::RestartDaemon { request } => {
            serde_json::to_value(api.restart_daemon(request)?).map_err(Into::into)
        }
        HelperCommand::ReloadDaemon { request } => {
            serde_json::to_value(api.reload_daemon(request)?).map_err(Into::into)
        }
        HelperCommand::ReadLogs { request } => {
            serde_json::to_value(api.read_logs(request)?).map_err(Into::into)
        }
        HelperCommand::GetAvailablePort => {
            serde_json::to_value(api.available_port()?).map_err(Into::into)
        }
    }
}

fn set_socket_owner(socket_path: &Path, allowed_uid: u32) -> Result<()> {
    let output = Command::new("chown")
        .arg(allowed_uid.to_string())
        .arg(socket_path)
        .output()
        .context("Failed to hand the helper socket to the desktop user")?;

    if !output.status.success() {
        bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .to_string()
                .if_empty("Failed to change helper socket ownership.")
        );
    }

    let mut permissions = fs::metadata(socket_path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(socket_path, permissions)?;
    Ok(())
}

trait StringExt {
    fn if_empty(self, fallback: &str) -> String;
}

impl StringExt for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::dto::DomainPatternRequest;

    #[test]
    fn read_commands_do_not_restore_config_ownership() {
        assert!(!HelperCommand::Ping.may_write_config());
        assert!(!HelperCommand::GetBootstrap.may_write_config());
        assert!(!HelperCommand::ListDomains.may_write_config());
        assert!(!HelperCommand::GetAvailablePort.may_write_config());
    }

    #[test]
    fn config_mutations_restore_config_ownership() {
        assert!(HelperCommand::InstallRoxy.may_write_config());
        assert!(HelperCommand::ClearLicense.may_write_config());
        assert!(
            HelperCommand::UnregisterDomain {
                request: DomainPatternRequest {
                    domain: "demo.localify".into(),
                    wildcard: false,
                }
            }
            .may_write_config()
        );
    }

    #[test]
    fn daemon_mutations_do_not_restore_config_ownership() {
        assert!(!HelperCommand::StopDaemon.may_write_config());
        assert!(!HelperCommand::RefreshDomainCertificates.may_write_config());
    }

    #[test]
    fn restores_parent_ownership_for_localify_application_support_config() {
        assert!(should_restore_config_parent_ownership(
            std::path::Path::new("/Users/dev/Library/Application Support/Localify Dev/config.toml")
        ));
        assert!(should_restore_config_parent_ownership(
            std::path::Path::new(
                "/Users/dev/Library/Application Support/Localify Desktop/config.toml"
            )
        ));
    }

    #[test]
    fn does_not_restore_parent_ownership_for_arbitrary_config_paths() {
        assert!(!should_restore_config_parent_ownership(
            std::path::Path::new("/tmp/localify/config.toml")
        ));
        assert!(!should_restore_config_parent_ownership(
            std::path::Path::new("/Users/dev/project/config.toml")
        ));
    }
}
