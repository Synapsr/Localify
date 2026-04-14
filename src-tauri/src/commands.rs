use roxy::desktop::DesktopApi;
use roxy::desktop::dto::{
    AddRouteRequest, AppConfigDto, AvailablePortDto, BootstrapDto, ConfigUpdateRequest,
    DaemonLaunchDto, DaemonStatusDto, DomainInfoDto, DomainPatternRequest, InstallResultDto,
    LogsDto, LogsRequest, RegisterDomainRequest, RegisterDomainResultDto, RegistrationDto,
    RegistrationMutationDto, RemoveRouteRequest, StartDaemonRequest, UninstallPreviewDto,
    UninstallResultDto, UnregisterDomainResultDto,
};

pub struct AppState {
    pub api: DesktopApi,
}

fn map_error<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
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
pub fn get_daemon_status(state: tauri::State<'_, AppState>) -> Result<DaemonStatusDto, String> {
    state.api.daemon_status().map_err(map_error)
}

#[tauri::command]
pub fn list_domains(state: tauri::State<'_, AppState>) -> Result<Vec<DomainInfoDto>, String> {
    state.api.list_domains().map_err(map_error)
}

#[tauri::command]
pub fn get_available_port(state: tauri::State<'_, AppState>) -> Result<AvailablePortDto, String> {
    state.api.available_port().map_err(map_error)
}

#[tauri::command]
pub fn install_roxy(state: tauri::State<'_, AppState>) -> Result<InstallResultDto, String> {
    state.api.install().map_err(map_error)
}

#[tauri::command]
pub fn get_uninstall_preview(
    state: tauri::State<'_, AppState>,
) -> Result<UninstallPreviewDto, String> {
    state.api.uninstall_preview().map_err(map_error)
}

#[tauri::command]
pub fn uninstall_roxy(state: tauri::State<'_, AppState>) -> Result<UninstallResultDto, String> {
    state.api.uninstall().map_err(map_error)
}

#[tauri::command]
pub fn register_domain(
    state: tauri::State<'_, AppState>,
    request: RegisterDomainRequest,
) -> Result<RegisterDomainResultDto, String> {
    state.api.register_domain(request).map_err(map_error)
}

#[tauri::command]
pub fn unregister_domain(
    state: tauri::State<'_, AppState>,
    request: DomainPatternRequest,
) -> Result<UnregisterDomainResultDto, String> {
    state.api.unregister_domain(request).map_err(map_error)
}

#[tauri::command]
pub fn add_route(
    state: tauri::State<'_, AppState>,
    request: AddRouteRequest,
) -> Result<RegistrationMutationDto, String> {
    state.api.add_route(request).map_err(map_error)
}

#[tauri::command]
pub fn remove_route(
    state: tauri::State<'_, AppState>,
    request: RemoveRouteRequest,
) -> Result<RegistrationMutationDto, String> {
    state.api.remove_route(request).map_err(map_error)
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
    state: tauri::State<'_, AppState>,
    request: StartDaemonRequest,
) -> Result<DaemonLaunchDto, String> {
    state.api.start_daemon(request).map_err(map_error)
}

#[tauri::command]
pub fn stop_daemon(state: tauri::State<'_, AppState>) -> Result<DaemonStatusDto, String> {
    state.api.stop_daemon().map_err(map_error)
}

#[tauri::command]
pub fn restart_daemon(
    state: tauri::State<'_, AppState>,
    request: StartDaemonRequest,
) -> Result<DaemonLaunchDto, String> {
    state.api.restart_daemon(request).map_err(map_error)
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
