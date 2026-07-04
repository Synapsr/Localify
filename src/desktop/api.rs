use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::application::StepOutcome;
use crate::application::daemon_status::QueryDaemonStatus;
use crate::application::install::Install;
use crate::application::manage_routes::ManageRoutes;
use crate::application::ports::DaemonConnection;
use crate::application::register_domain::RegisterDomain;
use crate::application::restart_daemon::RestartDaemon;
use crate::application::start_daemon::StartDaemon;
use crate::application::stop_daemon::StopDaemon;
use crate::application::uninstall::Uninstall;
use crate::application::unregister_domain::UnregisterDomain;
use crate::branding::BRANDING;
use crate::desktop::dto::{
    ActivateLicenseRequest, AddProjectServiceRequest, AddProjectServiceResultDto, AddRouteRequest,
    AgentSkillAgentDto, AgentSkillInstallResultDto, AgentSkillStatusDto, AppConfigDto,
    AssignDomainProjectRequest, AttachProjectWorktreeRequest, AttachProjectWorktreeResultDto,
    AvailablePortDto, BootstrapDto, CertificateRefreshDto, CliAccessDto, ConfigUpdateRequest,
    CreateProjectRequest, CreateProjectWithDomainsRequest, DaemonLaunchDto, DaemonStatusDto,
    DetectProjectInitPlanDto, DetectProjectInitPlanRequest, DiscoverProjectWorktreesRequest,
    DnsResolutionDto, DomainInfoDto, DomainPatternRequest, EnsureProjectEnvironmentRequest,
    EnsureProjectEnvironmentResultDto, EnvironmentDto, EnvironmentFileCandidateDto,
    EnvironmentFileCopyItemDto, EnvironmentFileCopyPreviewDto, EnvironmentFileCopyRequest,
    EnvironmentFileCopyStatusDto, EnvironmentFileDto, EnvironmentFileListDto,
    EnvironmentFileRequest, FolderInitServiceDto, ImportOfflineLicenseRequest,
    InitProjectFromFolderRequest, InitProjectFromFolderResultDto, InitProjectRequest,
    InitProjectResultDto, InstallResultDto, KeygenSettingsUpdateRequest, KillPortConflictRequest,
    LauncherAvailabilityDto, LogsDto, LogsRequest, MaterializeProjectServicesRequest,
    MaterializeProjectServicesResultDto, NodeTrustDto, OpenEnvInEditorRequest, OpenInEditorRequest,
    OpenProjectUrlRequest, PortConflictDto, ProjectEnvironmentInfoDto,
    ProjectEnvironmentServiceInfoDto, ProjectInfoDto, ProjectServiceInfoDto,
    ProjectWorktreeDiscoveryDto, ProjectWorktreeDto, ProjectWorktreeServiceCandidateDto,
    ProjectWorktreeStatusDto, RegisterDomainRequest, RegisterDomainResultDto, RegistrationDto,
    RegistrationMutationDto, RemoveProjectEnvironmentRequest, RemoveProjectEnvironmentResultDto,
    RemoveProjectRequest, RemoveProjectResultDto, RemoveRouteRequest, RouteDto,
    RunLocalDevServerRequest, SaveEnvironmentFileRequest, SetProjectServiceDomainRequest,
    SetProjectServiceDomainResultDto, SettingsExportRequest, SettingsExportResultDto,
    SettingsImportPreviewDto, SettingsImportPreviewProjectDto, SettingsImportPreviewRequest,
    SettingsImportRequest, SettingsImportResultDto, SettingsTransferDomainDto,
    SettingsTransferProjectDto, SettingsTransferSettingsDto, SettingsTransferSnapshotDto,
    StartDaemonRequest, StepDto, StopLocalDevServerRequest, UninstallPreviewDto,
    UninstallResultDto, UnregisterDomainResultDto, UpdateDomainRequest,
    UpdateLauncherPreferencesRequest, UpdateProjectRequest,
};
use crate::desktop::home_dir::user_home_dir;
use crate::desktop::node_trust;
use crate::desktop::port_conflicts::{
    PortConflictProcess, kill_daemon_port_conflict, list_daemon_port_conflicts,
};
use crate::desktop::{LauncherPreferencesStore, ProjectRootStore};
use crate::domain::{DomainPattern, DomainRegistration, PathPrefix, Route, RouteTarget};
use crate::infrastructure::certs::CertificateService;
use crate::infrastructure::config::{
    Config, ConfigStore, ConfiguredDomain, ConfiguredProject, ConfiguredProjectEnvironment,
    ConfiguredService, ProjectEnvironmentConfig, ProjectEnvironmentServiceConfig,
    ProjectServiceConfig,
};
use crate::infrastructure::dns::{SystemResolutionCheck, check_system_resolution, get_dns_service};
use crate::infrastructure::filesystem::FileSystemSetup;
use crate::infrastructure::licensing::{
    FREE_MAX_PROJECTS, KeygenClient, ResolvedLicenseStatus, build_offline_imported_license,
    current_machine_fingerprint, mask_license_key, resolve_license_state,
};
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::network::{get_lan_ip, get_network_info};
use crate::infrastructure::paths::RoxyPaths;
use crate::infrastructure::pid::PidFile;
use crate::infrastructure::port_check::ensure_daemon_ports_available;
use crate::infrastructure::target_probe::is_route_target_reachable;
use crate::infrastructure::{editor, local_dev};
use crate::project_init::{
    FolderInitSelection, LocalAppUpdatePolicy, apply_local_app_updates, detect_folder_init_plan,
    resolve_folder_init_selection,
};
use crate::system_helper::{SystemHelperClient, current_uid, helper_socket_path};

const CLI_INSTALL_DIR: &str = "/usr/local/bin";
const SETTINGS_TRANSFER_VERSION: u32 = 1;
const SETTINGS_TRANSFER_SCOPE_ALL: &str = "all";
const SETTINGS_TRANSFER_SCOPE_PROJECTS: &str = "projects";
const SETTINGS_IMPORT_MODE_REPLACE_ALL: &str = "replaceAll";
const SETTINGS_IMPORT_MODE_PROJECTS: &str = "projects";
const ENV_FILE_CANDIDATES: [&str; 4] = [
    ".env",
    ".env.local",
    ".env.development.local",
    ".env.development",
];
const ROOT_ENV_FILE_FALLBACK: &str = ".env.local";
const SERVICE_ENV_FILE_FALLBACK: &str = ".env";
const MAX_ENV_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DesktopApi {
    config_path: PathBuf,
    binary_path: PathBuf,
    direct_mode: bool,
}

fn dns_resolution_status(daemon_running: bool) -> DnsResolutionDto {
    if !daemon_running {
        return DnsResolutionDto {
            checked: false,
            message: Some("DNS resolution is checked after the daemon starts.".into()),
            ..DnsResolutionDto::default()
        };
    }

    map_dns_resolution_check(check_system_resolution(Duration::from_millis(1500)))
}

fn map_dns_resolution_check(check: SystemResolutionCheck) -> DnsResolutionDto {
    DnsResolutionDto {
        checked: true,
        healthy: check.healthy,
        hostname: check.hostname,
        resolved_ips: check
            .resolved_ips
            .into_iter()
            .map(|address| address.to_string())
            .collect(),
        message: check.message,
    }
}

impl DesktopApi {
    pub fn new(config_path: PathBuf, binary_path: PathBuf) -> Self {
        Self {
            config_path,
            binary_path,
            direct_mode: false,
        }
    }

    pub fn new_direct(config_path: PathBuf, binary_path: PathBuf) -> Self {
        Self {
            config_path,
            binary_path,
            direct_mode: true,
        }
    }

    pub fn bootstrap(&self) -> Result<BootstrapDto> {
        if let Some(helper) = self.connected_helper_client()? {
            let mut bootstrap = helper.get_bootstrap()?;
            bootstrap.environment = self.environment();
            bootstrap.daemon.ca_installed = self.effective_desktop_ca_installed()?;
            bootstrap.launcher_preferences = self.effective_launcher_preferences()?.into();
            bootstrap.launcher_availability = self.launcher_availability();
            bootstrap.projects = self.enrich_projects_with_local_paths(bootstrap.projects)?;
            bootstrap.services = self.list_project_services()?;
            bootstrap.project_environments = self.list_project_environments()?;
            return Ok(bootstrap);
        }

        Ok(BootstrapDto {
            environment: self.environment(),
            config: self.read_config()?,
            daemon: self.daemon_status()?,
            licensing: self.licensing_status()?,
            launcher_preferences: self.effective_launcher_preferences()?.into(),
            launcher_availability: self.launcher_availability(),
            domains: self.list_domains()?,
            projects: self.list_projects()?,
            services: self.list_project_services()?,
            project_environments: self.list_project_environments()?,
        })
    }

    pub fn environment(&self) -> EnvironmentDto {
        EnvironmentDto {
            app_name: BRANDING.app_name.into(),
            desktop_name: BRANDING.runtime_desktop_name(),
            config_path: self.config_path.display().to_string(),
            binary_path: self.binary_path.display().to_string(),
            binary_exists: self.binary_path.exists(),
            fixed_domain_suffix: BRANDING.domain_suffix().into(),
            cli_helper_ready: self.direct_mode || self.cli_helper_ready(),
            cli_access_ready: self.cli_access_ready(),
            cli_command_path: self.cli_command_path().display().to_string(),
            node_trust: self.node_trust_status_dto(),
        }
    }

    pub fn setup_cli_access(&self) -> Result<CliAccessDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.setup_cli_access();
        }

        self.install_cli_access()
    }

    /// Recreates the privileged helper so the desktop app can recover from a
    /// stale launchd service or a helper that accepts connections but stops
    /// answering requests.
    pub fn restart_system_helper(&self) -> Result<()> {
        if !self.should_use_system_helper() {
            return Ok(());
        }

        let socket_path = self.helper_socket_path()?;
        let uid = current_uid()?;
        let commands = [
            vec![
                "helper".into(),
                "uninstall-service".into(),
                "--uid".into(),
                uid.to_string(),
            ],
            vec![
                "helper".into(),
                "ensure-running".into(),
                "--socket".into(),
                socket_path.display().to_string(),
                "--uid".into(),
                uid.to_string(),
            ],
        ];

        self.run_cli_commands_with_admin(&commands)?;
        Ok(())
    }

    pub fn setup_node_trust(&self) -> Result<NodeTrustDto> {
        let setup = node_trust::install(&self.ca_cert_path())?;
        Ok(map_node_trust_dto(setup.status, setup.already_ready, None))
    }

    pub fn agent_skill_status(&self) -> Result<AgentSkillStatusDto> {
        map_agent_skill_status(crate::agent_skill::agent_skill_status()?)
    }

    pub fn install_agent_skill(&self) -> Result<AgentSkillInstallResultDto> {
        map_agent_skill_install_result(crate::agent_skill::install_agent_skill()?)
    }

    pub fn read_config(&self) -> Result<AppConfigDto> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.get_config();
        }

        let config = self.config_store().load()?;
        Ok(map_config(self.config_path.as_path(), config))
    }

    pub fn update_config(&self, request: ConfigUpdateRequest) -> Result<AppConfigDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.update_config(request);
        }

        self.ensure_config_writable()?;
        let mut config = self.config_store().load()?;

        if let Some(daemon) = request.daemon {
            config.daemon = daemon.into();
        }
        if let Some(paths) = request.paths {
            config.paths = paths.into();
        }
        if let Some(docker) = request.docker {
            config.docker = docker.into();
        }
        if let Some(agent) = request.agent {
            config.agent = agent.into();
        }

        config.validate()?;
        self.config_store().save(&config)?;
        Ok(map_config(self.config_path.as_path(), config))
    }

    pub fn licensing_status(&self) -> Result<crate::desktop::dto::LicensingDto> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.get_licensing_status();
        }

        let config = self.config_store().load()?;
        let project_count = config.configured_projects().len();
        let domain_count = config.configured_domains().len();
        let current_machine = current_machine_fingerprint().ok();

        Ok(map_licensing_status(
            &config.licensing,
            project_count,
            domain_count,
            None,
            current_machine.as_ref(),
        ))
    }

    pub fn update_keygen_settings(
        &self,
        request: KeygenSettingsUpdateRequest,
    ) -> Result<crate::desktop::dto::LicensingDto> {
        if self.should_use_system_helper() {
            return self
                .ensure_helper_running()?
                .update_keygen_settings(request);
        }

        self.ensure_config_writable()?;
        let licensing = self
            .config_store()
            .update_keygen_config(crate::config::KeygenConfig {
                api_url: request.api_url.trim().to_string(),
                account: request.account.trim().to_string(),
                product: request
                    .product
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                public_key: request
                    .public_key
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
            })?;
        let config = self.config_store().load()?;
        let current_machine = current_machine_fingerprint().ok();

        Ok(map_licensing_status(
            &licensing,
            config.configured_projects().len(),
            config.configured_domains().len(),
            None,
            current_machine.as_ref(),
        ))
    }

    pub fn activate_license(
        &self,
        request: ActivateLicenseRequest,
    ) -> Result<crate::desktop::dto::LicensingDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.activate_license(request);
        }

        self.ensure_config_writable()?;
        let key = request.key.trim();
        if key.is_empty() {
            bail!("License key is required.");
        }

        let config_store = self.config_store();
        let current = config_store.load()?;
        let client = KeygenClient::new()?;
        let machine = current_machine_fingerprint()
            .context("Failed to identify this machine for Keygen activation")?;
        let validation =
            client.validate_and_activate_license_key(&current.licensing.keygen, key, &machine)?;

        if !validation.valid {
            bail!(validation.detail);
        }

        let licensing =
            config_store.store_licensing(Some(validation.license), validation.offline_license)?;
        let updated = config_store.load()?;

        Ok(map_licensing_status(
            &licensing,
            updated.configured_projects().len(),
            updated.configured_domains().len(),
            None,
            Some(&machine),
        ))
    }

    pub fn clear_license(&self) -> Result<crate::desktop::dto::LicensingDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.clear_license();
        }

        self.ensure_config_writable()?;
        let licensing = self.config_store().store_licensing(None, None)?;
        let config = self.config_store().load()?;
        let current_machine = current_machine_fingerprint().ok();

        Ok(map_licensing_status(
            &licensing,
            config.configured_projects().len(),
            config.configured_domains().len(),
            None,
            current_machine.as_ref(),
        ))
    }

    pub fn refresh_license(&self) -> Result<crate::desktop::dto::LicensingDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.refresh_license();
        }

        self.ensure_config_writable()?;
        let config_store = self.config_store();
        let current = config_store.load()?;
        let Some(existing_license) = current.licensing.license.clone() else {
            let project_count = current.configured_projects().len();
            let domain_count = current.configured_domains().len();
            let current_machine = current_machine_fingerprint().ok();

            return Ok(map_licensing_status(
                &current.licensing,
                project_count,
                domain_count,
                None,
                current_machine.as_ref(),
            ));
        };

        let client = KeygenClient::new()?;
        let machine = current_machine_fingerprint()
            .context("Failed to identify this machine for Keygen refresh")?;
        let validation = client.validate_and_activate_license_key(
            &current.licensing.keygen,
            &existing_license.key,
            &machine,
        )?;

        let mut stored = validation.license;
        if stored.machine_id.is_none()
            && existing_license.machine_fingerprint == stored.machine_fingerprint
        {
            stored.machine_id = existing_license.machine_id.clone();
        }
        if !validation.valid {
            stored.validation_code = Some(validation.code.clone());
            stored.validation_detail = Some(validation.detail.clone());
        }

        let offline_license = validation
            .offline_license
            .or(current.licensing.offline_license.clone());
        let licensing = config_store.store_licensing(Some(stored), offline_license)?;
        let updated = config_store.load()?;
        let last_error = (!validation.valid).then_some(validation.detail);

        Ok(map_licensing_status(
            &licensing,
            updated.configured_projects().len(),
            updated.configured_domains().len(),
            last_error,
            Some(&machine),
        ))
    }

    pub fn import_offline_license(
        &self,
        request: ImportOfflineLicenseRequest,
    ) -> Result<crate::desktop::dto::LicensingDto> {
        if self.should_use_system_helper() {
            return self
                .ensure_helper_running()?
                .import_offline_license(request);
        }

        self.ensure_config_writable()?;
        let certificate = request.certificate.trim();
        if certificate.is_empty() {
            bail!("Offline license certificate is required.");
        }

        let config_store = self.config_store();
        let current = config_store.load()?;
        let key = request
            .key
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                current
                    .licensing
                    .license
                    .as_ref()
                    .map(|license| license.key.clone())
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "A license key is required to import an offline Keygen license file."
                )
            })?;

        let (license, offline_license) =
            build_offline_imported_license(&current.licensing.keygen, certificate, &key)?;
        let licensing = config_store.store_licensing(Some(license), Some(offline_license))?;
        let updated = config_store.load()?;
        let current_machine = current_machine_fingerprint().ok();

        Ok(map_licensing_status(
            &licensing,
            updated.configured_projects().len(),
            updated.configured_domains().len(),
            None,
            current_machine.as_ref(),
        ))
    }

    pub fn daemon_status(&self) -> Result<DaemonStatusDto> {
        if let Some(helper) = self.connected_helper_client()? {
            let mut status = helper.get_daemon_status()?;
            status.ca_installed = self.effective_desktop_ca_installed()?;
            return Ok(status);
        }

        let config = self.config_store().load()?;
        let paths = config.paths.clone();

        let pid_file = PidFile::new(paths.pid_file.clone());
        let certs = CertificateService::new(&paths);
        let network = get_network_info();
        let status = QueryDaemonStatus::new(&pid_file, &certs, &network).execute()?;

        Ok(DaemonStatusDto {
            running: status.pid.is_some(),
            pid: status.pid,
            lan_ip: status.lan_ip.to_string(),
            ca_installed: status.ca_installed,
            http_port: config.daemon.http_port,
            https_port: config.daemon.https_port,
            dns_port: config.daemon.dns_port,
            dns_resolution: dns_resolution_status(status.pid.is_some()),
        })
    }

    pub fn list_domains(&self) -> Result<Vec<DomainInfoDto>> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.list_domains();
        }

        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let cert_trusted = certs.is_trusted().ok();
        let configured_domains = self.config_store().list_configured_domains()?;
        let daemon = MgmtSocketClient::new(&paths.socket_path);

        let (active_config_domains, external_domains) = match daemon.list_registration_info() {
            Ok(registrations) => {
                let mut active_patterns = HashMap::new();
                let mut external = Vec::new();

                for info in registrations {
                    if info.registration.source().to_string() == "config" {
                        active_patterns
                            .insert(info.registration.display_pattern(), info.last_accessed_at);
                    } else {
                        external.push(info);
                    }
                }

                (active_patterns, external)
            }
            Err(_) => (HashMap::new(), Vec::new()),
        };

        let mut domains = configured_domains
            .into_iter()
            .map(|domain| {
                let has_cert = certs.exists(domain.registration.pattern());
                let last_accessed_at = active_config_domains
                    .get(&domain.registration.display_pattern())
                    .copied()
                    .flatten();
                let active =
                    active_config_domains.contains_key(&domain.registration.display_pattern());
                let target_reachable = registration_has_reachable_target(&domain.registration);

                map_configured_domain_info(
                    domain,
                    has_cert,
                    cert_trusted,
                    active,
                    target_reachable,
                    last_accessed_at,
                )
            })
            .collect::<Vec<_>>();

        domains.extend(external_domains.into_iter().map(|info| {
            let has_cert = certs.exists(info.registration.pattern());
            let target_reachable = registration_has_reachable_target(&info.registration);
            map_domain_info(
                info.registration,
                DomainInfoDetails {
                    has_cert,
                    cert_trusted,
                    project_id: None,
                    project_name: None,
                    project_enabled: true,
                    active: true,
                    target_reachable,
                    last_accessed_at: info.last_accessed_at,
                },
            )
        }));

        domains.sort_by(|left, right| left.registration.pattern.cmp(&right.registration.pattern));
        Ok(domains)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectInfoDto>> {
        if let Some(helper) = self.connected_helper_client()? {
            return self.enrich_projects_with_local_paths(helper.list_projects()?);
        }

        let (_, paths) = self.load_config_and_paths()?;
        let domain_accesses = daemon_domain_accesses(&paths.socket_path);
        let configured_domains = self.config_store().list_configured_domains()?;
        let project_statuses = configured_domains.into_iter().fold(
            HashMap::<String, (usize, bool, Option<u64>)>::new(),
            |mut statuses, domain| {
                if let Some(project_id) = domain.project_id {
                    let access = domain_accesses
                        .get(&domain.registration.display_pattern())
                        .copied();
                    let status = statuses.entry(project_id).or_insert((0, false, None));
                    status.0 += 1;
                    status.1 |= registration_has_reachable_target(&domain.registration);
                    status.2 = latest_accessed_at(status.2, access);
                }
                statuses
            },
        );

        let mut projects = self
            .config_store()
            .list_projects()?
            .into_iter()
            .map(|project| {
                let (domain_count, has_reachable_domain, last_accessed_at) = project_statuses
                    .get(&project.id)
                    .copied()
                    .unwrap_or((0, false, None));
                map_project_info(
                    project,
                    domain_count,
                    has_reachable_domain,
                    last_accessed_at,
                )
            })
            .collect::<Vec<_>>();
        projects = self.enrich_projects_with_local_paths(projects)?;
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(projects)
    }

    pub fn list_project_services(&self) -> Result<Vec<ProjectServiceInfoDto>> {
        let config_store = self.config_store();
        let config = config_store.load()?;
        config_store
            .list_services()?
            .into_iter()
            .map(|service| {
                map_project_service_info(
                    service,
                    &config_store,
                    config.daemon.http_port,
                    config.daemon.https_port,
                )
            })
            .collect()
    }

    pub fn list_project_environments(&self) -> Result<Vec<ProjectEnvironmentInfoDto>> {
        let environments = self.config_store().list_project_environments(None)?;
        let environments = self.enrich_project_environments_with_local_paths(environments)?;

        Ok(environments
            .into_iter()
            .map(map_project_environment_info)
            .collect())
    }

    fn configured_project_with_local_root(
        &self,
        project_id: &str,
        root_path: Option<&str>,
    ) -> Result<ConfiguredProject> {
        let mut project = self
            .config_store()
            .list_projects()?
            .into_iter()
            .find(|project| {
                project.id == project_id || project.name.eq_ignore_ascii_case(project_id)
            })
            .ok_or_else(|| anyhow!("Project not found: {project_id}"))?;
        if let Some(root_path) = root_path
            .map(str::trim)
            .filter(|root_path| !root_path.is_empty())
        {
            project.root_path = Some(root_path.to_string());
            return Ok(project);
        }
        let should_use_local_root = project
            .root_path
            .as_deref()
            .map(configured_project_root_is_unusable_for_worktrees)
            .unwrap_or(true);
        if should_use_local_root
            && let Some(root_path) = self.local_project_root_paths()?.get(&project.id)
        {
            project.root_path = Some(root_path.clone());
        }
        Ok(project)
    }

    pub fn discover_project_worktrees(
        &self,
        request: DiscoverProjectWorktreesRequest,
    ) -> Result<ProjectWorktreeDiscoveryDto> {
        let project_id = request.project_id.trim();
        if project_id.is_empty() {
            bail!("Project id is required to discover worktrees.");
        }

        let project = self.configured_project_with_local_root(project_id, None)?;
        let primary_root = project_root_path(&project)?;
        let primary_root = fs::canonicalize(&primary_root).unwrap_or(primary_root);
        let primary_common_dir = git_common_dir(&primary_root).ok_or_else(|| {
            anyhow!(
                "Project '{}' is not inside a Git repository, so Localify cannot discover worktrees.",
                project.id
            )
        })?;
        let configured_environments = self
            .config_store()
            .list_project_environments(Some(&project.id))?;
        let configured_environments =
            self.enrich_project_environments_with_local_paths(configured_environments)?;
        let mut used_slugs = configured_environments
            .iter()
            .map(|environment| environment.slug.clone())
            .collect::<HashSet<_>>();
        let services = self
            .config_store()
            .list_services()?
            .into_iter()
            .filter(|service| service.project_id == project.id)
            .collect::<Vec<_>>();
        let worktrees = git_worktrees(&primary_root)?
            .into_iter()
            .map(|worktree| {
                map_project_worktree(
                    &project,
                    &primary_root,
                    &primary_common_dir,
                    &configured_environments,
                    &services,
                    &mut used_slugs,
                    worktree,
                )
            })
            .collect::<Vec<_>>();

        Ok(ProjectWorktreeDiscoveryDto {
            project_id: project.id,
            project_name: project.name,
            primary_root_path: primary_root.display().to_string(),
            worktrees,
        })
    }

    pub fn attach_project_worktree(
        &self,
        request: AttachProjectWorktreeRequest,
    ) -> Result<AttachProjectWorktreeResultDto> {
        if self.should_use_system_helper_for_worktree_attach() {
            let helper = self.ensure_helper_running()?;
            match helper.attach_project_worktree(request.clone()) {
                Ok(result) => return Ok(result),
                Err(error) if should_restart_helper_after_attach_error(&error, &request) => {
                    self.restart_system_helper().with_context(|| {
                        "The system helper is running an older build for worktree attach, and Localify could not restart it automatically."
                    })?;
                    return self
                        .ensure_helper_running()?
                        .attach_project_worktree(request);
                }
                Err(error) => return Err(error),
            }
        }

        self.attach_project_worktree_direct(request)
    }

    pub(crate) fn attach_project_worktree_direct(
        &self,
        request: AttachProjectWorktreeRequest,
    ) -> Result<AttachProjectWorktreeResultDto> {
        self.ensure_config_writable()?;
        let project_id = request.project_id.trim();
        if project_id.is_empty() {
            bail!("Project id is required to attach a worktree.");
        }
        let root_path = PathBuf::from(request.root_path.trim());
        if root_path.as_os_str().is_empty() {
            bail!("Worktree root path is required.");
        }
        let root_path = fs::canonicalize(&root_path).with_context(|| {
            format!("Failed to resolve worktree root '{}'.", root_path.display())
        })?;

        let project = self
            .configured_project_with_local_root(project_id, request.primary_root_path.as_deref())?;
        let (primary_root, primary_common_dir) =
            resolve_attach_primary_git_root(&project, &root_path)?;
        if root_path == primary_root {
            bail!("The primary project checkout is already the primary environment.");
        }

        let git = detect_worktree_git_metadata(&root_path);
        if git.common_dir.as_ref() != Some(&primary_common_dir) {
            bail!(
                "Worktree '{}' does not belong to the same Git repository as project '{}'.",
                root_path.display(),
                project.id
            );
        }

        let existing = existing_environment_for_root(
            &self.enrich_project_environments_with_local_paths(
                self.config_store()
                    .list_project_environments(Some(&project.id))?,
            )?,
            &root_path,
        );
        if let Some(environment) = existing {
            return Ok(AttachProjectWorktreeResultDto {
                environment: map_project_environment_info(environment.clone()),
                created: false,
                created_domains: Vec::new(),
                daemon_reloaded: false,
            });
        }

        let services = self
            .config_store()
            .list_services()?
            .into_iter()
            .filter(|service| service.project_id == project.id)
            .collect::<Vec<_>>();
        if services.is_empty() {
            bail!(
                "Project '{}' has no services. Add a service before attaching worktree environments.",
                project.id
            );
        }

        let configured_environments = self
            .config_store()
            .list_project_environments(Some(&project.id))?;
        let mut used_slugs = configured_environments
            .iter()
            .map(|environment| environment.slug.clone())
            .collect::<HashSet<_>>();
        let name = worktree_environment_name(git.branch.as_deref(), &root_path);
        let slug = unique_environment_slug(&name, &root_path, &mut used_slugs)?;
        let domain_mappings = environment_service_domains(&project.id, &services, &slug, false);
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let created_domains = ensure_environment_domains(
            &project.id,
            &domain_mappings,
            &self.config_store(),
            &certs,
        )?;

        let environment = ProjectEnvironmentConfig {
            name,
            slug: slug.clone(),
            primary: false,
            root_path: Some(root_path.display().to_string()),
            git_common_dir: Some(primary_common_dir.display().to_string()),
            git_branch: git.branch,
            services: domain_mappings,
        };

        let persisted = match self
            .config_store()
            .upsert_project_environment(&project.id, environment)
        {
            Ok(environment) => environment,
            Err(error) => {
                rollback_environment_domains(&created_domains, &self.config_store(), &certs);
                return Err(error.into());
            }
        };

        let daemon_reloaded = match MgmtSocketClient::new(&paths.socket_path).reload() {
            Ok(()) => true,
            Err(crate::application::ports::DaemonConnectionError::NotRunning) => false,
            Err(_) => false,
        };

        Ok(AttachProjectWorktreeResultDto {
            environment: map_project_environment_info(persisted),
            created: true,
            created_domains: created_domains
                .iter()
                .map(|domain| domain.to_string())
                .collect(),
            daemon_reloaded,
        })
    }

    pub fn create_project(&self, request: CreateProjectRequest) -> Result<ProjectInfoDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.create_project(request);
        }

        self.ensure_config_writable()?;
        let project = self.config_store().create_project(&request.name)?;
        Ok(map_project_info(project, 0, false, None))
    }

    pub fn create_project_with_domains(
        &self,
        request: CreateProjectWithDomainsRequest,
    ) -> Result<ProjectInfoDto> {
        if self.should_use_system_helper() {
            return self
                .ensure_helper_running()?
                .create_project_with_domains(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let config_store = self.config_store();
        let register_domain = RegisterDomain::new(&config_store, &certs);
        let unregister_domain = UnregisterDomain::new(&config_store, &certs);

        let project = config_store.create_project(&request.name)?;
        let mut created_patterns = Vec::new();

        for domain in &request.domains {
            let pattern = match DomainPattern::from_name(&domain.domain, domain.wildcard) {
                Ok(pattern) => pattern,
                Err(error) => {
                    rollback_created_project(
                        &config_store,
                        &unregister_domain,
                        &project.id,
                        &created_patterns,
                    )?;
                    return Err(error.into());
                }
            };

            let routes = match domain
                .routes
                .iter()
                .map(|route| parse_desktop_route(route))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(routes) => routes,
                Err(error) => {
                    rollback_created_project(
                        &config_store,
                        &unregister_domain,
                        &project.id,
                        &created_patterns,
                    )?;
                    return Err(anyhow::anyhow!(
                        "Invalid route for '{}': {error}",
                        domain.domain
                    ));
                }
            };

            let result = match register_domain.execute(pattern.clone(), routes) {
                Ok(result) => result,
                Err(error) => {
                    rollback_created_project(
                        &config_store,
                        &unregister_domain,
                        &project.id,
                        &created_patterns,
                    )?;
                    return Err(error);
                }
            };

            if let Err(error) = config_store
                .update_domain_with_project(result.registration, Some(project.id.clone()))
            {
                rollback_created_project(
                    &config_store,
                    &unregister_domain,
                    &project.id,
                    &created_patterns,
                )?;
                return Err(error.into());
            }

            created_patterns.push(pattern);
        }

        let (_, has_reachable_domain) = project_domain_summary(&config_store, &project.id)?;
        Ok(map_project_info(
            project,
            request.domains.len(),
            has_reachable_domain,
            None,
        ))
    }

    pub fn init_project(&self, request: InitProjectRequest) -> Result<InitProjectResultDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.init_project(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let config_store = self.config_store();
        let register_domain = RegisterDomain::new(&config_store, &certs);
        let unregister_domain = UnregisterDomain::new(&config_store, &certs);
        let InitProjectRequest {
            name,
            root_path,
            services,
        } = request;
        let project = if let Some(root_path) = root_path.clone() {
            config_store.create_project_with_root(&name, Some(root_path))?
        } else {
            config_store.create_project(&name)?
        };
        let mut created_patterns = Vec::new();

        for service in &services {
            let pattern = DomainPattern::from_name(&service.domain, false)?;
            let route = parse_desktop_route(&service.target).map_err(|error| {
                anyhow::anyhow!("Invalid target for '{}': {error}", service.domain)
            })?;
            let result = register_domain.execute(pattern.clone(), vec![route])?;
            config_store
                .update_domain_with_project(result.registration, Some(project.id.clone()))?;
            created_patterns.push(pattern);
        }

        let project_services = services
            .iter()
            .filter_map(init_project_service_config)
            .collect::<Vec<_>>();
        if !project_services.is_empty()
            && let Err(error) = config_store.set_project_services(&project.id, project_services)
        {
            rollback_created_project(
                &config_store,
                &unregister_domain,
                &project.id,
                &created_patterns,
            )?;
            return Err(error.into());
        }

        let daemon_reloaded = match MgmtSocketClient::new(&paths.socket_path).reload() {
            Ok(()) => true,
            Err(crate::application::ports::DaemonConnectionError::NotRunning) => false,
            Err(error) => {
                rollback_created_project(
                    &config_store,
                    &unregister_domain,
                    &project.id,
                    &created_patterns,
                )?;
                return Err(anyhow::anyhow!(
                    "Failed to reload the running daemon: {error}"
                ));
            }
        };

        let has_reachable_domain = project_domain_summary(&config_store, &project.id)?.1;

        Ok(InitProjectResultDto {
            project: map_project_info(project, services.len(), has_reachable_domain, None),
            services,
            daemon_reloaded,
        })
    }

    /// Registers a new domain for an existing project and records the service
    /// metadata (path, command) used by `localify dev`/`run`.
    ///
    /// Returns an error if the project does not exist, the domain is already
    /// registered, or the service metadata cannot be persisted (the domain
    /// registration is rolled back in that case).
    pub fn add_project_service(
        &self,
        request: AddProjectServiceRequest,
    ) -> Result<AddProjectServiceResultDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.add_project_service(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let config_store = self.config_store();
        let project = configured_project(&config_store, &request.project_id)?;
        let pattern = DomainPattern::from_name(&request.domain, false)?;
        let route = parse_desktop_route(&request.target)
            .map_err(|error| anyhow!("Invalid target for '{}': {error}", request.domain))?;
        let result = RegisterDomain::new(&config_store, &certs).execute(pattern, vec![route])?;
        config_store
            .update_domain_with_project(result.registration.clone(), Some(project.id.clone()))?;

        let service = crate::desktop::dto::InitProjectServiceRequest {
            domain: request.domain.clone(),
            target: request.target.clone(),
            service_name: request.service_name.clone(),
            service_path: request.service_path.clone(),
            command: request.command.clone(),
        };
        if let Some(project_service) = init_project_service_config(&service)
            && let Err(error) = config_store.add_project_service(&project.id, project_service)
        {
            let _ =
                UnregisterDomain::new(&config_store, &certs).execute(result.registration.pattern());
            return Err(error.into());
        }

        Ok(AddProjectServiceResultDto {
            domain: result.registration.display_pattern(),
            target: request.target,
            daemon_reloaded: reload_running_daemon(&paths)?,
        })
    }

    /// Replaces the primary domain of a project service, or of a service inside
    /// a specific project environment when `environment_slug` is set.
    ///
    /// The existing routes are kept; a certificate is generated for the new
    /// domain (falling back to HTTP with a warning when generation fails).
    pub fn set_project_service_domain(
        &self,
        request: SetProjectServiceDomainRequest,
    ) -> Result<SetProjectServiceDomainResultDto> {
        if self.should_use_system_helper() {
            return self
                .ensure_helper_running()?
                .set_project_service_domain(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let config_store = self.config_store();
        let project = configured_project(&config_store, &request.project_id)?;

        let (service_id, current_domain) = match request.environment_slug.as_deref() {
            Some(slug) => {
                environment_service_mapping(&config_store, &project, slug, &request.service_id)?
            }
            None => {
                let service = project_service(&config_store, &project, &request.service_id)?;
                (service.id, service.primary_domain)
            }
        };

        let current_pattern = domain_pattern_from_display(&current_domain)?;
        let current_registration = config_store
            .get_domain(&current_pattern)?
            .ok_or_else(|| anyhow!("Domain '{}' is not registered.", current_domain))?;
        let new_pattern = domain_pattern_from_display(&request.domain)?;
        let cert_installed = match certs.create_and_install(&new_pattern) {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "  Certificate warning: failed to generate certificate for '{new_pattern}': {error}"
                );
                false
            }
        };
        let mut new_registration = DomainRegistration::with_source(
            new_pattern,
            current_registration.routes().to_vec(),
            current_registration.source(),
        );
        if cert_installed {
            new_registration.enable_https();
        }
        let new_domain = new_registration.display_pattern();

        match request.environment_slug {
            Some(slug) => config_store.replace_project_environment_service_domain(
                &project.id,
                crate::infrastructure::config::ProjectEnvironmentServiceDomainUpdate {
                    environment_slug: slug,
                    service_id: service_id.clone(),
                    current_domain: current_domain.clone(),
                    new_registration,
                },
            )?,
            None => config_store.replace_project_service_domain(
                &project.id,
                &service_id,
                crate::infrastructure::config::ProjectServiceDomainUpdate {
                    current_domain: current_domain.clone(),
                    new_registration,
                },
            )?,
        }

        Ok(SetProjectServiceDomainResultDto {
            service_id,
            previous_domain: current_domain,
            domain: new_domain,
            daemon_reloaded: reload_running_daemon(&paths)?,
        })
    }

    /// Converts inferred (domain-derived) services of a project into explicit
    /// service entries by re-detecting apps under the project root.
    ///
    /// Only rewrites the config when at least one service gained a detected
    /// path or command; returns how many services were materialized.
    pub fn materialize_project_services(
        &self,
        request: MaterializeProjectServicesRequest,
    ) -> Result<MaterializeProjectServicesResultDto> {
        if self.should_use_system_helper() {
            return self
                .ensure_helper_running()?
                .materialize_project_services(request);
        }

        self.ensure_config_writable()?;
        let config_store = self.config_store();
        let project = configured_project(&config_store, &request.project_id)?;
        let root = PathBuf::from(&request.root_path);
        let services = config_store
            .list_services()?
            .into_iter()
            .filter(|service| service.project_id == project.id)
            .collect::<Vec<_>>();
        if services.iter().all(|service| !service.inferred) {
            return Ok(MaterializeProjectServicesResultDto { materialized: 0 });
        }

        let detected = detect_folder_init_plan(&root, Some(&project.name))?;
        let (project_services, materialized) =
            materialized_project_services(&services, &root, &detected.services)?;
        if materialized > 0 {
            config_store.set_project_services(&project.id, project_services)?;
        }

        Ok(MaterializeProjectServicesResultDto { materialized })
    }

    /// Creates or updates a project environment for the given root, registering
    /// per-environment domains (with reserved local ports) for non-primary
    /// environments.
    ///
    /// Created domains are rolled back when persisting the environment fails.
    pub fn ensure_project_environment(
        &self,
        request: EnsureProjectEnvironmentRequest,
    ) -> Result<EnsureProjectEnvironmentResultDto> {
        if self.should_use_system_helper() {
            return self
                .ensure_helper_running()?
                .ensure_project_environment(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let config_store = self.config_store();
        let project = configured_project(&config_store, &request.project_id)?;
        let environment_root = PathBuf::from(request.root_path.trim());
        if environment_root.as_os_str().is_empty() {
            bail!("Environment root path is required.");
        }

        let services = config_store
            .list_services()?
            .into_iter()
            .filter(|service| service.project_id == project.id)
            .collect::<Vec<_>>();
        if services.is_empty() {
            bail!(
                "Project '{}' has no services. Add one with '{} project add-service --path <app>'.",
                project.id,
                BRANDING.cli_command()
            );
        }

        let git = detect_worktree_git_metadata(&environment_root);
        let name = environment_display_name(
            request.name,
            request.primary,
            git.branch.as_deref(),
            &environment_root,
        )?;
        let slug = if request.primary {
            "primary".to_string()
        } else {
            slugify(&name)
                .ok_or_else(|| anyhow!("Environment name must contain letters or numbers"))?
        };

        let domain_mappings =
            environment_service_domains(&project.id, &services, &slug, request.primary);
        let created_domains = if request.primary {
            Vec::new()
        } else {
            ensure_environment_domains(&project.id, &domain_mappings, &config_store, &certs)?
        };

        let environment = ProjectEnvironmentConfig {
            name,
            slug,
            primary: request.primary,
            root_path: Some(environment_root.display().to_string()),
            git_common_dir: git.common_dir.map(|path| path.display().to_string()),
            git_branch: git.branch,
            services: domain_mappings,
        };

        let persisted = match config_store.upsert_project_environment(&project.id, environment) {
            Ok(environment) => environment,
            Err(error) => {
                rollback_environment_domains(&created_domains, &config_store, &certs);
                return Err(error.into());
            }
        };

        Ok(EnsureProjectEnvironmentResultDto {
            environment: map_project_environment_info(persisted),
            created_domains: created_domains
                .iter()
                .map(|pattern| pattern.display_pattern())
                .collect(),
            daemon_reloaded: reload_running_daemon(&paths)?,
        })
    }

    pub fn detect_project_init_plan(
        &self,
        request: DetectProjectInitPlanRequest,
    ) -> Result<DetectProjectInitPlanDto> {
        let root_path = PathBuf::from(&request.root_path);
        let plan = detect_folder_init_plan(&root_path, None)?;

        Ok(DetectProjectInitPlanDto {
            root_path: request.root_path,
            name: plan.name,
            services: plan
                .services
                .into_iter()
                .map(|service| FolderInitServiceDto {
                    id: service.id,
                    label: service.label,
                    display_path: service.display_path,
                    service_name: service.service_name,
                    service_path: service.service_path,
                    command: service.command,
                    domain: service.domain,
                    target: service.target,
                    enabled: service.enabled,
                    can_update_dev_script: service.can_update_dev_script,
                    update_dev_script: service.update_dev_script,
                })
                .collect(),
        })
    }

    pub fn init_project_from_folder(
        &self,
        request: InitProjectFromFolderRequest,
    ) -> Result<InitProjectFromFolderResultDto> {
        let root_path = PathBuf::from(&request.root_path);
        let selections = request
            .services
            .iter()
            .map(|service| FolderInitSelection {
                id: service.id.clone(),
                domain: service.domain.clone(),
                target: service.target.clone(),
                enabled: service.enabled,
                update_dev_script: service.update_dev_script,
                service_name: service.service_name.clone(),
                service_path: service.service_path.clone(),
                command: service.command.clone(),
            })
            .collect::<Vec<_>>();
        let config = self.config_store().load()?;
        let writes_package_scripts = request
            .services
            .iter()
            .any(|service| service.update_dev_script)
            || config.agent.promote_dev_script;
        let resolved = resolve_folder_init_selection(
            &root_path,
            &request.name,
            &selections,
            LocalAppUpdatePolicy::from_dev_script_preference(config.agent.promote_dev_script)
                .with_package_scripts(writes_package_scripts),
        )?;
        let init_result = self.init_project(InitProjectRequest {
            name: request.name,
            root_path: Some(root_path.display().to_string()),
            services: resolved
                .services
                .iter()
                .map(|service| crate::desktop::dto::InitProjectServiceRequest {
                    domain: service.domain.clone(),
                    target: service.target.clone(),
                    service_name: service.service_name.clone(),
                    service_path: service.service_path.clone(),
                    command: service.command.clone(),
                })
                .collect(),
        })?;
        self.store_project_root_path(&init_result.project.id, &root_path)?;
        let mut project = init_result.project;
        project.root_path = Some(root_path.display().to_string());
        let update_results = apply_local_app_updates(
            &resolved.package_updates,
            &resolved.next_config_updates,
            &resolved.vite_config_updates,
        );

        Ok(InitProjectFromFolderResultDto {
            project,
            services: init_result.services,
            daemon_reloaded: init_result.daemon_reloaded,
            updated_files: update_results.updated,
            skipped_file_updates: update_results.skipped,
        })
    }

    pub fn update_project(&self, request: UpdateProjectRequest) -> Result<ProjectInfoDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.update_project(request);
        }

        let UpdateProjectRequest {
            id,
            name,
            enabled,
            root_path: _root_path,
        } = request;

        self.ensure_config_writable()?;
        let configured_domains = self.config_store().list_configured_domains()?;
        let (domain_count, has_reachable_domain) = configured_domains
            .iter()
            .filter(|domain| domain.project_id.as_deref() == Some(id.as_str()))
            .fold((0, false), |(count, reachable), domain| {
                (
                    count + 1,
                    reachable || registration_has_reachable_target(&domain.registration),
                )
            });
        let project = self
            .config_store()
            .update_project(&id, name, enabled, None)?;
        let project = map_project_info(project, domain_count, has_reachable_domain, None);
        let mut projects = self.enrich_projects_with_local_paths(vec![project])?;
        projects.pop().ok_or_else(|| {
            anyhow!(
                "Failed to enrich updated project '{}': no project was returned",
                id
            )
        })
    }

    pub fn open_in_editor(&self, request: OpenInEditorRequest) -> Result<()> {
        crate::desktop::editor::open_in_editor(Path::new(&request.path))
    }

    /// Resolves the env file target and opens it without reading or creating it.
    pub fn open_env_in_editor(&self, request: OpenEnvInEditorRequest) -> Result<()> {
        let bootstrap = self.bootstrap()?;
        let target = resolve_environment_file_target(
            &bootstrap,
            &request.project_id,
            &request.environment_slug,
            request.service_id.as_deref(),
            request.relative_path.as_deref(),
        )?;
        ensure_environment_file_target(&target)?;
        crate::desktop::editor::open_file_in_editor(&target.path)
    }

    pub fn list_environment_files(
        &self,
        request: EnvironmentFileRequest,
    ) -> Result<EnvironmentFileListDto> {
        let bootstrap = self.bootstrap()?;
        let files = environment_file_candidates(
            &bootstrap,
            &request.project_id,
            &request.environment_slug,
            request.service_id.as_deref(),
        )?
        .into_iter()
        .map(EnvironmentFileCandidate::into_dto)
        .collect();

        Ok(EnvironmentFileListDto { files })
    }

    pub fn read_environment_file(
        &self,
        request: EnvironmentFileRequest,
    ) -> Result<EnvironmentFileDto> {
        let bootstrap = self.bootstrap()?;
        let target = resolve_environment_file_target(
            &bootstrap,
            &request.project_id,
            &request.environment_slug,
            request.service_id.as_deref(),
            request.relative_path.as_deref(),
        )?;
        read_environment_file_target(&target)
    }

    pub fn save_environment_file(
        &self,
        request: SaveEnvironmentFileRequest,
    ) -> Result<EnvironmentFileDto> {
        let bootstrap = self.bootstrap()?;
        let target = resolve_environment_file_target(
            &bootstrap,
            &request.project_id,
            &request.environment_slug,
            request.service_id.as_deref(),
            request.relative_path.as_deref(),
        )?;
        ensure_environment_file_target(&target)?;

        let current_modified_at = environment_file_modified_at(&target.path)?;
        if current_modified_at != request.expected_modified_at {
            bail!("Environment file changed on disk after it was opened. Reopen it before saving.");
        }

        if current_modified_at.is_some() {
            fs::write(&target.path, request.content).with_context(|| {
                format!(
                    "Failed to save environment file '{}'",
                    target.relative_path.display()
                )
            })?;
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target.path)
                .with_context(|| {
                    format!(
                        "Failed to create environment file '{}'",
                        target.relative_path.display()
                    )
                })?;
            file.write_all(request.content.as_bytes())
                .with_context(|| {
                    format!(
                        "Failed to save environment file '{}'",
                        target.relative_path.display()
                    )
                })?;
        }

        read_environment_file_target(&target)
    }

    pub fn preview_environment_file_copy(
        &self,
        request: EnvironmentFileCopyRequest,
    ) -> Result<EnvironmentFileCopyPreviewDto> {
        let bootstrap = self.bootstrap()?;
        environment_file_copy_preview(&bootstrap, &request)
    }

    pub fn copy_environment_files(
        &self,
        request: EnvironmentFileCopyRequest,
    ) -> Result<EnvironmentFileCopyPreviewDto> {
        let bootstrap = self.bootstrap()?;
        let preview = environment_file_copy_preview(&bootstrap, &request)?;
        let mut items = Vec::with_capacity(preview.items.len());

        for item in preview.items {
            let should_copy = matches!(
                item.status,
                EnvironmentFileCopyStatusDto::Create | EnvironmentFileCopyStatusDto::Overwrite
            );
            if !should_copy {
                items.push(EnvironmentFileCopyItemDto {
                    status: match item.status {
                        EnvironmentFileCopyStatusDto::SkipExists => {
                            EnvironmentFileCopyStatusDto::Skipped
                        }
                        other => other,
                    },
                    ..item
                });
                continue;
            }

            let source = environment_file_target_for_relative_path(
                &bootstrap,
                &request.project_id,
                &request.source_environment_slug,
                &item.relative_path,
            )?;
            let target = environment_file_target_for_relative_path(
                &bootstrap,
                &request.project_id,
                &request.target_environment_slug,
                &item.relative_path,
            )?;
            let content = fs::read_to_string(&source.path).with_context(|| {
                format!(
                    "Failed to read environment file '{}'. Make sure it is valid UTF-8 text.",
                    item.relative_path
                )
            })?;

            if target.path.exists() && !request.overwrite {
                items.push(EnvironmentFileCopyItemDto {
                    status: EnvironmentFileCopyStatusDto::Skipped,
                    target_exists: true,
                    message: Some("Target file already exists.".into()),
                    ..item
                });
                continue;
            }

            if let Some(parent) = target.path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create folder for environment file '{}'",
                        item.relative_path
                    )
                })?;
            }
            if target.path.exists() {
                fs::write(&target.path, content).with_context(|| {
                    format!(
                        "Failed to overwrite environment file '{}'",
                        item.relative_path
                    )
                })?;
            } else {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target.path)
                    .with_context(|| {
                        format!("Failed to create environment file '{}'", item.relative_path)
                    })?;
                file.write_all(content.as_bytes()).with_context(|| {
                    format!("Failed to copy environment file '{}'", item.relative_path)
                })?;
            }

            items.push(EnvironmentFileCopyItemDto {
                status: EnvironmentFileCopyStatusDto::Copied,
                target_exists: true,
                message: None,
                ..item
            });
        }

        Ok(environment_file_copy_summary(items))
    }

    pub fn open_project_url(&self, request: OpenProjectUrlRequest) -> Result<()> {
        crate::desktop::browser::open_project_url(&request.url, request.browser)
    }

    pub fn open_project_url_with_preferred_browser(&self, url: &str) -> Result<()> {
        let preferences = self.effective_launcher_preferences()?;
        crate::desktop::browser::open_project_url(url, preferences.browser)
    }

    pub fn update_launcher_preferences(
        &self,
        request: UpdateLauncherPreferencesRequest,
    ) -> Result<crate::desktop::dto::LauncherPreferencesDto> {
        let preferences = self.normalize_launcher_preferences(request.into());
        self.launcher_preferences_store()?.save(&preferences)?;
        Ok(preferences.into())
    }

    pub fn export_settings(
        &self,
        request: SettingsExportRequest,
    ) -> Result<SettingsExportResultDto> {
        validate_settings_export_request(&request)?;
        let snapshot = self.build_settings_snapshot(&request)?;
        let path = PathBuf::from(request.file_path.trim());
        write_settings_snapshot(&path, &snapshot)?;

        Ok(SettingsExportResultDto {
            file_path: path.display().to_string(),
            mode: request.mode,
            includes_settings: snapshot.settings.is_some(),
            standalone_domain_count: snapshot.standalone_domains.len(),
            project_count: snapshot.projects.len(),
        })
    }

    pub fn preview_settings_import(
        &self,
        request: SettingsImportPreviewRequest,
    ) -> Result<SettingsImportPreviewDto> {
        let path = import_file_path(&request.file_path)?;
        let snapshot = read_settings_snapshot(&path)?;
        validate_settings_snapshot(&snapshot)?;

        Ok(SettingsImportPreviewDto {
            file_path: path.display().to_string(),
            version: snapshot.version,
            scope: snapshot.scope,
            includes_settings: snapshot.settings.is_some(),
            standalone_domain_count: snapshot.standalone_domains.len(),
            project_count: snapshot.projects.len(),
            projects: snapshot
                .projects
                .iter()
                .map(|project| SettingsImportPreviewProjectDto {
                    id: project.id.clone(),
                    name: project.name.clone(),
                    enabled: project.enabled,
                    domain_count: project.domains.len(),
                    has_root_path: project.root_path.is_some(),
                })
                .collect(),
        })
    }

    pub fn import_settings(
        &self,
        request: SettingsImportRequest,
    ) -> Result<SettingsImportResultDto> {
        let path = import_file_path(&request.file_path)?;
        let snapshot = read_settings_snapshot(&path)?;
        validate_settings_snapshot(&snapshot)?;
        validate_settings_import_request(&request, &snapshot)?;
        let licensing = self.licensing_status()?;

        match request.mode.as_str() {
            SETTINGS_IMPORT_MODE_REPLACE_ALL => {
                validate_settings_snapshot_targets(&snapshot)?;
                ensure_snapshot_within_license_limits(&licensing, &snapshot)?;
                self.import_settings_replace_all(snapshot)
            }
            SETTINGS_IMPORT_MODE_PROJECTS => {
                let selected_projects = select_snapshot_projects(&snapshot, &request.project_ids)?;
                let selected_snapshot = SettingsTransferSnapshotDto {
                    version: snapshot.version,
                    scope: snapshot.scope.clone(),
                    settings: snapshot.settings.clone(),
                    standalone_domains: Vec::new(),
                    projects: selected_projects.clone(),
                };
                validate_settings_snapshot_targets(&selected_snapshot)?;
                ensure_project_import_within_license_limits(self, &licensing, &selected_projects)?;
                self.import_settings_projects(selected_projects)
            }
            other => bail!("Unsupported import mode '{other}'."),
        }
    }

    pub fn run_local_dev_server(&self, _request: RunLocalDevServerRequest) -> Result<()> {
        bail!("Service launch now uses the integrated Localify terminal.")
    }

    pub fn stop_local_dev_server(&self, request: StopLocalDevServerRequest) -> Result<()> {
        crate::infrastructure::local_dev::stop_local_dev_server(&request.target)
    }

    fn build_settings_snapshot(
        &self,
        request: &SettingsExportRequest,
    ) -> Result<SettingsTransferSnapshotDto> {
        let bootstrap = self.bootstrap()?;
        let selected_project_ids = request
            .project_ids
            .iter()
            .map(|project_id| project_id.trim())
            .filter(|project_id| !project_id.is_empty())
            .collect::<HashSet<_>>();
        let include_all = request.mode == SETTINGS_TRANSFER_SCOPE_ALL;

        if !include_all {
            let available = bootstrap
                .projects
                .iter()
                .map(|project| project.id.as_str())
                .collect::<HashSet<_>>();
            let missing = selected_project_ids
                .iter()
                .filter(|project_id| !available.contains(**project_id))
                .copied()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                bail!("Cannot export unknown project IDs: {}.", missing.join(", "));
            }
        }

        let mut domains_by_project = HashMap::<String, Vec<SettingsTransferDomainDto>>::new();
        let mut standalone_domains = Vec::new();

        for domain in &bootstrap.domains {
            let export_domain = export_domain_snapshot(domain.registration.clone())?;
            if let Some(project_id) = domain.project_id.as_deref() {
                domains_by_project
                    .entry(project_id.to_string())
                    .or_default()
                    .push(export_domain);
            } else if include_all {
                standalone_domains.push(export_domain);
            }
        }

        let projects = bootstrap
            .projects
            .into_iter()
            .filter(|project| include_all || selected_project_ids.contains(project.id.as_str()))
            .map(|project| {
                let project_id = project.id;
                let domains = domains_by_project.remove(&project_id).unwrap_or_default();

                SettingsTransferProjectDto {
                    id: project_id,
                    name: project.name,
                    enabled: project.enabled,
                    root_path: project.root_path,
                    domains,
                }
            })
            .collect::<Vec<_>>();

        let settings = include_all.then_some(SettingsTransferSettingsDto {
            daemon: bootstrap.config.daemon,
            docker: bootstrap.config.docker,
            agent: bootstrap.config.agent,
            keygen: KeygenSettingsUpdateRequest {
                api_url: bootstrap.licensing.api_url,
                account: bootstrap.licensing.account,
                product: bootstrap.licensing.product,
                public_key: bootstrap.licensing.public_key,
            },
            launcher_preferences: bootstrap.launcher_preferences,
        });

        Ok(SettingsTransferSnapshotDto {
            version: SETTINGS_TRANSFER_VERSION,
            scope: request.mode.clone(),
            settings,
            standalone_domains,
            projects,
        })
    }

    fn import_settings_replace_all(
        &self,
        snapshot: SettingsTransferSnapshotDto,
    ) -> Result<SettingsImportResultDto> {
        let daemon_was_running = self.daemon_status()?.running;
        let current = self.bootstrap()?;

        for project in current.projects {
            self.remove_project(RemoveProjectRequest {
                id: project.id,
                delete_domains: true,
            })?;
        }

        for domain in current
            .domains
            .into_iter()
            .filter(|domain| domain.project_id.is_none())
        {
            self.unregister_domain(DomainPatternRequest {
                domain: domain.registration.domain,
                wildcard: domain.registration.wildcard,
            })?;
        }

        let applied_settings = if let Some(settings) = snapshot.settings.clone() {
            self.update_config(ConfigUpdateRequest {
                daemon: Some(settings.daemon),
                paths: None,
                docker: Some(settings.docker),
                agent: Some(settings.agent),
            })?;
            self.apply_imported_keygen_settings(settings.keygen)?;
            self.update_launcher_preferences(UpdateLauncherPreferencesRequest {
                editor: settings.launcher_preferences.editor,
                terminal: settings.launcher_preferences.terminal,
                browser: settings.launcher_preferences.browser,
                run_action: settings.launcher_preferences.run_action,
            })?;
            true
        } else {
            false
        };

        let standalone_domain_count =
            import_standalone_domains(self, &snapshot.standalone_domains)?;
        let (project_domain_count, skipped_root_paths) = import_projects(self, &snapshot.projects)?;
        let (daemon_reloaded, daemon_reload_error) =
            reload_daemon_if_running(self, daemon_was_running);

        Ok(SettingsImportResultDto {
            mode: SETTINGS_IMPORT_MODE_REPLACE_ALL.into(),
            imported_projects: snapshot.projects.len(),
            imported_domains: project_domain_count,
            imported_standalone_domains: standalone_domain_count,
            applied_settings,
            skipped_root_paths,
            daemon_reloaded,
            daemon_reload_error,
        })
    }

    fn import_settings_projects(
        &self,
        selected_projects: Vec<SettingsTransferProjectDto>,
    ) -> Result<SettingsImportResultDto> {
        let daemon_was_running = self.daemon_status()?.running;
        ensure_project_import_has_no_conflicts(self, &selected_projects)?;

        let existing_projects = self
            .list_projects()?
            .into_iter()
            .map(|project| project.id)
            .collect::<HashSet<_>>();

        for project in &selected_projects {
            if existing_projects.contains(project.id.as_str()) {
                self.remove_project(RemoveProjectRequest {
                    id: project.id.clone(),
                    delete_domains: true,
                })?;
            }
        }

        let (project_domain_count, skipped_root_paths) = import_projects(self, &selected_projects)?;
        let (daemon_reloaded, daemon_reload_error) =
            reload_daemon_if_running(self, daemon_was_running);

        Ok(SettingsImportResultDto {
            mode: SETTINGS_IMPORT_MODE_PROJECTS.into(),
            imported_projects: selected_projects.len(),
            imported_domains: project_domain_count,
            imported_standalone_domains: 0,
            applied_settings: false,
            skipped_root_paths,
            daemon_reloaded,
            daemon_reload_error,
        })
    }

    fn enrich_projects_with_local_paths(
        &self,
        projects: Vec<ProjectInfoDto>,
    ) -> Result<Vec<ProjectInfoDto>> {
        let local_projects = self.local_project_root_paths()?;

        Ok(projects
            .into_iter()
            .map(|mut project| {
                if project.root_path.is_none() {
                    project.root_path = local_projects.get(&project.id).cloned();
                }
                project
            })
            .collect())
    }

    fn enrich_project_environments_with_local_paths(
        &self,
        environments: Vec<ConfiguredProjectEnvironment>,
    ) -> Result<Vec<ConfiguredProjectEnvironment>> {
        let local_projects = self.local_project_root_paths()?;

        Ok(apply_local_project_root_paths_to_environments(
            environments,
            &local_projects,
        ))
    }

    fn local_project_root_paths(&self) -> Result<HashMap<String, String>> {
        match self.project_root_store() {
            Ok(store) => match store.load() {
                Ok(local_projects) => Ok(local_projects),
                Err(error) if is_missing_home_error(&error) => Ok(HashMap::new()),
                Err(error) => Err(error),
            },
            Err(error) if is_missing_home_error(&error) => Ok(HashMap::new()),
            Err(error) => Err(error),
        }
    }

    fn apply_imported_keygen_settings(&self, request: KeygenSettingsUpdateRequest) -> Result<()> {
        let licensing = self.licensing_status()?;
        if licensing.active_license.is_some() || licensing.offline_license.is_some() {
            return Ok(());
        }

        self.update_keygen_settings(request)?;
        Ok(())
    }

    pub fn remove_project(&self, request: RemoveProjectRequest) -> Result<RemoveProjectResultDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.remove_project(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let config_store = self.config_store();

        let project = config_store
            .list_projects()?
            .into_iter()
            .find(|project| project.id == request.id)
            .ok_or_else(|| anyhow::anyhow!("Project '{}' not found.", request.id))?;

        let attached_domains = config_store
            .list_configured_domains()?
            .into_iter()
            .filter(|domain| domain.project_id.as_deref() == Some(project.id.as_str()))
            .collect::<Vec<_>>();

        if !attached_domains.is_empty() && !request.delete_domains {
            anyhow::bail!(
                "Project '{}' still has domains. Delete the attached domains first, or retry with deleteDomains enabled.",
                project.id
            );
        }

        let domain_count = attached_domains.len();
        let mut removed_domains = Vec::with_capacity(domain_count);

        if request.delete_domains {
            let unregister_domain = UnregisterDomain::new(&config_store, &certs);
            for domain in &attached_domains {
                unregister_domain.execute(domain.registration.pattern())?;
                removed_domains.push(domain.registration.display_pattern());
            }
        }

        config_store.remove_project(&project.id)?;
        let _ = self.remove_project_root_path(&project.id);

        let daemon_reloaded = match MgmtSocketClient::new(&paths.socket_path).reload() {
            Ok(()) => true,
            Err(crate::application::ports::DaemonConnectionError::NotRunning) => false,
            Err(_) => false,
        };

        Ok(RemoveProjectResultDto {
            project: map_project_info(project, domain_count, false, None),
            removed_domains,
            daemon_reloaded,
        })
    }

    pub fn remove_project_environment(
        &self,
        request: RemoveProjectEnvironmentRequest,
    ) -> Result<RemoveProjectEnvironmentResultDto> {
        if self.should_use_system_helper() {
            return self
                .ensure_helper_running()?
                .remove_project_environment(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let config_store = self.config_store();
        let environment = self
            .list_project_environments()?
            .into_iter()
            .find(|environment| {
                environment.project_id == request.project_id
                    && environment.slug == request.environment_slug
            })
            .ok_or_else(|| {
                anyhow!(
                    "Project environment '{}' was not found for project '{}'.",
                    request.environment_slug,
                    request.project_id
                )
            })?;

        if environment.primary {
            bail!("The primary environment cannot be removed.");
        }

        let environment_domains = environment
            .services
            .iter()
            .map(|service| service.domain.as_str())
            .collect::<HashSet<_>>();
        let attached_domains = config_store
            .list_configured_domains()?
            .into_iter()
            .filter(|domain| {
                environment_domains.contains(domain.registration.display_pattern().as_str())
            })
            .collect::<Vec<_>>();
        let mut removed_domains = Vec::with_capacity(attached_domains.len());
        let unregister_domain = UnregisterDomain::new(&config_store, &certs);
        for domain in &attached_domains {
            unregister_domain.execute(domain.registration.pattern())?;
            removed_domains.push(domain.registration.display_pattern());
        }

        config_store.remove_project_environment(&request.project_id, &request.environment_slug)?;

        let daemon_reloaded = match MgmtSocketClient::new(&paths.socket_path).reload() {
            Ok(()) => true,
            Err(crate::application::ports::DaemonConnectionError::NotRunning) => false,
            Err(_) => false,
        };

        Ok(RemoveProjectEnvironmentResultDto {
            environment,
            removed_domains,
            daemon_reloaded,
        })
    }

    pub fn assign_domain_project(
        &self,
        request: AssignDomainProjectRequest,
    ) -> Result<DomainInfoDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.assign_domain_project(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let cert_trusted = certs.is_trusted().ok();
        let pattern = DomainPattern::from_name(&request.domain, request.wildcard)?;
        let registration = self
            .config_store()
            .get_domain(&pattern)?
            .ok_or_else(|| anyhow::anyhow!("Domain '{}' not registered", pattern))?;

        self.config_store()
            .update_domain_with_project(registration, request.project_id)?;

        let configured_domain = self
            .config_store()
            .list_configured_domains()?
            .into_iter()
            .find(|domain| domain.registration.display_pattern() == pattern.display_pattern())
            .ok_or_else(|| anyhow::anyhow!("Domain '{}' not registered", pattern))?;

        let daemon = MgmtSocketClient::new(&paths.socket_path);
        let (active, last_accessed_at) = daemon
            .list_registration_info()
            .map(|registrations| {
                registrations.into_iter().find_map(|info| {
                    (info.registration.display_pattern() == pattern.display_pattern())
                        .then_some(info.last_accessed_at)
                })
            })
            .ok()
            .flatten()
            .map_or((false, None), |last_accessed_at| (true, last_accessed_at));

        let has_cert = certs.exists(configured_domain.registration.pattern());
        let target_reachable = registration_has_reachable_target(&configured_domain.registration);
        Ok(map_configured_domain_info(
            configured_domain,
            has_cert,
            cert_trusted,
            active,
            target_reachable,
            last_accessed_at,
        ))
    }

    pub fn available_port(&self) -> Result<AvailablePortDto> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.get_available_port();
        }

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .context("Failed to ask the OS for an available local port")?;
        let port = listener
            .local_addr()
            .context("Failed to read the generated local port")?
            .port();

        Ok(AvailablePortDto { port })
    }

    pub fn install(&self) -> Result<InstallResultDto> {
        if self.should_use_system_helper() {
            return self.install_with_system_helper();
        }

        let daemon_was_running = self
            .daemon_status()
            .map(|status| status.running)
            .unwrap_or(false);
        let (config, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let dns = get_dns_service()?;
        let network = get_network_info();
        let system = FileSystemSetup::new(&paths);

        let result = Install::new(
            &certs,
            &self.config_store(),
            &dns,
            &network,
            &system,
            config.daemon.dns_port,
        )
        .execute()?;

        let mut steps = result
            .steps
            .into_iter()
            .map(|(label, outcome)| map_step(Some(label), outcome))
            .collect::<Vec<_>>();
        let refresh = self.refresh_domain_certificates_local()?;
        steps.extend(refresh.steps);
        if refresh.refreshed {
            steps.push(self.restart_daemon_after_certificate_refresh_step(daemon_was_running));
        }
        if !self.direct_mode {
            steps.push(self.cli_access_step_with(|_| self.install_cli_access()));
            if let Some(step) = self.node_trust_step() {
                steps.push(step);
            }
        }
        if let Some(step) = agent_skill_install_step(config.agent.install_skill) {
            steps.push(step);
        }

        Ok(InstallResultDto {
            lan_ip: result.lan_ip.to_string(),
            steps,
        })
    }

    fn install_with_system_helper(&self) -> Result<InstallResultDto> {
        self.run_privileged_desktop_install()?;

        let helper = self.ensure_helper_running()?;
        let daemon_was_running = helper
            .get_daemon_status()
            .map(|status| status.running)
            .unwrap_or(false);
        let mut steps = vec![StepDto {
            label: Some("System setup".into()),
            status: "success".into(),
            message: format!(
                "{} system setup completed with administrator privileges.",
                BRANDING.app_name
            ),
        }];

        if let Some(step) = self.ensure_desktop_ca_trust_step()? {
            steps.push(step);
        }

        let refresh = helper.refresh_domain_certificates()?;
        steps.extend(refresh.steps);
        if refresh.refreshed {
            steps.push(restart_daemon_after_certificate_refresh_step_with(
                daemon_was_running,
                || helper.restart_daemon(StartDaemonRequest { verbose: false }),
            ));
        }

        let status = self.daemon_status()?;
        steps.push(root_ca_verification_step(status.ca_installed));
        steps.push(self.cli_access_step_with(|_| helper.setup_cli_access()));
        if let Some(step) = self.node_trust_step() {
            steps.push(step);
        }
        let install_agent_skill = self
            .config_store()
            .load()
            .map(|config| config.agent.install_skill)
            .unwrap_or(true);
        if let Some(step) = agent_skill_install_step(install_agent_skill) {
            steps.push(step);
        }

        Ok(InstallResultDto {
            lan_ip: status.lan_ip,
            steps,
        })
    }

    pub fn refresh_domain_certificates(&self) -> Result<CertificateRefreshDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.refresh_domain_certificates();
        }

        self.refresh_domain_certificates_local()
    }

    fn refresh_domain_certificates_local(&self) -> Result<CertificateRefreshDto> {
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let configured_domains = self.config_store().list_configured_domains()?;
        let https_domains = configured_domains
            .iter()
            .filter(|domain| domain.registration.is_https_enabled())
            .collect::<Vec<_>>();

        if https_domains.is_empty() {
            return Ok(CertificateRefreshDto {
                refreshed: false,
                steps: vec![StepDto {
                    label: Some("Domain certificates".into()),
                    status: "skipped".into(),
                    message: "No HTTPS domain certificates need to be refreshed.".into(),
                }],
            });
        }

        let mut refreshed = 0usize;
        let mut failures = Vec::new();
        for domain in https_domains {
            match certs.create_and_install(domain.registration.pattern()) {
                Ok(()) => refreshed += 1,
                Err(error) => failures.push(format!("{}: {error}", domain.registration.pattern())),
            }
        }

        let message = if failures.is_empty() {
            format!(
                "Refreshed {refreshed} HTTPS domain certificate{}.",
                plural_suffix(refreshed)
            )
        } else {
            let preview = failures
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "Refreshed {refreshed} HTTPS domain certificate{}, but {} failed: {preview}",
                plural_suffix(refreshed),
                failures.len()
            )
        };

        Ok(CertificateRefreshDto {
            refreshed: refreshed > 0,
            steps: vec![StepDto {
                label: Some("Domain certificates".into()),
                status: if failures.is_empty() {
                    "success".into()
                } else {
                    "warning".into()
                },
                message,
            }],
        })
    }

    fn restart_daemon_after_certificate_refresh_step(&self, daemon_was_running: bool) -> StepDto {
        restart_daemon_after_certificate_refresh_step_with(daemon_was_running, || {
            self.restart_daemon(StartDaemonRequest { verbose: false })
        })
    }

    pub fn uninstall_preview(&self) -> Result<UninstallPreviewDto> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.get_uninstall_preview();
        }

        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let dns = get_dns_service()?;
        let system = FileSystemSetup::new(&paths);
        let pid_file = PidFile::new(paths.pid_file.clone());

        let preview = Uninstall::new(
            &self.config_store(),
            &certs,
            &pid_file,
            &dns,
            &system,
            paths.data_dir.display().to_string(),
        )
        .preview()?;

        Ok(UninstallPreviewDto {
            domain_count: preview.domain_count,
            data_dir: preview.data_dir,
        })
    }

    pub fn uninstall(&self) -> Result<UninstallResultDto> {
        if self.should_use_system_helper() {
            let desktop_trust_step = self.remove_desktop_ca_trust_step();
            let helper = self.ensure_helper_running()?;
            let mut result = helper.uninstall_roxy()?;
            let uid = current_uid()?;

            if let Some(step) = desktop_trust_step {
                result.steps.push(step);
            }
            if let Some(step) = self.remove_node_trust_step() {
                result.steps.push(step);
            }

            match self.run_cli_command_with_admin(&[
                "helper".into(),
                "uninstall-service".into(),
                "--uid".into(),
                uid.to_string(),
            ]) {
                Ok(_) => result.steps.push(StepDto {
                    label: Some("Remove privileged helper".into()),
                    status: "success".into(),
                    message: format!("Removed the persistent {} desktop helper.", BRANDING.app_name),
                }),
                Err(error) => result.steps.push(StepDto {
                    label: Some("Remove privileged helper".into()),
                    status: "warning".into(),
                    message: format!(
                        "{} was uninstalled, but the persistent desktop helper could not be removed: {error}",
                        BRANDING.app_name
                    ),
                }),
            }

            return Ok(result);
        }

        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let dns = get_dns_service()?;
        let system = FileSystemSetup::new(&paths);
        let pid_file = PidFile::new(paths.pid_file.clone());

        let result = Uninstall::new(
            &self.config_store(),
            &certs,
            &pid_file,
            &dns,
            &system,
            paths.data_dir.display().to_string(),
        )
        .execute()?;

        let mut steps = result
            .steps
            .into_iter()
            .map(|(label, outcome)| map_step(Some(label), outcome))
            .collect::<Vec<_>>();
        if !self.direct_mode {
            steps.push(self.remove_cli_access_step());
            if let Some(step) = self.remove_node_trust_step() {
                steps.push(step);
            }
        }

        Ok(UninstallResultDto { steps })
    }

    pub fn register_domain(
        &self,
        request: RegisterDomainRequest,
    ) -> Result<RegisterDomainResultDto> {
        if self.should_use_system_helper() {
            let routes = request
                .routes
                .iter()
                .map(|route| normalize_desktop_route(route))
                .collect::<Result<Vec<_>>>()?;

            return self
                .ensure_helper_running()?
                .register_domain(RegisterDomainRequest { routes, ..request });
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let pattern = DomainPattern::from_name(&request.domain, request.wildcard)?;
        let routes = request
            .routes
            .iter()
            .map(|route| parse_desktop_route(route))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| anyhow::anyhow!("Invalid route: {error}"))?;

        let result = RegisterDomain::new(&self.config_store(), &certs).execute(pattern, routes)?;
        self.config_store()
            .update_domain_with_project(result.registration.clone(), request.project_id)?;

        Ok(RegisterDomainResultDto {
            registration: map_registration(result.registration),
            cert_outcome: map_step(None, result.cert_outcome),
        })
    }

    pub fn update_domain(&self, request: UpdateDomainRequest) -> Result<RegisterDomainResultDto> {
        if self.should_use_system_helper() {
            let routes = request
                .routes
                .iter()
                .map(|route| normalize_desktop_route(route))
                .collect::<Result<Vec<_>>>()?;

            return self
                .ensure_helper_running()?
                .update_domain(UpdateDomainRequest { routes, ..request });
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let current_pattern =
            DomainPattern::from_name(&request.current_domain, request.current_wildcard)?;
        let pattern = DomainPattern::from_name(&request.domain, request.wildcard)?;
        let routes = request
            .routes
            .iter()
            .map(|route| parse_desktop_route(route))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| anyhow::anyhow!("Invalid route: {error}"))?;

        if routes.is_empty() {
            bail!("At least one route is required.");
        }

        let existing = self
            .config_store()
            .get_domain(&current_pattern)?
            .ok_or_else(|| anyhow::anyhow!("Domain '{}' not registered", current_pattern))?;
        if current_pattern.display_pattern() != pattern.display_pattern()
            && self.config_store().get_domain(&pattern)?.is_some()
        {
            bail!("Domain '{}' is already registered.", pattern);
        }

        let mut registration = DomainRegistration::new(pattern.clone(), routes);
        let pattern_changed = current_pattern.display_pattern() != pattern.display_pattern();
        let mut cert_outcome = if !pattern_changed {
            if existing.is_https_enabled() {
                registration.enable_https();
            }
            StepOutcome::Skipped("Domain pattern unchanged.".into())
        } else {
            match certs.create_and_install(&pattern) {
                Ok(()) => {
                    registration.enable_https();
                    StepOutcome::Success("Certificate installed and trusted.".into())
                }
                Err(error) => StepOutcome::Warning(format!(
                    "Failed to generate certificate: {error}. HTTPS will not be available for this domain."
                )),
            }
        };

        self.config_store().replace_domain_with_project(
            &current_pattern,
            registration.clone(),
            request.project_id,
        )?;

        if pattern_changed
            && certs.exists(&current_pattern)
            && let Err(error) = certs.remove(&current_pattern)
            && !matches!(cert_outcome, StepOutcome::Warning(_))
        {
            cert_outcome = StepOutcome::Warning(format!(
                "Domain updated, but failed to remove the old certificate: {error}."
            ));
        }

        Ok(RegisterDomainResultDto {
            registration: map_registration(registration),
            cert_outcome: map_step(None, cert_outcome),
        })
    }

    pub fn unregister_domain(
        &self,
        request: DomainPatternRequest,
    ) -> Result<UnregisterDomainResultDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.unregister_domain(request);
        }

        self.ensure_config_writable()?;
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let pattern = parse_pattern(&request)?;
        let result = UnregisterDomain::new(&self.config_store(), &certs).execute(&pattern)?;

        Ok(UnregisterDomainResultDto {
            registration: map_registration(result.registration),
            cert_outcome: map_step(None, result.cert_outcome),
        })
    }

    pub fn add_route(&self, request: AddRouteRequest) -> Result<RegistrationMutationDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.add_route(request);
        }

        self.ensure_config_writable()?;
        let pattern = DomainPattern::from_name(&request.domain, request.wildcard)?;
        let path = PathPrefix::new(&request.path)?;
        let target = RouteTarget::parse(&request.target)?;

        let route = ManageRoutes::new(&self.config_store()).add_route(&pattern, path, target)?;
        let registration = ManageRoutes::new(&self.config_store()).list_routes(&pattern)?;

        Ok(RegistrationMutationDto {
            registration: map_registration(registration),
            route: Some(map_route(&route)),
        })
    }

    pub fn remove_route(&self, request: RemoveRouteRequest) -> Result<RegistrationMutationDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.remove_route(request);
        }

        self.ensure_config_writable()?;
        let pattern = DomainPattern::from_name(&request.domain, request.wildcard)?;
        let path = PathPrefix::new(&request.path)?;

        let config_store = self.config_store();
        let routes = ManageRoutes::new(&config_store);
        routes.remove_route(&pattern, &path)?;
        let registration = routes.list_routes(&pattern)?;

        Ok(RegistrationMutationDto {
            registration: map_registration(registration),
            route: None,
        })
    }

    pub fn list_routes(&self, request: DomainPatternRequest) -> Result<RegistrationDto> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.list_routes(request);
        }

        let pattern = parse_pattern(&request)?;
        let registration = ManageRoutes::new(&self.config_store()).list_routes(&pattern)?;
        Ok(map_registration(registration))
    }

    pub fn list_port_conflicts(&self) -> Result<Vec<PortConflictDto>> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.list_port_conflicts();
        }

        let (config, _) = self.load_config_and_paths()?;
        let conflicts = list_daemon_port_conflicts(
            config.daemon.http_port,
            config.daemon.https_port,
            config.daemon.dns_port,
        )?;

        Ok(conflicts.into_iter().map(map_port_conflict).collect())
    }

    pub fn kill_port_conflict(&self, request: KillPortConflictRequest) -> Result<PortConflictDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.kill_port_conflict(request);
        }

        let (config, _) = self.load_config_and_paths()?;
        let conflict = kill_daemon_port_conflict(
            config.daemon.http_port,
            config.daemon.https_port,
            config.daemon.dns_port,
            &request.protocol,
            request.port,
            request.pid,
        )?;

        Ok(map_port_conflict(conflict))
    }

    pub fn start_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.start_daemon(request);
        }

        let (config, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file.clone());
        let ready = StartDaemon::new(&pid_file, &config.daemon).preflight()?;
        ensure_daemon_ports_available(ready.http_port, ready.https_port, config.daemon.dns_port)?;
        let child = self
            .spawn_daemon_process(request.verbose)
            .with_context(|| {
                format!("Failed to launch daemon via {}", self.binary_path.display())
            })?;
        let pid = self.wait_for_daemon_start(child, &pid_file, &paths.log_file)?;

        Ok(DaemonLaunchDto {
            pid,
            lan_ip: get_lan_ip().to_string(),
            http_port: ready.http_port,
            https_port: ready.https_port,
            dns_port: config.daemon.dns_port,
        })
    }

    pub fn stop_daemon(&self) -> Result<DaemonStatusDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.stop_daemon();
        }

        let (_, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file);
        StopDaemon::new(&pid_file).execute()?;
        self.daemon_status()
    }

    pub fn restart_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.restart_daemon(request);
        }

        let config_store = self.config_store();
        let (_, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file);
        let ready = RestartDaemon::new(&pid_file, &config_store).restart()?;
        ensure_daemon_ports_available(
            ready.daemon_config.http_port,
            ready.daemon_config.https_port,
            ready.daemon_config.dns_port,
        )?;
        let child = self.spawn_daemon_process(request.verbose)?;
        let pid = self.wait_for_daemon_start(
            child,
            &PidFile::new(ready.paths.pid_file.clone()),
            &ready.paths.log_file,
        )?;

        Ok(DaemonLaunchDto {
            pid,
            lan_ip: get_lan_ip().to_string(),
            http_port: ready.daemon_config.http_port,
            https_port: ready.daemon_config.https_port,
            dns_port: ready.daemon_config.dns_port,
        })
    }

    pub fn reload_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.reload_daemon(request);
        }

        let config_store = self.config_store();
        let (_, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file);
        let ready = RestartDaemon::new(&pid_file, &config_store).reload()?;
        ensure_daemon_ports_available(
            ready.daemon_config.http_port,
            ready.daemon_config.https_port,
            ready.daemon_config.dns_port,
        )?;
        let child = self.spawn_daemon_process(request.verbose)?;
        let pid = self.wait_for_daemon_start(
            child,
            &PidFile::new(ready.paths.pid_file.clone()),
            &ready.paths.log_file,
        )?;

        Ok(DaemonLaunchDto {
            pid,
            lan_ip: get_lan_ip().to_string(),
            http_port: ready.daemon_config.http_port,
            https_port: ready.daemon_config.https_port,
            dns_port: ready.daemon_config.dns_port,
        })
    }

    pub fn read_logs(&self, request: LogsRequest) -> Result<LogsDto> {
        if let Some(helper) = self.connected_helper_client()? {
            return helper.read_logs(request);
        }

        let (_, paths) = self.load_config_and_paths()?;
        let log_path = paths.log_file;

        if request.clear {
            if let Some(parent) = log_path.parent() {
                fs::create_dir_all(parent).context("Failed to prepare log directory")?;
            }
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&log_path)
                .context("Failed to clear log file")?;

            return Ok(LogsDto {
                path: log_path.display().to_string(),
                exists: true,
                cleared: true,
                content: String::new(),
            });
        }

        if !log_path.exists() {
            return Ok(LogsDto {
                path: log_path.display().to_string(),
                exists: false,
                cleared: false,
                content: String::new(),
            });
        }

        Ok(LogsDto {
            path: log_path.display().to_string(),
            exists: true,
            cleared: false,
            content: tail_lines(&log_path, request.lines)?,
        })
    }

    fn spawn_daemon_process(&self, verbose: bool) -> Result<std::process::Child> {
        if !self.binary_path.exists() {
            bail!(
                "{} CLI binary not found at {}. Build the root CLI first or set {}.",
                BRANDING.app_name,
                self.binary_path.display(),
                BRANDING.env_var("CLI_PATH")
            );
        }

        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("--config")
            .arg(&self.config_path)
            .arg("start")
            .arg("--foreground")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if verbose {
            cmd.env(BRANDING.env_var("LOG"), "debug");
        }
        if let Some(profile) = BRANDING.active_profile() {
            cmd.env(BRANDING.profile_env(), profile);
        }

        cmd.spawn().map_err(Into::into)
    }

    fn wait_for_daemon_start(
        &self,
        mut child: std::process::Child,
        pid_file: &PidFile,
        log_path: &Path,
    ) -> Result<u32> {
        let deadline = Instant::now() + Duration::from_secs(3);

        loop {
            if let Some(pid) = pid_file.get_running_pid()? {
                return Ok(pid);
            }

            if let Some(status) = child.try_wait()? {
                bail!(daemon_start_failure_message(status, log_path));
            }

            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!(daemon_start_timeout_message(log_path));
            }

            thread::sleep(Duration::from_millis(100));
        }
    }

    fn load_config_and_paths(&self) -> Result<(Config, RoxyPaths)> {
        let config = self.config_store().load()?;
        let paths = config.paths.clone();
        Ok((config, paths))
    }

    fn config_store(&self) -> ConfigStore {
        ConfigStore::new(self.config_path.clone())
    }

    fn project_root_store(&self) -> Result<ProjectRootStore> {
        ProjectRootStore::new_default()
    }

    fn launcher_preferences_store(&self) -> Result<LauncherPreferencesStore> {
        LauncherPreferencesStore::new_default()
    }

    pub fn service_dev_command(
        &self,
        project_id: &str,
        environment_slug: &str,
        service_id: &str,
    ) -> Vec<String> {
        let mut command = Vec::new();
        if let Some(profile) = BRANDING.active_profile() {
            command.push(format!("{}={profile}", BRANDING.profile_env()));
        }
        command.extend([
            self.binary_path.display().to_string(),
            "--config".into(),
            self.config_path.display().to_string(),
            "dev".into(),
            "--project".into(),
            project_id.to_string(),
            "--env".into(),
            environment_slug.to_string(),
            service_id.to_string(),
        ]);
        command
    }

    fn launcher_preferences(&self) -> Result<crate::desktop::LauncherPreferences> {
        self.launcher_preferences_store()?.load()
    }

    fn effective_launcher_preferences(&self) -> Result<crate::desktop::LauncherPreferences> {
        match self.launcher_preferences() {
            Ok(preferences) => Ok(self.normalize_launcher_preferences(preferences)),
            Err(error) if is_missing_home_error(&error) => {
                Ok(crate::desktop::LauncherPreferences::default())
            }
            Err(error) => Err(error),
        }
    }

    fn normalize_launcher_preferences(
        &self,
        preferences: crate::desktop::LauncherPreferences,
    ) -> crate::desktop::LauncherPreferences {
        let available_editors = editor::available_editor_preferences();
        let available_terminals = local_dev::available_terminal_preferences();

        crate::desktop::LauncherPreferences {
            editor: if preferences.editor == crate::desktop::EditorPreference::Auto
                || available_editors.contains(&preferences.editor)
            {
                preferences.editor
            } else {
                crate::desktop::EditorPreference::Auto
            },
            terminal: if available_terminals.contains(&preferences.terminal) {
                preferences.terminal
            } else {
                available_terminals
                    .first()
                    .copied()
                    .unwrap_or(crate::desktop::TerminalPreference::Terminal)
            },
            browser: preferences.browser,
            run_action: preferences.run_action,
        }
    }

    fn launcher_availability(&self) -> LauncherAvailabilityDto {
        LauncherAvailabilityDto {
            available_editors: editor::available_editor_preferences()
                .into_iter()
                .map(|id| crate::desktop::dto::EditorOptionDto {
                    id,
                    label: editor::editor_label(id).into(),
                })
                .collect(),
            available_terminals: local_dev::available_terminal_preferences()
                .into_iter()
                .map(|id| crate::desktop::dto::TerminalOptionDto {
                    id,
                    label: local_dev::terminal_label(id).into(),
                })
                .collect(),
        }
    }

    fn store_project_root_path(&self, project_id: &str, root_path: &Path) -> Result<()> {
        if self.direct_mode {
            return Ok(());
        }

        match self.project_root_store()?.set(project_id, root_path) {
            Ok(()) => Ok(()),
            Err(error) if is_missing_home_error(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn remove_project_root_path(&self, project_id: &str) -> Result<()> {
        if self.direct_mode {
            return Ok(());
        }

        match self.project_root_store()?.remove(project_id) {
            Ok(()) => Ok(()),
            Err(error) if is_missing_home_error(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn should_use_system_helper(&self) -> bool {
        !self.direct_mode && (self.config_path.starts_with("/etc") || BRANDING.is_dev_profile())
    }

    fn should_use_system_helper_for_worktree_attach(&self) -> bool {
        !self.direct_mode && self.config_path.starts_with("/etc")
    }

    fn cli_helper_ready(&self) -> bool {
        self.connected_helper_client().ok().flatten().is_some()
    }

    fn cli_access_ready(&self) -> bool {
        cli_command_points_to_binary(&self.cli_command_path(), &self.binary_path)
    }

    fn cli_command_path(&self) -> PathBuf {
        PathBuf::from(CLI_INSTALL_DIR).join(BRANDING.cli_command())
    }

    fn install_cli_access(&self) -> Result<CliAccessDto> {
        self.ensure_cli_binary_exists()?;

        let command_path = self.cli_command_path();
        let already_ready = cli_command_points_to_binary(&command_path, &self.binary_path);

        if !already_ready {
            ensure_cli_command_link(&command_path, &self.binary_path)?;
        }

        Ok(CliAccessDto {
            ready: true,
            command_path: command_path.display().to_string(),
            already_ready,
        })
    }

    fn cli_access_step_with(
        &self,
        setup: impl FnOnce(&SystemHelperClient) -> Result<CliAccessDto>,
    ) -> StepDto {
        let label = Some("Install command line access".into());

        if self.should_use_system_helper() {
            let helper = match self.ensure_helper_running() {
                Ok(helper) => helper,
                Err(error) => {
                    return StepDto {
                        label,
                        status: "warning".into(),
                        message: format!(
                            "{} finished installing, but the `{}` terminal command could not be enabled: {error}",
                            BRANDING.app_name,
                            BRANDING.cli_command(),
                        ),
                    };
                }
            };

            return map_cli_access_step(label, setup(&helper));
        }

        map_cli_access_step(label, self.install_cli_access())
    }

    fn remove_cli_access_step(&self) -> StepDto {
        let label = Some("Remove command line access".into());

        match remove_cli_command_link(&self.cli_command_path(), &self.binary_path) {
            Ok(CliRemovalOutcome::Removed(path)) => StepDto {
                label,
                status: "success".into(),
                message: format!("Removed `{}` from {}.", BRANDING.cli_command(), path),
            },
            Ok(CliRemovalOutcome::Missing) => StepDto {
                label,
                status: "skipped".into(),
                message: format!(
                    "The `{}` command was already absent from {}.",
                    BRANDING.cli_command(),
                    self.cli_command_path().display(),
                ),
            },
            Ok(CliRemovalOutcome::Unmanaged(path)) => StepDto {
                label,
                status: "skipped".into(),
                message: format!(
                    "Left {} in place because it is not the symlink managed by {}.",
                    path, BRANDING.app_name,
                ),
            },
            Err(error) => StepDto {
                label,
                status: "warning".into(),
                message: format!(
                    "{} was uninstalled, but the `{}` terminal command could not be removed: {error}",
                    BRANDING.app_name,
                    BRANDING.cli_command(),
                ),
            },
        }
    }

    fn node_trust_status_dto(&self) -> NodeTrustDto {
        let ca_cert_path = self.ca_cert_path();

        match node_trust::status(&ca_cert_path) {
            Ok(status) => {
                let already_ready = status.ready;
                map_node_trust_dto(status, already_ready, None)
            }
            Err(error) => NodeTrustDto {
                ready: false,
                already_ready: false,
                ca_cert_path: ca_cert_path.display().to_string(),
                shell_snippet_path: String::new(),
                shell_integration_ready: false,
                launchctl_ready: false,
                last_error: Some(error.to_string()),
            },
        }
    }

    fn node_trust_step(&self) -> Option<StepDto> {
        if self.direct_mode {
            return None;
        }

        let label = Some("Enable Node.js certificate trust".into());

        Some(match self.setup_node_trust() {
            Ok(status) if status.already_ready => StepDto {
                label,
                status: "skipped".into(),
                message: format!(
                    "Node.js certificate trust was already enabled via {} and {}.",
                    status.shell_snippet_path, status.ca_cert_path
                ),
            },
            Ok(status) => StepDto {
                label,
                status: "success".into(),
                message: format!(
                    "Enabled Node.js certificate trust for new terminals and restarted apps using {}.",
                    status.ca_cert_path
                ),
            },
            Err(error) => StepDto {
                label,
                status: "warning".into(),
                message: format!(
                    "{} finished installing, but Node.js certificate trust could not be enabled automatically: {error}",
                    BRANDING.app_name
                ),
            },
        })
    }

    fn remove_node_trust_step(&self) -> Option<StepDto> {
        if self.direct_mode {
            return None;
        }

        let label = Some("Remove Node.js certificate trust".into());

        Some(match node_trust::uninstall(&self.ca_cert_path()) {
            Ok(removal) if removal.removed => StepDto {
                label,
                status: "success".into(),
                message: format!(
                    "Removed the {} Node.js trust integration from your user shell environment.",
                    BRANDING.app_name
                ),
            },
            Ok(_) => StepDto {
                label,
                status: "skipped".into(),
                message: format!(
                    "No {} Node.js trust integration was present for this user.",
                    BRANDING.app_name
                ),
            },
            Err(error) => StepDto {
                label,
                status: "warning".into(),
                message: format!(
                    "{} was uninstalled, but the Node.js trust integration could not be fully removed: {error}",
                    BRANDING.app_name
                ),
            },
        })
    }

    fn connected_helper_client(&self) -> Result<Option<SystemHelperClient>> {
        if !self.should_use_system_helper() {
            return Ok(None);
        }

        let client = SystemHelperClient::new(self.helper_socket_path()?);
        if client.ping().is_err() {
            return Ok(None);
        }

        if self.helper_matches_binary_path()? && self.helper_matches_runtime_config(&client)? {
            return Ok(Some(client));
        }

        eprintln!(
            "ignoring running {} helper because it does not match the current runtime at {}",
            BRANDING.app_name,
            self.binary_path.display()
        );
        Ok(None)
    }

    fn ensure_helper_running(&self) -> Result<SystemHelperClient> {
        let socket_path = self.helper_socket_path()?;
        let client = SystemHelperClient::new(socket_path.clone());
        if client.ping().is_ok()
            && self.helper_matches_binary_path()?
            && self.helper_matches_runtime_config(&client)?
        {
            return Ok(client);
        }

        let uid = current_uid()?;
        let args = vec![
            "helper".into(),
            "ensure-running".into(),
            "--socket".into(),
            socket_path.display().to_string(),
            "--uid".into(),
            uid.to_string(),
        ];
        self.run_cli_command_with_admin(&args)?;

        for _ in 0..50 {
            if client.ping().is_ok() {
                return Ok(client);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        bail!(
            "{} system helper did not become ready in time.",
            BRANDING.app_name
        )
    }

    fn run_privileged_desktop_install(&self) -> Result<()> {
        let socket_path = self.helper_socket_path()?;
        let uid = current_uid()?;
        let commands = [
            vec!["install".into()],
            vec![
                "helper".into(),
                "uninstall-service".into(),
                "--uid".into(),
                uid.to_string(),
            ],
            vec![
                "helper".into(),
                "ensure-running".into(),
                "--socket".into(),
                socket_path.display().to_string(),
                "--uid".into(),
                uid.to_string(),
            ],
        ];

        self.run_cli_commands_with_admin(&commands)?;
        Ok(())
    }

    fn helper_matches_runtime_config(&self, client: &SystemHelperClient) -> Result<bool> {
        if !BRANDING.is_dev_profile() {
            return Ok(true);
        }

        let helper_config = match client.get_config() {
            Ok(config) => config,
            Err(_) => return Ok(false),
        };
        let current_config = self.config_store().load()?;
        let expected_path = self.config_path.display().to_string();

        Ok(helper_config.config_path == expected_path
            && helper_config.daemon.http_port == current_config.daemon.http_port
            && helper_config.daemon.https_port == current_config.daemon.https_port
            && helper_config.daemon.dns_port == current_config.daemon.dns_port)
    }

    #[cfg(target_os = "macos")]
    fn helper_matches_binary_path(&self) -> Result<bool> {
        let uid = current_uid()?;
        let plist_path = PathBuf::from("/Library/LaunchDaemons").join(format!(
            "{}.{}.plist",
            BRANDING.helper_service_prefix(),
            uid
        ));

        if !plist_path.exists() {
            return Ok(false);
        }

        let plist = fs::read_to_string(&plist_path).with_context(|| {
            format!(
                "Failed to read the {} helper plist at {}",
                BRANDING.app_name,
                plist_path.display()
            )
        })?;

        let expected_binary =
            fs::canonicalize(&self.binary_path).unwrap_or_else(|_| self.binary_path.clone());
        Ok(plist.contains(&escape_xml(&expected_binary.display().to_string())))
    }

    #[cfg(not(target_os = "macos"))]
    fn helper_matches_binary_path(&self) -> Result<bool> {
        Ok(true)
    }

    #[cfg(target_os = "macos")]
    fn run_cli_command_with_admin(&self, args: &[String]) -> Result<String> {
        self.run_cli_commands_with_admin(&[args.to_vec()])
    }

    #[cfg(target_os = "macos")]
    fn run_cli_commands_with_admin(&self, commands: &[Vec<String>]) -> Result<String> {
        self.ensure_cli_binary_exists()?;

        let command = cli_shell_commands(&self.binary_path, &self.config_path, commands);
        let script = format!(
            "do shell script \"{}\" with administrator privileges",
            escape_applescript_string(&command)
        );

        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .context(format!(
                "Failed to launch macOS administrator prompt for {}",
                BRANDING.app_name
            ))?;

        if output.status.success() {
            return Ok(command_output(&output));
        }

        bail!(cli_failure_message(&output));
    }

    #[cfg(not(target_os = "macos"))]
    fn run_cli_command_with_admin(&self, _args: &[String]) -> Result<String> {
        self.run_cli_commands_with_admin(&[])
    }

    #[cfg(not(target_os = "macos"))]
    fn run_cli_commands_with_admin(&self, _commands: &[Vec<String>]) -> Result<String> {
        bail!(
            "Desktop system-mode actions currently require macOS administrator integration. \
             Run the equivalent `{}` CLI command manually on this platform.",
            BRANDING.cli_command()
        )
    }

    fn ensure_cli_binary_exists(&self) -> Result<()> {
        if self.binary_path.exists() {
            return Ok(());
        }

        bail!(
            "{} CLI binary not found at {}. Build the root CLI first or set {}.",
            BRANDING.app_name,
            self.binary_path.display(),
            BRANDING.env_var("CLI_PATH")
        );
    }

    fn helper_socket_path(&self) -> Result<PathBuf> {
        Ok(helper_socket_path(current_uid()?))
    }

    fn ca_cert_path(&self) -> PathBuf {
        self.config_store()
            .load()
            .map(|config| config.paths.data_dir.join("ca.crt"))
            .unwrap_or_else(|_| RoxyPaths::default().data_dir.join("ca.crt"))
    }

    #[cfg(target_os = "macos")]
    fn effective_desktop_ca_installed(&self) -> Result<bool> {
        let cert_path = self.ca_cert_path();

        if macos_user_ca_is_trusted(&cert_path)? {
            return Ok(true);
        }

        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        certs.is_ca_installed().map_err(Into::into)
    }

    #[cfg(not(target_os = "macos"))]
    fn effective_desktop_ca_installed(&self) -> Result<bool> {
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        certs.is_ca_installed().map_err(Into::into)
    }

    #[cfg(target_os = "macos")]
    fn ensure_desktop_ca_trust_step(&self) -> Result<Option<StepDto>> {
        Ok(Some(macos_ensure_user_ca_trust(&self.ca_cert_path())?))
    }

    #[cfg(not(target_os = "macos"))]
    fn ensure_desktop_ca_trust_step(&self) -> Result<Option<StepDto>> {
        Ok(None)
    }

    #[cfg(target_os = "macos")]
    fn remove_desktop_ca_trust_step(&self) -> Option<StepDto> {
        let cert_path = self.ca_cert_path();

        Some(match macos_remove_user_ca_trust(&cert_path) {
            Ok(step) => step,
            Err(error) => StepDto {
                label: Some("Remove CA trust from your macOS account".into()),
                status: "warning".into(),
                message: format!(
                    "{} removed its system components, but the CA could not be fully removed from your macOS login keychain: {error}",
                    BRANDING.app_name
                ),
            },
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn remove_desktop_ca_trust_step(&self) -> Option<StepDto> {
        None
    }

    fn ensure_config_writable(&self) -> Result<()> {
        if self.config_path.exists() {
            OpenOptions::new()
                .write(true)
                .open(&self.config_path)
                .with_context(|| self.config_write_guidance())?;
            return Ok(());
        }

        let Some(parent) = self.config_path.parent() else {
            bail!(
                "Config path {} has no parent directory.",
                self.config_path.display()
            );
        };

        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create config directory {}.\n{}",
                parent.display(),
                self.config_write_guidance()
            )
        })?;

        let probe_path = parent.join(format!(
            ".{}-write-test-{}",
            BRANDING.slug(),
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe_path)
        {
            Ok(_) => {
                let _ = fs::remove_file(&probe_path);
                Ok(())
            }
            Err(error) => Err(error).with_context(|| self.config_write_guidance()),
        }
    }

    fn config_write_guidance(&self) -> String {
        format!(
            "Config file {} is not writable by the current user.\n\
             {} stores domains and routes in that file.\n\
             Fix the file permissions, or launch the desktop app with \
             {} pointing to a user-writable config file.",
            self.config_path.display(),
            BRANDING.desktop_name,
            BRANDING.env_var("CONFIG_PATH")
        )
    }
}

fn parse_pattern(request: &DomainPatternRequest) -> Result<DomainPattern> {
    DomainPattern::from_name(&request.domain, request.wildcard).map_err(Into::into)
}

fn validate_settings_export_request(request: &SettingsExportRequest) -> Result<()> {
    let path = transfer_file_path(&request.file_path)?;
    if path.is_dir() {
        bail!("Export path '{}' is a directory.", path.display());
    }

    match request.mode.as_str() {
        SETTINGS_TRANSFER_SCOPE_ALL => Ok(()),
        SETTINGS_TRANSFER_SCOPE_PROJECTS => {
            if request
                .project_ids
                .iter()
                .all(|project_id| project_id.trim().is_empty())
            {
                bail!("Select at least one project to export.");
            }
            Ok(())
        }
        other => bail!("Unsupported export mode '{other}'."),
    }
}

fn validate_settings_import_request(
    request: &SettingsImportRequest,
    snapshot: &SettingsTransferSnapshotDto,
) -> Result<()> {
    match request.mode.as_str() {
        SETTINGS_IMPORT_MODE_REPLACE_ALL => {
            if snapshot.settings.is_none() {
                bail!(
                    "This file does not contain the portable global settings needed for a full import."
                );
            }
            Ok(())
        }
        SETTINGS_IMPORT_MODE_PROJECTS => {
            if request
                .project_ids
                .iter()
                .all(|project_id| project_id.trim().is_empty())
            {
                bail!("Select at least one project to import.");
            }
            Ok(())
        }
        other => bail!("Unsupported import mode '{other}'."),
    }
}

fn validate_settings_snapshot(snapshot: &SettingsTransferSnapshotDto) -> Result<()> {
    if snapshot.version != SETTINGS_TRANSFER_VERSION {
        bail!(
            "Unsupported settings export version '{}'.",
            snapshot.version
        );
    }

    if !matches!(
        snapshot.scope.as_str(),
        SETTINGS_TRANSFER_SCOPE_ALL | SETTINGS_TRANSFER_SCOPE_PROJECTS
    ) {
        bail!("Unsupported export scope '{}'.", snapshot.scope);
    }

    let mut project_ids = HashSet::new();
    let mut domain_keys = HashSet::new();

    for project in &snapshot.projects {
        let expected_id = normalize_project_snapshot_id(&project.name)?;
        if project.id.trim().is_empty() {
            bail!("Exported project IDs cannot be empty.");
        }
        if project.id != expected_id {
            bail!(
                "Exported project '{}' has an invalid ID '{}'.",
                project.name,
                project.id
            );
        }
        if !project_ids.insert(project.id.as_str()) {
            bail!(
                "Project '{}' appears more than once in the export file.",
                project.id
            );
        }

        for domain in &project.domains {
            validate_snapshot_domain(domain)?;
            let key = snapshot_domain_key(&domain.domain, domain.wildcard);
            if !domain_keys.insert(key.clone()) {
                bail!(
                    "Domain '{}' appears more than once in the export file.",
                    key
                );
            }
        }
    }

    for domain in &snapshot.standalone_domains {
        validate_snapshot_domain(domain)?;
        let key = snapshot_domain_key(&domain.domain, domain.wildcard);
        if !domain_keys.insert(key.clone()) {
            bail!(
                "Domain '{}' appears more than once in the export file.",
                key
            );
        }
    }

    Ok(())
}

fn validate_snapshot_domain(domain: &SettingsTransferDomainDto) -> Result<()> {
    if domain.domain.trim().is_empty() {
        bail!("Exported domains cannot be empty.");
    }
    if domain.routes.is_empty() {
        bail!("Domain '{}' has no routes to import.", domain.domain);
    }
    if domain.routes.iter().any(|route| route.trim().is_empty()) {
        bail!("Domain '{}' has an empty route entry.", domain.domain);
    }
    Ok(())
}

fn validate_settings_snapshot_targets(snapshot: &SettingsTransferSnapshotDto) -> Result<()> {
    for domain in snapshot.standalone_domains.iter().chain(
        snapshot
            .projects
            .iter()
            .flat_map(|project| project.domains.iter()),
    ) {
        for route in &domain.routes {
            validate_import_route(route)?;
        }
    }
    Ok(())
}

fn validate_import_route(route: &str) -> Result<()> {
    let parsed = parse_desktop_route(route)?;
    if let RouteTarget::StaticFiles(path) = parsed.target()
        && !path.is_dir()
    {
        bail!(
            "Static files route '{}' points to '{}' but that folder does not exist on this machine.",
            route,
            path.display()
        );
    }
    Ok(())
}

fn ensure_snapshot_within_license_limits(
    licensing: &crate::desktop::dto::LicensingDto,
    snapshot: &SettingsTransferSnapshotDto,
) -> Result<()> {
    if let Some(max_projects) = licensing.limits.max_projects
        && snapshot.projects.len() > max_projects
    {
        bail!(
            "This import contains {} projects, but the current {} plan allows up to {}.",
            snapshot.projects.len(),
            licensing.limits.plan,
            max_projects
        );
    }

    if let Some(max_domains_per_project) = licensing.limits.max_domains_per_project {
        for project in &snapshot.projects {
            if project.domains.len() > max_domains_per_project {
                bail!(
                    "Project '{}' contains {} domains, but the current {} plan allows up to {} domains per project.",
                    project.name,
                    project.domains.len(),
                    licensing.limits.plan,
                    max_domains_per_project
                );
            }
        }
    }

    Ok(())
}

fn ensure_project_import_within_license_limits(
    api: &DesktopApi,
    licensing: &crate::desktop::dto::LicensingDto,
    projects: &[SettingsTransferProjectDto],
) -> Result<()> {
    if let Some(max_projects) = licensing.limits.max_projects {
        let existing_project_ids = api
            .list_projects()?
            .into_iter()
            .map(|project| project.id)
            .collect::<HashSet<_>>();
        let replacing_project_ids = projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<HashSet<_>>();
        let retained_project_count = existing_project_ids
            .iter()
            .filter(|project_id| !replacing_project_ids.contains(project_id.as_str()))
            .count();
        let final_project_count = retained_project_count + projects.len();

        if final_project_count > max_projects {
            bail!(
                "This import would leave {} projects configured, but the current {} plan allows up to {}.",
                final_project_count,
                licensing.limits.plan,
                max_projects
            );
        }
    }

    if let Some(max_domains_per_project) = licensing.limits.max_domains_per_project {
        for project in projects {
            if project.domains.len() > max_domains_per_project {
                bail!(
                    "Project '{}' contains {} domains, but the current {} plan allows up to {} domains per project.",
                    project.name,
                    project.domains.len(),
                    licensing.limits.plan,
                    max_domains_per_project
                );
            }
        }
    }

    Ok(())
}

fn transfer_file_path(file_path: &str) -> Result<PathBuf> {
    let trimmed = file_path.trim();
    if trimmed.is_empty() {
        bail!("File path is required.");
    }

    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        bail!("File path '{}' must be absolute.", path.display());
    }

    Ok(path)
}

fn import_file_path(file_path: &str) -> Result<PathBuf> {
    let path = transfer_file_path(file_path)?;
    if !path.is_file() {
        bail!("Import file '{}' was not found.", path.display());
    }
    Ok(path)
}

fn read_settings_snapshot(path: &Path) -> Result<SettingsTransferSnapshotDto> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read settings export '{}'.", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse settings export '{}'.", path.display()))
}

fn write_settings_snapshot(path: &Path, snapshot: &SettingsTransferSnapshotDto) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create the export directory '{}'.",
                parent.display()
            )
        })?;
    }

    let content = serde_json::to_string_pretty(snapshot)
        .context("Failed to serialize the settings export file")?;
    fs::write(path, format!("{content}\n"))
        .with_context(|| format!("Failed to write settings export '{}'.", path.display()))
}

fn export_domain_snapshot(registration: RegistrationDto) -> Result<SettingsTransferDomainDto> {
    Ok(SettingsTransferDomainDto {
        domain: registration.domain,
        wildcard: registration.wildcard,
        routes: registration
            .routes
            .into_iter()
            .map(export_route_string)
            .collect::<Result<Vec<_>>>()?,
    })
}

fn init_project_service_config(
    service: &crate::desktop::dto::InitProjectServiceRequest,
) -> Option<ProjectServiceConfig> {
    let name = service.service_name.as_ref()?.trim();
    if name.is_empty() {
        return None;
    }

    Some(ProjectServiceConfig {
        name: name.to_string(),
        primary_domain: service.domain.clone(),
        domains: Vec::new(),
        path: service.service_path.clone(),
        command: service.command.clone(),
    })
}

fn export_route_string(route: RouteDto) -> Result<String> {
    let path = route.path.trim();
    let target = route.target.trim();

    if path.is_empty() || target.is_empty() {
        bail!("Cannot export an empty route.");
    }

    if path == "/" {
        return Ok(format!("/={target}"));
    }

    Ok(format!("{path}={target}"))
}

fn select_snapshot_projects(
    snapshot: &SettingsTransferSnapshotDto,
    selected_project_ids: &[String],
) -> Result<Vec<SettingsTransferProjectDto>> {
    let selected_ids = selected_project_ids
        .iter()
        .map(|project_id| project_id.trim())
        .filter(|project_id| !project_id.is_empty())
        .collect::<HashSet<_>>();
    let projects = snapshot
        .projects
        .iter()
        .filter(|project| selected_ids.contains(project.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    if projects.len() != selected_ids.len() {
        let available = snapshot
            .projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<HashSet<_>>();
        let missing = selected_ids
            .iter()
            .filter(|project_id| !available.contains(**project_id))
            .copied()
            .collect::<Vec<_>>();
        bail!(
            "The export file does not contain project IDs: {}.",
            missing.join(", ")
        );
    }

    Ok(projects)
}

fn ensure_project_import_has_no_conflicts(
    api: &DesktopApi,
    projects: &[SettingsTransferProjectDto],
) -> Result<()> {
    let replacing_projects = projects
        .iter()
        .map(|project| project.id.as_str())
        .collect::<HashSet<_>>();
    let imported_domains = projects
        .iter()
        .flat_map(|project| {
            project.domains.iter().map(move |domain| {
                (
                    snapshot_domain_key(&domain.domain, domain.wildcard),
                    (project.id.as_str(), project.name.as_str()),
                )
            })
        })
        .collect::<HashMap<_, _>>();

    for current in api.list_domains()? {
        let key = snapshot_domain_key(&current.registration.domain, current.registration.wildcard);
        let Some(&(import_project_id, import_project_name)) = imported_domains.get(&key) else {
            continue;
        };

        if current.project_id.as_deref() == Some(import_project_id) {
            continue;
        }

        if current
            .project_id
            .as_deref()
            .is_some_and(|project_id| replacing_projects.contains(project_id))
        {
            continue;
        }

        let current_owner = current
            .project_name
            .clone()
            .unwrap_or_else(|| "an ungrouped domain".into());
        bail!(
            "Domain '{}' already belongs to {}. Remove it first, or import the matching project instead of '{}'.",
            current.registration.pattern,
            current_owner,
            import_project_name
        );
    }

    Ok(())
}

fn import_standalone_domains(
    api: &DesktopApi,
    domains: &[SettingsTransferDomainDto],
) -> Result<usize> {
    for domain in domains {
        api.register_domain(RegisterDomainRequest {
            domain: domain.domain.clone(),
            wildcard: domain.wildcard,
            routes: domain.routes.clone(),
            project_id: None,
        })?;
    }

    Ok(domains.len())
}

fn import_projects(
    api: &DesktopApi,
    projects: &[SettingsTransferProjectDto],
) -> Result<(usize, usize)> {
    let mut imported_domains = 0;
    let mut skipped_root_paths = 0;

    for project in projects {
        let created = api.create_project_with_domains(CreateProjectWithDomainsRequest {
            name: project.name.clone(),
            domains: project
                .domains
                .iter()
                .map(|domain| crate::desktop::dto::CreateProjectDomainRequest {
                    domain: domain.domain.clone(),
                    wildcard: domain.wildcard,
                    routes: domain.routes.clone(),
                })
                .collect(),
        })?;

        if !project.enabled {
            api.update_project(UpdateProjectRequest {
                id: created.id.clone(),
                name: None,
                enabled: Some(false),
                root_path: None,
            })?;
        }

        if sync_imported_project_root_path(api, &created.id, project.root_path.as_deref())? {
            skipped_root_paths += 1;
        }

        imported_domains += project.domains.len();
    }

    Ok((imported_domains, skipped_root_paths))
}

fn sync_imported_project_root_path(
    api: &DesktopApi,
    project_id: &str,
    root_path: Option<&str>,
) -> Result<bool> {
    let Some(root_path) = root_path
        .map(str::trim)
        .filter(|root_path| !root_path.is_empty())
    else {
        api.remove_project_root_path(project_id)?;
        return Ok(false);
    };

    let path = Path::new(root_path);
    if path.is_absolute() && path.is_dir() {
        api.store_project_root_path(project_id, path)?;
        return Ok(false);
    }

    api.remove_project_root_path(project_id)?;
    Ok(true)
}

fn reload_daemon_if_running(api: &DesktopApi, daemon_was_running: bool) -> (bool, Option<String>) {
    if !daemon_was_running {
        return (false, None);
    }

    match api.reload_daemon(StartDaemonRequest { verbose: false }) {
        Ok(_) => (true, None),
        Err(error) => (false, Some(error.to_string())),
    }
}

fn snapshot_domain_key(domain: &str, wildcard: bool) -> String {
    format!("{}:{}", wildcard, domain.trim().to_lowercase())
}

fn normalize_project_snapshot_id(name: &str) -> Result<String> {
    let mut slug = String::new();
    let mut previous_was_dash = false;

    for ch in name.trim().chars().flat_map(|ch| ch.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_was_dash = false;
            continue;
        }

        if slug.is_empty() || previous_was_dash {
            continue;
        }

        slug.push('-');
        previous_was_dash = true;
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        bail!("Project names must contain letters or numbers.");
    }

    Ok(slug)
}

fn rollback_created_project(
    config_store: &ConfigStore,
    unregister_domain: &UnregisterDomain<'_>,
    project_id: &str,
    created_patterns: &[DomainPattern],
) -> Result<()> {
    for pattern in created_patterns.iter().rev() {
        unregister_domain.execute(pattern).with_context(|| {
            format!("Failed to roll back domain '{}'", pattern.display_pattern())
        })?;
    }

    config_store
        .remove_project(project_id)
        .with_context(|| format!("Failed to roll back project '{project_id}'"))?;

    Ok(())
}

fn parse_desktop_route(route: &str) -> Result<Route> {
    if route.contains('=') {
        return Route::parse(route).map_err(Into::into);
    }

    let path = PathPrefix::new("/")?;
    let target = RouteTarget::parse(route)?;
    Ok(Route::new(path, target))
}

fn normalize_desktop_route(route: &str) -> Result<String> {
    let trimmed = route.trim();
    if trimmed.contains('=') {
        Route::parse(trimmed).map_err(|error| anyhow::anyhow!("Invalid route: {error}"))?;
        return Ok(trimmed.to_string());
    }

    RouteTarget::parse(trimmed)
        .map_err(|error| anyhow::anyhow!("Invalid route target: {error}"))?;
    Ok(format!("/={trimmed}"))
}

fn command_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
        (true, true) => String::new(),
    }
}

fn cli_failure_message(output: &std::process::Output) -> String {
    let message = command_output(output);
    if message.is_empty() {
        format!(
            "{} CLI command failed without diagnostic output.",
            BRANDING.app_name
        )
    } else {
        message
    }
}

fn daemon_start_failure_message(status: std::process::ExitStatus, log_path: &Path) -> String {
    let exit_detail = status
        .code()
        .map(|code| format!(" with exit code {code}"))
        .unwrap_or_else(|| " after receiving a termination signal".into());
    let log_excerpt = startup_log_excerpt(log_path);

    if log_excerpt.is_empty() {
        return format!(
            "{} daemon exited during startup{}.\nNo startup logs were written yet. Run `{} start --foreground` in a terminal to inspect the failure directly.",
            BRANDING.app_name,
            exit_detail,
            BRANDING.cli_command()
        );
    }

    format!(
        "{} daemon exited during startup{}.\nRecent log output:\n{}",
        BRANDING.app_name, exit_detail, log_excerpt
    )
}

fn daemon_start_timeout_message(log_path: &Path) -> String {
    let log_excerpt = startup_log_excerpt(log_path);

    if log_excerpt.is_empty() {
        return format!(
            "{} did not become ready within 3 seconds.\nRun `{} start --foreground` in a terminal to inspect the startup failure directly.",
            BRANDING.app_name,
            BRANDING.cli_command()
        );
    }

    format!(
        "{} did not become ready within 3 seconds.\nRecent log output:\n{}",
        BRANDING.app_name, log_excerpt
    )
}

fn startup_log_excerpt(log_path: &Path) -> String {
    if !log_path.exists() {
        return String::new();
    }

    tail_lines(log_path, 20)
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(target_os = "macos")]
fn is_missing_macos_certificate_error(message: &str) -> bool {
    message.contains("could not be found")
        || message.contains("The specified item could not be found")
        || message.contains("The certificate was not found")
        || message.contains("Unable to delete certificate matching")
}

fn cli_shell_command(binary_path: &Path, config_path: &Path, args: &[String]) -> String {
    cli_shell_command_parts(binary_path, config_path, args).join(" ")
}

fn cli_shell_commands(binary_path: &Path, config_path: &Path, commands: &[Vec<String>]) -> String {
    if let [args] = commands {
        return cli_shell_command(binary_path, config_path, args);
    }

    commands
        .iter()
        .map(|args| cli_shell_command_parts(binary_path, config_path, args).join(" "))
        .collect::<Vec<_>>()
        .join(" && ")
}

fn cli_shell_command_parts(binary_path: &Path, config_path: &Path, args: &[String]) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(profile) = BRANDING.active_profile() {
        parts.push(format!(
            "{}={}",
            BRANDING.profile_env(),
            shell_escape(&profile)
        ));
    }

    parts.extend([
        shell_escape(&binary_path.to_string_lossy()),
        shell_escape("--config"),
        shell_escape(&config_path.to_string_lossy()),
    ]);
    parts.extend(args.iter().map(|arg| shell_escape(arg)));
    parts
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn cli_command_points_to_binary(command_path: &Path, binary_path: &Path) -> bool {
    match (
        fs::canonicalize(command_path),
        fs::canonicalize(binary_path),
    ) {
        (Ok(command_target), Ok(binary_target)) => command_target == binary_target,
        _ => false,
    }
}

fn ensure_cli_command_link(command_path: &Path, binary_path: &Path) -> Result<()> {
    if cli_command_points_to_binary(command_path, binary_path) {
        return Ok(());
    }

    if let Ok(metadata) = fs::symlink_metadata(command_path) {
        if metadata.file_type().is_symlink() {
            fs::remove_file(command_path).with_context(|| {
                format!(
                    "Failed to replace the existing {} CLI link at {}",
                    BRANDING.app_name,
                    command_path.display()
                )
            })?;
        } else {
            bail!(
                "Cannot install the `{}` command because {} already exists and is not a symlink managed by {}.\nRemove or rename that file, then try again.",
                BRANDING.cli_command(),
                command_path.display(),
                BRANDING.app_name,
            );
        }
    }

    if let Some(parent) = command_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create the CLI install directory at {}",
                parent.display()
            )
        })?;
    }

    symlink(binary_path, command_path).with_context(|| {
        format!(
            "Failed to link `{}` to {}",
            command_path.display(),
            binary_path.display(),
        )
    })?;

    Ok(())
}

enum CliRemovalOutcome {
    Removed(String),
    Missing,
    Unmanaged(String),
}

fn remove_cli_command_link(command_path: &Path, binary_path: &Path) -> Result<CliRemovalOutcome> {
    let metadata = match fs::symlink_metadata(command_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CliRemovalOutcome::Missing);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to inspect the installed `{}` command at {}",
                    BRANDING.cli_command(),
                    command_path.display()
                )
            });
        }
    };

    if !metadata.file_type().is_symlink() {
        return Ok(CliRemovalOutcome::Unmanaged(
            command_path.display().to_string(),
        ));
    }

    if !cli_command_points_to_binary(command_path, binary_path) {
        return Ok(CliRemovalOutcome::Unmanaged(
            command_path.display().to_string(),
        ));
    }

    fs::remove_file(command_path).with_context(|| {
        format!(
            "Failed to remove the installed `{}` command at {}",
            BRANDING.cli_command(),
            command_path.display()
        )
    })?;

    Ok(CliRemovalOutcome::Removed(
        command_path.display().to_string(),
    ))
}

fn map_cli_access_step(label: Option<String>, result: Result<CliAccessDto>) -> StepDto {
    match result {
        Ok(access) if access.already_ready => StepDto {
            label,
            status: "skipped".into(),
            message: format!(
                "The `{}` command was already available at {}.",
                BRANDING.cli_command(),
                access.command_path,
            ),
        },
        Ok(access) => StepDto {
            label,
            status: "success".into(),
            message: format!(
                "Installed `{}` at {}.",
                BRANDING.cli_command(),
                access.command_path,
            ),
        },
        Err(error) => StepDto {
            label,
            status: "warning".into(),
            message: format!(
                "{} finished installing, but the `{}` terminal command could not be enabled: {error}",
                BRANDING.app_name,
                BRANDING.cli_command(),
            ),
        },
    }
}

fn root_ca_verification_step(ca_installed: bool) -> StepDto {
    if ca_installed {
        return StepDto {
            label: Some("Root CA".into()),
            status: "success".into(),
            message: "Root CA is trusted by macOS.".into(),
        };
    }

    StepDto {
        label: Some("Root CA".into()),
        status: "warning".into(),
        message: "Root CA setup finished, but macOS still does not trust it. HTTPS certificates will not work until the CA is trusted.".into(),
    }
}

fn map_node_trust_dto(
    status: node_trust::NodeTrustStatus,
    already_ready: bool,
    last_error: Option<String>,
) -> NodeTrustDto {
    NodeTrustDto {
        ready: status.ready,
        already_ready,
        ca_cert_path: status.ca_cert_path.display().to_string(),
        shell_snippet_path: status.shell_snippet_path.display().to_string(),
        shell_integration_ready: status.shell_integration_ready,
        launchctl_ready: status.launchctl_ready,
        last_error,
    }
}

#[cfg(target_os = "macos")]
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
fn macos_user_ca_is_trusted(cert_path: &Path) -> Result<bool> {
    if !cert_path.exists() {
        return Ok(false);
    }

    let keychain_path = macos_login_keychain_path()?;
    let cert_path = path_as_utf8(cert_path)?;
    let keychain_path = path_as_utf8(&keychain_path)?;
    let output = Command::new("security")
        .args([
            "verify-cert",
            "-c",
            &cert_path,
            "-p",
            "basic",
            "-L",
            "-k",
            &keychain_path,
        ])
        .output()
        .context(format!(
            "Failed to verify the {} CA trust in the current macOS account",
            BRANDING.app_name
        ))?;

    Ok(output.status.success())
}

#[cfg(target_os = "macos")]
fn macos_ensure_user_ca_trust(cert_path: &Path) -> Result<StepDto> {
    if !cert_path.exists() {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: "warning".into(),
            message: format!(
                "The {} CA file does not exist yet, so it could not be trusted by this macOS account.",
                BRANDING.app_name
            ),
        });
    }

    let keychain_path = macos_login_keychain_path()?;
    let removed_stale = match delete_stale_macos_keychain_certificates(&keychain_path, cert_path) {
        Ok(removed) => removed,
        Err(error) => {
            return Ok(StepDto {
                label: Some("Trust CA in your macOS account".into()),
                status: "warning".into(),
                message: format!(
                    "macOS could not remove stale {} CA entries before applying trust: {error}",
                    BRANDING.app_name
                ),
            });
        }
    };

    if macos_keychain_contains_certificate(&keychain_path, cert_path)?
        && macos_user_ca_is_trusted(cert_path)?
    {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: if removed_stale > 0 {
                "success".into()
            } else {
                "skipped".into()
            },
            message: if removed_stale > 0 {
                format!(
                    "Removed stale {} CA entries and verified the current CA trust for this macOS account.",
                    BRANDING.app_name
                )
            } else {
                format!(
                    "The {} CA was already trusted by this macOS account.",
                    BRANDING.app_name
                )
            },
        });
    }

    if let Err(error) = delete_macos_keychain_certificates(&keychain_path) {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: "warning".into(),
            message: format!(
                "macOS could not remove stale {} CA entries before applying trust: {error}",
                BRANDING.app_name
            ),
        });
    }

    let cert_path_value = path_as_utf8(cert_path)?;
    let keychain_path_value = path_as_utf8(&keychain_path)?;
    let output = Command::new("security")
        .args([
            "add-trusted-cert",
            "-r",
            "trustRoot",
            "-k",
            &keychain_path_value,
            &cert_path_value,
        ])
        .output()
        .context(format!(
            "Failed to ask macOS to trust the {} CA for this account",
            BRANDING.app_name
        ))?;

    if !output.status.success() {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: "warning".into(),
            message: format!(
                "macOS did not trust the {} CA for this account: {}",
                BRANDING.app_name,
                cli_failure_message(&output)
            ),
        });
    }

    if !macos_keychain_contains_certificate(&keychain_path, cert_path)?
        || !macos_user_ca_is_trusted(cert_path)?
    {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: "warning".into(),
            message: format!(
                "macOS accepted the {} CA trust request, but verification still failed.",
                BRANDING.app_name
            ),
        });
    }

    Ok(StepDto {
        label: Some("Trust CA in your macOS account".into()),
        status: "success".into(),
        message: format!(
            "The {} CA is trusted by this macOS account.",
            BRANDING.app_name
        ),
    })
}

#[cfg(target_os = "macos")]
fn macos_remove_user_ca_trust(cert_path: &Path) -> Result<StepDto> {
    let keychain_path = macos_login_keychain_path()?;
    let had_trust = macos_user_ca_is_trusted(cert_path)?;

    if cert_path.exists() {
        let cert_path = path_as_utf8(cert_path)?;
        let output = Command::new("security")
            .args(["remove-trusted-cert", &cert_path])
            .output()
            .context(format!(
                "Failed to remove the trusted {} CA from the current macOS account",
                BRANDING.app_name
            ))?;

        if !output.status.success() {
            let message = cli_failure_message(&output);
            let not_found = is_missing_macos_certificate_error(&message);

            if !not_found {
                bail!(
                    "Failed to remove the trusted {} CA from your macOS login keychain: {}",
                    BRANDING.app_name,
                    message
                );
            }
        }
    }

    let removed_certs = delete_macos_keychain_certificates(&keychain_path)?;

    if had_trust || removed_certs > 0 {
        return Ok(StepDto {
            label: Some("Remove CA trust from your macOS account".into()),
            status: "success".into(),
            message: format!(
                "Removed the {} local CA from your macOS login keychain for this account.",
                BRANDING.app_name
            ),
        });
    }

    Ok(StepDto {
        label: Some("Remove CA trust from your macOS account".into()),
        status: "skipped".into(),
        message: format!(
            "No {} CA trust entry was present in your macOS login keychain.",
            BRANDING.app_name
        ),
    })
}

#[cfg(target_os = "macos")]
fn macos_login_keychain_path() -> Result<PathBuf> {
    let output = Command::new("security")
        .arg("login-keychain")
        .output()
        .context("Failed to resolve the current macOS login keychain")?;

    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        let keychain = raw.trim().trim_matches('"');
        if !keychain.is_empty() {
            return Ok(PathBuf::from(keychain));
        }
    }

    Ok(user_home_dir()?.join("Library/Keychains/login.keychain-db"))
}

#[cfg(target_os = "macos")]
fn delete_macos_keychain_certificates(keychain_path: &Path) -> Result<usize> {
    let mut removed = 0;

    for hash in find_macos_keychain_certificate_hashes(keychain_path)? {
        delete_macos_keychain_certificate_hash(keychain_path, &hash)?;
        removed += 1;
    }

    Ok(removed)
}

#[cfg(target_os = "macos")]
fn delete_stale_macos_keychain_certificates(
    keychain_path: &Path,
    cert_path: &Path,
) -> Result<usize> {
    let current_hash = certificate_sha1(cert_path)?;
    let mut removed = 0;

    for hash in find_macos_keychain_certificate_hashes(keychain_path)? {
        if hash != current_hash {
            delete_macos_keychain_certificate_hash(keychain_path, &hash)?;
            removed += 1;
        }
    }

    Ok(removed)
}

#[cfg(target_os = "macos")]
fn macos_keychain_contains_certificate(keychain_path: &Path, cert_path: &Path) -> Result<bool> {
    let current_hash = certificate_sha1(cert_path)?;
    Ok(find_macos_keychain_certificate_hashes(keychain_path)?
        .iter()
        .any(|hash| hash == &current_hash))
}

#[cfg(target_os = "macos")]
fn find_macos_keychain_certificate_hashes(keychain_path: &Path) -> Result<Vec<String>> {
    let keychain_path = path_as_utf8(keychain_path)?;
    let output = Command::new("security")
        .args([
            "find-certificate",
            "-c",
            BRANDING.ca_name().as_str(),
            "-a",
            "-Z",
            &keychain_path,
        ])
        .output()
        .context(format!(
            "Failed to find {} CA entries in the macOS login keychain",
            BRANDING.app_name
        ))?;

    if output.status.success() {
        return Ok(parse_security_sha1_hashes(&String::from_utf8_lossy(
            &output.stdout,
        )));
    }

    let message = cli_failure_message(&output);
    if is_missing_macos_certificate_error(&message) {
        return Ok(Vec::new());
    }

    bail!(
        "Failed to find {} CA entries in your macOS login keychain: {}",
        BRANDING.app_name,
        message
    )
}

#[cfg(target_os = "macos")]
fn delete_macos_keychain_certificate_hash(keychain_path: &Path, hash: &str) -> Result<()> {
    let keychain_path = path_as_utf8(keychain_path)?;
    let output = Command::new("security")
        .args(["delete-certificate", "-Z", hash, &keychain_path])
        .output()
        .context(format!(
            "Failed to remove a stale {} CA from the macOS login keychain",
            BRANDING.app_name
        ))?;

    if output.status.success() {
        return Ok(());
    }

    let message = cli_failure_message(&output);
    if is_missing_macos_certificate_error(&message) {
        return Ok(());
    }

    bail!(
        "Failed to delete the {} CA from your macOS login keychain: {}",
        BRANDING.app_name,
        message
    )
}

#[cfg(target_os = "macos")]
fn certificate_sha1(cert_path: &Path) -> Result<String> {
    let cert_path = path_as_utf8(cert_path)?;
    let output = Command::new("openssl")
        .args(["x509", "-in", &cert_path, "-noout", "-fingerprint", "-sha1"])
        .output()
        .context(format!(
            "Failed to calculate the {} CA fingerprint with openssl",
            BRANDING.app_name
        ))?;

    if output.status.success() {
        return parse_openssl_certificate_sha1(&String::from_utf8_lossy(&output.stdout));
    }

    bail!(
        "Failed to calculate the {} CA fingerprint: {}",
        BRANDING.app_name,
        cli_failure_message(&output)
    )
}

#[cfg(target_os = "macos")]
fn parse_security_sha1_hashes(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("SHA-1 hash:"))
        .map(normalize_certificate_hash)
        .filter(|hash| !hash.is_empty())
        .collect()
}

#[cfg(target_os = "macos")]
fn parse_openssl_certificate_sha1(output: &str) -> Result<String> {
    output
        .lines()
        .find_map(|line| line.split_once('=').map(|(_, value)| value))
        .map(normalize_certificate_hash)
        .filter(|hash| !hash.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Unexpected openssl fingerprint output: {}", output.trim()))
}

#[cfg(target_os = "macos")]
fn normalize_certificate_hash(value: &str) -> String {
    value.trim().replace(':', "").to_ascii_uppercase()
}

#[cfg(target_os = "macos")]
fn path_as_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(|value| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8: {}", path.display()))
}

fn is_missing_home_error(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .contains("HOME is not set for the current user.")
}

fn map_licensing_status(
    licensing: &crate::config::LicensingConfig,
    project_count: usize,
    domain_count: usize,
    last_error: Option<String>,
    current_machine: Option<&crate::infrastructure::licensing::MachineFingerprint>,
) -> crate::desktop::dto::LicensingDto {
    let resolved = resolve_license_state(licensing);
    let active_license =
        resolved
            .active_license
            .as_ref()
            .map(|license| crate::desktop::dto::ActiveLicenseDto {
                key_masked: mask_license_key(&license.key),
                license_id: license.license_id.clone(),
                name: license.name.clone(),
                expiry: license.expiry.clone(),
                last_validated_at: license.last_validated_at.clone(),
                validation_code: license.validation_code.clone(),
                validation_detail: license.validation_detail.clone(),
                machine_id: license.machine_id.clone(),
                machine_fingerprint: license.machine_fingerprint.clone(),
                machine_name: license.machine_name.clone(),
                machine_hostname: license.machine_hostname.clone(),
                machine_platform: license.machine_platform.clone(),
                response_signature_verified: license.response_signature_verified,
                response_signature_date: license.response_signature_date.clone(),
            });
    let offline_license =
        resolved
            .offline_license
            .as_ref()
            .map(|license| crate::desktop::dto::OfflineLicenseDto {
                algorithm: license.algorithm.clone(),
                issued: licensing
                    .offline_license
                    .as_ref()
                    .and_then(|value| value.issued.clone()),
                file_expiry: license.file_expiry.clone(),
                license_id: Some(license.license_id.clone()),
                name: license.name.clone(),
                license_expiry: license.license_expiry.clone(),
                plan: license.plan.clone(),
                max_projects: license.max_projects,
                max_domains_per_project: license.max_domains_per_project,
            });
    let configured = licensing.keygen.is_configured();
    let status = match resolved.status {
        ResolvedLicenseStatus::NotConfigured => "notConfigured",
        ResolvedLicenseStatus::Unlicensed => "unlicensed",
        ResolvedLicenseStatus::Licensed => "licensed",
        ResolvedLicenseStatus::LicensedOffline => "licensedOffline",
        ResolvedLicenseStatus::Invalid => "invalid",
    };

    crate::desktop::dto::LicensingDto {
        status: status.into(),
        configured,
        api_url: licensing.keygen.effective_api_url().to_string(),
        account: licensing.keygen.effective_account().to_string(),
        product: licensing.keygen.effective_product().map(str::to_owned),
        public_key: licensing.keygen.effective_public_key().map(str::to_owned),
        limits: crate::desktop::dto::LicenseLimitsDto {
            plan: resolved.limits.plan,
            max_projects: resolved.limits.max_projects,
            max_domains_per_project: resolved.limits.max_domains_per_project,
        },
        free_limits: crate::desktop::dto::LicenseLimitsDto {
            plan: "Free".into(),
            max_projects: Some(FREE_MAX_PROJECTS),
            max_domains_per_project: None,
        },
        project_count,
        domain_count,
        last_error: resolved.last_error.or(last_error),
        current_machine: current_machine.map(|machine| crate::desktop::dto::MachineDto {
            fingerprint: machine.fingerprint.clone(),
            name: machine.name.clone(),
            hostname: machine.hostname.clone(),
            platform: machine.platform.clone(),
        }),
        active_license,
        offline_cache_present: licensing.offline_license.is_some(),
        offline_license,
    }
}

fn map_config(config_path: &Path, config: Config) -> AppConfigDto {
    AppConfigDto {
        config_path: config_path.display().to_string(),
        daemon: config.daemon.into(),
        paths: config.paths.into(),
        docker: config.docker.into(),
        agent: config.agent.into(),
    }
}

fn map_agent_skill_status(
    status: crate::agent_skill::AgentSkillStatus,
) -> Result<AgentSkillStatusDto> {
    Ok(AgentSkillStatusDto {
        skill_dir: status.skill_dir.display().to_string(),
        npx_available: status.npx_available,
        detected_agents: status
            .detected_agents
            .into_iter()
            .map(|agent| AgentSkillAgentDto {
                id: agent.id.into(),
                label: agent.label.into(),
                detected: agent.detected,
                installed: agent.installed,
                skills_dir: agent.skills_dir.display().to_string(),
            })
            .collect(),
        install_command: status.install_command,
    })
}

fn map_agent_skill_install_result(
    result: crate::agent_skill::AgentSkillInstallResult,
) -> Result<AgentSkillInstallResultDto> {
    Ok(AgentSkillInstallResultDto {
        status: map_agent_skill_status(result.status)?,
        installed_agents: result.installed_agents,
        stdout: result.stdout,
        stderr: result.stderr,
    })
}

fn map_configured_domain_info(
    value: ConfiguredDomain,
    has_cert: bool,
    cert_trusted: Option<bool>,
    active: bool,
    target_reachable: bool,
    last_accessed_at: Option<u64>,
) -> DomainInfoDto {
    map_domain_info(
        value.registration,
        DomainInfoDetails {
            has_cert,
            cert_trusted,
            project_id: value.project_id,
            project_name: value.project_name,
            project_enabled: value.project_enabled,
            active,
            target_reachable,
            last_accessed_at,
        },
    )
}

struct DomainInfoDetails {
    has_cert: bool,
    cert_trusted: Option<bool>,
    project_id: Option<String>,
    project_name: Option<String>,
    project_enabled: bool,
    active: bool,
    target_reachable: bool,
    last_accessed_at: Option<u64>,
}

fn map_domain_info(
    registration: crate::domain::DomainRegistration,
    details: DomainInfoDetails,
) -> DomainInfoDto {
    DomainInfoDto {
        registration: map_registration(registration),
        has_cert: details.has_cert,
        cert_trusted: details.cert_trusted,
        project_id: details.project_id,
        project_name: details.project_name,
        project_enabled: details.project_enabled,
        active: details.active,
        target_reachable: details.target_reachable,
        last_accessed_at: details.last_accessed_at,
    }
}

fn map_project_info(
    value: ConfiguredProject,
    domain_count: usize,
    has_reachable_domain: bool,
    last_accessed_at: Option<u64>,
) -> ProjectInfoDto {
    ProjectInfoDto {
        id: value.id,
        name: value.name,
        enabled: value.enabled,
        domain_count,
        has_reachable_domain,
        root_path: value.root_path,
        last_accessed_at,
    }
}

fn map_project_service_info(
    value: ConfiguredService,
    config_store: &ConfigStore,
    http_port: u16,
    https_port: u16,
) -> Result<ProjectServiceInfoDto> {
    let public_host = value
        .primary_domain
        .strip_prefix("*.")
        .unwrap_or(&value.primary_domain)
        .to_string();
    let https_enabled = config_store
        .find_registration_for_domain(&value.primary_domain)?
        .map(|registration| registration.is_https_enabled())
        .unwrap_or(false);
    let (scheme, port, default_port) = if https_enabled {
        ("https", https_port, 443)
    } else {
        ("http", http_port, 80)
    };
    let public_domain = if port == default_port {
        public_host
    } else {
        format!("{public_host}:{port}")
    };

    Ok(ProjectServiceInfoDto {
        id: value.id,
        name: value.name,
        project_id: value.project_id,
        project_name: value.project_name,
        project_enabled: value.project_enabled,
        primary_domain: value.primary_domain,
        url: format!("{scheme}://{public_domain}"),
        domains: value.domains,
        path: value.path,
        command: value.command,
        target: value.primary_target.map(|target| target.to_string()),
        inferred: value.inferred,
    })
}

fn map_project_environment_info(value: ConfiguredProjectEnvironment) -> ProjectEnvironmentInfoDto {
    ProjectEnvironmentInfoDto {
        project_id: value.project_id,
        project_name: value.project_name,
        name: value.name,
        slug: value.slug,
        primary: value.primary,
        root_path: value.root_path,
        git_common_dir: value.git_common_dir,
        git_branch: value.git_branch,
        services: value
            .services
            .into_iter()
            .map(|service| ProjectEnvironmentServiceInfoDto {
                service_id: service.service_id,
                domain: service.domain,
            })
            .collect(),
    }
}

#[derive(Debug, Clone)]
struct GitWorktreeInfo {
    root_path: PathBuf,
    head: Option<String>,
    branch: Option<String>,
    bare: bool,
    detached: bool,
}

#[derive(Debug, Clone)]
struct WorktreeGitMetadata {
    common_dir: Option<PathBuf>,
    branch: Option<String>,
}

fn git_worktrees(root: &Path) -> Result<Vec<GitWorktreeInfo>> {
    let output = git_command(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .with_context(|| {
            format!(
                "Failed to discover Git worktrees from '{}'. Make sure Git is installed.",
                root.display()
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "Failed to discover Git worktrees from '{}'.{}",
            root.display(),
            if stderr.is_empty() {
                String::new()
            } else {
                format!(" Git said: {stderr}")
            }
        );
    }

    Ok(parse_git_worktree_porcelain(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_git_worktree_porcelain(value: &str) -> Vec<GitWorktreeInfo> {
    let mut worktrees = Vec::new();
    let mut current: Option<GitWorktreeInfo> = None;

    for line in value.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(worktree) = current.replace(GitWorktreeInfo {
                root_path: PathBuf::from(path),
                head: None,
                branch: None,
                bare: false,
                detached: false,
            }) {
                worktrees.push(worktree);
            }
            continue;
        }

        let Some(worktree) = current.as_mut() else {
            continue;
        };
        if let Some(head) = line.strip_prefix("HEAD ") {
            worktree.head = Some(head.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ") {
            worktree.branch = Some(normalize_git_branch(branch));
        } else if line == "bare" {
            worktree.bare = true;
        } else if line == "detached" {
            worktree.detached = true;
        }
    }

    if let Some(worktree) = current {
        worktrees.push(worktree);
    }

    worktrees
}

fn normalize_git_branch(value: &str) -> String {
    value
        .strip_prefix("refs/heads/")
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn detect_worktree_git_metadata(root: &Path) -> WorktreeGitMetadata {
    WorktreeGitMetadata {
        common_dir: git_common_dir(root),
        branch: git_output(root, &["branch", "--show-current"]),
    }
}

fn git_common_dir(root: &Path) -> Option<PathBuf> {
    git_output(root, &["rev-parse", "--git-common-dir"]).map(|value| {
        let path = PathBuf::from(value);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        fs::canonicalize(&path).unwrap_or(path)
    })
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = git_command(root).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn git_command(root: &Path) -> Command {
    let safe_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut command = Command::new(git_executable());
    command
        .arg("-c")
        .arg(format!("safe.directory={}", safe_root.display()))
        .arg("-C")
        .arg(root);
    command
}

fn git_executable() -> &'static Path {
    for candidate in [
        Path::new("/usr/bin/git"),
        Path::new("/opt/homebrew/bin/git"),
        Path::new("/usr/local/bin/git"),
    ] {
        if candidate.exists() {
            return candidate;
        }
    }

    Path::new("git")
}

fn map_project_worktree(
    project: &ConfiguredProject,
    primary_root: &Path,
    primary_common_dir: &Path,
    environments: &[ConfiguredProjectEnvironment],
    services: &[ConfiguredService],
    used_slugs: &mut HashSet<String>,
    worktree: GitWorktreeInfo,
) -> ProjectWorktreeDto {
    let root_path = fs::canonicalize(&worktree.root_path).unwrap_or(worktree.root_path);
    let git = detect_worktree_git_metadata(&root_path);
    let branch = worktree.branch.clone().or(git.branch.clone());
    let name = worktree_environment_name(branch.as_deref(), &root_path);
    let attached_environment = existing_environment_for_root(environments, &root_path);
    let primary = root_path == primary_root;
    let same_repo = git.common_dir.as_deref() == Some(primary_common_dir);
    let status = if worktree.bare || !same_repo {
        ProjectWorktreeStatusDto::Unavailable
    } else if primary {
        ProjectWorktreeStatusDto::Primary
    } else if attached_environment.is_some() {
        ProjectWorktreeStatusDto::Attached
    } else {
        ProjectWorktreeStatusDto::Available
    };
    let slug = attached_environment
        .as_ref()
        .map(|environment| environment.slug.clone())
        .or_else(|| {
            if matches!(status, ProjectWorktreeStatusDto::Available) {
                unique_environment_slug(&name, &root_path, used_slugs).ok()
            } else {
                slugify(&name)
            }
        })
        .unwrap_or_else(|| stable_agent_slug(&root_path));
    let unavailable_reason = if worktree.bare {
        Some("Bare worktrees cannot be attached.".into())
    } else if !same_repo {
        Some("This checkout belongs to another Git repository.".into())
    } else {
        None
    };

    ProjectWorktreeDto {
        root_path: root_path.display().to_string(),
        name,
        slug,
        status,
        attached_environment_slug: attached_environment.map(|environment| environment.slug.clone()),
        git_branch: branch,
        git_head: worktree.head,
        git_common_dir: git.common_dir.map(|path| path.display().to_string()),
        existing_environment_file_count: existing_environment_file_count(&root_path),
        service_candidates: worktree_service_candidates(project, services, &root_path),
        unavailable_reason,
    }
}

fn worktree_environment_name(branch: Option<&str>, root: &Path) -> String {
    if let Some(branch) = branch.map(str::trim).filter(|branch| !branch.is_empty()) {
        return branch.to_string();
    }

    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| stable_agent_slug(root))
}

fn unique_environment_slug(
    name: &str,
    root: &Path,
    used_slugs: &mut HashSet<String>,
) -> Result<String> {
    let base = slugify(name).unwrap_or_else(|| stable_agent_slug(root));
    if used_slugs.insert(base.clone()) {
        return Ok(base);
    }

    let slug = format!("{}-{}", base, stable_path_suffix(root));
    if used_slugs.insert(slug.clone()) {
        return Ok(slug);
    }

    for index in 2..100 {
        let candidate = format!("{slug}-{index}");
        if used_slugs.insert(candidate.clone()) {
            return Ok(candidate);
        }
    }

    bail!("Could not derive a unique environment slug for '{}'.", name)
}

fn slugify(value: &str) -> Option<String> {
    let mut slug = String::new();
    let mut previous_was_dash = false;

    for ch in value.trim().chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_was_dash = false;
            continue;
        }

        if slug.is_empty() || previous_was_dash {
            continue;
        }

        slug.push('-');
        previous_was_dash = true;
    }

    let slug = slug.trim_matches('-').to_string();
    (!slug.is_empty()).then_some(slug)
}

fn stable_agent_slug(root: &Path) -> String {
    format!("worktree-{}", stable_path_suffix(root))
}

fn stable_path_suffix(root: &Path) -> String {
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    hex_prefix(&digest, 4)
}

fn hex_prefix(bytes: &[u8], count: usize) -> String {
    bytes
        .iter()
        .take(count)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn existing_environment_for_root(
    environments: &[ConfiguredProjectEnvironment],
    root: &Path,
) -> Option<ConfiguredProjectEnvironment> {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    environments.iter().find_map(|environment| {
        let environment_root = environment
            .root_path
            .as_ref()
            .map(PathBuf::from)
            .map(|path| fs::canonicalize(&path).unwrap_or(path))?;
        (environment_root == root).then(|| environment.clone())
    })
}

fn existing_environment_file_count(root: &Path) -> usize {
    let mut count = existing_env_files(root).len();
    for nested_root in discover_nested_environment_file_roots(root) {
        count += existing_env_files(&nested_root).len();
    }
    count
}

fn worktree_service_candidates(
    project: &ConfiguredProject,
    services: &[ConfiguredService],
    root: &Path,
) -> Vec<ProjectWorktreeServiceCandidateDto> {
    let Ok(plan) = detect_folder_init_plan(root, Some(&project.name)) else {
        return Vec::new();
    };
    let existing_ids = services
        .iter()
        .map(|service| normalize_service_identity(&service.id))
        .collect::<HashSet<_>>();
    let existing_paths = services
        .iter()
        .filter_map(|service| service.path.as_deref())
        .map(normalize_service_identity)
        .collect::<HashSet<_>>();

    plan.services
        .into_iter()
        .filter(|service| {
            let id = normalize_service_identity(&service.id);
            let path = service
                .service_path
                .as_deref()
                .map(normalize_service_identity);
            !existing_ids.contains(&id)
                && path
                    .as_ref()
                    .map(|path| !existing_paths.contains(path))
                    .unwrap_or(true)
        })
        .map(|service| ProjectWorktreeServiceCandidateDto {
            id: service.id,
            label: service.label,
            display_path: service.display_path,
            service_path: service.service_path,
        })
        .collect()
}

fn normalize_service_identity(value: &str) -> String {
    value
        .trim()
        .trim_matches('/')
        .to_ascii_lowercase()
        .replace('\\', "/")
}

fn project_root_path(project: &ConfiguredProject) -> Result<PathBuf> {
    project
        .root_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("Project '{}' has no root path.", project.id))
}

fn configured_project_root_is_unusable_for_worktrees(root_path: &str) -> bool {
    let root_path = Path::new(root_path);
    !root_path.exists() || git_common_dir(root_path).is_none()
}

fn resolve_attach_primary_git_root(
    project: &ConfiguredProject,
    worktree_root: &Path,
) -> Result<(PathBuf, PathBuf)> {
    if let Ok(root) = project_root_path(project) {
        let root = fs::canonicalize(&root).unwrap_or(root);
        if let Some(common_dir) = git_common_dir(&root) {
            return Ok((root, common_dir));
        }
    }

    if let Some(root) = infer_primary_worktree_root(worktree_root)
        && let Some(common_dir) = git_common_dir(&root)
    {
        return Ok((root, common_dir));
    }

    bail!(
        "Project '{}' is not inside a Git repository, so Localify cannot attach worktrees. Set the project root to the primary checkout and try again.",
        project.id
    )
}

fn infer_primary_worktree_root(worktree_root: &Path) -> Option<PathBuf> {
    git_worktrees(worktree_root)
        .ok()?
        .into_iter()
        .filter(|worktree| !worktree.bare)
        .map(|worktree| fs::canonicalize(&worktree.root_path).unwrap_or(worktree.root_path))
        .find(|root| git_common_dir(root).is_some())
}

fn environment_service_domains(
    project_id: &str,
    services: &[ConfiguredService],
    environment_slug: &str,
    primary: bool,
) -> Vec<ProjectEnvironmentServiceConfig> {
    let multi_service = services.len() > 1;
    services
        .iter()
        .map(|service| {
            let domain = if primary {
                service.primary_domain.clone()
            } else if multi_service {
                format!(
                    "{}--{}.{}{}",
                    service.id,
                    environment_slug,
                    project_id,
                    BRANDING.domain_suffix()
                )
            } else {
                format!(
                    "{}.{}{}",
                    environment_slug,
                    project_id,
                    BRANDING.domain_suffix()
                )
            };
            ProjectEnvironmentServiceConfig {
                service_id: service.id.clone(),
                domain,
            }
        })
        .collect()
}

fn ensure_environment_domains(
    project_id: &str,
    domains: &[ProjectEnvironmentServiceConfig],
    config_store: &ConfigStore,
    certs: &CertificateService,
) -> Result<Vec<DomainPattern>> {
    let mut missing = Vec::new();
    for service in domains {
        let pattern = DomainPattern::from_name(&service.domain, false)?;
        if let Some(registration) = config_store.get_domain(&pattern)? {
            let owner = config_store.domain_project_id(&pattern)?;
            if owner.as_deref() != Some(project_id) {
                bail!(
                    "Domain '{}' already exists outside project '{}'. Choose another environment name.",
                    service.domain,
                    project_id
                );
            }
            ensure_existing_domain_https(project_id, registration, config_store, certs)?;
            continue;
        }
        missing.push((service.service_id.clone(), pattern));
    }

    let ports = reserve_ports(missing.len())?;
    let register_domain = RegisterDomain::new(config_store, certs);
    let mut created = Vec::new();
    for ((service_id, pattern), port) in missing.into_iter().zip(ports.into_iter()) {
        let route = Route::parse(&format!("/={port}"))
            .with_context(|| format!("Failed to create route for service '{service_id}'"))?;
        match register_domain.execute(pattern.clone(), vec![route]) {
            Ok(result) => {
                if let Err(error) = config_store
                    .update_domain_with_project(result.registration, Some(project_id.into()))
                {
                    rollback_environment_domains(&created, config_store, certs);
                    let _ = UnregisterDomain::new(config_store, certs).execute(&pattern);
                    return Err(error.into());
                }
                created.push(pattern);
            }
            Err(error) => {
                rollback_environment_domains(&created, config_store, certs);
                return Err(error);
            }
        }
    }

    Ok(created)
}

fn configured_project(config_store: &ConfigStore, project_id: &str) -> Result<ConfiguredProject> {
    config_store
        .list_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| anyhow!("Project not found: {project_id}"))
}

fn project_service(
    config_store: &ConfigStore,
    project: &ConfiguredProject,
    service_id: &str,
) -> Result<ConfiguredService> {
    config_store
        .list_services()?
        .into_iter()
        .filter(|service| service.project_id == project.id)
        .find(|service| service.id == service_id || service.name.eq_ignore_ascii_case(service_id))
        .ok_or_else(|| {
            anyhow!(
                "Service '{}' was not found in project '{}'. Run '{} services list' to inspect configured services.",
                service_id,
                project.id,
                BRANDING.cli_command()
            )
        })
}

fn environment_service_mapping(
    config_store: &ConfigStore,
    project: &ConfiguredProject,
    environment_slug: &str,
    service_id: &str,
) -> Result<(String, String)> {
    let environment = config_store
        .list_project_environments(Some(&project.id))?
        .into_iter()
        .find(|environment| environment.slug == environment_slug)
        .ok_or_else(|| {
            anyhow!(
                "Project environment '{environment_slug}' was not found for project '{}'. Create it with: {} project env ensure --project {} --name {environment_slug}",
                project.id,
                BRANDING.cli_command(),
                project.id
            )
        })?;

    environment
        .services
        .iter()
        .find(|mapping| {
            mapping.service_id == service_id || mapping.service_id.eq_ignore_ascii_case(service_id)
        })
        .map(|mapping| (mapping.service_id.clone(), mapping.domain.clone()))
        .ok_or_else(|| {
            anyhow!(
                "Service '{service_id}' is unavailable in environment '{}'. Re-run '{} project env ensure --project {} --name {}'.",
                environment.slug,
                BRANDING.cli_command(),
                project.id,
                environment.slug
            )
        })
}

fn domain_pattern_from_display(value: &str) -> Result<DomainPattern> {
    let value = value.trim();
    let (domain, wildcard) = value
        .strip_prefix("*.")
        .map(|domain| (domain, true))
        .unwrap_or((value, false));
    DomainPattern::from_name(domain, wildcard).map_err(Into::into)
}

/// Reloads the running daemon after a config mutation.
///
/// Returns whether the daemon was reloaded; a daemon that is not running is
/// not an error, but a daemon that fails to reload is.
fn reload_running_daemon(paths: &RoxyPaths) -> Result<bool> {
    match MgmtSocketClient::new(&paths.socket_path).reload() {
        Ok(()) => Ok(true),
        Err(crate::application::ports::DaemonConnectionError::NotRunning) => Ok(false),
        Err(error) => bail!("Failed to reload the running daemon: {error}"),
    }
}

fn environment_display_name(
    requested: Option<String>,
    primary: bool,
    git_branch: Option<&str>,
    root: &Path,
) -> Result<String> {
    if let Some(name) = requested {
        let name = name.trim();
        if name.is_empty() {
            bail!("Environment name cannot be empty");
        }
        return Ok(name.to_string());
    }

    if primary {
        return Ok("Primary".to_string());
    }

    if let Some(branch) = git_branch.filter(|branch| !branch.trim().is_empty()) {
        return Ok(branch.to_string());
    }

    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("Could not infer an environment name. Pass --name <label>."))?;
    Ok(name.to_string())
}

fn materialized_project_services(
    services: &[ConfiguredService],
    root: &Path,
    detected_services: &[crate::project_init::FolderInitServicePlan],
) -> Result<(Vec<ProjectServiceConfig>, usize)> {
    let detected = detected_service_matches(root, detected_services)?;
    let mut materialized_count = 0;
    let project_services = services
        .iter()
        .map(|service| {
            let mut config = project_service_config_from_existing(service);
            if service.inferred
                && let Some(detected) = detected_service_for_existing(service, &detected)
            {
                let next_path = detected.service_path.clone();
                let next_command = detected.command.clone();
                if config.path != next_path || config.command != next_command {
                    materialized_count += 1;
                }
                config.path = next_path;
                config.command = next_command;
            }
            config
        })
        .collect();

    Ok((project_services, materialized_count))
}

struct DetectedServiceMatch<'a> {
    service: &'a crate::project_init::FolderInitServicePlan,
    legacy_target: Option<String>,
}

fn detected_service_matches<'a>(
    root: &Path,
    services: &'a [crate::project_init::FolderInitServicePlan],
) -> Result<Vec<DetectedServiceMatch<'a>>> {
    services
        .iter()
        .map(|service| {
            let package_dir = service
                .service_path
                .as_ref()
                .map(|path| root.join(path))
                .unwrap_or_else(|| root.to_path_buf());
            Ok(DetectedServiceMatch {
                service,
                legacy_target: crate::project_init::detect_current_dir_localify_target(
                    &package_dir,
                )?,
            })
        })
        .collect()
}

fn detected_service_for_existing<'a>(
    service: &ConfiguredService,
    detected: &'a [DetectedServiceMatch<'a>],
) -> Option<&'a crate::project_init::FolderInitServicePlan> {
    service_proxy_port(service)
        .and_then(|port| {
            detected
                .iter()
                .find(|candidate| candidate.legacy_target.as_deref() == Some(port.as_str()))
        })
        .or_else(|| {
            let service_id = normalize_display_path(&service.id).to_ascii_lowercase();
            let service_name = normalize_display_path(&service.name).to_ascii_lowercase();
            detected.iter().find(|candidate| {
                let detected_name =
                    normalize_display_path(&candidate.service.service_name).to_ascii_lowercase();
                detected_name == service_id || detected_name == service_name
            })
        })
        .map(|candidate| candidate.service)
}

fn service_proxy_port(service: &ConfiguredService) -> Option<String> {
    match service.primary_target.as_ref()? {
        RouteTarget::Proxy(target) => Some(target.port().value().to_string()),
        RouteTarget::StaticFiles(_) => None,
    }
}

fn project_service_config_from_existing(service: &ConfiguredService) -> ProjectServiceConfig {
    ProjectServiceConfig {
        name: service.name.clone(),
        primary_domain: service.primary_domain.clone(),
        domains: service
            .domains
            .iter()
            .filter(|domain| domain.as_str() != service.primary_domain)
            .cloned()
            .collect(),
        path: service.path.clone(),
        command: service.command.clone(),
    }
}

fn normalize_display_path(path: &str) -> String {
    path.trim_matches('/').replace('\\', "/")
}

fn ensure_existing_domain_https(
    project_id: &str,
    mut registration: DomainRegistration,
    config_store: &ConfigStore,
    certs: &dyn crate::application::ports::CertificateManager,
) -> Result<()> {
    if registration.is_https_enabled() {
        return Ok(());
    }

    match certs.create_and_install(registration.pattern()) {
        Ok(()) => {
            registration.enable_https();
            config_store.update_domain_with_project(registration, Some(project_id.into()))?;
        }
        Err(error) => {
            eprintln!(
                "  Certificate warning: failed to enable HTTPS for '{}': {error}",
                registration.display_pattern()
            );
        }
    }

    Ok(())
}

fn reserve_ports(count: usize) -> Result<Vec<u16>> {
    let mut listeners = Vec::with_capacity(count);
    let mut ports = Vec::with_capacity(count);
    for _ in 0..count {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .context("Failed to ask the OS for an available local port")?;
        let port = listener
            .local_addr()
            .context("Failed to read the generated local port")?
            .port();
        listeners.push(listener);
        ports.push(port);
    }
    drop(listeners);
    Ok(ports)
}

fn rollback_environment_domains(
    patterns: &[DomainPattern],
    config_store: &ConfigStore,
    certs: &CertificateService,
) {
    let unregister_domain = UnregisterDomain::new(config_store, certs);
    for pattern in patterns {
        let _ = unregister_domain.execute(pattern);
    }
}

fn is_stale_system_helper_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("system helper closed the connection without replying")
        && message.contains("older build")
}

fn is_attach_project_not_git_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("is not inside a Git repository") && message.contains("attach worktrees")
}

fn should_restart_helper_after_attach_error(
    error: &anyhow::Error,
    request: &AttachProjectWorktreeRequest,
) -> bool {
    if is_stale_system_helper_error(error) {
        return true;
    }

    is_attach_project_not_git_error(error) && attach_request_points_to_valid_git_worktree(request)
}

fn attach_request_points_to_valid_git_worktree(request: &AttachProjectWorktreeRequest) -> bool {
    let worktree_root = PathBuf::from(request.root_path.trim());
    if worktree_root.as_os_str().is_empty() {
        return false;
    }
    let worktree_root = fs::canonicalize(&worktree_root).unwrap_or(worktree_root);
    let Some(worktree_common_dir) = git_common_dir(&worktree_root) else {
        return false;
    };

    let primary_common_dir = request
        .primary_root_path
        .as_deref()
        .map(str::trim)
        .filter(|root_path| !root_path.is_empty())
        .and_then(|root_path| git_common_dir(Path::new(root_path)))
        .or_else(|| {
            infer_primary_worktree_root(&worktree_root).and_then(|root| git_common_dir(&root))
        });

    primary_common_dir.as_ref() == Some(&worktree_common_dir)
}

#[derive(Debug, Clone)]
struct EnvironmentFileTarget {
    root: PathBuf,
    path: PathBuf,
    relative_path: PathBuf,
}

#[derive(Debug, Clone)]
struct EnvironmentFileCandidate {
    target: EnvironmentFileTarget,
    root_label: String,
    exists: bool,
}

impl EnvironmentFileCandidate {
    fn into_dto(self) -> EnvironmentFileCandidateDto {
        EnvironmentFileCandidateDto {
            file_name: self
                .target
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| ROOT_ENV_FILE_FALLBACK.into()),
            relative_path: self.target.relative_path.to_string_lossy().into_owned(),
            root_label: self.root_label,
            exists: self.exists,
        }
    }
}

#[derive(Debug, Clone)]
struct EnvironmentFileRoot {
    path: PathBuf,
    fallback_file_name: &'static str,
}

fn resolve_environment_file_target(
    bootstrap: &BootstrapDto,
    project_id: &str,
    environment_slug: &str,
    service_id: Option<&str>,
    relative_path: Option<&str>,
) -> Result<EnvironmentFileTarget> {
    if let Some(relative_path) = relative_path.and_then(normalize_optional_relative_path) {
        let requested = normalize_environment_file_relative_path(relative_path)?;
        let requested_display = requested.display().to_string();
        return environment_file_candidates(bootstrap, project_id, environment_slug, service_id)?
            .into_iter()
            .find(|candidate| candidate.target.relative_path == requested)
            .map(|candidate| candidate.target)
            .ok_or_else(|| {
                anyhow!(
                    "Environment file '{}' is not available for this project environment.",
                    requested_display
                )
            });
    }

    resolve_default_environment_file_target(bootstrap, project_id, environment_slug, service_id)
}

fn resolve_default_environment_file_target(
    bootstrap: &BootstrapDto,
    project_id: &str,
    environment_slug: &str,
    service_id: Option<&str>,
) -> Result<EnvironmentFileTarget> {
    let project_id = project_id.trim();
    let environment_slug = environment_slug.trim();

    if project_id.is_empty() {
        bail!("Project id is required to open an env file.");
    }
    if environment_slug.is_empty() {
        bail!("Environment slug is required to open an env file.");
    }

    let environment = bootstrap
        .project_environments
        .iter()
        .find(|environment| {
            environment.project_id == project_id && environment.slug == environment_slug
        })
        .ok_or_else(|| {
            anyhow!(
                "Project environment '{}' was not found for project '{}'.",
                environment_slug,
                project_id
            )
        })?;
    let environment_root = environment_root_path(environment)?;
    let target_root = match service_id.map(str::trim) {
        Some(service_id) if !service_id.is_empty() => {
            let service = resolve_environment_service(bootstrap, environment, service_id)?;
            service_environment_file_root(&environment_root, service)
        }
        _ => default_environment_file_root(bootstrap, environment, &environment_root),
    };

    let path = env_file_target(&target_root.path, target_root.fallback_file_name);
    let relative_path = path
        .strip_prefix(&environment_root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| {
            path.file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(target_root.fallback_file_name))
        });

    Ok(EnvironmentFileTarget {
        root: target_root.path,
        path,
        relative_path,
    })
}

fn environment_file_candidates(
    bootstrap: &BootstrapDto,
    project_id: &str,
    environment_slug: &str,
    service_id: Option<&str>,
) -> Result<Vec<EnvironmentFileCandidate>> {
    let project_id = project_id.trim();
    let environment_slug = environment_slug.trim();

    if project_id.is_empty() {
        bail!("Project id is required to open an env file.");
    }
    if environment_slug.is_empty() {
        bail!("Environment slug is required to open an env file.");
    }

    let environment = bootstrap
        .project_environments
        .iter()
        .find(|environment| {
            environment.project_id == project_id && environment.slug == environment_slug
        })
        .ok_or_else(|| {
            anyhow!(
                "Project environment '{}' was not found for project '{}'.",
                environment_slug,
                project_id
            )
        })?;
    let environment_root = environment_root_path(environment)?;

    match service_id.and_then(normalize_optional_relative_path) {
        Some(service_id) => {
            let service = resolve_environment_service(bootstrap, environment, service_id)?;
            Ok(service_environment_file_candidates(
                &environment_root,
                service,
            ))
        }
        None => Ok(environment_environment_file_candidates(
            bootstrap,
            environment,
            &environment_root,
        )),
    }
}

fn normalize_optional_relative_path(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() { None } else { Some(value) }
}

fn environment_environment_file_candidates(
    bootstrap: &BootstrapDto,
    environment: &ProjectEnvironmentInfoDto,
    environment_root: &Path,
) -> Vec<EnvironmentFileCandidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    push_environment_file_candidates(
        environment_root,
        environment_root,
        ROOT_ENV_FILE_FALLBACK,
        "Project root".into(),
        &mut candidates,
        &mut seen,
        true,
    );

    for environment_service in &environment.services {
        let Some(service) = bootstrap.services.iter().find(|service| {
            service.project_id == environment.project_id
                && service.id == environment_service.service_id
        }) else {
            continue;
        };

        let service_root = service_env_root(environment_root, service);
        if service_root == environment_root || env_file_target_if_exists(&service_root).is_none() {
            continue;
        }

        push_environment_file_candidates(
            environment_root,
            &service_root,
            SERVICE_ENV_FILE_FALLBACK,
            service.name.clone(),
            &mut candidates,
            &mut seen,
            false,
        );
    }

    for discovered_root in discover_nested_environment_file_roots(environment_root) {
        push_environment_file_candidates(
            environment_root,
            &discovered_root,
            SERVICE_ENV_FILE_FALLBACK,
            environment_file_root_label(environment_root, &discovered_root),
            &mut candidates,
            &mut seen,
            false,
        );
    }

    candidates
}

fn service_environment_file_candidates(
    environment_root: &Path,
    service: &ProjectServiceInfoDto,
) -> Vec<EnvironmentFileCandidate> {
    let service_root = service_env_root(environment_root, service);
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    if service_root != environment_root {
        push_environment_file_candidates(
            environment_root,
            &service_root,
            SERVICE_ENV_FILE_FALLBACK,
            service.name.clone(),
            &mut candidates,
            &mut seen,
            true,
        );
        return candidates;
    }

    if env_file_target_if_exists(environment_root).is_some() {
        push_environment_file_candidates(
            environment_root,
            environment_root,
            SERVICE_ENV_FILE_FALLBACK,
            "Project root".into(),
            &mut candidates,
            &mut seen,
            true,
        );
    }

    for discovered_root in discover_nested_environment_file_roots(environment_root) {
        push_environment_file_candidates(
            environment_root,
            &discovered_root,
            SERVICE_ENV_FILE_FALLBACK,
            environment_file_root_label(environment_root, &discovered_root),
            &mut candidates,
            &mut seen,
            true,
        );
    }

    if candidates.is_empty() {
        push_environment_file_candidates(
            environment_root,
            environment_root,
            SERVICE_ENV_FILE_FALLBACK,
            "Project root".into(),
            &mut candidates,
            &mut seen,
            true,
        );
    }

    candidates
}

fn push_environment_file_candidates(
    environment_root: &Path,
    root: &Path,
    fallback_file_name: &'static str,
    root_label: String,
    candidates: &mut Vec<EnvironmentFileCandidate>,
    seen: &mut HashSet<PathBuf>,
    include_missing_fallback: bool,
) {
    let existing_files = existing_env_files(root);
    let files = if existing_files.is_empty() && include_missing_fallback {
        vec![root.join(fallback_file_name)]
    } else {
        existing_files
    };

    for path in files {
        let relative_path = environment_file_relative_path(environment_root, &path);
        if !seen.insert(relative_path.clone()) {
            continue;
        }
        let exists = path.is_file();

        candidates.push(EnvironmentFileCandidate {
            target: EnvironmentFileTarget {
                root: root.to_path_buf(),
                path,
                relative_path,
            },
            root_label: root_label.clone(),
            exists,
        });
    }
}

fn existing_env_files(root: &Path) -> Vec<PathBuf> {
    ENV_FILE_CANDIDATES
        .iter()
        .map(|file_name| root.join(file_name))
        .filter(|candidate| candidate.is_file())
        .collect()
}

fn environment_file_relative_path(environment_root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(environment_root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| {
            path.file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(ROOT_ENV_FILE_FALLBACK))
        })
}

fn environment_file_root_relative_path(environment_root: &Path, root: &Path) -> String {
    root.strip_prefix(environment_root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

fn environment_file_root_label(environment_root: &Path, root: &Path) -> String {
    environment_file_root_relative_path(environment_root, root)
}

fn normalize_environment_file_relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() {
        bail!("Environment file path must be relative to the project environment.");
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::CurDir => {}
            _ => bail!("Environment file path must stay inside the project environment."),
        }
    }

    if normalized.as_os_str().is_empty() {
        bail!("Environment file path is required.");
    }

    Ok(normalized)
}

fn environment_file_copy_preview(
    bootstrap: &BootstrapDto,
    request: &EnvironmentFileCopyRequest,
) -> Result<EnvironmentFileCopyPreviewDto> {
    let relative_paths = environment_file_copy_relative_paths(bootstrap, request)?;
    let mut items = Vec::with_capacity(relative_paths.len());

    for relative_path in relative_paths {
        let source = environment_file_target_for_relative_path(
            bootstrap,
            &request.project_id,
            &request.source_environment_slug,
            &relative_path,
        )?;
        let target = environment_file_target_for_relative_path(
            bootstrap,
            &request.project_id,
            &request.target_environment_slug,
            &relative_path,
        )?;
        let source_exists = source.path.is_file();
        let target_exists = target.path.exists();
        let (status, message) = if !source_exists {
            (
                EnvironmentFileCopyStatusDto::Blocked,
                Some("Source file does not exist.".into()),
            )
        } else if let Err(error) = ensure_environment_file_copy_target(&target) {
            (
                EnvironmentFileCopyStatusDto::Blocked,
                Some(error.to_string()),
            )
        } else if target_exists && request.overwrite {
            (EnvironmentFileCopyStatusDto::Overwrite, None)
        } else if target_exists {
            (
                EnvironmentFileCopyStatusDto::SkipExists,
                Some("Target file already exists.".into()),
            )
        } else {
            (EnvironmentFileCopyStatusDto::Create, None)
        };

        items.push(EnvironmentFileCopyItemDto {
            file_name: source
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| ROOT_ENV_FILE_FALLBACK.into()),
            relative_path,
            status,
            source_exists,
            target_exists,
            message,
        });
    }

    Ok(environment_file_copy_summary(items))
}

fn environment_file_copy_relative_paths(
    bootstrap: &BootstrapDto,
    request: &EnvironmentFileCopyRequest,
) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let requested = request
        .relative_paths
        .iter()
        .filter_map(|path| normalize_optional_relative_path(path))
        .map(normalize_environment_file_relative_path)
        .collect::<Result<Vec<_>>>()?;
    let paths = if requested.is_empty() {
        environment_file_candidates(
            bootstrap,
            &request.project_id,
            &request.source_environment_slug,
            None,
        )?
        .into_iter()
        .filter(|candidate| candidate.exists)
        .map(|candidate| candidate.target.relative_path)
        .collect::<Vec<_>>()
    } else {
        requested
    };

    Ok(paths
        .into_iter()
        .filter_map(|path| {
            let value = path.to_string_lossy().into_owned();
            seen.insert(value.clone()).then_some(value)
        })
        .collect())
}

fn environment_file_target_for_relative_path(
    bootstrap: &BootstrapDto,
    project_id: &str,
    environment_slug: &str,
    relative_path: &str,
) -> Result<EnvironmentFileTarget> {
    let environment = resolve_project_environment(bootstrap, project_id, environment_slug)?;
    let environment_root = environment_root_path(environment)?;
    let relative_path = normalize_environment_file_relative_path(relative_path)?;
    if !is_supported_environment_file_name(&relative_path) {
        bail!(
            "Environment file '{}' is not one of Localify's supported env file names.",
            relative_path.display()
        );
    }
    let path = environment_root.join(&relative_path);

    Ok(EnvironmentFileTarget {
        root: environment_root,
        path,
        relative_path,
    })
}

fn resolve_project_environment<'a>(
    bootstrap: &'a BootstrapDto,
    project_id: &str,
    environment_slug: &str,
) -> Result<&'a ProjectEnvironmentInfoDto> {
    let project_id = project_id.trim();
    let environment_slug = environment_slug.trim();
    if project_id.is_empty() {
        bail!("Project id is required.");
    }
    if environment_slug.is_empty() {
        bail!("Environment slug is required.");
    }

    bootstrap
        .project_environments
        .iter()
        .find(|environment| {
            environment.project_id == project_id && environment.slug == environment_slug
        })
        .ok_or_else(|| {
            anyhow!(
                "Project environment '{}' was not found for project '{}'.",
                environment_slug,
                project_id
            )
        })
}

fn is_supported_environment_file_name(relative_path: &Path) -> bool {
    relative_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|file_name| ENV_FILE_CANDIDATES.contains(&file_name))
        .unwrap_or(false)
}

fn ensure_environment_file_copy_target(target: &EnvironmentFileTarget) -> Result<()> {
    if target.path.exists() {
        return ensure_existing_environment_file_target(target);
    }

    let root = target.root.canonicalize().with_context(|| {
        format!(
            "Failed to resolve environment folder '{}'",
            target.root.display()
        )
    })?;
    let parent = target
        .path
        .parent()
        .ok_or_else(|| anyhow!("Environment file target has no parent folder."))?;

    if !parent.exists() {
        bail!(
            "Target folder '{}' does not exist in this environment.",
            parent
                .strip_prefix(&target.root)
                .unwrap_or(parent)
                .display()
        );
    }
    let parent = parent.canonicalize().with_context(|| {
        format!(
            "Failed to resolve environment file folder '{}'",
            target.relative_path.display()
        )
    })?;
    if !parent.starts_with(&root) {
        bail!(
            "Environment file '{}' must stay inside the project environment.",
            target.relative_path.display()
        );
    }

    Ok(())
}

fn environment_file_copy_summary(
    items: Vec<EnvironmentFileCopyItemDto>,
) -> EnvironmentFileCopyPreviewDto {
    let mut create_count = 0;
    let mut overwrite_count = 0;
    let mut skip_count = 0;
    let mut blocked_count = 0;

    for item in &items {
        match item.status {
            EnvironmentFileCopyStatusDto::Create => create_count += 1,
            EnvironmentFileCopyStatusDto::Overwrite => overwrite_count += 1,
            EnvironmentFileCopyStatusDto::SkipExists | EnvironmentFileCopyStatusDto::Skipped => {
                skip_count += 1;
            }
            EnvironmentFileCopyStatusDto::Blocked => blocked_count += 1,
            EnvironmentFileCopyStatusDto::Copied => {}
        }
    }

    EnvironmentFileCopyPreviewDto {
        items,
        create_count,
        overwrite_count,
        skip_count,
        blocked_count,
    }
}

fn default_environment_file_root(
    bootstrap: &BootstrapDto,
    environment: &ProjectEnvironmentInfoDto,
    environment_root: &Path,
) -> EnvironmentFileRoot {
    if env_file_target_if_exists(environment_root).is_some() {
        return root_environment_file_root(environment_root);
    }

    let mut service_roots = Vec::new();
    for environment_service in &environment.services {
        let Some(service) = bootstrap.services.iter().find(|service| {
            service.project_id == environment.project_id
                && service.id == environment_service.service_id
        }) else {
            continue;
        };

        let service_root = service_env_root(environment_root, service);
        if service_root == environment_root {
            continue;
        }
        if env_file_target_if_exists(&service_root).is_some() {
            return service_environment_file_root_value(service_root);
        }
        if !service_roots.contains(&service_root) {
            service_roots.push(service_root);
        }
    }

    if let Some(discovered_root) = discover_nested_environment_file_root(environment_root) {
        return service_environment_file_root_value(discovered_root);
    }

    if service_roots.len() == 1 {
        return service_environment_file_root_value(service_roots.remove(0));
    }

    root_environment_file_root(environment_root)
}

fn discover_nested_environment_file_root(environment_root: &Path) -> Option<PathBuf> {
    discover_nested_environment_file_roots(environment_root)
        .into_iter()
        .next()
}

fn discover_nested_environment_file_roots(environment_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    for relative_path in ["apps/web", "apps/app", "web", "app", "frontend", "client"] {
        collect_environment_file_root(
            environment_root,
            &environment_root.join(relative_path),
            &mut roots,
        );
    }

    for workspace_dir in ["apps", "services", "packages"] {
        let workspace_root = environment_root.join(workspace_dir);
        let Ok(entries) = fs::read_dir(&workspace_root) else {
            continue;
        };
        let mut children = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();

        for child in children {
            collect_environment_file_root(environment_root, &child, &mut roots);
        }
    }

    roots
}

fn collect_environment_file_root(
    environment_root: &Path,
    candidate: &Path,
    roots: &mut Vec<PathBuf>,
) {
    if !candidate.is_dir() || !path_stays_inside_root(environment_root, candidate) {
        return;
    }
    if env_file_target_if_exists(candidate).is_none() {
        return;
    }
    if !roots.iter().any(|root| root == candidate) {
        roots.push(candidate.to_path_buf());
    }
}

fn environment_root_path(environment: &ProjectEnvironmentInfoDto) -> Result<PathBuf> {
    let root_path = environment
        .root_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "Project environment '{}' has no local folder linked.",
                environment.slug
            )
        })?;
    let path = PathBuf::from(root_path);

    if !path.is_absolute() {
        bail!(
            "Project environment '{}' path '{}' must be absolute.",
            environment.slug,
            path.display()
        );
    }
    if !path.is_dir() {
        bail!(
            "Project environment '{}' folder '{}' does not exist.",
            environment.slug,
            path.display()
        );
    }

    Ok(path)
}

fn resolve_environment_service<'a>(
    bootstrap: &'a BootstrapDto,
    environment: &ProjectEnvironmentInfoDto,
    service_id: &str,
) -> Result<&'a ProjectServiceInfoDto> {
    if !environment
        .services
        .iter()
        .any(|service| service.service_id == service_id)
    {
        bail!(
            "Service '{}' is not attached to environment '{}'.",
            service_id,
            environment.slug
        );
    }

    bootstrap
        .services
        .iter()
        .find(|service| service.project_id == environment.project_id && service.id == service_id)
        .ok_or_else(|| {
            anyhow!(
                "Service '{}' was not found for project '{}'.",
                service_id,
                environment.project_id
            )
        })
}

fn service_env_root(environment_root: &Path, service: &ProjectServiceInfoDto) -> PathBuf {
    let Some(service_path) = service.path.as_deref().map(str::trim) else {
        return environment_root.to_path_buf();
    };

    if service_path.is_empty() {
        return environment_root.to_path_buf();
    }

    let service_path = Path::new(service_path);
    if service_path.is_absolute() {
        return environment_root.to_path_buf();
    }

    let candidate = environment_root.join(service_path);
    if candidate.is_dir() && path_stays_inside_root(environment_root, &candidate) {
        return candidate;
    }

    environment_root.to_path_buf()
}

fn service_environment_file_root(
    environment_root: &Path,
    service: &ProjectServiceInfoDto,
) -> EnvironmentFileRoot {
    let service_root = service_env_root(environment_root, service);
    if service_root != environment_root {
        return service_environment_file_root_value(service_root);
    }
    if env_file_target_if_exists(environment_root).is_some() {
        return service_environment_file_root_value(service_root);
    }

    discover_nested_environment_file_root(environment_root)
        .map(service_environment_file_root_value)
        .unwrap_or_else(|| service_environment_file_root_value(service_root))
}

fn root_environment_file_root(root: &Path) -> EnvironmentFileRoot {
    EnvironmentFileRoot {
        path: root.to_path_buf(),
        fallback_file_name: ROOT_ENV_FILE_FALLBACK,
    }
}

fn service_environment_file_root_value(root: PathBuf) -> EnvironmentFileRoot {
    EnvironmentFileRoot {
        path: root,
        fallback_file_name: SERVICE_ENV_FILE_FALLBACK,
    }
}

fn path_stays_inside_root(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(candidate) = candidate.canonicalize() else {
        return false;
    };

    candidate.starts_with(root)
}

fn env_file_target(root: &Path, fallback_file_name: &str) -> PathBuf {
    env_file_target_if_exists(root).unwrap_or_else(|| root.join(fallback_file_name))
}

fn env_file_target_if_exists(root: &Path) -> Option<PathBuf> {
    ENV_FILE_CANDIDATES
        .iter()
        .map(|file_name| root.join(file_name))
        .find(|candidate| candidate.is_file())
}

fn read_environment_file_target(target: &EnvironmentFileTarget) -> Result<EnvironmentFileDto> {
    ensure_environment_file_target(target)?;

    let modified_at = environment_file_modified_at(&target.path)?;
    let exists = modified_at.is_some();
    let content = if exists {
        let metadata = fs::metadata(&target.path).with_context(|| {
            format!(
                "Failed to read environment file metadata for '{}'",
                target.relative_path.display()
            )
        })?;
        if metadata.len() > MAX_ENV_FILE_BYTES {
            bail!(
                "Environment file '{}' is too large to edit in Localify.",
                target.relative_path.display()
            );
        }
        fs::read_to_string(&target.path).with_context(|| {
            format!(
                "Failed to read environment file '{}'. Make sure it is valid UTF-8 text.",
                target.relative_path.display()
            )
        })?
    } else {
        String::new()
    };

    Ok(EnvironmentFileDto {
        file_name: target
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| ROOT_ENV_FILE_FALLBACK.into()),
        relative_path: target.relative_path.to_string_lossy().into_owned(),
        exists,
        content,
        modified_at,
    })
}

fn ensure_environment_file_target(target: &EnvironmentFileTarget) -> Result<()> {
    if target.path.exists() {
        return ensure_existing_environment_file_target(target);
    }

    let root = target.root.canonicalize().with_context(|| {
        format!(
            "Failed to resolve environment folder '{}'",
            target.root.display()
        )
    })?;
    let parent = target
        .path
        .parent()
        .ok_or_else(|| anyhow!("Environment file target has no parent folder."))?
        .canonicalize()
        .with_context(|| {
            format!(
                "Failed to resolve environment file folder '{}'",
                target.relative_path.display()
            )
        })?;

    if !parent.starts_with(&root) {
        bail!(
            "Environment file '{}' must stay inside the project environment.",
            target.relative_path.display()
        );
    }

    Ok(())
}

fn ensure_existing_environment_file_target(target: &EnvironmentFileTarget) -> Result<()> {
    let root = target.root.canonicalize().with_context(|| {
        format!(
            "Failed to resolve environment folder '{}'",
            target.root.display()
        )
    })?;
    let file = target.path.canonicalize().with_context(|| {
        format!(
            "Failed to resolve environment file '{}'",
            target.relative_path.display()
        )
    })?;

    if !file.starts_with(&root) {
        bail!(
            "Environment file '{}' must stay inside the project environment.",
            target.relative_path.display()
        );
    }
    if !file.is_file() {
        bail!(
            "Environment file '{}' is not a regular file.",
            target.relative_path.display()
        );
    }

    Ok(())
}

fn environment_file_modified_at(path: &Path) -> Result<Option<u64>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read environment file '{}'", path.display()));
        }
    };
    let modified = metadata
        .modified()
        .with_context(|| format!("Failed to read environment file '{}'", path.display()))?;

    Ok(Some(system_time_millis(modified)?))
}

fn system_time_millis(value: SystemTime) -> Result<u64> {
    let duration = value
        .duration_since(UNIX_EPOCH)
        .context("Environment file modified time is before the Unix epoch.")?;
    u64::try_from(duration.as_millis())
        .context("Environment file modified time is too large to represent.")
}

fn apply_local_project_root_paths_to_environments(
    environments: Vec<ConfiguredProjectEnvironment>,
    local_projects: &HashMap<String, String>,
) -> Vec<ConfiguredProjectEnvironment> {
    environments
        .into_iter()
        .map(|mut environment| {
            if environment.root_path.is_none() {
                environment.root_path = local_projects.get(&environment.project_id).cloned();
            }
            environment
        })
        .collect()
}

fn daemon_domain_accesses(socket_path: &Path) -> HashMap<String, u64> {
    MgmtSocketClient::new(socket_path)
        .list_registration_info()
        .map(|registrations| {
            registrations
                .into_iter()
                .filter_map(|info| {
                    info.last_accessed_at
                        .map(|timestamp| (info.registration.display_pattern(), timestamp))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn latest_accessed_at(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn registration_has_reachable_target(registration: &crate::domain::DomainRegistration) -> bool {
    registration
        .routes()
        .iter()
        .any(|route| is_route_target_reachable(route.target()))
}

fn project_domain_summary(config_store: &ConfigStore, project_id: &str) -> Result<(usize, bool)> {
    Ok(config_store
        .list_configured_domains()?
        .into_iter()
        .filter(|domain| domain.project_id.as_deref() == Some(project_id))
        .fold((0, false), |(count, reachable), domain| {
            (
                count + 1,
                reachable || registration_has_reachable_target(&domain.registration),
            )
        }))
}

fn map_registration(value: crate::domain::DomainRegistration) -> RegistrationDto {
    RegistrationDto {
        pattern: value.display_pattern(),
        domain: value.domain().as_str().to_string(),
        wildcard: value.is_wildcard(),
        https_enabled: value.is_https_enabled(),
        source: value.source().to_string(),
        routes: value.routes().iter().map(map_route).collect(),
    }
}

fn map_route(value: &crate::domain::Route) -> RouteDto {
    let target_kind = match value.target() {
        RouteTarget::Proxy(_) => "proxy",
        RouteTarget::StaticFiles(_) => "staticFiles",
    };

    RouteDto {
        path: value.path().as_str().to_string(),
        target: value.target().to_string(),
        target_kind: target_kind.into(),
    }
}

fn map_port_conflict(value: PortConflictProcess) -> PortConflictDto {
    PortConflictDto {
        protocol: value.kind.as_str().into(),
        port: value.port,
        pid: value.pid,
        process_name: value.process_name,
        user: value.user,
        command: value.command,
    }
}

fn map_step(label: Option<String>, outcome: StepOutcome) -> StepDto {
    match outcome {
        StepOutcome::Success(message) => StepDto {
            label,
            status: "success".into(),
            message,
        },
        StepOutcome::Warning(message) => StepDto {
            label,
            status: "warning".into(),
            message,
        },
        StepOutcome::Skipped(message) => StepDto {
            label,
            status: "skipped".into(),
            message,
        },
    }
}

fn agent_skill_install_step(enabled: bool) -> Option<StepDto> {
    if !enabled {
        return None;
    }

    Some(match crate::agent_skill::install_agent_skill() {
        Ok(result) => StepDto {
            label: Some("Agent skill".into()),
            status: "success".into(),
            message: format!(
                "Installed the Localify skill for {}.",
                result.installed_agents.join(", ")
            ),
        },
        Err(error) => StepDto {
            label: Some("Agent skill".into()),
            status: "skipped".into(),
            message: format!("Automatic skill installation skipped: {error}"),
        },
    })
}

fn restart_daemon_after_certificate_refresh_step_with(
    daemon_was_running: bool,
    restart: impl FnOnce() -> Result<DaemonLaunchDto>,
) -> StepDto {
    if !daemon_was_running {
        return StepDto {
            label: Some("Daemon restart".into()),
            status: "skipped".into(),
            message: "Daemon was not running; refreshed certificates will be used on next start."
                .into(),
        };
    }

    match restart() {
        Ok(launch) => StepDto {
            label: Some("Daemon restart".into()),
            status: "success".into(),
            message: format!(
                "Daemon restarted so HTTPS uses refreshed certificates (PID {}).",
                launch.pid
            ),
        },
        Err(error) => StepDto {
            label: Some("Daemon restart".into()),
            status: "warning".into(),
            message: format!(
                "Refreshed HTTPS certificates, but failed to restart the daemon: {error}. Restart Localify before retrying HTTPS."
            ),
        },
    }
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn tail_lines(path: &Path, line_count: usize) -> Result<String> {
    let content = fs::read_to_string(path).context("Failed to read log file")?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(line_count);
    let selected = lines[start..].join("\n");

    if selected.is_empty() {
        Ok(selected)
    } else {
        Ok(format!("{selected}\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CliRemovalOutcome, DesktopApi, EnvironmentFileTarget, ROOT_ENV_FILE_FALLBACK,
        apply_local_project_root_paths_to_environments, cli_command_points_to_binary,
        cli_shell_command, cli_shell_commands, ensure_cli_command_link,
        ensure_environment_file_target, ensure_existing_domain_https, env_file_target,
        environment_file_modified_at, environment_service_domains, export_route_string,
        git_command, infer_primary_worktree_root, is_stale_system_helper_error,
        normalize_desktop_route, normalize_project_snapshot_id, parse_desktop_route,
        parse_git_worktree_porcelain, read_environment_file_target, remove_cli_command_link,
        resolve_attach_primary_git_root, restart_daemon_after_certificate_refresh_step_with,
        service_env_root, shell_escape, should_restart_helper_after_attach_error, slugify,
        unique_environment_slug, validate_settings_snapshot,
    };
    use crate::config::{RoxyPaths, StoredLicense};
    use crate::desktop::dto::{
        AttachProjectWorktreeRequest, CreateProjectDomainRequest, CreateProjectWithDomainsRequest,
        EnvironmentFileRequest, InitProjectRequest, InitProjectServiceRequest,
        KeygenSettingsUpdateRequest, LogsRequest, ProjectServiceInfoDto,
        RemoveProjectEnvironmentRequest, RemoveProjectRequest, RouteDto,
        SaveEnvironmentFileRequest, SettingsTransferDomainDto, SettingsTransferProjectDto,
        SettingsTransferSnapshotDto,
    };
    use crate::domain::{DomainPattern, DomainRegistration, Route, RouteTarget};
    use crate::infrastructure::config::{
        Config, ConfigStore, ConfiguredProject, ConfiguredProjectEnvironment,
        ProjectEnvironmentConfig, ProjectEnvironmentServiceConfig,
    };
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;
    use tempfile::TempDir;

    #[test]
    fn parse_desktop_route_defaults_bare_target_to_root_path() {
        let route = parse_desktop_route("3000").unwrap();

        assert_eq!(route.path().as_str(), "/");
        assert!(matches!(route.target(), RouteTarget::Proxy(_)));
    }

    #[test]
    fn local_project_roots_fill_missing_environment_root_paths() {
        let mut local_projects = HashMap::new();
        local_projects.insert(
            "project-1".to_string(),
            "/Users/dev/Developer/acme".to_string(),
        );

        let environments = apply_local_project_root_paths_to_environments(
            vec![
                ConfiguredProjectEnvironment {
                    project_id: "project-1".to_string(),
                    project_name: "ACME".to_string(),
                    name: "Primary".to_string(),
                    slug: "primary".to_string(),
                    primary: true,
                    root_path: None,
                    git_common_dir: None,
                    git_branch: None,
                    services: Vec::new(),
                },
                ConfiguredProjectEnvironment {
                    project_id: "project-1".to_string(),
                    project_name: "ACME".to_string(),
                    name: "Worktree".to_string(),
                    slug: "worktree".to_string(),
                    primary: false,
                    root_path: Some("/Users/dev/Developer/acme-worktree".to_string()),
                    git_common_dir: None,
                    git_branch: Some("feature".to_string()),
                    services: Vec::new(),
                },
            ],
            &local_projects,
        );

        assert_eq!(
            environments[0].root_path.as_deref(),
            Some("/Users/dev/Developer/acme")
        );
        assert_eq!(
            environments[1].root_path.as_deref(),
            Some("/Users/dev/Developer/acme-worktree")
        );
    }

    #[test]
    fn configured_project_with_local_root_prefers_explicit_root_hint() {
        let (_temp_dir, api, store) = temp_api();
        let project = store.create_project("ACME").unwrap();

        let configured = api
            .configured_project_with_local_root(&project.id, Some("/Users/dev/Developer/acme/ACME"))
            .unwrap();

        assert_eq!(
            configured.root_path.as_deref(),
            Some("/Users/dev/Developer/acme/ACME")
        );
    }

    #[test]
    fn git_command_marks_checkout_safe_before_chdir() {
        let root = Path::new("/tmp/localify-client-app");
        let args = git_command(root)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args[0], "-c");
        assert_eq!(args[1], "safe.directory=/tmp/localify-client-app");
        assert_eq!(args[2], "-C");
        assert_eq!(args[3], "/tmp/localify-client-app");
    }

    #[test]
    fn attach_primary_git_root_falls_back_when_configured_root_is_not_git() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (primary_root, worktree_root) = create_git_repo_with_worktree(temp_dir.path());
        let non_git_root = temp_dir.path().join("not-git");
        fs::create_dir(&non_git_root).unwrap();
        let project = ConfiguredProject {
            id: "client-app".into(),
            name: "Client App".into(),
            enabled: true,
            root_path: Some(non_git_root.display().to_string()),
        };

        let (resolved_root, common_dir) =
            resolve_attach_primary_git_root(&project, &worktree_root).unwrap();

        assert_eq!(resolved_root, primary_root);
        assert_eq!(common_dir, primary_root.join(".git"));
    }

    #[test]
    fn inferred_primary_worktree_root_keeps_primary_checkout_identifiable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (primary_root, _worktree_root) = create_git_repo_with_worktree(temp_dir.path());

        let resolved_root = infer_primary_worktree_root(&primary_root).unwrap();

        assert_eq!(resolved_root, primary_root);
    }

    #[test]
    fn parse_git_worktree_porcelain_reads_standard_and_detached_entries() {
        let worktrees = parse_git_worktree_porcelain(
            "worktree /Users/dev/Developer/project\n\
             HEAD 1111111111111111111111111111111111111111\n\
             branch refs/heads/main\n\
             \n\
             worktree /Users/dev/.codex/worktrees/feature/project\n\
             HEAD 2222222222222222222222222222222222222222\n\
             detached\n",
        );

        assert_eq!(worktrees.len(), 2);
        assert_eq!(
            worktrees[0].root_path,
            Path::new("/Users/dev/Developer/project")
        );
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(
            worktrees[0].head.as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
        assert!(!worktrees[0].detached);
        assert_eq!(
            worktrees[1].root_path,
            Path::new("/Users/dev/.codex/worktrees/feature/project")
        );
        assert_eq!(worktrees[1].branch, None);
        assert!(worktrees[1].detached);
    }

    #[test]
    fn parse_git_worktree_porcelain_marks_bare_entries() {
        let worktrees = parse_git_worktree_porcelain(
            "worktree /Users/dev/Developer/project.git\n\
             HEAD 3333333333333333333333333333333333333333\n\
             bare\n",
        );

        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].bare);
    }

    #[test]
    fn unique_environment_slug_uses_stable_suffix_for_collisions() {
        let root = Path::new("/Users/dev/.codex/worktrees/clean-project/project");
        let mut used_slugs = HashSet::from(["feature-auth".to_string()]);

        let slug = unique_environment_slug("feature/auth", root, &mut used_slugs).unwrap();

        assert!(slug.starts_with("feature-auth-"));
        assert_eq!(slug.len(), "feature-auth-00000000".len());
        assert!(used_slugs.contains("feature-auth"));
        assert!(used_slugs.contains(&slug));

        let repeated = unique_environment_slug(
            "feature/auth",
            root,
            &mut HashSet::from(["feature-auth".to_string()]),
        )
        .unwrap();
        assert_eq!(slug, repeated);
    }

    #[test]
    fn slugify_keeps_localify_environment_names_readable() {
        assert_eq!(
            slugify("feature/worktree env"),
            Some("feature-worktree-env".into())
        );
        assert_eq!(slugify("  ---  "), None);
    }

    #[test]
    fn stale_system_helper_error_matches_closed_older_helper_message() {
        let error = anyhow::anyhow!(
            "Localify system helper closed the connection without replying. \
             It is likely running an older build."
        );

        assert!(is_stale_system_helper_error(&error));
    }

    #[test]
    fn attach_missing_root_path_error_does_not_restart_helper() {
        let error = anyhow::anyhow!("Project 'acme' has no root path.");
        let request = AttachProjectWorktreeRequest {
            project_id: "acme".into(),
            root_path: "/Users/dev/.codex/worktrees/0815/ACME".into(),
            primary_root_path: None,
        };

        assert!(!should_restart_helper_after_attach_error(&error, &request));
    }

    #[test]
    fn attach_not_git_project_error_does_not_restart_helper() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_root = temp_dir.path().join("project");
        let worktree_root = temp_dir.path().join("worktree");
        fs::create_dir(&project_root).unwrap();
        fs::create_dir(&worktree_root).unwrap();
        let error = anyhow::anyhow!(
            "Project 'acme' is not inside a Git repository, so Localify cannot attach worktrees."
        );
        let request = AttachProjectWorktreeRequest {
            project_id: "acme".into(),
            root_path: worktree_root.display().to_string(),
            primary_root_path: Some(project_root.display().to_string()),
        };

        assert!(!should_restart_helper_after_attach_error(&error, &request));
    }

    #[test]
    fn attach_not_git_project_error_restarts_helper_for_valid_git_worktree() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (primary_root, worktree_root) = create_git_repo_with_worktree(temp_dir.path());
        let error = anyhow::anyhow!(
            "Project 'acme' is not inside a Git repository, so Localify cannot attach worktrees."
        );
        let request = AttachProjectWorktreeRequest {
            project_id: "acme".into(),
            root_path: worktree_root.display().to_string(),
            primary_root_path: Some(primary_root.display().to_string()),
        };

        assert!(should_restart_helper_after_attach_error(&error, &request));
    }

    #[test]
    fn attach_stale_helper_error_restarts_helper_once() {
        let error = anyhow::anyhow!(
            "Localify system helper closed the connection without replying. \
             It is likely running an older build."
        );
        let request = AttachProjectWorktreeRequest {
            project_id: "acme".into(),
            root_path: "/Users/dev/.codex/worktrees/0815/ACME".into(),
            primary_root_path: Some("/Users/dev/Developer/acme/ACME".into()),
        };

        assert!(should_restart_helper_after_attach_error(&error, &request));
    }

    #[test]
    fn worktree_attach_uses_helper_only_for_system_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let user_api = DesktopApi::new(
            temp_dir.path().join("Localify Dev").join("config.toml"),
            temp_dir.path().join("localify"),
        );
        let system_api = DesktopApi::new(
            PathBuf::from("/etc/localify/config.toml"),
            temp_dir.path().join("localify"),
        );

        assert!(!user_api.should_use_system_helper_for_worktree_attach());
        assert!(system_api.should_use_system_helper_for_worktree_attach());
    }

    #[test]
    fn env_file_target_prefers_dotenv_when_it_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join(".env"), "selected").unwrap();
        fs::write(temp_dir.path().join(".env.local"), "ignored").unwrap();

        assert_eq!(
            env_file_target(temp_dir.path(), ROOT_ENV_FILE_FALLBACK),
            temp_dir.path().join(".env")
        );
    }

    #[test]
    fn env_file_target_falls_back_to_missing_env_local() {
        let temp_dir = tempfile::tempdir().unwrap();

        assert_eq!(
            env_file_target(temp_dir.path(), ROOT_ENV_FILE_FALLBACK),
            temp_dir.path().join(".env.local")
        );
        assert!(!temp_dir.path().join(".env.local").exists());
    }

    #[test]
    fn read_environment_file_target_returns_empty_missing_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target = EnvironmentFileTarget {
            root: temp_dir.path().to_path_buf(),
            path: temp_dir.path().join(".env.local"),
            relative_path: Path::new(".env.local").to_path_buf(),
        };

        let result = read_environment_file_target(&target).unwrap();

        assert!(!result.exists);
        assert_eq!(result.file_name, ".env.local");
        assert_eq!(result.relative_path, ".env.local");
        assert_eq!(result.content, "");
        assert_eq!(result.modified_at, None);
        assert!(!temp_dir.path().join(".env.local").exists());
    }

    #[test]
    fn save_environment_file_creates_missing_fallback_on_first_save() {
        let (temp_dir, api, store) = temp_api();
        let root = temp_dir.path().join("client-app");
        fs::create_dir(&root).unwrap();
        let project = store
            .create_project_with_root("Client App", Some(root.to_string_lossy().into_owned()))
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: project.id.clone(),
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap();
        let saved = api
            .save_environment_file(SaveEnvironmentFileRequest {
                project_id: project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
                content: "LOCALIFY_DOMAIN=client.localify\n".into(),
                expected_modified_at: loaded.modified_at,
            })
            .unwrap();

        assert!(saved.exists);
        assert_eq!(saved.relative_path, ".env.local");
        assert_eq!(
            fs::read_to_string(root.join(".env.local")).unwrap(),
            "LOCALIFY_DOMAIN=client.localify\n"
        );
    }

    #[test]
    fn read_environment_file_without_service_uses_existing_service_env() {
        let (temp_dir, api, _store) = temp_api();
        let root = temp_dir.path().join("client-app");
        let service_root = root.join("apps/web");
        fs::create_dir_all(&service_root).unwrap();
        fs::write(service_root.join(".env"), "APP_ENV=web\n").unwrap();

        let result = api
            .init_project(InitProjectRequest {
                name: "Client App".into(),
                root_path: Some(root.display().to_string()),
                services: vec![InitProjectServiceRequest {
                    domain: "client.localify".into(),
                    target: "3000".into(),
                    service_name: Some("web".into()),
                    service_path: Some("apps/web".into()),
                    command: None,
                }],
            })
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: result.project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap();

        assert!(loaded.exists);
        assert_eq!(loaded.file_name, ".env");
        assert_eq!(loaded.relative_path, "apps/web/.env");
        assert_eq!(loaded.content, "APP_ENV=web\n");
    }

    #[test]
    fn read_environment_file_without_service_keeps_root_env_priority() {
        let (temp_dir, api, _store) = temp_api();
        let root = temp_dir.path().join("client-app");
        let service_root = root.join("apps/web");
        fs::create_dir_all(&service_root).unwrap();
        fs::write(root.join(".env"), "APP_ENV=root\n").unwrap();
        fs::write(service_root.join(".env"), "APP_ENV=web\n").unwrap();

        let result = api
            .init_project(InitProjectRequest {
                name: "Client App".into(),
                root_path: Some(root.display().to_string()),
                services: vec![InitProjectServiceRequest {
                    domain: "client.localify".into(),
                    target: "3000".into(),
                    service_name: Some("web".into()),
                    service_path: Some("apps/web".into()),
                    command: None,
                }],
            })
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: result.project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap();

        assert_eq!(loaded.relative_path, ".env");
        assert_eq!(loaded.content, "APP_ENV=root\n");
    }

    #[test]
    fn list_environment_files_includes_root_and_monorepo_candidates() {
        let (temp_dir, api, _store) = temp_api();
        let root = temp_dir.path().join("client-app");
        let web_root = root.join("apps/web");
        let api_root = root.join("packages/api");
        fs::create_dir_all(&web_root).unwrap();
        fs::create_dir_all(&api_root).unwrap();
        fs::write(root.join(".env"), "APP_ENV=root\n").unwrap();
        fs::write(root.join(".env.local"), "APP_ENV=root-local\n").unwrap();
        fs::write(web_root.join(".env"), "APP_ENV=web\n").unwrap();
        fs::write(api_root.join(".env.local"), "APP_ENV=api-local\n").unwrap();

        let result = api
            .init_project(InitProjectRequest {
                name: "Client App".into(),
                root_path: Some(root.display().to_string()),
                services: vec![InitProjectServiceRequest {
                    domain: "client.localify".into(),
                    target: "3000".into(),
                    service_name: Some("web".into()),
                    service_path: Some("apps/web".into()),
                    command: None,
                }],
            })
            .unwrap();

        let files = api
            .list_environment_files(EnvironmentFileRequest {
                project_id: result.project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap()
            .files
            .into_iter()
            .map(|file| file.relative_path)
            .collect::<Vec<_>>();

        assert_eq!(
            files,
            vec![
                ".env".to_string(),
                ".env.local".to_string(),
                "apps/web/.env".to_string(),
                "packages/api/.env.local".to_string()
            ]
        );
    }

    #[test]
    fn read_environment_file_uses_selected_relative_path() {
        let (temp_dir, api, store) = temp_api();
        let root = temp_dir.path().join("client-app");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".env"), "APP_ENV=root\n").unwrap();
        fs::write(root.join(".env.local"), "APP_ENV=root-local\n").unwrap();
        let project = store
            .create_project_with_root("Client App", Some(root.to_string_lossy().into_owned()))
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: Some(".env.local".into()),
            })
            .unwrap();

        assert_eq!(loaded.relative_path, ".env.local");
        assert_eq!(loaded.content, "APP_ENV=root-local\n");
    }

    #[test]
    fn read_environment_file_without_service_discovers_legacy_nested_env() {
        let (temp_dir, api, store) = temp_api();
        let root = temp_dir.path().join("client-app");
        let service_root = root.join("apps/web");
        fs::create_dir_all(&service_root).unwrap();
        fs::write(service_root.join(".env"), "APP_ENV=legacy-web\n").unwrap();
        let project = store
            .create_project_with_root("Client App", Some(root.to_string_lossy().into_owned()))
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap();

        assert!(loaded.exists);
        assert_eq!(loaded.file_name, ".env");
        assert_eq!(loaded.relative_path, "apps/web/.env");
        assert_eq!(loaded.content, "APP_ENV=legacy-web\n");
    }

    #[test]
    fn read_environment_file_without_service_prefers_apps_web_for_legacy_project() {
        let (temp_dir, api, store) = temp_api();
        let root = temp_dir.path().join("client-app");
        let web_root = root.join("apps/web");
        let infra_root = root.join("packages/infra");
        fs::create_dir_all(&web_root).unwrap();
        fs::create_dir_all(&infra_root).unwrap();
        fs::write(web_root.join(".env"), "APP_ENV=web\n").unwrap();
        fs::write(infra_root.join(".env"), "APP_ENV=infra\n").unwrap();
        let project = store
            .create_project_with_root("Client App", Some(root.to_string_lossy().into_owned()))
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap();

        assert_eq!(loaded.relative_path, "apps/web/.env");
        assert_eq!(loaded.content, "APP_ENV=web\n");
    }

    #[test]
    fn read_environment_file_with_legacy_inferred_service_discovers_nested_env() {
        let (temp_dir, api, store) = temp_api();
        let root = temp_dir.path().join("cedre");
        let service_root = root.join("apps/web");
        fs::create_dir_all(&service_root).unwrap();
        fs::write(service_root.join(".env"), "APP_ENV=cedre-web\n").unwrap();
        let project = store
            .create_project_with_root("Cedre", Some(root.to_string_lossy().into_owned()))
            .unwrap();
        store
            .add_domain_with_project(
                DomainRegistration::new(
                    DomainPattern::from_name("cedre.localify", false).unwrap(),
                    vec![Route::parse("/=127.0.0.1:52205").unwrap()],
                ),
                Some(project.id.clone()),
            )
            .unwrap();
        let service_id = api
            .list_project_services()
            .unwrap()
            .into_iter()
            .find(|service| service.project_id == project.id)
            .unwrap()
            .id;

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: project.id,
                environment_slug: "primary".into(),
                service_id: Some(service_id),
                relative_path: None,
            })
            .unwrap();

        assert!(loaded.exists);
        assert_eq!(loaded.file_name, ".env");
        assert_eq!(loaded.relative_path, "apps/web/.env");
        assert_eq!(loaded.content, "APP_ENV=cedre-web\n");
    }

    #[test]
    fn save_environment_file_without_service_uses_single_service_dotenv_fallback() {
        let (temp_dir, api, _store) = temp_api();
        let root = temp_dir.path().join("client-app");
        let service_root = root.join("apps/web");
        fs::create_dir_all(&service_root).unwrap();

        let result = api
            .init_project(InitProjectRequest {
                name: "Client App".into(),
                root_path: Some(root.display().to_string()),
                services: vec![InitProjectServiceRequest {
                    domain: "client.localify".into(),
                    target: "3000".into(),
                    service_name: Some("web".into()),
                    service_path: Some("apps/web".into()),
                    command: None,
                }],
            })
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: result.project.id.clone(),
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap();
        let saved = api
            .save_environment_file(SaveEnvironmentFileRequest {
                project_id: result.project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
                content: "APP_ENV=web\n".into(),
                expected_modified_at: loaded.modified_at,
            })
            .unwrap();

        assert_eq!(loaded.relative_path, "apps/web/.env");
        assert_eq!(saved.relative_path, "apps/web/.env");
        assert_eq!(
            fs::read_to_string(service_root.join(".env")).unwrap(),
            "APP_ENV=web\n"
        );
    }

    #[test]
    fn save_environment_file_rejects_disk_changes_after_open() {
        let (temp_dir, api, store) = temp_api();
        let root = temp_dir.path().join("client-app");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".env.local"), "A=1\n").unwrap();
        let project = store
            .create_project_with_root("Client App", Some(root.to_string_lossy().into_owned()))
            .unwrap();

        let loaded = api
            .read_environment_file(EnvironmentFileRequest {
                project_id: project.id.clone(),
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
            })
            .unwrap();
        let original_modified_at = loaded.modified_at;
        for index in 0..10 {
            std::thread::sleep(Duration::from_millis(5));
            fs::write(root.join(".env.local"), format!("A={index}\n")).unwrap();
            if environment_file_modified_at(&root.join(".env.local")).unwrap()
                != original_modified_at
            {
                break;
            }
        }
        assert_ne!(
            environment_file_modified_at(&root.join(".env.local")).unwrap(),
            original_modified_at
        );

        let error = api
            .save_environment_file(SaveEnvironmentFileRequest {
                project_id: project.id,
                environment_slug: "primary".into(),
                service_id: None,
                relative_path: None,
                content: "A=2\n".into(),
                expected_modified_at: original_modified_at,
            })
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed on disk after it was opened")
        );
    }

    #[test]
    fn environment_file_target_rejects_symlink_outside_root() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("root");
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.env"), "SECRET=1\n").unwrap();
        symlink(outside.join("secret.env"), root.join(".env.local")).unwrap();
        let target = EnvironmentFileTarget {
            root: root.clone(),
            path: root.join(".env.local"),
            relative_path: Path::new(".env.local").to_path_buf(),
        };

        let error = ensure_environment_file_target(&target).unwrap_err();

        assert!(error.to_string().contains("must stay inside"));
    }

    #[test]
    fn service_env_root_uses_existing_relative_service_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let service_root = temp_dir.path().join("apps/web");
        fs::create_dir_all(&service_root).unwrap();
        let service = test_project_service(Some("apps/web"));

        assert_eq!(service_env_root(temp_dir.path(), &service), service_root);
    }

    #[test]
    fn service_env_root_falls_back_when_service_path_is_unreliable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("repo");
        let outside = temp_dir.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let service = test_project_service(Some("../outside"));

        assert_eq!(service_env_root(&root, &service), root);
    }

    #[test]
    fn parse_desktop_route_preserves_explicit_path_syntax() {
        let route = parse_desktop_route("/api=3001").unwrap();

        assert_eq!(route.path().as_str(), "/api");
        assert!(matches!(route.target(), RouteTarget::Proxy(_)));
    }

    #[test]
    fn normalize_desktop_route_wraps_bare_target_at_root() {
        assert_eq!(normalize_desktop_route("3000").unwrap(), "/=3000");
    }

    #[test]
    fn export_route_string_preserves_root_route_explicitly() {
        let route = export_route_string(RouteDto {
            path: "/".into(),
            target: "3000".into(),
            target_kind: "proxy".into(),
        })
        .unwrap();

        assert_eq!(route, "/=3000");
    }

    #[test]
    fn normalize_project_snapshot_id_matches_project_slug_rules() {
        assert_eq!(
            normalize_project_snapshot_id(" Client App ").unwrap(),
            "client-app"
        );
    }

    #[test]
    fn validate_settings_snapshot_rejects_duplicate_domains() {
        let snapshot = SettingsTransferSnapshotDto {
            version: 1,
            scope: "all".into(),
            settings: None,
            standalone_domains: vec![SettingsTransferDomainDto {
                domain: "demo.localify".into(),
                wildcard: false,
                routes: vec!["/=3000".into()],
            }],
            projects: vec![SettingsTransferProjectDto {
                id: "client-app".into(),
                name: "Client App".into(),
                enabled: true,
                root_path: None,
                domains: vec![SettingsTransferDomainDto {
                    domain: "demo.localify".into(),
                    wildcard: false,
                    routes: vec!["/=3001".into()],
                }],
            }],
        };

        let error = validate_settings_snapshot(&snapshot).unwrap_err();
        assert!(error.to_string().contains("appears more than once"));
    }

    #[test]
    fn shell_escape_quotes_single_quotes_safely() {
        assert_eq!(shell_escape("my'app"), "'my'\"'\"'app'");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_security_sha1_hashes_extracts_hashes() {
        let output = "\
SHA-256 hash: AE97C0D9A5C003B5CC12D1AC0DCF40FB6FF54A54559B537AB9132E1E3055237B
SHA-1 hash: C8:DE:C7:92:EA:38:EF:36:4E:DC:AE:D5:33:41:41:5A:E8:46:0A:D5
keychain: \"/Users/test/Library/Keychains/login.keychain-db\"
SHA-1 hash: EF0D835649DD3E4D1D109862315A09B1EA40C27F
";

        assert_eq!(
            super::parse_security_sha1_hashes(output),
            vec![
                "C8DEC792EA38EF364EDCAED53341415AE8460AD5",
                "EF0D835649DD3E4D1D109862315A09B1EA40C27F"
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_openssl_certificate_sha1_normalizes_hash() {
        let output =
            "sha1 Fingerprint=C8:DE:C7:92:EA:38:EF:36:4E:DC:AE:D5:33:41:41:5A:E8:46:0A:D5\n";

        assert_eq!(
            super::parse_openssl_certificate_sha1(output).unwrap(),
            "C8DEC792EA38EF364EDCAED53341415AE8460AD5"
        );
    }

    #[test]
    fn cli_shell_command_quotes_all_args() {
        let command = cli_shell_command(
            Path::new("/tmp/localify"),
            Path::new("/etc/localify/config.toml"),
            &[
                "register".into(),
                "my app.localify".into(),
                "--route".into(),
                "/=3000".into(),
            ],
        );

        assert_eq!(
            command,
            "'/tmp/localify' '--config' '/etc/localify/config.toml' 'register' 'my app.localify' '--route' '/=3000'"
        );
    }

    #[test]
    fn cli_shell_commands_runs_commands_in_one_admin_session() {
        let command = cli_shell_commands(
            Path::new("/tmp/localify"),
            Path::new("/etc/localify/config.toml"),
            &[
                vec!["install".into()],
                vec![
                    "helper".into(),
                    "uninstall-service".into(),
                    "--uid".into(),
                    "501".into(),
                ],
                vec![
                    "helper".into(),
                    "ensure-running".into(),
                    "--socket".into(),
                    "/var/run/localify-helper-501.sock".into(),
                    "--uid".into(),
                    "501".into(),
                ],
            ],
        );

        assert_eq!(
            command,
            "'/tmp/localify' '--config' '/etc/localify/config.toml' 'install' && '/tmp/localify' '--config' '/etc/localify/config.toml' 'helper' 'uninstall-service' '--uid' '501' && '/tmp/localify' '--config' '/etc/localify/config.toml' 'helper' 'ensure-running' '--socket' '/var/run/localify-helper-501.sock' '--uid' '501'"
        );
    }

    #[test]
    fn ensure_cli_command_link_creates_missing_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("localify");
        let command_path = temp_dir.path().join("bin").join("localify");

        fs::write(&binary_path, "binary").unwrap();
        ensure_cli_command_link(&command_path, &binary_path).unwrap();

        assert!(command_path.exists());
        assert!(cli_command_points_to_binary(&command_path, &binary_path));
    }

    #[test]
    fn ensure_cli_command_link_replaces_stale_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("localify");
        let stale_path = temp_dir.path().join("stale-localify");
        let command_path = temp_dir.path().join("bin").join("localify");

        fs::write(&binary_path, "binary").unwrap();
        fs::write(&stale_path, "stale").unwrap();
        fs::create_dir_all(command_path.parent().unwrap()).unwrap();
        symlink(&stale_path, &command_path).unwrap();

        ensure_cli_command_link(&command_path, &binary_path).unwrap();

        assert!(cli_command_points_to_binary(&command_path, &binary_path));
    }

    #[test]
    fn ensure_cli_command_link_rejects_regular_files() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("localify");
        let command_path = temp_dir.path().join("bin").join("localify");

        fs::write(&binary_path, "binary").unwrap();
        fs::create_dir_all(command_path.parent().unwrap()).unwrap();
        fs::write(&command_path, "occupied").unwrap();

        let error = ensure_cli_command_link(&command_path, &binary_path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("already exists and is not a symlink")
        );
    }

    #[test]
    fn remove_cli_command_link_removes_managed_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("localify");
        let command_path = temp_dir.path().join("bin").join("localify");

        fs::write(&binary_path, "binary").unwrap();
        ensure_cli_command_link(&command_path, &binary_path).unwrap();

        let outcome = remove_cli_command_link(&command_path, &binary_path).unwrap();

        assert!(matches!(outcome, CliRemovalOutcome::Removed(_)));
        assert!(!command_path.exists());
    }

    #[test]
    fn remove_cli_command_link_skips_missing_path() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("localify");
        let command_path = temp_dir.path().join("bin").join("localify");

        fs::write(&binary_path, "binary").unwrap();

        let outcome = remove_cli_command_link(&command_path, &binary_path).unwrap();

        assert!(matches!(outcome, CliRemovalOutcome::Missing));
    }

    #[test]
    fn remove_cli_command_link_skips_unmanaged_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("localify");
        let other_binary_path = temp_dir.path().join("other-localify");
        let command_path = temp_dir.path().join("bin").join("localify");

        fs::write(&binary_path, "binary").unwrap();
        fs::write(&other_binary_path, "other").unwrap();
        fs::create_dir_all(command_path.parent().unwrap()).unwrap();
        symlink(&other_binary_path, &command_path).unwrap();

        let outcome = remove_cli_command_link(&command_path, &binary_path).unwrap();

        assert!(matches!(outcome, CliRemovalOutcome::Unmanaged(_)));
        assert!(command_path.exists());
    }

    #[test]
    fn ensure_config_writable_creates_missing_parent_directory() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("missing").join("config.toml");
        let api = DesktopApi::new_direct(config_path.clone(), temp_dir.path().join("localify"));

        api.ensure_config_writable().unwrap();

        assert!(config_path.parent().unwrap().is_dir());
        assert!(!config_path.exists());
    }

    #[test]
    fn imported_keygen_settings_apply_when_unlicensed() {
        let (_temp_dir, api, store) = temp_api();

        api.apply_imported_keygen_settings(KeygenSettingsUpdateRequest {
            api_url: "https://keygen.example.test".into(),
            account: "account_123".into(),
            product: Some("product_123".into()),
            public_key: Some("public_key_123".into()),
        })
        .unwrap();

        let config = store.load().unwrap();
        assert_eq!(
            config.licensing.keygen.api_url,
            "https://keygen.example.test"
        );
        assert_eq!(config.licensing.keygen.account, "account_123");
        assert_eq!(
            config.licensing.keygen.product.as_deref(),
            Some("product_123")
        );
        assert_eq!(
            config.licensing.keygen.public_key.as_deref(),
            Some("public_key_123")
        );
        assert!(config.licensing.license.is_none());
    }

    #[test]
    fn imported_keygen_settings_preserve_active_license() {
        let (_temp_dir, api, store) = temp_api();

        store
            .store_license(Some(StoredLicense {
                key: "TEST-KEY-1234".into(),
                license_id: "lic_123".into(),
                name: Some("Pro".into()),
                max_projects: Some(10),
                validation_code: Some("VALID".into()),
                ..StoredLicense::default()
            }))
            .unwrap();

        api.apply_imported_keygen_settings(KeygenSettingsUpdateRequest {
            api_url: "https://keygen.example.test".into(),
            account: "account_123".into(),
            product: Some("product_123".into()),
            public_key: Some("public_key_123".into()),
        })
        .unwrap();

        let config = store.load().unwrap();
        assert!(config.licensing.license.is_some());
        assert_ne!(
            config.licensing.keygen.api_url,
            "https://keygen.example.test"
        );

        for index in 1..=6 {
            store.create_project(&format!("Project {index}")).unwrap();
        }
    }

    #[test]
    fn create_project_with_domains_persists_project_and_domains() {
        let (_temp_dir, api, store) = temp_api();

        let project = api
            .create_project_with_domains(CreateProjectWithDomainsRequest {
                name: "Client App".into(),
                domains: vec![
                    CreateProjectDomainRequest {
                        domain: "client.localify".into(),
                        wildcard: false,
                        routes: vec!["3000".into()],
                    },
                    CreateProjectDomainRequest {
                        domain: "api.client.localify".into(),
                        wildcard: false,
                        routes: vec!["3001".into()],
                    },
                ],
            })
            .unwrap();

        assert_eq!(project.name, "Client App");
        assert_eq!(project.domain_count, 2);

        let configured_projects = store.list_projects().unwrap();
        assert_eq!(configured_projects.len(), 1);
        assert_eq!(configured_projects[0].id, project.id);

        let configured_domains = store.list_configured_domains().unwrap();
        assert_eq!(configured_domains.len(), 2);
        assert!(
            configured_domains
                .iter()
                .all(|domain| domain.project_id.as_deref() == Some(project.id.as_str()))
        );
    }

    #[test]
    fn init_project_persists_explicit_services() {
        let (temp_dir, api, store) = temp_api();
        let root = temp_dir.path().join("repo");
        fs::create_dir_all(root.join("apps/web")).unwrap();

        let result = api
            .init_project(InitProjectRequest {
                name: "Client App".into(),
                root_path: Some(root.display().to_string()),
                services: vec![InitProjectServiceRequest {
                    domain: "client-app.localify".into(),
                    target: "3000".into(),
                    service_name: Some("web".into()),
                    service_path: Some("apps/web".into()),
                    command: Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]),
                }],
            })
            .unwrap();

        let services = store.list_services().unwrap();

        assert_eq!(result.services.len(), 1);
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].project_id, result.project.id);
        assert_eq!(services[0].id, "web");
        assert_eq!(services[0].primary_domain, "client-app.localify");
        assert_eq!(services[0].path.as_deref(), Some("apps/web"));
        assert_eq!(
            services[0].command,
            Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()])
        );
        assert!(!services[0].inferred);
    }

    #[test]
    fn create_project_with_domains_rolls_back_on_invalid_domain_route() {
        let (_temp_dir, api, store) = temp_api();

        let error = api
            .create_project_with_domains(CreateProjectWithDomainsRequest {
                name: "Broken App".into(),
                domains: vec![
                    CreateProjectDomainRequest {
                        domain: "broken.localify".into(),
                        wildcard: false,
                        routes: vec!["3000".into()],
                    },
                    CreateProjectDomainRequest {
                        domain: "api.broken.localify".into(),
                        wildcard: false,
                        routes: vec!["not-a-route-target".into()],
                    },
                ],
            })
            .unwrap_err();

        assert!(error.to_string().contains("Invalid route"));
        assert!(store.list_projects().unwrap().is_empty());
        assert!(store.list_configured_domains().unwrap().is_empty());
    }

    #[test]
    fn remove_project_deletes_attached_domains() {
        let (_temp_dir, api, store) = temp_api();

        let project = api
            .create_project_with_domains(CreateProjectWithDomainsRequest {
                name: "Client App".into(),
                domains: vec![
                    CreateProjectDomainRequest {
                        domain: "client.localify".into(),
                        wildcard: false,
                        routes: vec!["3000".into()],
                    },
                    CreateProjectDomainRequest {
                        domain: "api.client.localify".into(),
                        wildcard: false,
                        routes: vec!["3001".into()],
                    },
                ],
            })
            .unwrap();

        let result = api
            .remove_project(RemoveProjectRequest {
                id: project.id.clone(),
                delete_domains: true,
            })
            .unwrap();

        assert_eq!(result.project.id, project.id);
        assert_eq!(result.removed_domains.len(), 2);
        assert!(!result.daemon_reloaded);
        assert!(store.list_projects().unwrap().is_empty());
        assert!(store.list_configured_domains().unwrap().is_empty());
    }

    #[test]
    fn remove_project_environment_deletes_environment_domains() {
        let (_temp_dir, api, store) = temp_api();

        let project = api
            .create_project_with_domains(CreateProjectWithDomainsRequest {
                name: "Client App".into(),
                domains: vec![CreateProjectDomainRequest {
                    domain: "client.localify".into(),
                    wildcard: false,
                    routes: vec!["3000".into()],
                }],
            })
            .unwrap();
        let service = store
            .list_services()
            .unwrap()
            .into_iter()
            .find(|service| service.project_id == project.id)
            .unwrap();
        let environment_domain = "feat-auth.client.localify";
        store
            .add_domain_with_project(
                DomainRegistration::new(
                    DomainPattern::from_name(environment_domain, false).unwrap(),
                    vec![Route::parse("/=3001").unwrap()],
                ),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "feat/auth".into(),
                    slug: "feat-auth".into(),
                    primary: false,
                    root_path: Some("/tmp/client-app-feat-auth".into()),
                    git_common_dir: Some("/tmp/client-app/.git".into()),
                    git_branch: Some("feat/auth".into()),
                    services: vec![ProjectEnvironmentServiceConfig {
                        service_id: service.id,
                        domain: environment_domain.into(),
                    }],
                },
            )
            .unwrap();

        let result = api
            .remove_project_environment(RemoveProjectEnvironmentRequest {
                project_id: project.id.clone(),
                environment_slug: "feat-auth".into(),
            })
            .unwrap();
        let domains = store
            .list_configured_domains()
            .unwrap()
            .into_iter()
            .map(|domain| domain.registration.display_pattern())
            .collect::<Vec<_>>();
        let environments = store.list_project_environments(Some(&project.id)).unwrap();

        assert_eq!(result.environment.slug, "feat-auth");
        assert_eq!(result.removed_domains, vec![environment_domain]);
        assert!(!result.daemon_reloaded);
        assert_eq!(domains, vec!["client.localify"]);
        assert!(
            environments
                .iter()
                .all(|environment| environment.slug != "feat-auth")
        );
    }

    #[test]
    fn list_domains_and_projects_mark_reachable_targets_as_active() {
        let (_temp_dir, api, _store) = temp_api();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        let project = api
            .create_project_with_domains(CreateProjectWithDomainsRequest {
                name: "Client App".into(),
                domains: vec![CreateProjectDomainRequest {
                    domain: "client.localify".into(),
                    wildcard: false,
                    routes: vec![port.to_string()],
                }],
            })
            .unwrap();

        let domain = api
            .list_domains()
            .unwrap()
            .into_iter()
            .find(|domain| domain.project_id.as_deref() == Some(project.id.as_str()))
            .unwrap();
        let listed_project = api
            .list_projects()
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == project.id)
            .unwrap();

        assert!(domain.target_reachable);
        assert!(listed_project.has_reachable_domain);
    }

    #[test]
    fn list_domains_and_projects_mark_unreachable_targets_as_inactive() {
        let (_temp_dir, api, _store) = temp_api();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let project = api
            .create_project_with_domains(CreateProjectWithDomainsRequest {
                name: "Client App".into(),
                domains: vec![CreateProjectDomainRequest {
                    domain: "client.localify".into(),
                    wildcard: false,
                    routes: vec![port.to_string()],
                }],
            })
            .unwrap();

        let domain = api
            .list_domains()
            .unwrap()
            .into_iter()
            .find(|domain| domain.project_id.as_deref() == Some(project.id.as_str()))
            .unwrap();
        let listed_project = api
            .list_projects()
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == project.id)
            .unwrap();

        assert!(!domain.target_reachable);
        assert!(!listed_project.has_reachable_domain);
    }

    #[test]
    fn refresh_domain_certificates_skips_when_no_https_domains_exist() {
        let (_temp_dir, api, _store) = temp_api();

        let result = api.refresh_domain_certificates_local().unwrap();

        assert!(!result.refreshed);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].status, "skipped");
    }

    #[test]
    fn certificate_refresh_restart_step_skips_stopped_daemon() {
        let step = restart_daemon_after_certificate_refresh_step_with(false, || {
            panic!("restart should not run for a stopped daemon")
        });

        assert_eq!(step.status, "skipped");
    }

    #[test]
    fn read_logs_clear_truncates_file_without_removing_it() {
        let (_temp_dir, api, store) = temp_api();
        let config = store.load().unwrap();
        let log_path = config.paths.log_file;

        std::fs::write(&log_path, "line one\nline two\n").unwrap();

        let result = api
            .read_logs(LogsRequest {
                lines: 50,
                clear: true,
            })
            .unwrap();

        assert!(result.cleared);
        assert!(result.exists);
        assert_eq!(result.content, "");
        assert!(log_path.exists());
        assert_eq!(std::fs::read_to_string(&log_path).unwrap(), "");
    }

    #[test]
    fn add_project_service_persists_explicit_service_metadata() {
        let (temp_dir, api, store) = temp_api();
        let project = store
            .create_project_with_root("Client App", Some(temp_dir.path().display().to_string()))
            .unwrap();

        let result = api
            .add_project_service(crate::desktop::dto::AddProjectServiceRequest {
                project_id: project.id.clone(),
                domain: format!(
                    "web.client-app{}",
                    crate::branding::BRANDING.domain_suffix()
                ),
                target: "3000".into(),
                service_name: Some("web".into()),
                service_path: Some("apps/web".into()),
                command: Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]),
            })
            .unwrap();

        assert!(!result.daemon_reloaded);
        let services = store.list_services().unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "web");
        assert_eq!(
            services[0].primary_domain,
            format!(
                "web.client-app{}",
                crate::branding::BRANDING.domain_suffix()
            )
        );
        assert_eq!(services[0].path.as_deref(), Some("apps/web"));
        assert_eq!(
            services[0].command,
            Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()])
        );
        assert!(!services[0].inferred);
    }

    #[test]
    fn materialize_inferred_services_reuses_legacy_target_mapping() {
        let (temp_dir, api, store) = temp_api();

        let root = temp_dir.path().join("ACME");
        let app_dir = root.join("apps/org-space");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("package.json"),
            r#"{"name":"org-space","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52164","dev":"next dev"}}"#,
        )
        .unwrap();

        let project = store
            .create_project_with_root("ACME", Some(root.display().to_string()))
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration(
                    &format!("acteur.acme{}", crate::branding::BRANDING.domain_suffix()),
                    52164,
                ),
                Some(project.id.clone()),
            )
            .unwrap();

        let result = api
            .materialize_project_services(crate::desktop::dto::MaterializeProjectServicesRequest {
                project_id: project.id.clone(),
                root_path: root.display().to_string(),
            })
            .unwrap();
        let services = store.list_services().unwrap();

        assert_eq!(result.materialized, 1);
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "acteur");
        assert_eq!(services[0].path.as_deref(), Some("apps/org-space"));
        assert_eq!(
            services[0].command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
        assert!(!services[0].inferred);
    }

    #[test]
    fn non_primary_single_service_domain_uses_environment_label() {
        let domains = environment_service_domains(
            "demo",
            &[configured_test_service(
                "web",
                &format!("demo{}", crate::branding::BRANDING.domain_suffix()),
            )],
            "feat-auth",
            false,
        );

        assert_eq!(
            domains[0].domain,
            format!(
                "feat-auth.demo{}",
                crate::branding::BRANDING.domain_suffix()
            )
        );
    }

    #[test]
    fn non_primary_multi_service_domain_uses_service_and_environment_label() {
        let domains = environment_service_domains(
            "demo",
            &[
                configured_test_service(
                    "web",
                    &format!("demo{}", crate::branding::BRANDING.domain_suffix()),
                ),
                configured_test_service(
                    "api",
                    &format!("api.demo{}", crate::branding::BRANDING.domain_suffix()),
                ),
            ],
            "feat-auth",
            false,
        );

        assert_eq!(
            domains[0].domain,
            format!(
                "web--feat-auth.demo{}",
                crate::branding::BRANDING.domain_suffix()
            )
        );
        assert_eq!(
            domains[1].domain,
            format!(
                "api--feat-auth.demo{}",
                crate::branding::BRANDING.domain_suffix()
            )
        );
    }

    #[test]
    fn primary_environment_keeps_service_domains() {
        let primary = format!("demo{}", crate::branding::BRANDING.domain_suffix());
        let domains = environment_service_domains(
            "demo",
            &[configured_test_service("web", &primary)],
            "primary",
            true,
        );

        assert_eq!(domains[0].domain, primary);
    }

    #[test]
    fn ensure_existing_domain_https_upgrades_http_domain_when_cert_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.toml"));
        let project = store.create_project("Demo").unwrap();
        let domain = format!(
            "feat-auth.demo{}",
            crate::branding::BRANDING.domain_suffix()
        );
        let pattern = DomainPattern::from_name(&domain, false).unwrap();
        store
            .add_domain_with_project(proxy_registration(&domain, 3000), Some(project.id.clone()))
            .unwrap();
        let registration = store.get_domain(&pattern).unwrap().unwrap();

        ensure_existing_domain_https(
            &project.id,
            registration,
            &store,
            &crate::application::testkit::InMemoryCertificateManager::new(),
        )
        .unwrap();

        let registration = store.get_domain(&pattern).unwrap().unwrap();
        assert!(registration.is_https_enabled());
        assert_eq!(
            store.domain_project_id(&pattern).unwrap().as_deref(),
            Some(project.id.as_str())
        );
    }

    #[test]
    fn ensure_existing_domain_https_keeps_http_when_cert_still_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.toml"));
        let project = store.create_project("Demo").unwrap();
        let domain = format!(
            "feat-auth.demo{}",
            crate::branding::BRANDING.domain_suffix()
        );
        let pattern = DomainPattern::from_name(&domain, false).unwrap();
        store
            .add_domain_with_project(proxy_registration(&domain, 3000), Some(project.id.clone()))
            .unwrap();
        let registration = store.get_domain(&pattern).unwrap().unwrap();

        ensure_existing_domain_https(
            &project.id,
            registration,
            &store,
            &crate::application::testkit::InMemoryCertificateManager::always_failing(),
        )
        .unwrap();

        let registration = store.get_domain(&pattern).unwrap().unwrap();
        assert!(!registration.is_https_enabled());
    }

    fn configured_test_service(id: &str, primary_domain: &str) -> super::ConfiguredService {
        super::ConfiguredService {
            id: id.into(),
            name: id.into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: Some("/tmp/demo".into()),
            primary_domain: primary_domain.into(),
            domains: vec![primary_domain.into()],
            path: None,
            command: None,
            primary_target: None,
            inferred: false,
        }
    }

    fn proxy_registration(domain: &str, port: u16) -> DomainRegistration {
        DomainRegistration::new(
            DomainPattern::from_name(domain, false).unwrap(),
            vec![Route::parse(&format!("/={port}")).unwrap()],
        )
    }

    fn temp_api() -> (TempDir, DesktopApi, ConfigStore) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let store = ConfigStore::new(config_path.clone());
        let paths = RoxyPaths {
            data_dir: temp_dir.path().join("data"),
            pid_file: temp_dir.path().join("roxy.pid"),
            log_file: temp_dir.path().join("roxy.log"),
            certs_dir: temp_dir.path().join("certs"),
            socket_path: temp_dir.path().join("roxy.sock"),
        };
        let mut config = Config::default();
        config.paths = paths;

        store.save(&config).unwrap();

        let api = DesktopApi::new_direct(config_path, temp_dir.path().join("roxy"));

        (temp_dir, api, store)
    }

    fn test_project_service(path: Option<&str>) -> ProjectServiceInfoDto {
        ProjectServiceInfoDto {
            id: "web".into(),
            name: "web".into(),
            project_id: "project-1".into(),
            project_name: "Client App".into(),
            project_enabled: true,
            primary_domain: "client.localify".into(),
            url: "https://client.localify".into(),
            domains: Vec::new(),
            path: path.map(Into::into),
            command: None,
            target: None,
            inferred: false,
        }
    }

    fn create_git_repo_with_worktree(temp_dir: &Path) -> (PathBuf, PathBuf) {
        let primary_root = temp_dir.join("client-app");
        let worktree_root = temp_dir.join("client-app-worktree");
        fs::create_dir(&primary_root).unwrap();
        run_git(&primary_root, &["init"]);
        run_git(&primary_root, &["checkout", "-b", "main"]);
        fs::write(primary_root.join("README.md"), "test\n").unwrap();
        run_git(&primary_root, &["add", "README.md"]);
        run_git(
            &primary_root,
            &[
                "-c",
                "user.email=localify@example.test",
                "-c",
                "user.name=Localify Test",
                "commit",
                "-m",
                "init",
            ],
        );
        let worktree_arg = worktree_root.to_string_lossy().to_string();
        run_git(
            &primary_root,
            &["worktree", "add", "-b", "feature/test", &worktree_arg],
        );

        (
            fs::canonicalize(primary_root).unwrap(),
            fs::canonicalize(worktree_root).unwrap(),
        )
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "git -C {} {} failed\nstdout:\n{}\nstderr:\n{}",
            root.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
