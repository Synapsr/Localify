use serde::{Deserialize, Serialize};

use crate::config::{DaemonConfig, DockerConfig, RoxyPaths};

fn default_project_enabled() -> bool {
    true
}

fn default_cli_access_ready() -> bool {
    false
}

fn default_cli_command_path() -> String {
    "/usr/local/bin/localify".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDto {
    pub app_name: String,
    pub desktop_name: String,
    pub config_path: String,
    pub binary_path: String,
    pub binary_exists: bool,
    pub fixed_domain_suffix: String,
    pub cli_helper_ready: bool,
    #[serde(default = "default_cli_access_ready")]
    pub cli_access_ready: bool,
    #[serde(default = "default_cli_command_path")]
    pub cli_command_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonConfigDto {
    pub http_port: u16,
    pub https_port: u16,
    pub dns_port: u16,
    pub log_level: String,
}

impl From<DaemonConfig> for DaemonConfigDto {
    fn from(value: DaemonConfig) -> Self {
        Self {
            http_port: value.http_port,
            https_port: value.https_port,
            dns_port: value.dns_port,
            log_level: value.log_level,
        }
    }
}

impl From<DaemonConfigDto> for DaemonConfig {
    fn from(value: DaemonConfigDto) -> Self {
        Self {
            http_port: value.http_port,
            https_port: value.https_port,
            dns_port: value.dns_port,
            log_level: value.log_level,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoxyPathsDto {
    pub data_dir: String,
    pub pid_file: String,
    pub log_file: String,
    pub certs_dir: String,
    pub socket_path: String,
}

impl From<RoxyPaths> for RoxyPathsDto {
    fn from(value: RoxyPaths) -> Self {
        Self {
            data_dir: value.data_dir.display().to_string(),
            pid_file: value.pid_file.display().to_string(),
            log_file: value.log_file.display().to_string(),
            certs_dir: value.certs_dir.display().to_string(),
            socket_path: value.socket_path.display().to_string(),
        }
    }
}

impl From<RoxyPathsDto> for RoxyPaths {
    fn from(value: RoxyPathsDto) -> Self {
        Self {
            data_dir: value.data_dir.into(),
            pid_file: value.pid_file.into(),
            log_file: value.log_file.into(),
            certs_dir: value.certs_dir.into(),
            socket_path: value.socket_path.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerConfigDto {
    pub enabled: bool,
}

impl From<DockerConfig> for DockerConfigDto {
    fn from(value: DockerConfig) -> Self {
        Self {
            enabled: value.enabled,
        }
    }
}

impl From<DockerConfigDto> for DockerConfig {
    fn from(value: DockerConfigDto) -> Self {
        Self {
            enabled: value.enabled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigDto {
    pub config_path: String,
    pub daemon: DaemonConfigDto,
    pub paths: RoxyPathsDto,
    pub docker: DockerConfigDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdateRequest {
    pub daemon: Option<DaemonConfigDto>,
    pub paths: Option<RoxyPathsDto>,
    pub docker: Option<DockerConfigDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDto {
    pub path: String,
    pub target: String,
    pub target_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationDto {
    pub pattern: String,
    pub domain: String,
    pub wildcard: bool,
    pub https_enabled: bool,
    pub source: String,
    pub routes: Vec<RouteDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainInfoDto {
    pub registration: RegistrationDto,
    #[serde(default)]
    pub has_cert: bool,
    #[serde(default)]
    pub cert_trusted: Option<bool>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default = "default_project_enabled")]
    pub project_enabled: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub target_reachable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfoDto {
    pub id: String,
    pub name: String,
    #[serde(default = "default_project_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub domain_count: usize,
    #[serde(default)]
    pub has_reachable_domain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatusDto {
    pub running: bool,
    pub pid: Option<u32>,
    pub lan_ip: String,
    pub ca_installed: bool,
    pub http_port: u16,
    pub https_port: u16,
    pub dns_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapDto {
    pub environment: EnvironmentDto,
    pub config: AppConfigDto,
    pub daemon: DaemonStatusDto,
    #[serde(default)]
    pub domains: Vec<DomainInfoDto>,
    #[serde(default)]
    pub projects: Vec<ProjectInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDto {
    pub label: Option<String>,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResultDto {
    pub lan_ip: String,
    pub steps: Vec<StepDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliAccessDto {
    pub ready: bool,
    pub command_path: String,
    pub already_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortConflictDto {
    pub protocol: String,
    pub port: u16,
    pub pid: u32,
    pub process_name: String,
    pub user: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KillPortConflictRequest {
    pub protocol: String,
    pub port: u16,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPreviewDto {
    pub domain_count: usize,
    pub data_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResultDto {
    pub steps: Vec<StepDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterDomainRequest {
    pub domain: String,
    pub wildcard: bool,
    pub routes: Vec<String>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectDomainRequest {
    pub domain: String,
    pub wildcard: bool,
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectWithDomainsRequest {
    pub name: String,
    #[serde(default)]
    pub domains: Vec<CreateProjectDomainRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitProjectServiceRequest {
    pub domain: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitProjectRequest {
    pub name: String,
    pub services: Vec<InitProjectServiceRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitProjectResultDto {
    pub project: ProjectInfoDto,
    pub services: Vec<InitProjectServiceRequest>,
    pub daemon_reloaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignDomainProjectRequest {
    pub domain: String,
    pub wildcard: bool,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectRequest {
    pub id: String,
    pub name: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProjectRequest {
    pub id: String,
    #[serde(default)]
    pub delete_domains: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProjectResultDto {
    pub project: ProjectInfoDto,
    #[serde(default)]
    pub removed_domains: Vec<String>,
    pub daemon_reloaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailablePortDto {
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectProjectInitPlanRequest {
    pub root_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInitServiceDto {
    pub id: String,
    pub label: String,
    pub display_path: String,
    pub domain: String,
    pub target: String,
    pub enabled: bool,
    pub can_update_dev_script: bool,
    pub update_dev_script: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectProjectInitPlanDto {
    pub root_path: String,
    pub name: String,
    pub services: Vec<FolderInitServiceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitProjectFromFolderServiceRequest {
    pub id: String,
    pub domain: String,
    pub target: String,
    pub enabled: bool,
    pub update_dev_script: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitProjectFromFolderRequest {
    pub root_path: String,
    pub name: String,
    pub services: Vec<InitProjectFromFolderServiceRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitProjectFromFolderResultDto {
    pub project: ProjectInfoDto,
    pub services: Vec<InitProjectServiceRequest>,
    pub daemon_reloaded: bool,
    pub updated_scripts: Vec<String>,
    pub skipped_script_updates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainPatternRequest {
    pub domain: String,
    pub wildcard: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRouteRequest {
    pub domain: String,
    pub wildcard: bool,
    pub path: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveRouteRequest {
    pub domain: String,
    pub wildcard: bool,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationMutationDto {
    pub registration: RegistrationDto,
    pub route: Option<RouteDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterDomainResultDto {
    pub registration: RegistrationDto,
    pub cert_outcome: StepDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnregisterDomainResultDto {
    pub registration: RegistrationDto,
    pub cert_outcome: StepDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartDaemonRequest {
    pub verbose: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonLaunchDto {
    pub pid: u32,
    pub lan_ip: String,
    pub http_port: u16,
    pub https_port: u16,
    pub dns_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsRequest {
    pub lines: usize,
    pub clear: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsDto {
    pub path: String,
    pub exists: bool,
    pub cleared: bool,
    pub content: String,
}
