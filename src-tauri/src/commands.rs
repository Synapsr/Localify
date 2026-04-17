use roxy::desktop::DesktopApi;
use roxy::desktop::dto::{
    AddRouteRequest, AppConfigDto, AssignDomainProjectRequest, AvailablePortDto, BootstrapDto,
    CliAccessDto, ConfigUpdateRequest, CreateProjectRequest, CreateProjectWithDomainsRequest,
    DaemonLaunchDto, DaemonStatusDto, DomainInfoDto, DomainPatternRequest, InstallResultDto,
    KillPortConflictRequest, LogsDto, LogsRequest, PortConflictDto, ProjectInfoDto,
    RegisterDomainRequest, RegisterDomainResultDto, RegistrationDto, RegistrationMutationDto,
    RemoveProjectRequest, RemoveProjectResultDto, RemoveRouteRequest, StartDaemonRequest,
    UninstallPreviewDto, UninstallResultDto, UnregisterDomainResultDto, UpdateProjectRequest,
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
pub fn list_projects(state: tauri::State<'_, AppState>) -> Result<Vec<ProjectInfoDto>, String> {
    state.api.list_projects().map_err(map_error)
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
pub fn setup_cli_access(state: tauri::State<'_, AppState>) -> Result<CliAccessDto, String> {
    state.api.setup_cli_access().map_err(map_error)
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
pub fn uninstall_roxy(state: tauri::State<'_, AppState>) -> Result<UninstallResultDto, String> {
    state.api.uninstall().map_err(map_error)
}

#[tauri::command]
pub fn create_project(
    state: tauri::State<'_, AppState>,
    request: CreateProjectRequest,
) -> Result<ProjectInfoDto, String> {
    state.api.create_project(request).map_err(map_error)
}

#[tauri::command]
pub fn create_project_with_domains(
    state: tauri::State<'_, AppState>,
    request: CreateProjectWithDomainsRequest,
) -> Result<ProjectInfoDto, String> {
    state.api.create_project_with_domains(request).map_err(map_error)
}

#[tauri::command]
pub fn update_project(
    state: tauri::State<'_, AppState>,
    request: UpdateProjectRequest,
) -> Result<ProjectInfoDto, String> {
    state.api.update_project(request).map_err(map_error)
}

#[tauri::command]
pub fn remove_project(
    state: tauri::State<'_, AppState>,
    request: RemoveProjectRequest,
) -> Result<RemoveProjectResultDto, String> {
    state.api.remove_project(request).map_err(map_error)
}

#[tauri::command]
pub fn assign_domain_project(
    state: tauri::State<'_, AppState>,
    request: AssignDomainProjectRequest,
) -> Result<DomainInfoDto, String> {
    state.api.assign_domain_project(request).map_err(map_error)
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
