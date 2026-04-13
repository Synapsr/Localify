use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::application::StepOutcome;
use crate::application::daemon_status::QueryDaemonStatus;
use crate::application::install::Install;
use crate::application::list_all_domains::{DomainInfo, ListAllDomains};
use crate::application::manage_routes::ManageRoutes;
use crate::application::register_domain::RegisterDomain;
use crate::application::restart_daemon::RestartDaemon;
use crate::application::start_daemon::StartDaemon;
use crate::application::stop_daemon::StopDaemon;
use crate::application::uninstall::Uninstall;
use crate::application::unregister_domain::UnregisterDomain;
use crate::desktop::dto::{
    AddRouteRequest, AppConfigDto, BootstrapDto, ConfigUpdateRequest, DaemonLaunchDto,
    DaemonStatusDto, DomainInfoDto, DomainPatternRequest, EnvironmentDto, InstallResultDto,
    LogsDto, LogsRequest, RegisterDomainRequest, RegisterDomainResultDto, RegistrationDto,
    RegistrationMutationDto, RemoveRouteRequest, RouteDto, StartDaemonRequest, StepDto,
    UninstallPreviewDto, UninstallResultDto, UnregisterDomainResultDto,
};
use crate::domain::{DomainPattern, PathPrefix, Route, RouteTarget};
use crate::infrastructure::certs::CertificateService;
use crate::infrastructure::config::{Config, ConfigStore};
use crate::infrastructure::dns::get_dns_service;
use crate::infrastructure::filesystem::FileSystemSetup;
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::network::{get_lan_ip, get_network_info};
use crate::infrastructure::paths::RoxyPaths;
use crate::infrastructure::pid::PidFile;

#[derive(Debug, Clone)]
pub struct DesktopApi {
    config_path: PathBuf,
    binary_path: PathBuf,
}

impl DesktopApi {
    pub fn new(config_path: PathBuf, binary_path: PathBuf) -> Self {
        Self {
            config_path,
            binary_path,
        }
    }

    pub fn bootstrap(&self) -> Result<BootstrapDto> {
        Ok(BootstrapDto {
            environment: self.environment(),
            config: self.read_config()?,
            daemon: self.daemon_status()?,
            domains: self.list_domains()?,
        })
    }

    pub fn environment(&self) -> EnvironmentDto {
        EnvironmentDto {
            config_path: self.config_path.display().to_string(),
            binary_path: self.binary_path.display().to_string(),
            binary_exists: self.binary_path.exists(),
            fixed_domain_suffix: ".roxy".into(),
        }
    }

    pub fn read_config(&self) -> Result<AppConfigDto> {
        let config = self.config_store().load()?;
        Ok(map_config(self.config_path.as_path(), config))
    }

    pub fn update_config(&self, request: ConfigUpdateRequest) -> Result<AppConfigDto> {
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
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let daemon = MgmtSocketClient::new(&paths.socket_path);
        let result = ListAllDomains::new(&daemon, &self.config_store(), &certs).execute()?;

        Ok(result.domains.into_iter().map(map_domain_info).collect())
    }

    pub fn install(&self) -> Result<InstallResultDto> {
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

        Ok(InstallResultDto {
            lan_ip: result.lan_ip.to_string(),
            steps: result
                .steps
                .into_iter()
                .map(|(label, outcome)| map_step(Some(label), outcome))
                .collect(),
        })
    }

    pub fn uninstall_preview(&self) -> Result<UninstallPreviewDto> {
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

        Ok(UninstallResultDto {
            steps: result
                .steps
                .into_iter()
                .map(|(label, outcome)| map_step(Some(label), outcome))
                .collect(),
        })
    }

    pub fn register_domain(
        &self,
        request: RegisterDomainRequest,
    ) -> Result<RegisterDomainResultDto> {
        let (_, paths) = self.load_config_and_paths()?;
        let certs = CertificateService::new(&paths);
        let pattern = DomainPattern::from_name(&request.domain, request.wildcard)?;
        let routes = request
            .routes
            .iter()
            .map(|route| Route::parse(route))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| anyhow::anyhow!("Invalid route: {error}"))?;

        let result = RegisterDomain::new(&self.config_store(), &certs).execute(pattern, routes)?;

        Ok(RegisterDomainResultDto {
            registration: map_registration(result.registration),
            cert_outcome: map_step(None, result.cert_outcome),
        })
    }

    pub fn unregister_domain(
        &self,
        request: DomainPatternRequest,
    ) -> Result<UnregisterDomainResultDto> {
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
        let pattern = parse_pattern(&request)?;
        let registration = ManageRoutes::new(&self.config_store()).list_routes(&pattern)?;
        Ok(map_registration(registration))
    }

    pub fn start_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        let (config, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file.clone());
        let ready = StartDaemon::new(&pid_file, &config.daemon).preflight()?;

        let child = self
            .spawn_daemon_process(request.verbose)
            .with_context(|| {
                format!("Failed to launch daemon via {}", self.binary_path.display())
            })?;

        Ok(DaemonLaunchDto {
            pid: child.id(),
            lan_ip: get_lan_ip().to_string(),
            http_port: ready.http_port,
            https_port: ready.https_port,
            dns_port: config.daemon.dns_port,
        })
    }

    pub fn stop_daemon(&self) -> Result<DaemonStatusDto> {
        let (_, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file);
        StopDaemon::new(&pid_file).execute()?;
        self.daemon_status()
    }

    pub fn restart_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        let config_store = self.config_store();
        let (_, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file);
        let ready = RestartDaemon::new(&pid_file, &config_store).restart()?;
        let child = self.spawn_daemon_process(request.verbose)?;

        Ok(DaemonLaunchDto {
            pid: child.id(),
            lan_ip: get_lan_ip().to_string(),
            http_port: ready.daemon_config.http_port,
            https_port: ready.daemon_config.https_port,
            dns_port: ready.daemon_config.dns_port,
        })
    }

    pub fn reload_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        let config_store = self.config_store();
        let (_, paths) = self.load_config_and_paths()?;
        let pid_file = PidFile::new(paths.pid_file);
        let ready = RestartDaemon::new(&pid_file, &config_store).reload()?;
        let child = self.spawn_daemon_process(request.verbose)?;

        Ok(DaemonLaunchDto {
            pid: child.id(),
            lan_ip: get_lan_ip().to_string(),
            http_port: ready.daemon_config.http_port,
            https_port: ready.daemon_config.https_port,
            dns_port: ready.daemon_config.dns_port,
        })
    }

    pub fn read_logs(&self, request: LogsRequest) -> Result<LogsDto> {
        let (_, paths) = self.load_config_and_paths()?;
        let log_path = paths.log_file;

        if request.clear {
            if log_path.exists() {
                fs::remove_file(&log_path).context("Failed to clear log file")?;
            }

            return Ok(LogsDto {
                path: log_path.display().to_string(),
                exists: false,
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
                "Roxy CLI binary not found at {}. Build the root CLI first or set ROXY_CLI_PATH.",
                self.binary_path.display()
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
            cmd.env("ROXY_LOG", "debug");
        }

        cmd.spawn().map_err(Into::into)
    }

    fn load_config_and_paths(&self) -> Result<(Config, RoxyPaths)> {
        let config = self.config_store().load()?;
        let paths = config.paths.clone();
        Ok((config, paths))
    }

    fn config_store(&self) -> ConfigStore {
        ConfigStore::new(self.config_path.clone())
    }
}

fn parse_pattern(request: &DomainPatternRequest) -> Result<DomainPattern> {
    DomainPattern::from_name(&request.domain, request.wildcard).map_err(Into::into)
}

fn map_config(config_path: &Path, config: Config) -> AppConfigDto {
    AppConfigDto {
        config_path: config_path.display().to_string(),
        daemon: config.daemon.into(),
        paths: config.paths.into(),
        docker: config.docker.into(),
    }
}

fn map_domain_info(value: DomainInfo) -> DomainInfoDto {
    DomainInfoDto {
        registration: map_registration(value.registration),
        has_cert: value.has_cert,
        cert_trusted: value.cert_trusted,
    }
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
