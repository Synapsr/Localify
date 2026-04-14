use std::fs;
use std::fs::OpenOptions;
use std::net::TcpListener;
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
    AddRouteRequest, AppConfigDto, AvailablePortDto, BootstrapDto, ConfigUpdateRequest,
    DaemonLaunchDto, DaemonStatusDto, DomainInfoDto, DomainPatternRequest, EnvironmentDto,
    InstallResultDto, LogsDto, LogsRequest, RegisterDomainRequest, RegisterDomainResultDto,
    RegistrationDto, RegistrationMutationDto, RemoveRouteRequest, RouteDto, StartDaemonRequest,
    StepDto, UninstallPreviewDto, UninstallResultDto, UnregisterDomainResultDto,
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
use crate::system_helper::{SystemHelperClient, current_uid, helper_socket_path};

const ROXY_CA_NAME: &str = "Roxy Local Development CA";

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
            return Ok(bootstrap);
        }

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
        let daemon = MgmtSocketClient::new(&paths.socket_path);
        let result = ListAllDomains::new(&daemon, &self.config_store(), &certs).execute()?;

        Ok(result.domains.into_iter().map(map_domain_info).collect())
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
                    message: "Removed the persistent Roxy desktop helper.".into(),
                }),
                Err(error) => result.steps.push(StepDto {
                    label: Some("Remove privileged helper".into()),
                    status: "warning".into(),
                    message: format!(
                        "Roxy was uninstalled, but the persistent desktop helper could not be removed: {error}"
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

    pub fn start_daemon(&self, request: StartDaemonRequest) -> Result<DaemonLaunchDto> {
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.start_daemon(request);
        }

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
        if self.should_use_system_helper() {
            return self.ensure_helper_running()?.reload_daemon(request);
        }

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
        if let Some(helper) = self.connected_helper_client()? {
            return helper.read_logs(request);
        }

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

    fn should_use_system_helper(&self) -> bool {
        !self.direct_mode && self.config_path.starts_with("/etc")
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

        bail!("Roxy system helper did not become ready in time.")
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
            .context("Failed to launch macOS administrator prompt for Roxy")?;

        if output.status.success() {
            return Ok(command_output(&output));
        }

        bail!(cli_failure_message(&output));
    }

    #[cfg(not(target_os = "macos"))]
    fn run_cli_command_with_admin(&self, _args: &[String]) -> Result<String> {
        bail!(
            "Desktop system-mode actions currently require macOS administrator integration. \
             Run the equivalent `roxy` CLI command manually on this platform."
        )
    }

    fn ensure_cli_binary_exists(&self) -> Result<()> {
        if self.binary_path.exists() {
            return Ok(());
        }

        bail!(
            "Roxy CLI binary not found at {}. Build the root CLI first or set ROXY_CLI_PATH.",
            self.binary_path.display()
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
                    "Roxy removed its system components, but the CA could not be fully removed from your macOS login keychain: {error}"
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

        let probe_path = parent.join(format!(".roxy-write-test-{}", std::process::id()));
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
             Roxy Desktop stores domains and routes in that file.\n\
             Fix the file permissions, or launch the desktop app with \
             ROXY_CONFIG_PATH pointing to a user-writable config file.",
            self.config_path.display()
        )
    }
}

fn parse_pattern(request: &DomainPatternRequest) -> Result<DomainPattern> {
    DomainPattern::from_name(&request.domain, request.wildcard).map_err(Into::into)
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
        "Roxy CLI command failed without diagnostic output.".into()
    } else {
        message
    }
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
        .context("Failed to verify the Roxy CA trust in the current macOS account")?;

    Ok(output.status.success())
}

#[cfg(target_os = "macos")]
fn macos_ensure_user_ca_trust(cert_path: &Path) -> Result<StepDto> {
    if !cert_path.exists() {
        bail!(
            "Roxy installed its system components, but the CA certificate was not found at {}.",
            cert_path.display()
        );
    }

    if macos_user_ca_is_trusted(cert_path)? {
        return Ok(StepDto {
            label: Some("Trust CA in your macOS account".into()),
            status: "skipped".into(),
            message: "The Roxy local CA was already trusted in your macOS login keychain.".into(),
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
        .context("Failed to trust the Roxy CA in the current macOS account")?;

    if !output.status.success() {
        bail!(
            "Failed to trust the Roxy CA in your macOS login keychain: {}",
            cli_failure_message(&output)
        );
    }

    Ok(StepDto {
        label: Some("Trust CA in your macOS account".into()),
        status: "success".into(),
        message:
            "Trusted the Roxy local CA in your macOS login keychain for browsers and SSL clients."
                .into(),
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
            .context("Failed to remove the trusted Roxy CA from the current macOS account")?;

        if !output.status.success() {
            let message = cli_failure_message(&output);
            let not_found = message.contains("could not be found")
                || message.contains("The specified item could not be found")
                || message.contains("The certificate was not found");

            if !not_found {
                bail!(
                    "Failed to remove the trusted Roxy CA from your macOS login keychain: {}",
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
            message: "Removed the Roxy local CA from your macOS login keychain for this account."
                .into(),
        });
    }

    Ok(StepDto {
        label: Some("Remove CA trust from your macOS account".into()),
        status: "skipped".into(),
        message: "No Roxy CA trust entry was present in your macOS login keychain.".into(),
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
            .args(["delete-certificate", "-c", ROXY_CA_NAME, &keychain_path])
            .output()
            .context("Failed to remove a stale Roxy CA from the macOS login keychain")?;

        if output.status.success() {
            removed += 1;
            continue;
        }

        let message = cli_failure_message(&output);
        let not_found = message.contains("could not be found")
            || message.contains("The specified item could not be found");

        if not_found {
            break;
        }

        bail!(
            "Failed to delete the Roxy CA from your macOS login keychain: {}",
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

#[cfg(test)]
mod tests {
    use super::{cli_shell_command, normalize_desktop_route, parse_desktop_route, shell_escape};
    use crate::domain::RouteTarget;
    use std::path::Path;

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
            Path::new("/tmp/roxy"),
            Path::new("/etc/roxy/config.toml"),
            &[
                "register".into(),
                "my app.roxy".into(),
                "--route".into(),
                "/=3000".into(),
            ],
        );

        assert_eq!(
            command,
            "'/tmp/roxy' '--config' '/etc/roxy/config.toml' 'register' 'my app.roxy' '--route' '/=3000'"
        );
    }
}
