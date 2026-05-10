use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

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
    ActivateLicenseRequest, AddRouteRequest, AppConfigDto, AssignDomainProjectRequest,
    AvailablePortDto, BootstrapDto, CertificateRefreshDto, CliAccessDto, ConfigUpdateRequest,
    CreateProjectRequest, CreateProjectWithDomainsRequest, DaemonLaunchDto, DaemonStatusDto,
    DetectProjectInitPlanDto, DetectProjectInitPlanRequest, DomainInfoDto, DomainPatternRequest,
    EnvironmentDto, FolderInitServiceDto, ImportOfflineLicenseRequest,
    InitProjectFromFolderRequest, InitProjectFromFolderResultDto, InitProjectRequest,
    InitProjectResultDto, InstallResultDto, KeygenSettingsUpdateRequest, KillPortConflictRequest,
    LauncherAvailabilityDto, LogsDto, LogsRequest, NodeTrustDto, OpenInEditorRequest,
    OpenProjectUrlRequest, PortConflictDto, ProjectInfoDto, RegisterDomainRequest,
    RegisterDomainResultDto, RegistrationDto, RegistrationMutationDto, RemoveProjectRequest,
    RemoveProjectResultDto, RemoveRouteRequest, RouteDto, RunLocalDevServerRequest,
    SettingsExportRequest, SettingsExportResultDto, SettingsImportPreviewDto,
    SettingsImportPreviewProjectDto, SettingsImportPreviewRequest, SettingsImportRequest,
    SettingsImportResultDto, SettingsTransferDomainDto, SettingsTransferProjectDto,
    SettingsTransferSettingsDto, SettingsTransferSnapshotDto, StartDaemonRequest, StepDto,
    StopLocalDevServerRequest, UninstallPreviewDto, UninstallResultDto, UnregisterDomainResultDto,
    UpdateDomainRequest, UpdateLauncherPreferencesRequest, UpdateProjectRequest,
};
use crate::desktop::home_dir::user_home_dir;
use crate::desktop::node_trust;
use crate::desktop::port_conflicts::{
    PortConflictProcess, kill_daemon_port_conflict, list_daemon_port_conflicts,
};
use crate::desktop::{LauncherPreferencesStore, ProjectRootStore, RunActionPreference};
use crate::domain::{DomainPattern, DomainRegistration, PathPrefix, Route, RouteTarget};
use crate::infrastructure::certs::CertificateService;
use crate::infrastructure::config::{Config, ConfigStore, ConfiguredDomain, ConfiguredProject};
use crate::infrastructure::dns::get_dns_service;
use crate::infrastructure::filesystem::FileSystemSetup;
use crate::infrastructure::licensing::{
    FREE_MAX_PROJECTS, KeygenClient, ResolvedLicenseStatus,
    build_offline_imported_license, current_machine_fingerprint, mask_license_key,
    resolve_license_state,
};
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::network::{get_lan_ip, get_network_info};
use crate::infrastructure::paths::RoxyPaths;
use crate::infrastructure::pid::PidFile;
use crate::infrastructure::port_check::ensure_daemon_ports_available;
use crate::infrastructure::target_probe::is_route_target_reachable;
use crate::infrastructure::{editor, local_dev};
use crate::project_init::{
    FolderInitSelection, apply_local_app_updates, detect_folder_init_plan,
    resolve_folder_init_selection,
};
use crate::system_helper::{SystemHelperClient, current_uid, helper_socket_path};

const CLI_INSTALL_DIR: &str = "/usr/local/bin";
const SETTINGS_TRANSFER_VERSION: u32 = 1;
const SETTINGS_TRANSFER_SCOPE_ALL: &str = "all";
const SETTINGS_TRANSFER_SCOPE_PROJECTS: &str = "projects";
const SETTINGS_IMPORT_MODE_REPLACE_ALL: &str = "replaceAll";
const SETTINGS_IMPORT_MODE_PROJECTS: &str = "projects";

#[derive(Debug, Clone)]
pub struct DesktopApi {
    config_path: PathBuf,
    binary_path: PathBuf,
    direct_mode: bool,
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

    pub fn setup_node_trust(&self) -> Result<NodeTrustDto> {
        let setup = node_trust::install(&self.ca_cert_path())?;
        Ok(map_node_trust_dto(setup.status, setup.already_ready, None))
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

        let (active_config_domains, external_domains) = match daemon.list_registrations() {
            Ok(registrations) => {
                let mut active_patterns = std::collections::HashSet::new();
                let mut external = Vec::new();

                for registration in registrations {
                    if registration.source().to_string() == "config" {
                        active_patterns.insert(registration.display_pattern());
                    } else {
                        external.push(registration);
                    }
                }

                (active_patterns, external)
            }
            Err(_) => (std::collections::HashSet::new(), Vec::new()),
        };

        let mut domains = configured_domains
            .into_iter()
            .map(|domain| {
                let has_cert = certs.exists(domain.registration.pattern());
                let active = active_config_domains.contains(&domain.registration.display_pattern());
                let target_reachable = registration_has_reachable_target(&domain.registration);

                map_configured_domain_info(domain, has_cert, cert_trusted, active, target_reachable)
            })
            .collect::<Vec<_>>();

        domains.extend(external_domains.into_iter().map(|registration| {
            let has_cert = certs.exists(registration.pattern());
            let target_reachable = registration_has_reachable_target(&registration);
            map_domain_info(
                registration,
                DomainInfoDetails {
                    has_cert,
                    cert_trusted,
                    project_id: None,
                    project_name: None,
                    project_enabled: true,
                    active: true,
                    target_reachable,
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

        let configured_domains = self.config_store().list_configured_domains()?;
        let project_statuses = configured_domains.into_iter().fold(
            std::collections::HashMap::<String, (usize, bool)>::new(),
            |mut statuses, domain| {
                if let Some(project_id) = domain.project_id {
                    let status = statuses.entry(project_id).or_insert((0, false));
                    status.0 += 1;
                    status.1 |= registration_has_reachable_target(&domain.registration);
                }
                statuses
            },
        );

        let mut projects = self
            .config_store()
            .list_projects()?
            .into_iter()
            .map(|project| {
                let (domain_count, has_reachable_domain) = project_statuses
                    .get(&project.id)
                    .copied()
                    .unwrap_or((0, false));
                map_project_info(project, domain_count, has_reachable_domain)
            })
            .collect::<Vec<_>>();
        projects = self.enrich_projects_with_local_paths(projects)?;
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(projects)
    }

    pub fn create_project(&self, request: CreateProjectRequest) -> Result<ProjectInfoDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.create_project(request);
        }

        self.ensure_config_writable()?;
        let project = self.config_store().create_project(&request.name)?;
        Ok(map_project_info(project, 0, false))
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
            project: map_project_info(project, services.len(), has_reachable_domain),
            services,
            daemon_reloaded,
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
            })
            .collect::<Vec<_>>();
        let resolved = resolve_folder_init_selection(&root_path, &request.name, &selections)?;
        let init_result = self.init_project(InitProjectRequest {
            name: request.name,
            root_path: Some(root_path.display().to_string()),
            services: resolved
                .services
                .iter()
                .map(|service| crate::desktop::dto::InitProjectServiceRequest {
                    domain: service.domain.clone(),
                    target: service.target.clone(),
                })
                .collect(),
        })?;
        self.store_project_root_path(&init_result.project.id, &root_path)?;
        let mut project = init_result.project;
        project.root_path = Some(root_path.display().to_string());
        let update_results =
            apply_local_app_updates(&resolved.package_updates, &resolved.next_config_updates);

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
        let project = map_project_info(project, domain_count, has_reachable_domain);
        let mut projects = self.enrich_projects_with_local_paths(vec![project])?;
        Ok(projects
            .pop()
            .expect("enrich_projects_with_local_paths preserves project count"))
    }

    pub fn open_in_editor(&self, request: OpenInEditorRequest) -> Result<()> {
        crate::desktop::editor::open_in_editor(Path::new(&request.path))
    }

    pub fn open_project_url(&self, request: OpenProjectUrlRequest) -> Result<()> {
        crate::desktop::browser::open_project_url(&request.url, request.browser)
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

    pub fn run_local_dev_server(&self, request: RunLocalDevServerRequest) -> Result<()> {
        let preferences = self.effective_launcher_preferences()?;
        let project_root = Path::new(&request.root_path);
        crate::infrastructure::local_dev::open_local_dev_server_with_terminal(
            project_root,
            &request.target,
            preferences.terminal,
        )?;

        if preferences.run_action == RunActionPreference::EditorAndTerminal {
            crate::desktop::editor::open_in_editor_with_preference(
                project_root,
                preferences.editor,
            )?;
        }

        Ok(())
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
            })?;
            self.update_keygen_settings(settings.keygen)?;
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
        if self.direct_mode {
            return Ok(projects);
        }

        let local_projects = match self.project_root_store() {
            Ok(store) => match store.load() {
                Ok(local_projects) => local_projects,
                Err(error) if is_missing_home_error(&error) => HashMap::new(),
                Err(error) => return Err(error),
            },
            Err(error) if is_missing_home_error(&error) => HashMap::new(),
            Err(error) => return Err(error),
        };

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
            project: map_project_info(project, domain_count, false),
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
        let active = daemon
            .list_registrations()
            .map(|registrations| {
                registrations
                    .iter()
                    .any(|registration| registration.display_pattern() == pattern.display_pattern())
            })
            .unwrap_or(false);

        let has_cert = certs.exists(configured_domain.registration.pattern());
        let target_reachable = registration_has_reachable_target(&configured_domain.registration);
        Ok(map_configured_domain_info(
            configured_domain,
            has_cert,
            cert_trusted,
            active,
            target_reachable,
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
    if macos_user_ca_is_trusted(cert_path)? {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: "skipped".into(),
            message: format!(
                "The {} CA was already trusted by this macOS account.",
                BRANDING.app_name
            ),
        });
    }

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

    if !macos_user_ca_is_trusted(cert_path)? {
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
    let keychain_path = path_as_utf8(keychain_path)?;
    let mut removed = 0;

    loop {
        let output = Command::new("security")
            .args([
                "delete-certificate",
                "-c",
                BRANDING.ca_name().as_str(),
                &keychain_path,
            ])
            .output()
            .context(format!(
                "Failed to remove a stale {} CA from the macOS login keychain",
                BRANDING.app_name
            ))?;

        if output.status.success() {
            removed += 1;
            continue;
        }

        let message = cli_failure_message(&output);
        let not_found = is_missing_macos_certificate_error(&message);

        if not_found {
            break;
        }

        bail!(
            "Failed to delete the {} CA from your macOS login keychain: {}",
            BRANDING.app_name,
            message
        );
    }

    Ok(removed)
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
    }
}

fn map_configured_domain_info(
    value: ConfiguredDomain,
    has_cert: bool,
    cert_trusted: Option<bool>,
    active: bool,
    target_reachable: bool,
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
    }
}

fn map_project_info(
    value: ConfiguredProject,
    domain_count: usize,
    has_reachable_domain: bool,
) -> ProjectInfoDto {
    ProjectInfoDto {
        id: value.id,
        name: value.name,
        enabled: value.enabled,
        domain_count,
        has_reachable_domain,
        root_path: value.root_path,
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
        CliRemovalOutcome, DesktopApi, cli_command_points_to_binary, cli_shell_command,
        cli_shell_commands, ensure_cli_command_link, export_route_string, normalize_desktop_route,
        normalize_project_snapshot_id, parse_desktop_route, remove_cli_command_link,
        restart_daemon_after_certificate_refresh_step_with, shell_escape,
        validate_settings_snapshot,
    };
    use crate::config::RoxyPaths;
    use crate::desktop::dto::{
        CreateProjectDomainRequest, CreateProjectWithDomainsRequest, LogsRequest,
        RemoveProjectRequest, RouteDto, SettingsTransferDomainDto, SettingsTransferProjectDto,
        SettingsTransferSnapshotDto,
    };
    use crate::domain::RouteTarget;
    use crate::infrastructure::config::{Config, ConfigStore};
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn parse_desktop_route_defaults_bare_target_to_root_path() {
        let route = parse_desktop_route("3000").unwrap();

        assert_eq!(route.path().as_str(), "/");
        assert!(matches!(route.target(), RouteTarget::Proxy(_)));
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
}
