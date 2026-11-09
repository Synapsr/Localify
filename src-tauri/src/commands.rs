use localify::desktop::DesktopApi;
use localify::desktop::dto::{
    AddProjectServiceRequest, AddProjectServiceResultDto, AddRouteRequest,
    AgentMcpInstallResultDto, AgentMcpStatusDto, AppConfigDto, AssignDomainProjectRequest,
    AttachProjectWorktreeRequest, AttachProjectWorktreeResultDto, AvailablePortDto, BootstrapDto,
    CliAccessDto, ConfigUpdateRequest, CreateProjectRequest, CreateProjectWithDomainsRequest,
    DaemonLaunchDto, DaemonStatusDto, DetectProjectInitPlanDto, DetectProjectInitPlanRequest,
    DiscoverProjectWorktreesRequest, DomainInfoDto, DomainPatternRequest,
    EnvironmentFileCopyPreviewDto, EnvironmentFileCopyRequest, EnvironmentFileDto,
    EnvironmentFileListDto, EnvironmentFileRequest, InitProjectFromFolderRequest,
    InitProjectFromFolderResultDto, InstallResultDto, KillPortConflictRequest,
    LauncherPreferencesDto, LogsDto, LogsRequest, NodeTrustDto, OpenEnvInEditorRequest,
    OpenInEditorRequest, OpenProjectUrlRequest, PortConflictDto, ProjectInfoDto,
    ProjectWorktreeDiscoveryDto, RegisterDomainRequest, RegisterDomainResultDto, RegistrationDto,
    RegistrationMutationDto, RelinkProjectRootRequest, RemoveProjectEnvironmentRequest,
    RemoveProjectEnvironmentResultDto, RemoveProjectRequest, RemoveProjectResultDto,
    RemoveRouteRequest, RunLocalDevServerRequest, SaveEnvironmentFileRequest,
    SettingsExportRequest, SettingsExportResultDto, SettingsImportPreviewDto,
    SettingsImportPreviewRequest, SettingsImportRequest, SettingsImportResultDto,
    StartDaemonRequest, StepDto, StopLocalDevServerRequest, UninstallPreviewDto,
    UninstallResultDto, UnregisterDomainResultDto, UpdateDomainRequest,
    UpdateLauncherPreferencesRequest, UpdateProjectRequest,
};
use localify::desktop::dto::{
    CheckPublicUrlSlugRequest, CloudAccountDto, CloudAddonPricingDto, CloudBillingPageDto,
    CloudPlanDto, CloudSignInPollDto, CloudSignInStartDto, CloudSignOutDto, CreatePublicUrlRequest,
    PollCloudSignInRequest, PublicUrlDto, PublicUrlIdRequest, PublicUrlSlugCheckDto,
    PublicUrlSlugSuggestionDto, PublicUrlWithPasswordDto, SetPublicUrlProtectionRequest,
    StartCloudAddonCheckoutRequest, StartCloudCheckoutRequest, SuggestPublicUrlSlugRequest,
};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use tauri::async_runtime;
use tauri::{AppHandle, Runtime};

use localify::desktop::dto::{ServiceRunRequest, ServiceStopDto};
use localify::desktop::shell_environment::ShellEnvironmentDto;
use localify::desktop::terminal::{
    TerminalCreateSessionRequest, TerminalLaunchServiceRequest, TerminalLaunchServiceResultDto,
    TerminalManager, TerminalResizeRequest, TerminalRestartServiceRequest, TerminalSessionDto,
    TerminalSessionRequest, TerminalWriteRequest,
};
use localify::desktop::updates::{hand_off_update, open_app_blocker};
use localify::infrastructure::updates::UpdateStatus;

pub struct AppState {
    pub api: DesktopApi,
    pending_deep_link: Mutex<Option<DeepLinkPayloadDto>>,
}

impl AppState {
    pub fn with_api(api: DesktopApi) -> Self {
        Self {
            api,
            pending_deep_link: Mutex::new(None),
        }
    }

    pub fn set_pending_deep_link(&self, payload: DeepLinkPayloadDto) {
        *self.pending_deep_link_guard() = Some(payload);
    }

    pub fn take_pending_deep_link(&self) -> Option<DeepLinkPayloadDto> {
        self.pending_deep_link_guard().take()
    }

    fn pending_deep_link_guard(&self) -> MutexGuard<'_, Option<DeepLinkPayloadDto>> {
        match self.pending_deep_link.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepLinkPayloadDto {
    pub route: DeepLinkRouteDto,
    pub project_id: Option<String>,
    pub view: Option<String>,
    /// The service to start, for `LaunchService`.
    #[serde(default)]
    pub launch: Option<DeepLinkLaunchDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeepLinkRouteDto {
    Projects,
    Logs,
    /// Start a service in the terminal panel, as its launch button does.
    LaunchService,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeepLinkLaunchDto {
    pub project_id: String,
    pub environment_slug: String,
    pub service_id: String,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartStatusDto {
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAutostartRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsRequest {
    pub automatic: bool,
}

const INSTALL_ATTRIBUTION_PREFIX: &str = "localify:install-attribution:v1:";

fn map_error<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

/// Trust that is still pending does not count against the install: the machine
/// has no screen to confirm on, and everything else works.
fn install_completed(result: &InstallResultDto) -> bool {
    result.steps.iter().all(|step| step.status != "warning")
}

fn autostart_install_step<R: Runtime>(app: &AppHandle<R>) -> StepDto {
    match super::enable_autostart(app) {
        Ok(()) => StepDto {
            label: Some("Launch at login".into()),
            status: "success".into(),
            message: format!(
                "{} will open at login.",
                localify::branding::BRANDING.app_name
            ),
        },
        Err(error) => StepDto {
            label: Some("Launch at login".into()),
            status: "warning".into(),
            message: format!(
                "{} was installed, but launch at login could not be enabled: {error}",
                localify::branding::BRANDING.app_name
            ),
        },
    }
}

fn autostart_uninstall_step<R: Runtime>(app: &AppHandle<R>) -> StepDto {
    match super::disable_autostart(app) {
        Ok(()) => StepDto {
            label: Some("Launch at login".into()),
            status: "success".into(),
            message: format!(
                "{} will no longer open automatically at login.",
                localify::branding::BRANDING.app_name
            ),
        },
        Err(error) => StepDto {
            label: Some("Launch at login".into()),
            status: "warning".into(),
            message: format!(
                "{} was uninstalled, but launch at login could not be disabled: {error}",
                localify::branding::BRANDING.app_name
            ),
        },
    }
}

#[cfg(target_os = "macos")]
fn read_clipboard_text() -> Result<Option<String>, String> {
    let output = Command::new("pbpaste")
        .output()
        .map_err(|error| format!("Failed to read clipboard: {error}"))?;

    if !output.status.success() {
        return Ok(None);
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.starts_with(INSTALL_ATTRIBUTION_PREFIX) {
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

#[cfg(not(target_os = "macos"))]
fn read_clipboard_text() -> Result<Option<String>, String> {
    Ok(None)
}

#[tauri::command]
pub fn get_autostart_status(app: tauri::AppHandle) -> Result<AutostartStatusDto, String> {
    Ok(AutostartStatusDto {
        enabled: super::autostart_enabled(&app)?,
    })
}

#[tauri::command]
pub fn set_autostart_enabled(
    app: tauri::AppHandle,
    request: UpdateAutostartRequest,
) -> Result<AutostartStatusDto, String> {
    if request.enabled {
        super::enable_autostart(&app)?;
    } else {
        super::disable_autostart(&app)?;
    }

    get_autostart_status(app)
}

#[tauri::command]
pub async fn get_update_status(state: tauri::State<'_, AppState>) -> Result<UpdateStatus, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.update_status().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn set_automatic_updates(
    state: tauri::State<'_, AppState>,
    request: UpdateSettingsRequest,
) -> Result<UpdateStatus, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || {
        api.set_automatic_updates(request.automatic)
            .map_err(map_error)
    })
    .await
    .map_err(map_error)?
}

#[tauri::command]
pub async fn check_for_updates(state: tauri::State<'_, AppState>) -> Result<UpdateStatus, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.check_for_updates().map_err(map_error))
        .await
        .map_err(map_error)?
}

/// `Restart to update`. When the install can't be handed off, the app
/// says why and stays open.
#[tauri::command]
pub fn restart_to_update(app: tauri::AppHandle) -> Result<(), String> {
    // Checked again now: the installer would refuse this bundle after the
    // app quit, closing its tabs for nothing. A dev build is refused here
    // too, as the CLI next to it would open the installed app instead.
    if let Some(reason) = open_app_blocker() {
        return Err(reason);
    }
    let cli = super::sibling_binary_path().ok_or_else(|| {
        "This copy of Localify has no CLI in its bundle, so it can't install updates. Update it by hand."
            .to_string()
    })?;
    hand_off_update(&cli).map_err(|error| format!("{error:#}"))?;
    super::quit_app(&app);
    Ok(())
}

#[tauri::command]
pub async fn read_install_attribution_clipboard() -> Result<Option<String>, String> {
    async_runtime::spawn_blocking(read_clipboard_text)
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn get_bootstrap(state: tauri::State<'_, AppState>) -> Result<BootstrapDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.bootstrap().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub fn get_config(state: tauri::State<'_, AppState>) -> Result<AppConfigDto, String> {
    state.api.read_config().map_err(map_error)
}

#[tauri::command]
pub fn update_config(
    state: tauri::State<'_, AppState>,
    request: ConfigUpdateRequest,
) -> Result<AppConfigDto, String> {
    state.api.update_config(request).map_err(map_error)
}

#[tauri::command]
pub fn export_settings(
    state: tauri::State<'_, AppState>,
    request: SettingsExportRequest,
) -> Result<SettingsExportResultDto, String> {
    state.api.export_settings(request).map_err(map_error)
}

#[tauri::command]
pub fn preview_settings_import(
    state: tauri::State<'_, AppState>,
    request: SettingsImportPreviewRequest,
) -> Result<SettingsImportPreviewDto, String> {
    state
        .api
        .preview_settings_import(request)
        .map_err(map_error)
}

#[tauri::command]
pub fn import_settings(
    state: tauri::State<'_, AppState>,
    request: SettingsImportRequest,
) -> Result<SettingsImportResultDto, String> {
    state.api.import_settings(request).map_err(map_error)
}

#[tauri::command]
pub fn get_daemon_status(state: tauri::State<'_, AppState>) -> Result<DaemonStatusDto, String> {
    state.api.daemon_status().map_err(map_error)
}

#[tauri::command]
pub fn list_domains(state: tauri::State<'_, AppState>) -> Result<Vec<DomainInfoDto>, String> {
    state.api.list_domains().map_err(map_error)
}

#[tauri::command]
pub fn list_projects(state: tauri::State<'_, AppState>) -> Result<Vec<ProjectInfoDto>, String> {
    state.api.list_projects().map_err(map_error)
}

#[tauri::command]
pub fn get_available_port(state: tauri::State<'_, AppState>) -> Result<AvailablePortDto, String> {
    state.api.available_port().map_err(map_error)
}

#[tauri::command]
pub fn detect_project_init_plan(
    state: tauri::State<'_, AppState>,
    request: DetectProjectInitPlanRequest,
) -> Result<DetectProjectInitPlanDto, String> {
    state
        .api
        .detect_project_init_plan(request)
        .map_err(map_error)
}

#[tauri::command]
pub async fn install_localify(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<InstallResultDto, String> {
    let api = state.api.clone();

    let mut result = async_runtime::spawn_blocking(move || api.install().map_err(map_error))
        .await
        .map_err(map_error)??;

    if install_completed(&result) {
        result.steps.push(autostart_install_step(&app));
    }

    Ok(result)
}

#[tauri::command]
pub fn setup_cli_access(state: tauri::State<'_, AppState>) -> Result<CliAccessDto, String> {
    state.api.setup_cli_access().map_err(map_error)
}

#[tauri::command]
pub async fn trust_ca(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.trust_ca().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub fn setup_node_trust(state: tauri::State<'_, AppState>) -> Result<NodeTrustDto, String> {
    state.api.setup_node_trust().map_err(map_error)
}

#[tauri::command]
pub fn trust_ios_simulators(state: tauri::State<'_, AppState>) -> StepDto {
    state.api.trust_ios_simulators()
}

#[tauri::command]
pub fn get_agent_mcp_status(
    state: tauri::State<'_, AppState>,
) -> Result<AgentMcpStatusDto, String> {
    state.api.agent_mcp_status().map_err(map_error)
}

#[tauri::command]
pub fn install_agent_mcp(
    state: tauri::State<'_, AppState>,
) -> Result<AgentMcpInstallResultDto, String> {
    state.api.install_agent_mcp().map_err(map_error)
}

#[tauri::command]
pub async fn connect_agents_once(
    state: tauri::State<'_, AppState>,
) -> Result<Option<AgentMcpInstallResultDto>, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.connect_agents_once().map_err(map_error))
        .await
        .map_err(map_error)?
}

// Localify Cloud calls the network or the daemon, so each runs off the main thread.

#[tauri::command]
pub async fn get_cloud_account(
    state: tauri::State<'_, AppState>,
) -> Result<CloudAccountDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.cloud_account().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn start_cloud_sign_in(
    state: tauri::State<'_, AppState>,
) -> Result<CloudSignInStartDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.start_cloud_sign_in().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn poll_cloud_sign_in(
    state: tauri::State<'_, AppState>,
    request: PollCloudSignInRequest,
) -> Result<CloudSignInPollDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.poll_cloud_sign_in(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn sign_out_cloud(state: tauri::State<'_, AppState>) -> Result<CloudSignOutDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.sign_out_cloud().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn get_cloud_plan(
    state: tauri::State<'_, AppState>,
) -> Result<Option<CloudPlanDto>, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.cloud_plan().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn start_cloud_checkout(
    state: tauri::State<'_, AppState>,
    request: StartCloudCheckoutRequest,
) -> Result<CloudBillingPageDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.start_cloud_checkout(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn start_cloud_addon_checkout(
    state: tauri::State<'_, AppState>,
    request: StartCloudAddonCheckoutRequest,
) -> Result<CloudBillingPageDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || {
        api.start_cloud_addon_checkout(request).map_err(map_error)
    })
    .await
    .map_err(map_error)?
}

#[tauri::command]
pub async fn get_cloud_addon_pricing(
    state: tauri::State<'_, AppState>,
) -> Result<CloudAddonPricingDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.cloud_addon_pricing().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn open_cloud_billing_portal(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.open_cloud_billing_portal().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn refresh_public_tunnel(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.refresh_public_tunnel().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn list_public_urls(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<PublicUrlDto>, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.list_public_urls().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn create_public_url(
    state: tauri::State<'_, AppState>,
    request: CreatePublicUrlRequest,
) -> Result<PublicUrlWithPasswordDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.create_public_url(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn suggest_public_url_slug(
    state: tauri::State<'_, AppState>,
    request: SuggestPublicUrlSlugRequest,
) -> Result<PublicUrlSlugSuggestionDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.suggest_public_url_slug(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn check_public_url_slug(
    state: tauri::State<'_, AppState>,
    request: CheckPublicUrlSlugRequest,
) -> Result<PublicUrlSlugCheckDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.check_public_url_slug(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn remove_public_url(
    state: tauri::State<'_, AppState>,
    request: PublicUrlIdRequest,
) -> Result<(), String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.remove_public_url(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn set_public_url_protection(
    state: tauri::State<'_, AppState>,
    request: SetPublicUrlProtectionRequest,
) -> Result<PublicUrlWithPasswordDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.set_public_url_protection(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn claim_public_url(
    state: tauri::State<'_, AppState>,
    request: PublicUrlIdRequest,
) -> Result<PublicUrlDto, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.claim_public_url(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub fn list_port_conflicts(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<PortConflictDto>, String> {
    state.api.list_port_conflicts().map_err(map_error)
}

#[tauri::command]
pub fn kill_port_conflict(
    state: tauri::State<'_, AppState>,
    request: KillPortConflictRequest,
) -> Result<PortConflictDto, String> {
    state.api.kill_port_conflict(request).map_err(map_error)
}

#[tauri::command]
pub fn get_uninstall_preview(
    state: tauri::State<'_, AppState>,
) -> Result<UninstallPreviewDto, String> {
    state.api.uninstall_preview().map_err(map_error)
}

#[tauri::command]
pub async fn uninstall_localify(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<UninstallResultDto, String> {
    let api = state.api.clone();

    let mut result = async_runtime::spawn_blocking(move || api.uninstall().map_err(map_error))
        .await
        .map_err(map_error)??;

    result.steps.push(autostart_uninstall_step(&app));

    Ok(result)
}

#[tauri::command]
pub fn create_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: CreateProjectRequest,
) -> Result<ProjectInfoDto, String> {
    let result = state.api.create_project(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn create_project_with_domains(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: CreateProjectWithDomainsRequest,
) -> Result<ProjectInfoDto, String> {
    let result = state
        .api
        .create_project_with_domains(request)
        .map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn init_project_from_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: InitProjectFromFolderRequest,
) -> Result<InitProjectFromFolderResultDto, String> {
    let result = state
        .api
        .init_project_from_folder(request)
        .map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn add_project_service(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: AddProjectServiceRequest,
) -> Result<AddProjectServiceResultDto, String> {
    let result = state.api.add_project_service(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn update_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: UpdateProjectRequest,
) -> Result<ProjectInfoDto, String> {
    let result = state.api.update_project(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn relink_project_root(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: RelinkProjectRootRequest,
) -> Result<ProjectInfoDto, String> {
    let result = state.api.relink_project_root(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn remove_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: RemoveProjectRequest,
) -> Result<RemoveProjectResultDto, String> {
    let result = state.api.remove_project(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn remove_project_environment(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: RemoveProjectEnvironmentRequest,
) -> Result<RemoveProjectEnvironmentResultDto, String> {
    let result = state
        .api
        .remove_project_environment(request)
        .map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub async fn discover_project_worktrees(
    state: tauri::State<'_, AppState>,
    request: DiscoverProjectWorktreesRequest,
) -> Result<ProjectWorktreeDiscoveryDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || {
        api.discover_project_worktrees(request).map_err(map_error)
    })
    .await
    .map_err(map_error)?
}

#[tauri::command]
pub async fn attach_project_worktree(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: AttachProjectWorktreeRequest,
) -> Result<AttachProjectWorktreeResultDto, String> {
    let api = state.api.clone();

    let result = async_runtime::spawn_blocking(move || {
        api.attach_project_worktree(request).map_err(map_error)
    })
    .await
    .map_err(map_error)??;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn assign_domain_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: AssignDomainProjectRequest,
) -> Result<DomainInfoDto, String> {
    let result = state
        .api
        .assign_domain_project(request)
        .map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn register_domain(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: RegisterDomainRequest,
) -> Result<RegisterDomainResultDto, String> {
    let result = state.api.register_domain(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn update_domain(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: UpdateDomainRequest,
) -> Result<RegisterDomainResultDto, String> {
    let result = state.api.update_domain(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn unregister_domain(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: DomainPatternRequest,
) -> Result<UnregisterDomainResultDto, String> {
    let result = state.api.unregister_domain(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn add_route(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: AddRouteRequest,
) -> Result<RegistrationMutationDto, String> {
    let result = state.api.add_route(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn remove_route(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: RemoveRouteRequest,
) -> Result<RegistrationMutationDto, String> {
    let result = state.api.remove_route(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn list_routes(
    state: tauri::State<'_, AppState>,
    request: DomainPatternRequest,
) -> Result<RegistrationDto, String> {
    state.api.list_routes(request).map_err(map_error)
}

#[tauri::command]
pub fn start_daemon(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: StartDaemonRequest,
) -> Result<DaemonLaunchDto, String> {
    let result = state.api.start_daemon(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn stop_daemon(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<DaemonStatusDto, String> {
    let result = state.api.stop_daemon().map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn restart_daemon(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: StartDaemonRequest,
) -> Result<DaemonLaunchDto, String> {
    let result = state.api.restart_daemon(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(result)
}

#[tauri::command]
pub fn reload_daemon(
    state: tauri::State<'_, AppState>,
    request: StartDaemonRequest,
) -> Result<DaemonLaunchDto, String> {
    state.api.reload_daemon(request).map_err(map_error)
}

#[tauri::command]
pub fn read_logs(
    state: tauri::State<'_, AppState>,
    request: LogsRequest,
) -> Result<LogsDto, String> {
    state.api.read_logs(request).map_err(map_error)
}

#[tauri::command]
pub fn open_in_editor(
    state: tauri::State<'_, AppState>,
    request: OpenInEditorRequest,
) -> Result<(), String> {
    state.api.open_in_editor(request).map_err(map_error)
}

#[tauri::command]
pub async fn open_env_in_editor(
    state: tauri::State<'_, AppState>,
    request: OpenEnvInEditorRequest,
) -> Result<(), String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || api.open_env_in_editor(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn read_environment_file(
    state: tauri::State<'_, AppState>,
    request: EnvironmentFileRequest,
) -> Result<EnvironmentFileDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || api.read_environment_file(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn list_environment_files(
    state: tauri::State<'_, AppState>,
    request: EnvironmentFileRequest,
) -> Result<EnvironmentFileListDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || api.list_environment_files(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn save_environment_file(
    state: tauri::State<'_, AppState>,
    request: SaveEnvironmentFileRequest,
) -> Result<EnvironmentFileDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || api.save_environment_file(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn preview_environment_file_copy(
    state: tauri::State<'_, AppState>,
    request: EnvironmentFileCopyRequest,
) -> Result<EnvironmentFileCopyPreviewDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || {
        api.preview_environment_file_copy(request)
            .map_err(map_error)
    })
    .await
    .map_err(map_error)?
}

#[tauri::command]
pub async fn copy_environment_files(
    state: tauri::State<'_, AppState>,
    request: EnvironmentFileCopyRequest,
) -> Result<EnvironmentFileCopyPreviewDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || api.copy_environment_files(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub fn open_project_url(
    state: tauri::State<'_, AppState>,
    request: OpenProjectUrlRequest,
) -> Result<(), String> {
    state.api.open_project_url(request).map_err(map_error)
}

#[tauri::command]
pub fn run_local_dev_server(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: RunLocalDevServerRequest,
) -> Result<(), String> {
    state.api.run_local_dev_server(request).map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)
}

#[tauri::command]
pub fn stop_local_dev_server(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: StopLocalDevServerRequest,
) -> Result<(), String> {
    state
        .api
        .stop_local_dev_server(request)
        .map_err(map_error)?;
    super::sync_tray_menu(&app).map_err(map_error)
}

#[tauri::command]
pub fn update_launcher_preferences(
    state: tauri::State<'_, AppState>,
    request: UpdateLauncherPreferencesRequest,
) -> Result<LauncherPreferencesDto, String> {
    state
        .api
        .update_launcher_preferences(request)
        .map_err(map_error)
}

#[tauri::command]
pub fn terminal_list_sessions(
    terminal: tauri::State<'_, TerminalManager>,
) -> Result<Vec<TerminalSessionDto>, String> {
    Ok(terminal.list_sessions())
}

#[tauri::command]
pub fn terminal_create_session(
    state: tauri::State<'_, AppState>,
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalCreateSessionRequest,
) -> Result<TerminalSessionDto, String> {
    terminal
        .create_session(&state.api, request)
        .map_err(map_error)
}

/// Blocking: the first launch may wait for the shell environment.
#[tauri::command]
pub async fn terminal_launch_service(
    state: tauri::State<'_, AppState>,
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalLaunchServiceRequest,
) -> Result<TerminalLaunchServiceResultDto, String> {
    let api = state.api.clone();
    let terminal = terminal.inner().clone();
    async_runtime::spawn_blocking(move || terminal.launch_service(&api, request))
        .await
        .map_err(map_error)?
}

/// Blocking: the stop waits until the old run has ended.
#[tauri::command]
pub async fn terminal_restart_service(
    state: tauri::State<'_, AppState>,
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalRestartServiceRequest,
) -> Result<TerminalLaunchServiceResultDto, String> {
    let api = state.api.clone();
    let terminal = terminal.inner().clone();
    async_runtime::spawn_blocking(move || terminal.restart_service(&api, request))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn terminal_stop_service(
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalSessionRequest,
) -> Result<(), String> {
    let terminal = terminal.inner().clone();
    async_runtime::spawn_blocking(move || terminal.stop_service(request))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub fn terminal_write(
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalWriteRequest,
) -> Result<(), String> {
    terminal.write(request).map_err(map_error)
}

#[tauri::command]
pub fn terminal_resize(
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalResizeRequest,
) -> Result<(), String> {
    terminal.resize(request).map_err(map_error)
}

/// Blocking: closing a running service tab stops its run first.
#[tauri::command]
pub async fn terminal_close(
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalSessionRequest,
) -> Result<(), String> {
    let terminal = terminal.inner().clone();
    async_runtime::spawn_blocking(move || terminal.close(request))
        .await
        .map_err(map_error)?
}

/// Stops a service's last run through its session, wherever it runs.
/// `false` when it was not running.
#[tauri::command]
pub async fn stop_service_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: ServiceRunRequest,
) -> Result<ServiceStopDto, String> {
    let api = state.api.clone();
    let stopped = async_runtime::spawn_blocking(move || {
        api.stop_service_run(request)
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(map_error)??;
    super::sync_tray_menu(&app).map_err(map_error)?;
    Ok(stopped)
}

#[tauri::command]
pub fn shell_environment_status(
    terminal: tauri::State<'_, TerminalManager>,
) -> Result<ShellEnvironmentDto, String> {
    Ok(terminal.shell_environment())
}

/// Reads the shell environment again for the app's runs, and for the
/// daemon's background starts, which keep their own copy.
#[tauri::command]
pub async fn reload_shell_environment(
    terminal: tauri::State<'_, TerminalManager>,
    state: tauri::State<'_, AppState>,
) -> Result<ShellEnvironmentDto, String> {
    let terminal = terminal.inner().clone();
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || {
        let status = terminal.reload_shell_environment();
        // A daemon that is not running reads it when it starts.
        if let Err(error) = api.reload_remote_shell_environment() {
            eprintln!("The daemon could not read the shell environment again: {error:#}");
        }
        status
    })
    .await
    .map_err(map_error)
}

#[tauri::command]
pub fn consume_pending_deep_link(
    state: tauri::State<'_, AppState>,
) -> Result<Option<DeepLinkPayloadDto>, String> {
    Ok(state.take_pending_deep_link())
}
