use roxy::desktop::DesktopApi;
use roxy::desktop::dto::{
    ActivateLicenseRequest, AddRouteRequest, AppConfigDto, AssignDomainProjectRequest,
    AvailablePortDto, BootstrapDto, CliAccessDto, ConfigUpdateRequest, CreateProjectRequest,
    CreateProjectWithDomainsRequest, DaemonLaunchDto, DaemonStatusDto, DetectProjectInitPlanDto,
    DetectProjectInitPlanRequest, DomainInfoDto, DomainPatternRequest, ImportOfflineLicenseRequest,
    InitProjectFromFolderRequest, InitProjectFromFolderResultDto, InstallResultDto,
    KeygenSettingsUpdateRequest, KillPortConflictRequest, LauncherPreferencesDto, LicensingDto,
    LogsDto, LogsRequest, NodeTrustDto, OpenInEditorRequest, OpenProjectUrlRequest,
    PortConflictDto, ProjectInfoDto, RegisterDomainRequest, RegisterDomainResultDto,
    RegistrationDto, RegistrationMutationDto, RemoveProjectRequest, RemoveProjectResultDto,
    RemoveRouteRequest, RunLocalDevServerRequest, SettingsExportRequest, SettingsExportResultDto,
    SettingsImportPreviewDto, SettingsImportPreviewRequest, SettingsImportRequest,
    SettingsImportResultDto, StartDaemonRequest, StopLocalDevServerRequest, UninstallPreviewDto,
    UninstallResultDto, UnregisterDomainResultDto, UpdateDomainRequest,
    UpdateLauncherPreferencesRequest, UpdateProjectRequest,
};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use tauri::async_runtime;
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;

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
    SettingsLicensing,
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

const INSTALL_ATTRIBUTION_PREFIX: &str = "localify:install-attribution:v1:";

fn map_error<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
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
        return Ok(None);
    };

    let updater = app
        .updater_builder()
        .pubkey(super::updater_public_key())
        .endpoints(vec![tauri::Url::parse(endpoint).map_err(map_error)?])
        .map_err(map_error)?
        .build()
        .map_err(map_error)?;

    Ok(Some(updater))
}

pub(crate) async fn check_app_update_with_handle<R: Runtime>(
    app: AppHandle<R>,
) -> Result<AppUpdateCheckDto, String> {
    let Some(updater) = build_updater(&app)? else {
        return Ok(AppUpdateCheckDto {
            configured: false,
            update: None,
        });
    };

    let update = updater
        .check()
        .await
        .map_err(map_error)?
        .map(|update| AppUpdateInfoDto {
            version: update.version.to_string(),
            notes: update.body,
            pub_date: update.date.map(|value| value.to_string()),
        });

    Ok(AppUpdateCheckDto {
        configured: true,
        update,
    })
}

pub(crate) async fn install_app_update_with_handle<R: Runtime>(
    app: AppHandle<R>,
) -> Result<(), String> {
    let Some(updater) = build_updater(&app)? else {
        return Err("Desktop updater is not configured for this build.".into());
    };

    let Some(update) = updater.check().await.map_err(map_error)? else {
        return Ok(());
    };

    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(map_error)?;

    app.restart()
}

#[tauri::command]
pub async fn check_app_update(app: tauri::AppHandle) -> Result<AppUpdateCheckDto, String> {
    check_app_update_with_handle(app).await
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
pub fn get_bootstrap(state: tauri::State<'_, AppState>) -> Result<BootstrapDto, String> {
    state.api.bootstrap().map_err(map_error)
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
pub fn get_licensing_status(state: tauri::State<'_, AppState>) -> Result<LicensingDto, String> {
    state.api.licensing_status().map_err(map_error)
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
pub fn update_keygen_settings(
    state: tauri::State<'_, AppState>,
    request: KeygenSettingsUpdateRequest,
) -> Result<LicensingDto, String> {
    state.api.update_keygen_settings(request).map_err(map_error)
}

#[tauri::command]
pub fn activate_license(
    state: tauri::State<'_, AppState>,
    request: ActivateLicenseRequest,
) -> Result<LicensingDto, String> {
    state.api.activate_license(request).map_err(map_error)
}

#[tauri::command]
pub fn import_offline_license(
    state: tauri::State<'_, AppState>,
    request: ImportOfflineLicenseRequest,
) -> Result<LicensingDto, String> {
    state.api.import_offline_license(request).map_err(map_error)
}

#[tauri::command]
pub fn clear_license(state: tauri::State<'_, AppState>) -> Result<LicensingDto, String> {
    state.api.clear_license().map_err(map_error)
}

#[tauri::command]
pub fn refresh_license(state: tauri::State<'_, AppState>) -> Result<LicensingDto, String> {
    state.api.refresh_license().map_err(map_error)
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
pub async fn install_roxy(state: tauri::State<'_, AppState>) -> Result<InstallResultDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || api.install().map_err(map_error))
        .await
        .map_err(map_error)?
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
pub async fn uninstall_roxy(
    state: tauri::State<'_, AppState>,
) -> Result<UninstallResultDto, String> {
    let api = state.api.clone();

    async_runtime::spawn_blocking(move || api.uninstall().map_err(map_error))
        .await
        .map_err(map_error)?
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
pub fn consume_pending_deep_link(
    state: tauri::State<'_, AppState>,
) -> Result<Option<DeepLinkPayloadDto>, String> {
    Ok(state.take_pending_deep_link())
}
