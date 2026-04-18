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
    AddRouteRequest, AppConfigDto, AssignDomainProjectRequest, AvailablePortDto, BootstrapDto,
    CliAccessDto, ConfigUpdateRequest, CreateProjectRequest, CreateProjectWithDomainsRequest,
    DaemonLaunchDto, DaemonStatusDto, DetectProjectInitPlanDto, DetectProjectInitPlanRequest,
    DomainInfoDto, DomainPatternRequest, EnvironmentDto, FolderInitServiceDto,
    InitProjectFromFolderRequest, InitProjectFromFolderResultDto, InitProjectRequest,
    InitProjectResultDto, InstallResultDto, KillPortConflictRequest, LogsDto, LogsRequest,
    PortConflictDto, ProjectInfoDto, RegisterDomainRequest, RegisterDomainResultDto,
    RegistrationDto, RegistrationMutationDto, RemoveProjectRequest, RemoveProjectResultDto,
    RemoveRouteRequest, RouteDto, StartDaemonRequest, StepDto, UninstallPreviewDto,
    UninstallResultDto, UnregisterDomainResultDto, UpdateProjectRequest,
};
use crate::desktop::port_conflicts::{
    PortConflictProcess, kill_daemon_port_conflict, list_daemon_port_conflicts,
};
use crate::domain::{DomainPattern, PathPrefix, Route, RouteTarget};
use crate::infrastructure::certs::CertificateService;
use crate::infrastructure::config::{Config, ConfigStore, ConfiguredDomain, ConfiguredProject};
use crate::infrastructure::dns::get_dns_service;
use crate::infrastructure::filesystem::FileSystemSetup;
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::network::{get_lan_ip, get_network_info};
use crate::infrastructure::paths::RoxyPaths;
use crate::infrastructure::pid::PidFile;
use crate::infrastructure::port_check::ensure_daemon_ports_available;
use crate::infrastructure::target_probe::is_route_target_reachable;
use crate::project_init::{
    FolderInitSelection, apply_package_updates, detect_folder_init_plan,
    resolve_folder_init_selection,
};
use crate::system_helper::{SystemHelperClient, current_uid, helper_socket_path};

const CLI_INSTALL_DIR: &str = "/usr/local/bin";

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
            bootstrap.daemon.ca_installed = self.effective_desktop_ca_installed()?;
            bootstrap.environment.cli_helper_ready = true;
            return Ok(bootstrap);
        }

        Ok(BootstrapDto {
            environment: self.environment(),
            config: self.read_config()?,
            daemon: self.daemon_status()?,
            domains: self.list_domains()?,
            projects: self.list_projects()?,
        })
    }

    pub fn environment(&self) -> EnvironmentDto {
        EnvironmentDto {
            app_name: BRANDING.app_name.into(),
            desktop_name: BRANDING.desktop_name.into(),
            config_path: self.config_path.display().to_string(),
            binary_path: self.binary_path.display().to_string(),
            binary_exists: self.binary_path.exists(),
            fixed_domain_suffix: BRANDING.domain_suffix.into(),
            cli_helper_ready: self.direct_mode || self.cli_helper_ready(),
            cli_access_ready: self.cli_access_ready(),
            cli_command_path: self.cli_command_path().display().to_string(),
        }
    }

    pub fn setup_cli_access(&self) -> Result<CliAccessDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.setup_cli_access();
        }

        self.install_cli_access()
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
            return helper.list_projects();
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

        let project = config_store.create_project(&request.name)?;
        let mut created_patterns = Vec::new();

        for service in &request.services {
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
            project: map_project_info(project, request.services.len(), has_reachable_domain),
            services: request.services,
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
            services: resolved
                .services
                .iter()
                .map(|service| crate::desktop::dto::InitProjectServiceRequest {
                    domain: service.domain.clone(),
                    target: service.target.clone(),
                })
                .collect(),
        })?;
        let update_results = apply_package_updates(&resolved.package_updates);

        Ok(InitProjectFromFolderResultDto {
            project: init_result.project,
            services: init_result.services,
            daemon_reloaded: init_result.daemon_reloaded,
            updated_scripts: update_results.updated,
            skipped_script_updates: update_results.skipped,
        })
    }

    pub fn update_project(&self, request: UpdateProjectRequest) -> Result<ProjectInfoDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.update_project(request);
        }

        self.ensure_config_writable()?;
        let configured_domains = self.config_store().list_configured_domains()?;
        let (domain_count, has_reachable_domain) = configured_domains
            .iter()
            .filter(|domain| domain.project_id.as_deref() == Some(request.id.as_str()))
            .fold((0, false), |(count, reachable), domain| {
                (
                    count + 1,
                    reachable || registration_has_reachable_target(&domain.registration),
                )
            });
        let project =
            self.config_store()
                .update_project(&request.id, request.name, request.enabled)?;
        Ok(map_project_info(
            project,
            domain_count,
            has_reachable_domain,
        ))
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
            let mut result = self.ensure_helper_running()?.install_roxy()?;

            if let Some(step) = self.ensure_desktop_ca_trust_step()? {
                result.steps.push(step);
            }

            result
                .steps
                .push(self.cli_access_step_with(|helper| helper.setup_cli_access()));

            return Ok(result);
        }

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
        steps.push(self.cli_access_step_with(|_| self.install_cli_access()));

        Ok(InstallResultDto {
            lan_ip: result.lan_ip.to_string(),
            steps,
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
        steps.push(self.remove_cli_access_step());

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

    fn should_use_system_helper(&self) -> bool {
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

    fn connected_helper_client(&self) -> Result<Option<SystemHelperClient>> {
        if !self.should_use_system_helper() {
            return Ok(None);
        }

        let client = SystemHelperClient::new(self.helper_socket_path()?);
        if client.ping().is_ok() {
            Ok(Some(client))
        } else {
            Ok(None)
        }
    }

    fn ensure_helper_running(&self) -> Result<SystemHelperClient> {
        let socket_path = self.helper_socket_path()?;
        let client = SystemHelperClient::new(socket_path.clone());
        if client.ping().is_ok() {
            return Ok(client);
        }

        let uid = current_uid()?;
        self.run_cli_command_with_admin(&[
            "helper".into(),
            "ensure-running".into(),
            "--socket".into(),
            socket_path.display().to_string(),
            "--uid".into(),
            uid.to_string(),
        ])?;

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

    #[cfg(target_os = "macos")]
    fn run_cli_command_with_admin(&self, args: &[String]) -> Result<String> {
        self.ensure_cli_binary_exists()?;

        let command = cli_shell_command(&self.binary_path, &self.config_path, args);
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
        macos_user_ca_is_trusted(&self.ca_cert_path())
    }

    #[cfg(not(target_os = "macos"))]
    fn effective_desktop_ca_installed(&self) -> Result<bool> {
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        certs.is_ca_installed().map_err(Into::into)
    }

    #[cfg(target_os = "macos")]
    fn ensure_desktop_ca_trust_step(&self) -> Result<Option<StepDto>> {
        let cert_path = self.ca_cert_path();
        Ok(Some(macos_ensure_user_ca_trust(&cert_path)?))
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
            BRANDING.slug,
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
    let mut parts = vec![
        shell_escape(&binary_path.to_string_lossy()),
        shell_escape("--config"),
        shell_escape(&config_path.to_string_lossy()),
    ];
    parts.extend(args.iter().map(|arg| shell_escape(arg)));
    parts.join(" ")
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
        bail!(
            "{} installed its system components, but the CA certificate was not found at {}.",
            BRANDING.app_name,
            cert_path.display()
        );
    }

    if macos_user_ca_is_trusted(cert_path)? {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: "skipped".into(),
            message: format!(
                "The {} local CA was already trusted in your macOS login keychain.",
                BRANDING.app_name
            ),
        });
    }

    let keychain_path = macos_login_keychain_path()?;
    delete_macos_keychain_certificates(&keychain_path)?;

    let cert_path = path_as_utf8(cert_path)?;
    let keychain_path = path_as_utf8(&keychain_path)?;

    let output = Command::new("security")
        .args([
            "add-trusted-cert",
            "-r",
            "trustRoot",
            "-p",
            "ssl",
            "-p",
            "basic",
            "-k",
            &keychain_path,
            &cert_path,
        ])
        .output()
        .context(format!(
            "Failed to trust the {} CA in the current macOS account",
            BRANDING.app_name
        ))?;

    if !output.status.success() {
        bail!(
            "Failed to trust the {} CA in your macOS login keychain: {}",
            BRANDING.app_name,
            cli_failure_message(&output)
        );
    }

    Ok(StepDto {
        label: Some("Trust CA in your macOS account".into()),
        status: "success".into(),
        message: format!(
            "Trusted the {} local CA in your macOS login keychain for browsers and SSL clients.",
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

    let home_dir = std::env::var("HOME").context("Failed to determine the current home folder")?;
    Ok(PathBuf::from(home_dir).join("Library/Keychains/login.keychain-db"))
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
        ensure_cli_command_link, normalize_desktop_route, parse_desktop_route,
        remove_cli_command_link, shell_escape,
    };
    use crate::config::RoxyPaths;
    use crate::desktop::dto::{
        CreateProjectDomainRequest, CreateProjectWithDomainsRequest, LogsRequest,
        RemoveProjectRequest,
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
