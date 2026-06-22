use serde::{Deserialize, Serialize};

use crate::config::{
    AgentConfig, DaemonConfig, DockerConfig, KEYGEN_ACCOUNT_ID, KEYGEN_API_URL, KEYGEN_PRODUCT_ID,
    KEYGEN_PUBLIC_KEY, RoxyPaths,
};
use crate::desktop::{
    BrowserPreference, EditorPreference, LauncherPreferences, RunActionPreference,
    TerminalPreference,
};

fn default_project_enabled() -> bool {
    true
}

fn default_cli_access_ready() -> bool {
    false
}

fn default_cli_command_path() -> String {
    "/usr/local/bin/localify".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTrustDto {
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub already_ready: bool,
    #[serde(default)]
    pub ca_cert_path: String,
    #[serde(default)]
    pub shell_snippet_path: String,
    #[serde(default)]
    pub shell_integration_ready: bool,
    #[serde(default)]
    pub launchctl_ready: bool,
    #[serde(default)]
    pub last_error: Option<String>,
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
    #[serde(default)]
    pub node_trust: NodeTrustDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillAgentDto {
    pub id: String,
    pub label: String,
    pub detected: bool,
    pub installed: bool,
    pub skills_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillStatusDto {
    pub skill_dir: String,
    pub npx_available: bool,
    pub detected_agents: Vec<AgentSkillAgentDto>,
    pub install_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillInstallResultDto {
    pub status: AgentSkillStatusDto,
    pub installed_agents: Vec<String>,
    pub stdout: String,
    pub stderr: String,
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
pub struct AgentConfigDto {
    pub install_skill: bool,
    pub promote_dev_script: bool,
}

impl Default for AgentConfigDto {
    fn default() -> Self {
        AgentConfig::default().into()
    }
}

impl From<AgentConfig> for AgentConfigDto {
    fn from(value: AgentConfig) -> Self {
        Self {
            install_skill: value.install_skill,
            promote_dev_script: value.promote_dev_script,
        }
    }
}

impl From<AgentConfigDto> for AgentConfig {
    fn from(value: AgentConfigDto) -> Self {
        Self {
            install_skill: value.install_skill,
            promote_dev_script: value.promote_dev_script,
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
    pub agent: AgentConfigDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdateRequest {
    pub daemon: Option<DaemonConfigDto>,
    pub paths: Option<RoxyPathsDto>,
    pub docker: Option<DockerConfigDto>,
    pub agent: Option<AgentConfigDto>,
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
    #[serde(default)]
    pub last_accessed_at: Option<u64>,
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
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub last_accessed_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsResolutionDto {
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub resolved_ips: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
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
    #[serde(default)]
    pub dns_resolution: DnsResolutionDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapDto {
    pub environment: EnvironmentDto,
    pub config: AppConfigDto,
    pub daemon: DaemonStatusDto,
    #[serde(default)]
    pub licensing: LicensingDto,
    #[serde(default)]
    pub launcher_preferences: LauncherPreferencesDto,
    #[serde(default)]
    pub launcher_availability: LauncherAvailabilityDto,
    #[serde(default)]
    pub domains: Vec<DomainInfoDto>,
    #[serde(default)]
    pub projects: Vec<ProjectInfoDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherPreferencesDto {
    #[serde(default)]
    pub editor: EditorPreference,
    #[serde(default)]
    pub terminal: TerminalPreference,
    #[serde(default)]
    pub browser: BrowserPreference,
    #[serde(default)]
    pub run_action: RunActionPreference,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorOptionDto {
    pub id: EditorPreference,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOptionDto {
    pub id: TerminalPreference,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherAvailabilityDto {
    #[serde(default)]
    pub available_editors: Vec<EditorOptionDto>,
    #[serde(default)]
    pub available_terminals: Vec<TerminalOptionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsTransferSettingsDto {
    pub daemon: DaemonConfigDto,
    pub docker: DockerConfigDto,
    #[serde(default)]
    pub agent: AgentConfigDto,
    pub keygen: KeygenSettingsUpdateRequest,
    pub launcher_preferences: LauncherPreferencesDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsTransferDomainDto {
    pub domain: String,
    pub wildcard: bool,
    #[serde(default)]
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsTransferProjectDto {
    pub id: String,
    pub name: String,
    #[serde(default = "default_project_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub domains: Vec<SettingsTransferDomainDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsTransferSnapshotDto {
    pub version: u32,
    pub scope: String,
    #[serde(default)]
    pub settings: Option<SettingsTransferSettingsDto>,
    #[serde(default)]
    pub standalone_domains: Vec<SettingsTransferDomainDto>,
    #[serde(default)]
    pub projects: Vec<SettingsTransferProjectDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsExportRequest {
    pub file_path: String,
    pub mode: String,
    #[serde(default)]
    pub project_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsExportResultDto {
    pub file_path: String,
    pub mode: String,
    pub includes_settings: bool,
    pub standalone_domain_count: usize,
    pub project_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportPreviewRequest {
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportPreviewProjectDto {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub domain_count: usize,
    pub has_root_path: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportPreviewDto {
    pub file_path: String,
    pub version: u32,
    pub scope: String,
    pub includes_settings: bool,
    pub standalone_domain_count: usize,
    pub project_count: usize,
    #[serde(default)]
    pub projects: Vec<SettingsImportPreviewProjectDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportRequest {
    pub file_path: String,
    pub mode: String,
    #[serde(default)]
    pub project_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsImportResultDto {
    pub mode: String,
    pub imported_projects: usize,
    pub imported_domains: usize,
    pub imported_standalone_domains: usize,
    pub applied_settings: bool,
    pub skipped_root_paths: usize,
    pub daemon_reloaded: bool,
    #[serde(default)]
    pub daemon_reload_error: Option<String>,
}

impl From<LauncherPreferences> for LauncherPreferencesDto {
    fn from(value: LauncherPreferences) -> Self {
        Self {
            editor: value.editor,
            terminal: value.terminal,
            browser: value.browser,
            run_action: value.run_action,
        }
    }
}

impl From<LauncherPreferencesDto> for LauncherPreferences {
    fn from(value: LauncherPreferencesDto) -> Self {
        Self {
            editor: value.editor,
            terminal: value.terminal,
            browser: value.browser,
            run_action: value.run_action,
        }
    }
}

impl From<UpdateLauncherPreferencesRequest> for LauncherPreferences {
    fn from(value: UpdateLauncherPreferencesRequest) -> Self {
        Self {
            editor: value.editor,
            terminal: value.terminal,
            browser: value.browser,
            run_action: value.run_action,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseLimitsDto {
    pub plan: String,
    pub max_projects: Option<usize>,
    pub max_domains_per_project: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineDto {
    pub fingerprint: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveLicenseDto {
    pub key_masked: String,
    pub license_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub expiry: Option<String>,
    #[serde(default)]
    pub last_validated_at: Option<String>,
    #[serde(default)]
    pub validation_code: Option<String>,
    #[serde(default)]
    pub validation_detail: Option<String>,
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub machine_fingerprint: Option<String>,
    #[serde(default)]
    pub machine_name: Option<String>,
    #[serde(default)]
    pub machine_hostname: Option<String>,
    #[serde(default)]
    pub machine_platform: Option<String>,
    #[serde(default)]
    pub response_signature_verified: bool,
    #[serde(default)]
    pub response_signature_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineLicenseDto {
    pub algorithm: String,
    #[serde(default)]
    pub issued: Option<String>,
    #[serde(default)]
    pub file_expiry: Option<String>,
    #[serde(default)]
    pub license_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub license_expiry: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub max_projects: Option<usize>,
    #[serde(default)]
    pub max_domains_per_project: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicensingDto {
    pub status: String,
    pub configured: bool,
    pub api_url: String,
    pub account: String,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub public_key: Option<String>,
    pub limits: LicenseLimitsDto,
    pub free_limits: LicenseLimitsDto,
    pub project_count: usize,
    pub domain_count: usize,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub current_machine: Option<MachineDto>,
    #[serde(default)]
    pub active_license: Option<ActiveLicenseDto>,
    #[serde(default)]
    pub offline_cache_present: bool,
    #[serde(default)]
    pub offline_license: Option<OfflineLicenseDto>,
}

impl Default for LicensingDto {
    fn default() -> Self {
        Self {
            status: "unlicensed".into(),
            configured: true,
            api_url: KEYGEN_API_URL.into(),
            account: KEYGEN_ACCOUNT_ID.into(),
            product: Some(KEYGEN_PRODUCT_ID.into()),
            public_key: Some(KEYGEN_PUBLIC_KEY.into()),
            limits: LicenseLimitsDto {
                plan: "Free".into(),
                max_projects: Some(5),
                max_domains_per_project: None,
            },
            free_limits: LicenseLimitsDto {
                plan: "Free".into(),
                max_projects: Some(5),
                max_domains_per_project: None,
            },
            project_count: 0,
            domain_count: 0,
            last_error: None,
            current_machine: None,
            active_license: None,
            offline_cache_present: false,
            offline_license: None,
        }
    }
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
pub struct CertificateRefreshDto {
    pub refreshed: bool,
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
pub struct UpdateDomainRequest {
    pub current_domain: String,
    pub current_wildcard: bool,
    pub domain: String,
    pub wildcard: bool,
    pub routes: Vec<String>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeygenSettingsUpdateRequest {
    pub api_url: String,
    pub account: String,
    pub product: Option<String>,
    pub public_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateLicenseRequest {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOfflineLicenseRequest {
    pub certificate: String,
    pub key: Option<String>,
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
    #[serde(default)]
    pub root_path: Option<String>,
    pub services: Vec<InitProjectServiceRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenInEditorRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjectUrlRequest {
    pub url: String,
    #[serde(default)]
    pub browser: BrowserPreference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunLocalDevServerRequest {
    pub root_path: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopLocalDevServerRequest {
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLauncherPreferencesRequest {
    #[serde(default)]
    pub editor: EditorPreference,
    #[serde(default)]
    pub terminal: TerminalPreference,
    #[serde(default)]
    pub browser: BrowserPreference,
    #[serde(default)]
    pub run_action: RunActionPreference,
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
    pub root_path: Option<String>,
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
    pub updated_files: Vec<String>,
    pub skipped_file_updates: Vec<String>,
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
