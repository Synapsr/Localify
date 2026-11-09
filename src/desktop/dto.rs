use std::ffi::OsString;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use crate::infrastructure::launch_sessions::LaunchSessionDto;
pub use crate::infrastructure::running_services::{RunState, ServiceOrigin};

use crate::config::{AgentConfig, DaemonConfig, DockerConfig, LocalifyPaths};
use crate::desktop::{
    BrowserPreference, EditorPreference, LauncherPreferences, RunActionPreference,
    TerminalPreference,
};
use crate::infrastructure::running_services::{OutputAvailability, RunningService};

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
    /// The system still needs one administrator approval: a first run, an older
    /// install to replace, or a config change the system has not caught up with.
    #[serde(default)]
    pub system_setup_required: bool,
    #[serde(default = "default_cli_access_ready")]
    pub cli_access_ready: bool,
    #[serde(default = "default_cli_command_path")]
    pub cli_command_path: String,
    #[serde(default)]
    pub node_trust: NodeTrustDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMcpAgentDto {
    pub id: String,
    pub label: String,
    pub detected: bool,
    pub installed: bool,
    pub manual_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMcpStatusDto {
    pub server_name: String,
    pub server_command: String,
    pub agents: Vec<AgentMcpAgentDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMcpInstallResultDto {
    pub status: AgentMcpStatusDto,
    pub installed_agents: Vec<String>,
    /// Copies of the old Localify skill that were removed.
    pub removed_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonConfigDto {
    pub http_port: u16,
    pub https_port: u16,
    pub dns_port: u16,
    pub log_level: String,
    #[serde(default)]
    pub lan_access: bool,
}

impl From<DaemonConfig> for DaemonConfigDto {
    fn from(value: DaemonConfig) -> Self {
        Self {
            http_port: value.http_port,
            https_port: value.https_port,
            dns_port: value.dns_port,
            log_level: value.log_level,
            lan_access: value.lan_access,
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
            lan_access: value.lan_access,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalifyPathsDto {
    pub data_dir: String,
    pub pid_file: String,
    pub log_file: String,
    pub certs_dir: String,
    pub socket_path: String,
}

impl From<LocalifyPaths> for LocalifyPathsDto {
    fn from(value: LocalifyPaths) -> Self {
        Self {
            data_dir: value.data_dir.display().to_string(),
            pid_file: value.pid_file.display().to_string(),
            log_file: value.log_file.display().to_string(),
            certs_dir: value.certs_dir.display().to_string(),
            socket_path: value.socket_path.display().to_string(),
        }
    }
}

impl From<LocalifyPathsDto> for LocalifyPaths {
    fn from(value: LocalifyPathsDto) -> Self {
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
    /// Settings exported before agents got an MCP server call it `installSkill`.
    #[serde(alias = "installSkill")]
    pub connect_agents: bool,
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
            connect_agents: value.connect_agents,
            promote_dev_script: value.promote_dev_script,
        }
    }
}

impl From<AgentConfigDto> for AgentConfig {
    fn from(value: AgentConfigDto) -> Self {
        Self {
            connect_agents: value.connect_agents,
            promote_dev_script: value.promote_dev_script,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigDto {
    pub config_path: String,
    pub daemon: DaemonConfigDto,
    pub paths: LocalifyPathsDto,
    pub docker: DockerConfigDto,
    pub agent: AgentConfigDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdateRequest {
    pub daemon: Option<DaemonConfigDto>,
    pub paths: Option<LocalifyPathsDto>,
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
    /// True when the effective root path no longer exists on disk — the
    /// folder was likely moved or deleted and needs re-linking.
    #[serde(default)]
    pub root_path_missing: bool,
    #[serde(default)]
    pub last_accessed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkProjectRootRequest {
    pub project_id: String,
    pub root_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectServiceInfoDto {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub project_name: String,
    pub project_enabled: bool,
    pub primary_domain: String,
    pub url: String,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub inferred: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEnvironmentServiceInfoDto {
    pub service_id: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEnvironmentInfoDto {
    pub project_id: String,
    pub project_name: String,
    pub name: String,
    pub slug: String,
    pub primary: bool,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub git_common_dir: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub services: Vec<ProjectEnvironmentServiceInfoDto>,
    /// Profiles declared in this checkout's `localify.toml`, or the implicit
    /// Infisical ones.
    #[serde(default)]
    pub launch_profiles: Vec<LaunchProfileInfoDto>,
    /// Why `localify.toml` could not be read, shown instead of the profiles.
    #[serde(default)]
    pub launch_profiles_error: Option<String>,
}

/// A launch profile a service can start with. Addresses only, no values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProfileInfoDto {
    pub name: String,
    pub danger: bool,
    #[serde(default)]
    pub max_duration: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverProjectWorktreesRequest {
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachProjectWorktreeRequest {
    pub project_id: String,
    pub root_path: String,
    #[serde(default)]
    pub primary_root_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachProjectWorktreeResultDto {
    pub environment: ProjectEnvironmentInfoDto,
    pub created: bool,
    pub created_domains: Vec<String>,
    pub daemon_reloaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectWorktreeStatusDto {
    Primary,
    Attached,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorktreeServiceCandidateDto {
    pub id: String,
    pub label: String,
    pub display_path: String,
    #[serde(default)]
    pub service_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorktreeDto {
    pub root_path: String,
    pub name: String,
    pub slug: String,
    pub status: ProjectWorktreeStatusDto,
    #[serde(default)]
    pub attached_environment_slug: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_head: Option<String>,
    #[serde(default)]
    pub git_common_dir: Option<String>,
    #[serde(default)]
    pub existing_environment_file_count: usize,
    #[serde(default)]
    pub service_candidates: Vec<ProjectWorktreeServiceCandidateDto>,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorktreeDiscoveryDto {
    pub project_id: String,
    pub project_name: String,
    pub primary_root_path: String,
    pub worktrees: Vec<ProjectWorktreeDto>,
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
    /// The CA exists but macOS does not trust it yet: the install leaves that for
    /// later, and `trust_ca` finishes it.
    #[serde(default)]
    pub ca_trust_pending: bool,
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
    pub launcher_preferences: LauncherPreferencesDto,
    #[serde(default)]
    pub launcher_availability: LauncherAvailabilityDto,
    #[serde(default)]
    pub domains: Vec<DomainInfoDto>,
    #[serde(default)]
    pub projects: Vec<ProjectInfoDto>,
    #[serde(default)]
    pub services: Vec<ProjectServiceInfoDto>,
    #[serde(default)]
    pub project_environments: Vec<ProjectEnvironmentInfoDto>,
    /// Services currently running with a launch profile, from the daemon.
    #[serde(default)]
    pub launch_sessions: Vec<LaunchSessionDto>,
    /// The last run of every service Localify started.
    #[serde(default)]
    pub service_runs: Vec<ServiceRunDto>,
}

/// A service's last run, wherever it runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRunDto {
    pub project_id: String,
    pub environment: Option<String>,
    pub service_id: String,
    /// `None` for a legacy run.
    pub run_id: Option<String>,
    /// Started from a shell tab by an app from before run PTYs: it keeps
    /// running as it is, exports no output, and has no session to stop.
    pub legacy: bool,
    pub state: RunState,
    pub origin: ServiceOrigin,
    /// The executor's PID.
    pub pid: u32,
    /// Unix seconds.
    pub started_at: u64,
    /// The run's output stream, when it has one.
    pub output_run_id: Option<String>,
}

impl ServiceRunDto {
    /// `primary` is the slug of the project's primary checkout: a record
    /// without an environment is that one's.
    pub fn new(record: &RunningService, state: RunState, primary: Option<&str>) -> Self {
        Self {
            project_id: record.key.project_id.clone(),
            environment: record
                .key
                .environment
                .clone()
                .or_else(|| primary.map(str::to_string)),
            service_id: record.key.service_id.clone(),
            run_id: record.run_id().map(|id| id.to_string()),
            legacy: record.is_legacy(),
            state,
            origin: record.origin,
            pid: record.pid,
            started_at: record.started_at,
            output_run_id: match record.output_availability() {
                OutputAvailability::Available(run_id) => Some(run_id.to_string()),
                OutputAvailability::Terminal | OutputAvailability::LegacyRun => None,
            },
        }
    }
}

/// What a service's run PTY runs, before the app adds its launch id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRunCommand {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
}

/// What stopping a service's last run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceStopOutcome {
    Stopped,
    /// Its last run had already ended.
    AlreadyStopped,
    /// Localify has no record of it: what listens on its port, if anything,
    /// is not a run it knows.
    NoRecord,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStopDto {
    pub outcome: ServiceStopOutcome,
    /// Whether the app may offer to stop whatever listens on the port: only
    /// for a service with no record, never over a run Localify knows.
    pub offer_port_stop: bool,
}

impl From<ServiceStopOutcome> for ServiceStopDto {
    fn from(outcome: ServiceStopOutcome) -> Self {
        Self {
            outcome,
            offer_port_stop: outcome == ServiceStopOutcome::NoRecord,
        }
    }
}

/// One service in one project environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRunRequest {
    pub project_id: String,
    pub environment_slug: String,
    pub service_id: String,
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
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub service_path: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
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
pub struct AddProjectServiceRequest {
    pub project_id: String,
    pub domain: String,
    pub target: String,
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub service_path: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddProjectServiceResultDto {
    pub domain: String,
    pub target: String,
    pub daemon_reloaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProjectServiceDomainRequest {
    pub project_id: String,
    pub service_id: String,
    pub domain: String,
    #[serde(default)]
    pub environment_slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProjectServiceDomainResultDto {
    pub service_id: String,
    pub previous_domain: String,
    pub domain: String,
    pub daemon_reloaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializeProjectServicesRequest {
    pub project_id: String,
    pub root_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializeProjectServicesResultDto {
    pub materialized: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsureProjectEnvironmentRequest {
    pub project_id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub root_path: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsureProjectEnvironmentResultDto {
    pub environment: ProjectEnvironmentInfoDto,
    pub created_domains: Vec<String>,
    pub daemon_reloaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenInEditorRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenEnvInEditorRequest {
    pub project_id: String,
    pub environment_slug: String,
    #[serde(default)]
    pub service_id: Option<String>,
    #[serde(default)]
    pub relative_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFileRequest {
    pub project_id: String,
    pub environment_slug: String,
    #[serde(default)]
    pub service_id: Option<String>,
    #[serde(default)]
    pub relative_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFileCandidateDto {
    pub file_name: String,
    pub relative_path: String,
    pub root_label: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFileListDto {
    pub files: Vec<EnvironmentFileCandidateDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFileDto {
    pub file_name: String,
    pub relative_path: String,
    pub exists: bool,
    pub content: String,
    #[serde(default)]
    pub modified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveEnvironmentFileRequest {
    pub project_id: String,
    pub environment_slug: String,
    #[serde(default)]
    pub service_id: Option<String>,
    #[serde(default)]
    pub relative_path: Option<String>,
    pub content: String,
    #[serde(default)]
    pub expected_modified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFileCopyRequest {
    pub project_id: String,
    pub source_environment_slug: String,
    pub target_environment_slug: String,
    #[serde(default)]
    pub relative_paths: Vec<String>,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EnvironmentFileCopyStatusDto {
    Create,
    Overwrite,
    SkipExists,
    Blocked,
    Copied,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFileCopyItemDto {
    pub relative_path: String,
    pub file_name: String,
    pub status: EnvironmentFileCopyStatusDto,
    pub source_exists: bool,
    pub target_exists: bool,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFileCopyPreviewDto {
    pub items: Vec<EnvironmentFileCopyItemDto>,
    pub create_count: usize,
    pub overwrite_count: usize,
    pub skip_count: usize,
    pub blocked_count: usize,
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
pub struct RemoveProjectEnvironmentRequest {
    pub project_id: String,
    pub environment_slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProjectEnvironmentResultDto {
    pub environment: ProjectEnvironmentInfoDto,
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
    pub service_name: String,
    #[serde(default)]
    pub service_path: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
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
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub service_path: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelStateDto {
    Stopped,
    Starting,
    Connected,
    Error,
    /// The daemon is not running, or did not answer.
    Unknown,
}

/// This machine's tunnel only: the app never claims another machine is online.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatusDto {
    pub state: TunnelStateDto,
    pub connections: u32,
    pub error: Option<String>,
    /// What Localify Cloud last told the daemon; `None` before its first
    /// answer.
    pub runtime: Option<CloudRuntimeStateDto>,
    /// Why this Mac may not serve, when `runtime` is `blocked`.
    pub blocked: Option<CloudRuntimeBlockDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudRuntimeStateDto {
    Idle,
    Provisioning,
    Ready,
    Blocked,
    Banned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRuntimeBlockDto {
    /// `subscription_required`, `serving_slots_full` or `capacity`.
    pub reason: String,
    pub message: String,
    /// What lifts the block, when the account can buy it.
    pub upgrade: Option<CloudUpgradeDto>,
}

/// The command Localify Cloud names to lift a refusal, for a person to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUpgradeDto {
    /// `pro`, `addon_machine` or `addon_urls`.
    pub kind: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudPlanStatusDto {
    None,
    Trialing,
    Active,
    PastDue,
    Canceling,
    Override,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudPlanIntervalDto {
    Month,
    Year,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPlanCountsDto {
    pub serving_machines: u32,
    pub public_urls: u32,
}

/// The account's plan, from `GET /v1/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPlanDto {
    pub status: CloudPlanStatusDto,
    pub entitled: bool,
    pub interval: Option<CloudPlanIntervalDto>,
    /// RFC 3339, while trialing.
    pub trial_ends_at: Option<String>,
    /// RFC 3339: the next charge.
    pub renews_at: Option<String>,
    /// RFC 3339: when a canceled plan or a team access ends.
    pub ends_at: Option<String>,
    pub trial_available: bool,
    pub limits: CloudPlanCountsDto,
    pub usage: CloudPlanCountsDto,
    /// The add-on units the account holds, counted in `limits`.
    pub addons: CloudPlanAddonsDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPlanAddonsDto {
    pub machines: u32,
    /// Packs of 10 public URLs.
    pub url_packs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCloudCheckoutRequest {
    pub interval: CloudPlanIntervalDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudAddonDto {
    /// One serving machine per unit.
    Machine,
    /// Ten public URLs per unit.
    Urls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCloudAddonCheckoutRequest {
    pub addon: CloudAddonDto,
    /// 1 to 20.
    pub units: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudCurrencyDto {
    #[serde(rename = "USD")]
    Usd,
    #[serde(rename = "EUR")]
    Eur,
}

/// The add-ons' monthly prices per unit, in minor units: 300 is $3.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAddonPricingDto {
    pub currency: CloudCurrencyDto,
    pub machine: u32,
    pub url_pack: u32,
    /// False for the list prices, shown while Localify Cloud cannot answer.
    pub live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudBillingPageKindDto {
    Checkout,
    /// The plan was already active: the customer portal opened instead.
    Portal,
}

/// The page `DesktopApi` opened in the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBillingPageDto {
    pub kind: CloudBillingPageKindDto,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountDto {
    pub signed_in: bool,
    pub login: Option<String>,
    pub machine_name: Option<String>,
    pub tunnel: TunnelStatusDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSignInStartDto {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds between two polls.
    pub interval: u64,
    /// Seconds before the code expires.
    pub expires_in: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudSignInStatusDto {
    Pending,
    /// Wait 5 more seconds between polls.
    SlowDown,
    SignedIn,
    NotAllowed,
    Expired,
    /// The person declined on GitHub.
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSignInPollDto {
    pub status: CloudSignInStatusDto,
    /// Why, for `notAllowed`, `expired` and `denied`.
    pub message: Option<String>,
    /// With `slowDown`: the seconds GitHub now asks between polls, when it
    /// says.
    pub interval: Option<u64>,
    /// Set with `signedIn`.
    pub account: Option<CloudAccountDto>,
}

/// Signing out always forgets the token here; `warnings` says what did not
/// follow, such as the cloud not revoking it or the daemon not stopping the
/// tunnel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSignOutDto {
    pub account: CloudAccountDto,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollCloudSignInRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicUrlProtectionModeDto {
    Password,
    Public,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlProtectionDto {
    pub mode: PublicUrlProtectionModeDto,
}

/// No `Debug`: it may carry a password, in `password_link`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlDto {
    pub id: String,
    pub slug: String,
    pub url: String,
    pub local_domain: String,
    pub wildcard: bool,
    /// `None` when no machine serves it.
    pub machine_name: Option<String>,
    pub served_here: bool,
    pub protection: PublicUrlProtectionDto,
    /// The URL with its current password, which signs a browser in; set
    /// when this Mac set that password.
    pub password_link: Option<String>,
    /// `None` from a Localify Cloud older than the paid plan.
    pub state: Option<PublicUrlStateDto>,
    /// RFC 3339.
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicUrlStateDto {
    Serving,
    /// The plan does not cover it right now.
    Suspended,
    /// Removed for abuse.
    TakenDown,
}

/// No `Debug`: it carries a password.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlWithPasswordDto {
    pub public_url: PublicUrlDto,
    /// Generated by Localify Cloud and shown this once; `None` otherwise.
    pub password: Option<String>,
}

/// A password is always generated at creation; it can be changed after.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePublicUrlRequest {
    pub local_domain: String,
    pub wildcard: bool,
    /// The suggested one when missing.
    #[serde(default)]
    pub slug: Option<String>,
    pub mode: PublicUrlProtectionModeDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestPublicUrlSlugRequest {
    pub local_domain: String,
}

/// The first free slug that mirrors a local domain's name, and its URL: the
/// app takes the public domain from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlSlugSuggestionDto {
    pub slug: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckPublicUrlSlugRequest {
    pub slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlSlugCheckDto {
    /// Lower-cased.
    pub slug: String,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlIdRequest {
    pub id: String,
}

/// No `Debug`: it may carry a password.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPublicUrlProtectionRequest {
    pub id: String,
    pub mode: PublicUrlProtectionModeDto,
    /// With `password`: generated when missing.
    #[serde(default)]
    pub password: Option<String>,
}

// --- Remote machines (docs/remote-machines.md, "Desktop app") -------------
//
// The daemon's answers reach the window as it sent them; their shapes are in
// desktop-ui/src/types/localify.ts.

/// This machine's switches for paired machines; each one is optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSettingsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_starts: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMachineRequest {
    pub machine_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePairAnswerRequest {
    pub pairing_id: String,
    pub accept: bool,
}

/// A streaming management command the window listens to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RemoteStreamRequest {
    /// Pairing requests, presence and revocations, while the app runs.
    Events,
    /// A pairing this machine starts.
    Pair { machine_id: String },
    /// Another machine's inventory, while it is on screen.
    Watch { machine_id: String },
    /// A run's output on another machine, from `offset`, or from its tail.
    Follow {
        machine_id: String,
        run_id: String,
        #[serde(default)]
        offset: Option<u64>,
    },
}
