use localify::desktop::DesktopApi;
use localify::desktop::dto::{
    AddProjectServiceRequest, AddProjectServiceResultDto, AddRouteRequest,
    AgentSkillInstallResultDto, AgentSkillStatusDto, AppConfigDto, AssignDomainProjectRequest,
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
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::async_runtime;
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;

use crate::terminal::{
    TerminalCreateSessionRequest, TerminalLaunchServiceRequest, TerminalLaunchServiceResultDto,
    TerminalManager, TerminalResizeRequest, TerminalSessionDto, TerminalSessionRequest,
    TerminalWriteRequest,
};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeepLinkRouteDto {
    Projects,
    Logs,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfoDto {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateCheckDto {
    pub configured: bool,
    pub update: Option<AppUpdateInfoDto>,
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

const INSTALL_ATTRIBUTION_PREFIX: &str = "localify:install-attribution:v1:";
const UPDATE_DOWNLOAD_ATTEMPTS: usize = 3;
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

fn map_error<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

fn map_updater_error<E: std::fmt::Display>(stage: &str, error: E) -> String {
    let message = format!("{stage}: {error}");
    append_updater_log(&format!("error {message}"));
    message
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

fn desktop_state_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "Failed to resolve the user home directory.".to_string())?;

    Ok(home
        .join("Library/Application Support")
        .join(localify::branding::BRANDING.desktop_state_dir_name()))
}

fn append_updater_log(message: &str) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let Ok(directory) = desktop_state_dir() else {
        return;
    };

    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }

    let path = directory.join("updater.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{timestamp} {message}");
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

fn build_updater<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<tauri_plugin_updater::Updater>, String> {
    let Some(endpoint) = super::updater_endpoint() else {
        append_updater_log("build skipped updater_endpoint=none");
        return Ok(None);
    };

    append_updater_log(&format!("build endpoint={endpoint}"));
    let builder = app
        .updater_builder()
        .pubkey(super::updater_public_key())
        .timeout(UPDATE_CHECK_TIMEOUT)
        .endpoints(vec![tauri::Url::parse(endpoint).map_err(map_error)?])
        .map_err(|error| map_updater_error("configure updater endpoint", error))?;
    let updater = super::configure_updater_builder(builder)
        .build()
        .map_err(|error| map_updater_error("build updater", error))?;

    Ok(Some(updater))
}

pub(crate) async fn check_app_update_with_handle<R: Runtime>(
    app: AppHandle<R>,
) -> Result<AppUpdateCheckDto, String> {
    let current_version = app.package_info().version.to_string();
    append_updater_log(&format!("check start current_version={current_version}"));
    let Some(updater) = build_updater(&app)? else {
        return Ok(AppUpdateCheckDto {
            configured: false,
            update: None,
        });
    };

    let update = updater
        .check()
        .await
        .map_err(|error| map_updater_error("check update manifest", error))?
        .map(|update| AppUpdateInfoDto {
            version: update.version.to_string(),
            notes: update.body,
            pub_date: update.date.map(|value| value.to_string()),
        });
    match &update {
        Some(update) => append_updater_log(&format!("check available version={}", update.version)),
        None => append_updater_log("check no_update"),
    }

    Ok(AppUpdateCheckDto {
        configured: true,
        update,
    })
}

pub(crate) async fn install_app_update_with_handle<R: Runtime>(
    app: AppHandle<R>,
) -> Result<(), String> {
    append_updater_log("install start");
    let Some(updater) = build_updater(&app)? else {
        return Err("Desktop updater is not configured for this build.".into());
    };

    let Some(mut update) = updater
        .check()
        .await
        .map_err(|error| map_updater_error("install check update manifest", error))?
    else {
        append_updater_log("install no_update");
        return Ok(());
    };

    append_updater_log(&format!(
        "install candidate version={} url={}",
        update.version, update.download_url
    ));
    update.timeout = Some(UPDATE_DOWNLOAD_TIMEOUT);

    let mut downloaded_update = None;
    let mut last_download_error = None;
    for attempt in 1..=UPDATE_DOWNLOAD_ATTEMPTS {
        append_updater_log(&format!(
            "download start attempt={attempt}/{UPDATE_DOWNLOAD_ATTEMPTS}"
        ));
        let mut downloaded_bytes = 0usize;
        let mut expected_bytes = None;
        let mut next_progress_log = 5 * 1024 * 1024;
        let download_result = update
            .download(
                |chunk_size, content_length| {
                    downloaded_bytes += chunk_size;
                    expected_bytes = content_length;
                    if downloaded_bytes >= next_progress_log {
                        append_updater_log(&format!(
                            "download progress attempt={attempt} bytes={downloaded_bytes} expected={expected_bytes:?}"
                        ));
                        next_progress_log += 5 * 1024 * 1024;
                    }
                },
                || {
                    append_updater_log(&format!("download finished attempt={attempt}"));
                },
            )
            .await;

        match download_result {
            Ok(bytes) => {
                append_updater_log(&format!(
                    "download verified attempt={attempt} bytes={}",
                    bytes.len()
                ));
                downloaded_update = Some(bytes);
                last_download_error = None;
                break;
            }
            Err(error) => {
                let message = format!(
                    "{error}; attempt={attempt}; downloaded_bytes={downloaded_bytes}; expected_bytes={expected_bytes:?}"
                );
                append_updater_log(&format!("download error {message}"));
                last_download_error = Some(message);
            }
        }
    }

    let Some(downloaded_update) = downloaded_update else {
        let message = last_download_error.unwrap_or_else(|| "unknown download error".into());
        return Err(map_updater_error("download update", message));
    };

    update
        .install(&downloaded_update)
        .map_err(|error| map_updater_error("install downloaded update", error))?;

    append_updater_log("install finished restarting");
    app.restart()
}

#[tauri::command]
pub async fn check_app_update(app: tauri::AppHandle) -> Result<AppUpdateCheckDto, String> {
    check_app_update_with_handle(app).await
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
pub async fn install_app_update(app: tauri::AppHandle) -> Result<(), String> {
    install_app_update_with_handle(app).await
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
pub fn setup_node_trust(state: tauri::State<'_, AppState>) -> Result<NodeTrustDto, String> {
    state.api.setup_node_trust().map_err(map_error)
}

#[tauri::command]
pub fn trust_ios_simulators(state: tauri::State<'_, AppState>) -> StepDto {
    state.api.trust_ios_simulators()
}

#[tauri::command]
pub fn get_agent_skill_status(
    state: tauri::State<'_, AppState>,
) -> Result<AgentSkillStatusDto, String> {
    state.api.agent_skill_status().map_err(map_error)
}

#[tauri::command]
pub fn install_agent_skill(
    state: tauri::State<'_, AppState>,
) -> Result<AgentSkillInstallResultDto, String> {
    state.api.install_agent_skill().map_err(map_error)
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
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalCreateSessionRequest,
) -> Result<TerminalSessionDto, String> {
    terminal
        .create_session(app, &state.api, request)
        .map_err(map_error)
}

#[tauri::command]
pub fn terminal_launch_service(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalLaunchServiceRequest,
) -> Result<TerminalLaunchServiceResultDto, String> {
    terminal
        .launch_service(app, &state.api, request)
        .map_err(map_error)
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

#[tauri::command]
pub fn terminal_interrupt(
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalSessionRequest,
) -> Result<(), String> {
    terminal.interrupt(request).map_err(map_error)
}

#[tauri::command]
pub fn terminal_close(
    terminal: tauri::State<'_, TerminalManager>,
    request: TerminalSessionRequest,
) -> Result<(), String> {
    terminal.close(request).map_err(map_error)
}

#[tauri::command]
pub fn consume_pending_deep_link(
    state: tauri::State<'_, AppState>,
) -> Result<Option<DeepLinkPayloadDto>, String> {
    Ok(state.take_pending_deep_link())
}
