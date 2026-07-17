use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use localify::desktop::ProjectRootStore;
use localify::project_init::{LocalAppUpdatePolicy, detect_folder_init_plan};
use serde::Serialize;

use super::context::{AppContext, effective_current_dir};
use super::init_project::{self, InitProjectArgs};
use super::project_env::ensure_implicit_worktree_environment;
use crate::branding::BRANDING;
use crate::domain::DomainRegistration;
use crate::domain::value_objects::{ProxyTarget, RouteTarget};
use crate::infrastructure::config::{ConfiguredProjectEnvironment, ConfiguredService};
use crate::infrastructure::local_dev::{
    LocalDevLaunchPlan, resolve_local_dev_launch, resolve_localify_dev_launch,
    resolve_portable_local_dev_launch,
};
use crate::infrastructure::paths::LocalifyPaths;
use crate::infrastructure::project_marker::{read_project_marker, write_project_marker};
use localify::project_init::{
    apply_next_allowed_dev_origin, apply_vite_allowed_host, find_existing_next_config_path,
    find_existing_vite_config_path,
};

const DEFAULT_URL_ENV_KEYS: &[&str] = &[
    "LOCALIFY_URL",
    "LOCALIFY_ORIGIN",
    "NEXT_PUBLIC_APP_URL",
    "AUTH_URL",
    "NEXTAUTH_URL",
    "BETTER_AUTH_URL",
    "APP_URL",
    "SITE_URL",
    "VITE_LOCALIFY_URL",
    "VITE_APP_URL",
    "VITE_PUBLIC_APP_URL",
    "VITE_SITE_URL",
];

const DEFAULT_DOMAIN_ENV_KEYS: &[&str] = &[
    "LOCALIFY_DOMAIN",
    "NEXT_PUBLIC_APP_DOMAIN",
    "VITE_APP_DOMAIN",
];

const ENV_FILE_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.development",
    ".env.development.local",
    ".env.localify",
    ".env.localify.local",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Run,
    Dev,
}

impl RunMode {
    fn command_name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Dev => "dev",
        }
    }
}

pub struct RunArgs {
    pub mode: RunMode,
    pub service_name: Option<String>,
    pub project: Option<String>,
    pub environment: Option<String>,
    pub init_domain: Option<String>,
    pub override_command: Vec<String>,
}

pub fn list(ctx: &AppContext, json: bool) -> Result<()> {
    let ports = daemon_ports(ctx)?;
    let services = ctx.config_store.list_services()?;

    if json {
        let output = services
            .iter()
            .map(|service| {
                let endpoint = service_endpoint(service, ports, ctx)?;
                Ok(ServiceOutput::from_service(service, endpoint))
            })
            .collect::<Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if services.is_empty() {
        println!("No services configured.");
        println!("\nCreate a project with:");
        println!("  {} init-project my-app", BRANDING.cli_command());
        return Ok(());
    }

    println!("Services:\n");
    for service in services {
        let endpoint = service_endpoint(&service, ports, ctx)?;
        println!(
            "  {} [{}] -> {}",
            service.name,
            service.id,
            service_url(&service, endpoint)
        );
        println!(
            "    project: {} [{}]{}",
            service.project_name,
            service.project_id,
            if service.project_enabled {
                ""
            } else {
                " (disabled)"
            }
        );
        println!("    primary domain: {}", service.primary_domain);

        let additional_domains = service
            .domains
            .iter()
            .filter(|domain| domain.as_str() != service.primary_domain)
            .collect::<Vec<_>>();
        if !additional_domains.is_empty() {
            println!(
                "    additional domains: {}",
                additional_domains
                    .iter()
                    .map(|domain| domain.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        if let Some(target) = &service.primary_target {
            println!("    target: {target}");
        }
        if let Some(path) = &service.path {
            println!("    path: {path}");
        }
        if let Some(command) = &service.command {
            println!("    command: {}", command.join(" "));
        }
        println!(
            "    source: {}",
            if service.inferred {
                "inferred"
            } else {
                "explicit"
            }
        );
        println!();
    }

    Ok(())
}

pub fn get(
    service_name: String,
    project: Option<String>,
    environment: Option<String>,
    json: bool,
    ctx: &AppContext,
) -> Result<()> {
    let ports = daemon_ports(ctx)?;
    let service = resolve_service(&service_name, project.as_deref(), ctx)?;
    let service = resolve_environment_service(service, environment.as_deref(), ctx)?;
    let endpoint = service_endpoint(&service, ports, ctx)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ServiceOutput::from_service(&service, endpoint))?
        );
        return Ok(());
    }

    println!("{}", service_url(&service, endpoint));
    Ok(())
}

pub fn run(
    args: RunArgs,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
    ctx: &AppContext,
    paths: &LocalifyPaths,
) -> Result<()> {
    let RunArgs {
        mode,
        service_name,
        project,
        environment,
        init_domain,
        override_command,
    } = args;
    if init_domain.is_some()
        && (service_name.is_some() || project.is_some() || environment.is_some())
    {
        bail!(
            "--domain only applies when '{} dev' initializes a project from the current folder. For an existing service, use '{} project domain set <service> <domain>'.",
            BRANDING.cli_command(),
            BRANDING.cli_command()
        );
    }

    // A moved project folder would otherwise resolve to nothing and trigger a
    // duplicate auto-init below; the in-project marker re-links it first.
    if let Ok(cwd) = effective_current_dir()
        && let Some(healed) = super::project::heal_moved_project_root(&ctx.config_store, &cwd)
    {
        eprintln!(
            "Re-linked project '{}' [{}] to '{}' (folder moved).",
            healed.project_name,
            healed.project_id,
            healed.root_path.display()
        );
    }

    let has_override_command = !override_command.is_empty();
    let ports = daemon_ports(ctx)?;
    let implicit_environment = if environment.is_none() {
        ensure_implicit_worktree_environment(project.as_deref(), ctx)?
    } else {
        None
    };
    let effective_project = project.as_deref().or_else(|| {
        implicit_environment
            .as_ref()
            .map(|environment| environment.project_id.as_str())
    });
    let effective_environment = environment.as_deref().or_else(|| {
        implicit_environment
            .as_ref()
            .map(|environment| environment.slug.as_str())
    });

    if let Some(environment) = &implicit_environment {
        eprintln!(
            "Using project environment '{}' [{}] for worktree.",
            environment.name, environment.slug
        );
    }

    ensure_run_context_is_initialized(
        service_name.as_deref(),
        effective_project,
        effective_environment,
        init_domain.as_deref(),
        config_path,
        policy,
        ctx,
    )?;

    let service = service_with_launch_for_mode(
        resolve_run_service(
            service_name.as_deref(),
            effective_project,
            effective_environment,
            ctx,
        )?,
        mode,
    );
    if !service.project_enabled {
        bail!(
            "Service '{}' belongs to disabled project '{}'. Enable the project before running it.",
            service.id,
            service.project_id
        );
    }

    // Backfill the marker for pre-marker projects so a future move can
    // auto-heal. Never overwrites an existing marker.
    if let Some(root) = service.project_root_path.as_deref() {
        let root = Path::new(root);
        if root.is_dir() && read_project_marker(root).is_none() {
            let _ = write_project_marker(root, &service.project_id);
        }
    }

    let command = if !has_override_command {
        service.command.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Service '{}' has no detected command. Run it with an override, for example: {} {} {} -- pnpm dev",
                service.id,
                BRANDING.cli_command(),
                mode.command_name(),
                service.id
            )
        })?
    } else {
        override_command
    };

    let proxy_target = service_proxy_target(&service)?;
    if !is_supported_run_host(proxy_target.host()) {
        bail!(
            "Service '{}' points to '{}'. {} {} injects HOST=127.0.0.1 and only supports route targets on 127.0.0.1 or localhost in this slice. Update the route target or run the service manually.",
            service.id,
            proxy_target,
            BRANDING.cli_command(),
            mode.command_name()
        );
    }

    let port = proxy_target.port().value();
    ensure_port_available(port, &service)?;

    let workdir = service_workdir(&service)?;
    let command = if !has_override_command {
        prepare_local_dev_command(command, &workdir, port)?
    } else {
        command
    };
    warn_if_daemon_is_stopped(ctx);

    let endpoint = service_endpoint(&service, ports, ctx)?;
    let url = service_url(&service, endpoint);
    let app_host = service_public_host(&service);
    let app_domain = service_public_domain(&service, endpoint);
    ensure_next_allowed_dev_origin(&workdir, &app_host);
    ensure_vite_allowed_host(&workdir, &app_host);
    let url_envs = url_environment_overrides(&workdir, &url, &app_domain)?;
    eprintln!("Starting '{}' at {}", service.id, url);
    eprintln!("  route target: {proxy_target}");
    eprintln!("  injected HOST: 127.0.0.1");
    eprintln!("  injected PORT: {port}");
    eprintln!("  injected LOCALIFY_URL: {url}");
    eprintln!("  injected URL envs: {}", env_key_list(&url_envs));
    eprintln!("  cwd: {}", workdir.display());
    eprintln!("$ {}", command.join(" "));

    let mut child = Command::new(&command[0]);
    child
        .args(&command[1..])
        .current_dir(&workdir)
        .env("PORT", port.to_string())
        .env("HOST", "127.0.0.1");

    for (key, value) in &url_envs {
        child.env(key, value);
    }

    child
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let ca_cert_path = paths.data_dir.join("ca.crt");
    if ca_cert_path.exists() {
        child
            .env("NODE_EXTRA_CA_CERTS", ca_cert_path)
            .env_remove("NODE_USE_SYSTEM_CA");
    }

    let status = child
        .status()
        .with_context(|| format!("Failed to start command '{}'", command[0]))?;
    if !status.success() {
        bail!("Service '{}' exited with {}", service.id, status);
    }

    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceOutput {
    id: String,
    name: String,
    project_id: String,
    project_name: String,
    project_enabled: bool,
    primary_domain: String,
    url: String,
    domains: Vec<String>,
    path: Option<String>,
    command: Option<Vec<String>>,
    target: Option<String>,
    inferred: bool,
}

impl ServiceOutput {
    fn from_service(service: &ConfiguredService, endpoint: ServiceEndpoint) -> Self {
        Self {
            id: service.id.clone(),
            name: service.name.clone(),
            project_id: service.project_id.clone(),
            project_name: service.project_name.clone(),
            project_enabled: service.project_enabled,
            primary_domain: service.primary_domain.clone(),
            url: service_url(service, endpoint),
            domains: service.domains.clone(),
            path: service.path.clone(),
            command: service.command.clone(),
            target: service.primary_target.as_ref().map(ToString::to_string),
            inferred: service.inferred,
        }
    }
}

fn resolve_service(
    service_name: &str,
    project: Option<&str>,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    let project_roots = load_project_roots();
    let services = ctx
        .config_store
        .list_services()?
        .into_iter()
        .map(|service| service_with_project_root(service, &project_roots))
        .collect::<Vec<_>>();
    let cwd = effective_current_dir().ok();
    let cwd_project_id = project
        .is_none()
        .then(|| {
            cwd.as_deref()
                .and_then(|cwd| project_id_for_cwd(&services, cwd))
        })
        .flatten();

    let matches = services
        .into_iter()
        .filter(|service| service_matches(service, service_name))
        .filter(|service| {
            project
                .map(|project| project_matches(service, project))
                .unwrap_or(true)
        })
        .filter(|service| {
            cwd_project_id
                .as_deref()
                .map(|project_id| service.project_id == project_id)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [service] => Ok(service.clone()),
        [] => {
            let scope = project
                .map(|project| format!(" in project '{project}'"))
                .or_else(|| cwd_project_id.map(|project_id| format!(" in project '{project_id}'")))
                .unwrap_or_default();
            bail!(
                "No service '{service_name}' found{scope}. Run '{} services list' to inspect configured services.",
                BRANDING.cli_command()
            );
        }
        services => {
            let options = services
                .iter()
                .map(|service| {
                    format!(
                        "  {} get --project {} {}",
                        BRANDING.cli_command(),
                        service.project_id,
                        service.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("Service '{service_name}' is ambiguous. Use --project:\n{options}");
        }
    }
}

fn resolve_run_service(
    service_name: Option<&str>,
    project: Option<&str>,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    if let Some(service_name) = service_name {
        let service = resolve_service(service_name, project, ctx)?;
        return resolve_environment_service(service, environment, ctx);
    }

    let services = ctx
        .config_store
        .list_services()?
        .into_iter()
        .filter(|service| {
            project
                .map(|project| project_matches(service, project))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let services = resolve_environment_candidates(services, environment, ctx)?;
    resolve_service_from_context(services, project, environment)
}

fn resolve_environment_candidates(
    services: Vec<ConfiguredService>,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<Vec<ConfiguredService>> {
    let Some(environment) = environment else {
        return Ok(services);
    };

    let mut resolved = Vec::new();
    let mut first_error = None;

    for service in services {
        match resolve_environment_service(service, Some(environment), ctx) {
            Ok(service) => resolved.push(service),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error.to_string());
                }
            }
        }
    }

    if resolved.is_empty()
        && let Some(error) = first_error
    {
        bail!("{error}");
    }

    Ok(resolved)
}

fn resolve_service_from_context(
    services: Vec<ConfiguredService>,
    project: Option<&str>,
    environment: Option<&str>,
) -> Result<ConfiguredService> {
    if services.is_empty() {
        let scope = project
            .map(|project| format!(" in project '{project}'"))
            .unwrap_or_default();
        bail!(
            "No services configured{scope}. Run '{} services list' to inspect configured services.",
            BRANDING.cli_command()
        );
    }

    let project_roots = load_project_roots();
    let services = services
        .into_iter()
        .map(|service| service_with_project_root(service, &project_roots))
        .collect::<Vec<_>>();
    let cwd = effective_current_dir()?;
    let cwd_project_id = project
        .is_none()
        .then(|| project_id_for_cwd(&services, &cwd))
        .flatten();
    let mut candidates = services
        .into_iter()
        .filter(|service| {
            cwd_project_id
                .as_deref()
                .map(|project_id| service.project_id == project_id)
                .unwrap_or(true)
        })
        .map(service_with_local_dev_launch)
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        left.project_id
            .cmp(&right.project_id)
            .then_with(|| left.id.cmp(&right.id))
    });

    if let Some(service) = service_for_cwd(&candidates, &cwd) {
        return Ok(service);
    }

    match candidates.as_slice() {
        [service] => Ok(service.clone()),
        [] => bail!(
            "No service could be inferred from '{}'. Run '{} services list' or pass a service explicitly.",
            cwd.display(),
            BRANDING.cli_command()
        ),
        services if is_interactive_terminal() => prompt_service_selection(services, environment),
        services => {
            let options = run_options(services, environment);
            bail!(
                "Multiple services match '{}'. Pass one explicitly:\n{options}",
                cwd.display()
            );
        }
    }
}

fn ensure_run_context_is_initialized(
    service_name: Option<&str>,
    project: Option<&str>,
    environment: Option<&str>,
    init_domain: Option<&str>,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
    ctx: &AppContext,
) -> Result<()> {
    if service_name.is_some() || project.is_some() || environment.is_some() {
        return Ok(());
    }

    let cwd = effective_current_dir()?;
    let services = services_with_project_roots(ctx)?;
    if run_context_has_configured_project_or_service(&services, &cwd) {
        if let Some(domain) = init_domain {
            bail!(
                "Cannot use --domain '{domain}' because a Localify project is already configured for '{}'. Update the existing service domain with:\n  {} project domain set <service> {domain}",
                cwd.display(),
                BRANDING.cli_command()
            );
        }
        return Ok(());
    }

    let root = auto_init_root_for_cwd(&cwd)?;
    if !auto_init_root_has_runnable_service(&root)? {
        bail!(
            "No Localify project is configured for '{}', and no runnable dev app was detected. Run '{} init-project --yes' from the app root, or pass an existing service explicitly.",
            cwd.display(),
            BRANDING.cli_command()
        );
    }

    eprintln!(
        "No Localify project found for '{}'. Initializing it first...",
        root.display()
    );
    with_current_dir(&root, || {
        init_project::execute(
            InitProjectArgs {
                name: None,
                domain: None,
                target: None,
                with_api: false,
                api_domain: None,
                api_target: None,
                project_only: false,
                yes: !is_interactive_terminal(),
                write_scripts: false,
                detected_primary_domain: init_domain.map(ToOwned::to_owned),
            },
            config_path,
            policy,
        )
    })
}

fn services_with_project_roots(ctx: &AppContext) -> Result<Vec<ConfiguredService>> {
    let project_roots = load_project_roots();
    Ok(ctx
        .config_store
        .list_services()?
        .into_iter()
        .map(|service| service_with_project_root(service, &project_roots))
        .collect())
}

fn run_context_has_configured_project_or_service(
    services: &[ConfiguredService],
    cwd: &Path,
) -> bool {
    project_id_for_cwd(services, cwd).is_some()
        || service_for_cwd(
            &services
                .iter()
                .cloned()
                .map(service_with_local_dev_launch)
                .collect::<Vec<_>>(),
            cwd,
        )
        .is_some()
}

fn auto_init_root_for_cwd(cwd: &Path) -> Result<PathBuf> {
    let canonical_cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    for ancestor in cwd.ancestors() {
        if !looks_like_workspace_root(ancestor)? {
            continue;
        }
        if init_plan_contains_cwd(ancestor, &canonical_cwd)? {
            return Ok(ancestor.to_path_buf());
        }
    }

    Ok(cwd.to_path_buf())
}

fn looks_like_workspace_root(path: &Path) -> Result<bool> {
    if path.join("pnpm-workspace.yaml").is_file() {
        return Ok(true);
    }

    let package_json_path = path.join("package.json");
    if !package_json_path.is_file() {
        return Ok(false);
    }

    let content = fs::read_to_string(&package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;
    Ok(manifest.get("workspaces").is_some())
}

fn init_plan_contains_cwd(root: &Path, cwd: &Path) -> Result<bool> {
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let plan = detect_folder_init_plan(root, None)?;
    Ok(plan.services.iter().any(|service| {
        let Some(service_path) = service.service_path.as_deref() else {
            return cwd == canonical_root || cwd.starts_with(&canonical_root);
        };
        let service_root = canonical_root.join(service_path);
        cwd == service_root || cwd.starts_with(service_root)
    }))
}

fn auto_init_root_has_runnable_service(root: &Path) -> Result<bool> {
    let plan = detect_folder_init_plan(root, None)?;
    Ok(plan
        .services
        .iter()
        .any(|service| service.enabled && service.command.is_some()))
}

fn with_current_dir<T>(dir: &Path, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let original_dir = env::current_dir().context("Failed to determine the current directory")?;
    env::set_current_dir(dir).with_context(|| format!("Failed to enter '{}'", dir.display()))?;
    let result = action();
    let restore_result = env::set_current_dir(&original_dir)
        .with_context(|| format!("Failed to restore '{}'", original_dir.display()));

    match (result, restore_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore_error)) => Err(error.context(restore_error.to_string())),
    }
}

fn service_for_cwd(services: &[ConfiguredService], cwd: &Path) -> Option<ConfiguredService> {
    let mut matches = services
        .iter()
        .filter_map(|service| service_cwd_match(service, cwd))
        .collect::<Vec<_>>();

    matches.sort_by(|(_, left), (_, right)| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });

    let (_, best_path) = matches.first()?;
    let best_len = best_path.components().count();
    let best_matches = matches
        .into_iter()
        .filter(|(_, path)| path.components().count() == best_len)
        .map(|(service, _)| service)
        .collect::<Vec<_>>();

    match best_matches.as_slice() {
        [service] => Some(service.clone()),
        _ => None,
    }
}

fn service_cwd_match(
    service: &ConfiguredService,
    cwd: &Path,
) -> Option<(ConfiguredService, PathBuf)> {
    let root_path = PathBuf::from(service.project_root_path.as_ref()?);
    if let Some(service_path) = service_cwd_match_path(service, &root_path)
        && cwd.starts_with(&service_path)
    {
        return Some((service.clone(), service_path));
    }

    pathless_service_cwd_match(service, cwd, &root_path)
        .or_else(|| local_dev_service_cwd_match(service, cwd, &root_path))
}

fn service_cwd_match_path(service: &ConfiguredService, root_path: &Path) -> Option<PathBuf> {
    let path = service.path.as_deref()?.trim();
    if path.is_empty() {
        return None;
    }
    let path = Path::new(path);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root_path.join(path)
    })
}

fn pathless_service_cwd_match(
    service: &ConfiguredService,
    cwd: &Path,
    root_path: &Path,
) -> Option<(ConfiguredService, PathBuf)> {
    if service
        .path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
    {
        return None;
    }
    if !cwd.starts_with(root_path) {
        return None;
    }

    let relative_path = cwd.strip_prefix(root_path).ok()?;
    if relative_path.as_os_str().is_empty() {
        return None;
    }

    let folder_name = cwd.file_name()?.to_string_lossy();
    if !service.id.eq_ignore_ascii_case(&folder_name)
        && !service.name.eq_ignore_ascii_case(&folder_name)
    {
        return None;
    }

    let mut service = service.clone();
    service.path = Some(relative_path.display().to_string());
    Some((service, cwd.to_path_buf()))
}

fn local_dev_service_cwd_match(
    service: &ConfiguredService,
    cwd: &Path,
    root_path: &Path,
) -> Option<(ConfiguredService, PathBuf)> {
    if service
        .path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
    {
        return None;
    }

    let launch = local_dev_launch_for_service(service, root_path)?;
    if !cwd.starts_with(&launch.package_dir) {
        return None;
    }

    let service = apply_local_dev_launch(service.clone(), root_path, &launch);

    Some((service, launch.package_dir))
}

fn service_with_local_dev_launch(service: ConfiguredService) -> ConfiguredService {
    let service = service_without_legacy_dev_localify_command(service);
    let Some(root_path) = service.project_root_path.as_ref().map(PathBuf::from) else {
        return service;
    };

    if let Some(launch) = local_dev_launch_for_service(&service, &root_path) {
        let service = apply_local_dev_launch(service, &root_path, &launch);
        if !is_legacy_dev_localify_launch(&launch) {
            return service;
        }

        return portable_local_dev_launch_for_service(&service, &root_path)
            .map(|launch| apply_local_dev_launch(service.clone(), &root_path, &launch))
            .unwrap_or(service);
    }

    portable_local_dev_launch_for_service(&service, &root_path)
        .map(|launch| apply_local_dev_launch(service.clone(), &root_path, &launch))
        .unwrap_or(service)
}

fn service_with_launch_for_mode(service: ConfiguredService, mode: RunMode) -> ConfiguredService {
    match mode {
        RunMode::Run => service_with_local_dev_launch(service),
        RunMode::Dev => service_with_dev_launch(service),
    }
}

fn service_with_dev_launch(service: ConfiguredService) -> ConfiguredService {
    let service = service_without_legacy_dev_localify_command(service);
    let Some(root_path) = service.project_root_path.as_ref().map(PathBuf::from) else {
        return service;
    };
    let launch = localify_dev_launch_for_service(&service, &root_path);
    let Some(launch) = launch else {
        return service;
    };

    let mut service = service;
    service.command = None;
    apply_local_dev_launch(service, &root_path, &launch)
}

fn localify_dev_launch_for_service(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<LocalDevLaunchPlan> {
    if let Some(package_scope) = service_cwd_match_path(service, root_path) {
        return resolve_localify_dev_launch(root_path, Some(&package_scope)).ok();
    }

    if let Some(legacy_launch) = local_dev_launch_for_service(service, root_path)
        && let Ok(launch) = resolve_localify_dev_launch(root_path, Some(&legacy_launch.package_dir))
    {
        return Some(launch);
    }

    if let Some(package_scope) = package_scope_for_service_name(service, root_path)
        && let Ok(launch) = resolve_localify_dev_launch(root_path, Some(&package_scope))
    {
        return Some(launch);
    }

    resolve_localify_dev_launch(root_path, None).ok()
}

fn package_scope_for_service_name(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<PathBuf> {
    let labels = [
        service.id.trim().to_ascii_lowercase(),
        service.name.trim().to_ascii_lowercase(),
    ];

    for directory in ["apps", "packages", "services"] {
        let parent = root_path.join(directory);
        let Ok(entries) = fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.join("package.json").is_file() {
                continue;
            }

            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            if labels
                .iter()
                .any(|label| !label.is_empty() && label == &name)
            {
                return Some(path);
            }
        }
    }

    None
}

fn local_dev_launch_for_service(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<LocalDevLaunchPlan> {
    if service
        .path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
        && service.command.is_some()
    {
        return None;
    }

    let target = match service.primary_target.as_ref()? {
        RouteTarget::Proxy(target) => target.to_string(),
        RouteTarget::StaticFiles(_) => return None,
    };
    resolve_local_dev_launch(root_path, &target).ok()
}

fn inferred_path_from_configured_route_launch(service: &ConfiguredService) -> Option<String> {
    let root_path = service.project_root_path.as_ref().map(PathBuf::from)?;
    let launch = local_dev_launch_for_service(service, &root_path)?;
    let relative_path = launch
        .package_dir
        .strip_prefix(&root_path)
        .unwrap_or(&launch.package_dir);
    if relative_path.as_os_str().is_empty() {
        return None;
    }

    Some(relative_path.display().to_string())
}

fn portable_local_dev_launch_for_service(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<LocalDevLaunchPlan> {
    if service.command.is_some() {
        return None;
    }

    let package_scope = service_cwd_match_path(service, root_path);
    resolve_portable_local_dev_launch(root_path, package_scope.as_deref()).ok()
}

fn apply_local_dev_launch(
    mut service: ConfiguredService,
    root_path: &Path,
    launch: &LocalDevLaunchPlan,
) -> ConfiguredService {
    if service
        .path
        .as_deref()
        .is_none_or(|path| path.trim().is_empty())
    {
        let relative_path = launch
            .package_dir
            .strip_prefix(root_path)
            .unwrap_or(&launch.package_dir);
        if !relative_path.as_os_str().is_empty() {
            service.path = Some(relative_path.display().to_string());
        }
    }
    // `dev-localify` is legacy metadata only. Localify can use it to recover
    // old service paths, but should never choose it as a launch command.
    if service.command.is_none() && !is_legacy_dev_localify_launch(launch) {
        service.command = Some(launch.command_parts());
    }

    service
}

fn service_without_legacy_dev_localify_command(
    mut service: ConfiguredService,
) -> ConfiguredService {
    if service
        .command
        .as_deref()
        .is_some_and(command_invokes_dev_localify)
    {
        service.command = None;
    }
    service
}

fn is_legacy_dev_localify_launch(launch: &LocalDevLaunchPlan) -> bool {
    launch.script_name == "dev-localify"
}

fn command_invokes_dev_localify(command: &[String]) -> bool {
    match command {
        [binary, script] if binary == "yarn" && script == "dev-localify" => true,
        [binary, script]
            if matches!(binary.as_str(), "pnpm" | "bun") && script == "dev-localify" =>
        {
            true
        }
        [binary, run, script, ..]
            if matches!(binary.as_str(), "npm" | "pnpm" | "bun")
                && run == "run"
                && script == "dev-localify" =>
        {
            true
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageScriptRunner {
    Npm,
    DirectArgs,
    Yarn,
}

fn prepare_local_dev_command(
    command: Vec<String>,
    workdir: &Path,
    port: u16,
) -> Result<Vec<String>> {
    let Some((runner, script_name)) = package_script_invocation(&command) else {
        return Ok(command);
    };
    let Some(script) = read_package_script(workdir, script_name)? else {
        return Ok(command);
    };
    let args = localify_dev_args_for_script(&script, port);
    if args.is_empty() {
        return Ok(command);
    }

    Ok(append_package_script_args(command, runner, args))
}

fn package_script_invocation(command: &[String]) -> Option<(PackageScriptRunner, &str)> {
    match command {
        [binary, script] if binary == "yarn" => Some((PackageScriptRunner::Yarn, script.as_str())),
        [binary, run, script, ..] if binary == "npm" && run == "run" => {
            Some((PackageScriptRunner::Npm, script.as_str()))
        }
        [binary, run, script, ..] if matches!(binary.as_str(), "pnpm" | "bun") && run == "run" => {
            Some((PackageScriptRunner::DirectArgs, script.as_str()))
        }
        _ => None,
    }
}

fn read_package_script(workdir: &Path, script_name: &str) -> Result<Option<String>> {
    let package_json_path = workdir.join("package.json");
    if !package_json_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;

    Ok(manifest
        .get("scripts")
        .and_then(|scripts| scripts.get(script_name))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
}

fn localify_dev_args_for_script(script: &str, port: u16) -> Vec<String> {
    let trimmed = script.trim();
    if trimmed.contains("next dev") {
        return host_and_port_args(
            trimmed,
            "--hostname",
            &["--hostname", "-H"],
            "127.0.0.1",
            port,
        );
    }
    if is_vite_dev_script(trimmed) || contains_dev_command(trimmed, "astro dev") {
        return host_and_port_args(trimmed, "--host", &["--host"], "127.0.0.1", port);
    }
    if contains_dev_command(trimmed, "nuxt dev") {
        return host_and_port_args(trimmed, "--host", &["--host"], "127.0.0.1", port);
    }
    if contains_dev_command(trimmed, "email dev") {
        return arg_if_needed(trimmed, "--port", &["--port", "-p"], &port.to_string());
    }

    Vec::new()
}

fn host_and_port_args(
    script: &str,
    host_flag: &str,
    host_flags: &[&str],
    host: &str,
    port: u16,
) -> Vec<String> {
    let mut args = arg_if_needed(script, host_flag, host_flags, host);
    args.extend(arg_if_needed(
        script,
        "--port",
        &["--port", "-p"],
        &port.to_string(),
    ));
    args
}

fn arg_if_needed(script: &str, flag: &str, aliases: &[&str], desired: &str) -> Vec<String> {
    if script_has_flag_value(script, aliases, desired) {
        Vec::new()
    } else {
        vec![flag.into(), desired.into()]
    }
}

fn script_has_flag_value(script: &str, flags: &[&str], desired: &str) -> bool {
    let parts = script.split_whitespace().collect::<Vec<_>>();

    for (index, part) in parts.iter().enumerate() {
        for flag in flags {
            if part == flag && parts.get(index + 1).is_some_and(|value| *value == desired) {
                return true;
            }

            let prefix = format!("{flag}=");
            if part
                .strip_prefix(&prefix)
                .is_some_and(|value| value == desired)
            {
                return true;
            }
        }
    }

    false
}

fn is_vite_dev_script(script: &str) -> bool {
    script == "vite" || script.starts_with("vite ") || script.contains(" vite ")
}

fn contains_dev_command(script: &str, command: &str) -> bool {
    script == command
        || script.starts_with(&format!("{command} "))
        || script.contains(&format!(" {command}"))
}

fn append_package_script_args(
    mut command: Vec<String>,
    runner: PackageScriptRunner,
    args: Vec<String>,
) -> Vec<String> {
    match runner {
        PackageScriptRunner::Npm => {
            if !command.iter().any(|part| part == "--") {
                command.push("--".into());
            }
            command.extend(args);
        }
        PackageScriptRunner::DirectArgs | PackageScriptRunner::Yarn => command.extend(args),
    }
    command
}

fn prompt_service_selection(
    services: &[ConfiguredService],
    environment: Option<&str>,
) -> Result<ConfiguredService> {
    let labels = services
        .iter()
        .map(service_selection_label)
        .collect::<Vec<_>>();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select a service to run")
        .items(&labels)
        .default(0)
        .interact_opt()
        .context("Failed to read service selection")?;

    match selection {
        Some(index) => services.get(index).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid service selection. Pass one explicitly:\n{}",
                run_options(services, environment)
            )
        }),
        None => bail!(
            "Service selection cancelled. Pass one explicitly:\n{}",
            run_options(services, environment)
        ),
    }
}

fn service_selection_label(service: &ConfiguredService) -> String {
    let path = service.path.as_deref().unwrap_or(".");
    let command = service
        .command
        .as_ref()
        .map(|command| command.join(" "))
        .unwrap_or_else(|| "no command configured".into());
    format!("{} ({path}) - {command}", service.id)
}

fn run_options(services: &[ConfiguredService], environment: Option<&str>) -> String {
    services
        .iter()
        .map(|service| {
            let env_flag = environment
                .map(|environment| format!(" --env {environment}"))
                .unwrap_or_default();
            format!(
                "  {} dev {} --project {}{}",
                BRANDING.cli_command(),
                service.id,
                service.project_id,
                env_flag
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn service_matches(service: &ConfiguredService, service_name: &str) -> bool {
    service.id == service_name || service.name.eq_ignore_ascii_case(service_name)
}

fn project_matches(service: &ConfiguredService, project: &str) -> bool {
    service.project_id == project || service.project_name.eq_ignore_ascii_case(project)
}

fn resolve_environment_service(
    service: ConfiguredService,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    let Some(environment) = environment else {
        return Ok(service);
    };

    let environments = ctx
        .config_store
        .list_project_environments(Some(&service.project_id))?;
    let matches = environments
        .into_iter()
        .filter(|candidate| environment_matches(candidate, environment))
        .collect::<Vec<_>>();

    let environment = match matches.as_slice() {
        [environment] => environment.clone(),
        [] => bail!(
            "Project environment '{environment}' was not found for project '{}'. Create it with: {} project env ensure --project {} --name {}",
            service.project_id,
            BRANDING.cli_command(),
            service.project_id,
            environment
        ),
        _ => bail!(
            "Project environment '{environment}' is ambiguous in project '{}'. Use its slug.",
            service.project_id
        ),
    };

    service_for_environment(service, &environment, ctx)
}

fn environment_matches(environment: &ConfiguredProjectEnvironment, value: &str) -> bool {
    environment.slug == value || environment.name.eq_ignore_ascii_case(value)
}

fn service_for_environment(
    mut service: ConfiguredService,
    environment: &ConfiguredProjectEnvironment,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    let mapping = environment
        .services
        .iter()
        .find(|mapping| mapping.service_id == service.id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Service '{}' is unavailable in environment '{}'. Re-run '{} project env ensure --project {} --name {}'.",
                service.id,
                environment.slug,
                BRANDING.cli_command(),
                service.project_id,
                environment.slug
            )
        })?;

    let inferred_service_path = service
        .path
        .clone()
        .or_else(|| inferred_path_from_configured_route_launch(&service));
    service.primary_domain = mapping.domain.clone();
    service.domains = vec![mapping.domain.clone()];
    service.project_root_path = environment
        .root_path
        .clone()
        .or_else(|| service.project_root_path.clone());
    if service
        .path
        .as_deref()
        .is_none_or(|path| path.trim().is_empty())
    {
        service.path = inferred_service_path;
    }
    service.primary_target = domain_root_target(&mapping.domain, ctx)?;
    Ok(service)
}

fn domain_root_target(domain: &str, ctx: &AppContext) -> Result<Option<RouteTarget>> {
    let target = ctx
        .config_store
        .list_configured_domains()?
        .into_iter()
        .find(|configured| configured.registration.display_pattern() == domain)
        .and_then(|configured| root_route_target(configured.registration));
    Ok(target)
}

fn root_route_target(registration: DomainRegistration) -> Option<RouteTarget> {
    registration
        .routes()
        .iter()
        .find(|route| route.path().to_string() == "/")
        .map(|route| route.target().clone())
}

fn project_id_for_cwd(services: &[ConfiguredService], cwd: &Path) -> Option<String> {
    let mut candidates = services
        .iter()
        .filter_map(|service| {
            let root_path = service.project_root_path.as_ref()?;
            let root_path = PathBuf::from(root_path);
            if cwd.starts_with(&root_path) {
                Some((service.project_id.clone(), root_path))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|(_, left), (_, right)| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });
    candidates.dedup_by(|left, right| left.0 == right.0);

    let (project_id, root_path) = candidates.first()?;
    let root_len = root_path.components().count();
    let equally_specific = candidates
        .iter()
        .filter(|(_, candidate_root)| candidate_root.components().count() == root_len)
        .map(|(candidate_project_id, _)| candidate_project_id)
        .collect::<std::collections::HashSet<_>>();

    if equally_specific.len() == 1 {
        Some(project_id.clone())
    } else {
        None
    }
}

fn load_project_roots() -> HashMap<String, PathBuf> {
    ProjectRootStore::new_default()
        .and_then(|store| store.load())
        .unwrap_or_default()
        .into_iter()
        .map(|(project_id, root_path)| (project_id, PathBuf::from(root_path)))
        .collect()
}

fn service_with_project_root(
    mut service: ConfiguredService,
    project_roots: &HashMap<String, PathBuf>,
) -> ConfiguredService {
    let should_use_local_root = service
        .project_root_path
        .as_deref()
        .map(|root_path| !Path::new(root_path).exists())
        .unwrap_or(true);
    if should_use_local_root && let Some(root_path) = project_roots.get(&service.project_id) {
        service.project_root_path = Some(root_path.display().to_string());
    }
    service
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DaemonPorts {
    http: u16,
    https: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServiceEndpoint {
    scheme: &'static str,
    port: u16,
}

impl ServiceEndpoint {
    const fn http(port: u16) -> Self {
        Self {
            scheme: "http",
            port,
        }
    }

    const fn https(port: u16) -> Self {
        Self {
            scheme: "https",
            port,
        }
    }

    fn is_default_port(self) -> bool {
        matches!((self.scheme, self.port), ("http", 80) | ("https", 443))
    }
}

fn daemon_ports(ctx: &AppContext) -> Result<DaemonPorts> {
    let config = ctx.config_store.load()?.daemon;

    Ok(DaemonPorts {
        http: config.http_port,
        https: config.https_port,
    })
}

fn service_endpoint(
    service: &ConfiguredService,
    ports: DaemonPorts,
    ctx: &AppContext,
) -> Result<ServiceEndpoint> {
    let https_enabled = ctx
        .config_store
        .find_registration_for_domain(&service.primary_domain)?
        .map(|registration| registration.is_https_enabled())
        .unwrap_or(false);

    if https_enabled {
        Ok(ServiceEndpoint::https(ports.https))
    } else {
        Ok(ServiceEndpoint::http(ports.http))
    }
}

fn service_public_host(service: &ConfiguredService) -> String {
    let primary_domain = service.primary_domain.trim();
    let public_host = primary_domain
        .strip_prefix("*.")
        .unwrap_or(primary_domain)
        .to_string();
    debug_assert!(!public_host.starts_with("*."));
    public_host
}

fn service_public_domain(service: &ConfiguredService, endpoint: ServiceEndpoint) -> String {
    let public_host = service_public_host(service);
    if endpoint.is_default_port() {
        public_host
    } else {
        format!("{public_host}:{}", endpoint.port)
    }
}

fn service_url(service: &ConfiguredService, endpoint: ServiceEndpoint) -> String {
    format!(
        "{}://{}",
        endpoint.scheme,
        service_public_domain(service, endpoint)
    )
}

fn service_proxy_target(service: &ConfiguredService) -> Result<&ProxyTarget> {
    match service.primary_target.as_ref() {
        Some(RouteTarget::Proxy(target)) => Ok(target),
        Some(RouteTarget::StaticFiles(path)) => bail!(
            "Service '{}' primary domain points to static files at '{}', not a runnable proxy target.",
            service.id,
            path.display()
        ),
        None => bail!(
            "Service '{}' has no root proxy route for primary domain '{}'. Add a '/' route or pass through the existing service manually.",
            service.id,
            service.primary_domain
        ),
    }
}

fn is_supported_run_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost")
}

fn ensure_port_available(port: u16, service: &ConfiguredService) -> Result<()> {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(error) => bail!(
            "Port {port} for service '{}' is already in use ({error}). Stop that process or update the route target. Inspect with: lsof -nP -iTCP:{port} -sTCP:LISTEN",
            service.id
        ),
    }
}

fn service_workdir(service: &ConfiguredService) -> Result<PathBuf> {
    let root_path = service.project_root_path.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "Service '{}' is unavailable: project '{}' has no root path metadata.",
            service.id,
            service.project_id
        )
    })?;

    let root_path = PathBuf::from(root_path);
    let workdir = service
        .path
        .as_deref()
        .map(Path::new)
        .map(|path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root_path.join(path)
            }
        })
        .unwrap_or(root_path);

    if !workdir.exists() {
        bail!(
            "Service '{}' is unavailable: '{}' does not exist.",
            service.id,
            workdir.display()
        );
    }
    if !workdir.is_dir() {
        bail!(
            "Service '{}' is unavailable: '{}' is not a directory.",
            service.id,
            workdir.display()
        );
    }

    Ok(workdir)
}

fn warn_if_daemon_is_stopped(ctx: &AppContext) {
    if matches!(ctx.pid_file.is_running(), Ok(false)) {
        eprintln!(
            "Localify daemon does not appear to be running. Start it separately with '{} start'.",
            BRANDING.cli_command()
        );
    }
}

fn ensure_next_allowed_dev_origin(workdir: &Path, app_host: &str) {
    let Some(next_config_path) = find_existing_next_config_path(workdir) else {
        return;
    };

    match apply_next_allowed_dev_origin(&next_config_path, app_host) {
        Ok(true) => eprintln!(
            "  updated Next.js allowedDevOrigins: {}",
            next_config_path.display()
        ),
        Ok(false) => {}
        Err(error) => eprintln!(
            "  skipped Next.js allowedDevOrigins update: {} - {}",
            next_config_path.display(),
            error
        ),
    }
}

fn ensure_vite_allowed_host(workdir: &Path, app_host: &str) {
    let Some(vite_config_path) = find_existing_vite_config_path(workdir) else {
        return;
    };

    match apply_vite_allowed_host(&vite_config_path, app_host) {
        Ok(true) => eprintln!(
            "  updated Vite server.allowedHosts: {}",
            vite_config_path.display()
        ),
        Ok(false) => {}
        Err(error) => eprintln!(
            "  skipped Vite server.allowedHosts update: {} - {}",
            vite_config_path.display(),
            error
        ),
    }
}

fn url_environment_overrides(
    workdir: &Path,
    url: &str,
    app_domain: &str,
) -> Result<Vec<(String, String)>> {
    let mut keys = BTreeSet::new();
    for key in DEFAULT_URL_ENV_KEYS
        .iter()
        .chain(DEFAULT_DOMAIN_ENV_KEYS.iter())
    {
        keys.insert((*key).to_string());
    }
    for key in discover_url_env_keys(workdir)? {
        keys.insert(key);
    }

    Ok(keys
        .into_iter()
        .filter_map(|key| {
            let value = url_env_value(&key, url, app_domain)?;
            Some((key, value.to_string()))
        })
        .collect())
}

fn discover_url_env_keys(workdir: &Path) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    for file_name in ENV_FILE_NAMES {
        let path = workdir.join(file_name);
        if !path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read env file '{}'", path.display()))?;
        for line in content.lines() {
            if let Some(key) = parse_env_key(line)
                && is_safe_url_env_key(&key)
            {
                keys.push(key);
            }
        }
    }
    Ok(keys)
}

fn parse_env_key(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let assignment = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let key = assignment.split_once('=')?.0.trim();
    is_valid_env_key(key).then(|| key.to_string())
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_safe_url_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    !contains_sensitive_env_fragment(&key) && url_env_value_kind(&key).is_some()
}

fn url_env_value<'a>(key: &str, url: &'a str, app_domain: &'a str) -> Option<&'a str> {
    match url_env_value_kind(&key.to_ascii_uppercase())? {
        UrlEnvValueKind::Url => Some(url),
        UrlEnvValueKind::Domain => Some(app_domain),
    }
}

fn url_env_value_kind(key: &str) -> Option<UrlEnvValueKind> {
    if DEFAULT_URL_ENV_KEYS.contains(&key) {
        return Some(UrlEnvValueKind::Url);
    }
    if DEFAULT_DOMAIN_ENV_KEYS.contains(&key) {
        return Some(UrlEnvValueKind::Domain);
    }

    let base = public_env_base_name(key);
    if is_app_url_env_base(base) {
        return Some(UrlEnvValueKind::Url);
    }
    if is_app_domain_env_base(base) {
        return Some(UrlEnvValueKind::Domain);
    }

    None
}

fn public_env_base_name(key: &str) -> &str {
    for prefix in [
        "NEXT_PUBLIC_",
        "NUXT_PUBLIC_",
        "EXPO_PUBLIC_",
        "REACT_APP_",
        "VITE_",
        "PUBLIC_",
    ] {
        if let Some(base) = key.strip_prefix(prefix) {
            return base;
        }
    }
    key
}

fn is_app_url_env_base(base: &str) -> bool {
    matches!(
        base,
        "APP_URL"
            | "WEB_URL"
            | "SITE_URL"
            | "BASE_URL"
            | "PUBLIC_URL"
            | "FRONTEND_URL"
            | "CLIENT_URL"
            | "ROOT_URL"
            | "LOCALIFY_URL"
            | "AUTH_URL"
            | "BETTER_AUTH_URL"
            | "APP_ORIGIN"
            | "WEB_ORIGIN"
            | "SITE_ORIGIN"
            | "ORIGIN"
            | "URL"
    )
}

fn is_app_domain_env_base(base: &str) -> bool {
    matches!(
        base,
        "APP_DOMAIN"
            | "WEB_DOMAIN"
            | "SITE_DOMAIN"
            | "PUBLIC_DOMAIN"
            | "FRONTEND_DOMAIN"
            | "CLIENT_DOMAIN"
            | "LOCALIFY_DOMAIN"
            | "DOMAIN"
    )
}

fn contains_sensitive_env_fragment(key: &str) -> bool {
    [
        "SECRET", "TOKEN", "PASSWORD", "PASS", "KEY", "DSN", "DATABASE", "POSTGRES", "MYSQL",
        "MONGO", "REDIS", "SMTP", "RESEND", "STRIPE", "S3", "AWS", "R2", "BUCKET", "STORAGE",
        "DIRECTUS", "WEBHOOK", "PRIVATE", "INTERNAL",
    ]
    .iter()
    .any(|fragment| key.contains(fragment))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlEnvValueKind {
    Url,
    Domain,
}

fn env_key_list(envs: &[(String, String)]) -> String {
    envs.iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service(id: &str, root_path: &str, service_path: Option<&str>) -> ConfiguredService {
        ConfiguredService {
            id: id.into(),
            name: id.into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: Some(root_path.into()),
            primary_domain: format!("{id}.demo.localify"),
            domains: vec![format!("{id}.demo.localify")],
            path: service_path.map(Into::into),
            command: Some(vec!["pnpm".into(), "dev".into()]),
            primary_target: None,
            inferred: false,
        }
    }

    fn test_service_without_root(id: &str, service_path: Option<&str>) -> ConfiguredService {
        ConfiguredService {
            id: id.into(),
            name: id.into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: None,
            primary_domain: format!("{id}.demo.localify"),
            domains: vec![format!("{id}.demo.localify")],
            path: service_path.map(Into::into),
            command: Some(vec!["pnpm".into(), "dev".into()]),
            primary_target: None,
            inferred: false,
        }
    }

    #[test]
    fn run_host_support_matches_injected_host_contract() {
        assert!(is_supported_run_host("127.0.0.1"));
        assert!(is_supported_run_host("localhost"));
        assert!(!is_supported_run_host("::1"));
        assert!(!is_supported_run_host("127.0.0.2"));
    }

    #[test]
    fn service_url_strips_only_one_wildcard_prefix() {
        let service = ConfiguredService {
            id: "web".into(),
            name: "web".into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: None,
            primary_domain: "*.demo.localify".into(),
            domains: vec!["*.demo.localify".into()],
            path: None,
            command: None,
            primary_target: None,
            inferred: true,
        };

        assert_eq!(
            service_url(&service, ServiceEndpoint::https(443)),
            "https://demo.localify"
        );
    }

    #[test]
    fn service_url_includes_non_default_endpoint_port() {
        let service = ConfiguredService {
            id: "web".into(),
            name: "web".into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: None,
            primary_domain: "feat-auth.demo.localify.test".into(),
            domains: vec!["feat-auth.demo.localify.test".into()],
            path: None,
            command: None,
            primary_target: None,
            inferred: true,
        };

        assert_eq!(
            service_url(&service, ServiceEndpoint::https(8443)),
            "https://feat-auth.demo.localify.test:8443"
        );
        assert_eq!(
            service_url(&service, ServiceEndpoint::http(8080)),
            "http://feat-auth.demo.localify.test:8080"
        );
    }

    #[test]
    fn service_selection_label_includes_path_and_command() {
        let service = test_service("web", "/repo", Some("apps/app"));

        assert_eq!(
            service_selection_label(&service),
            "web (apps/app) - pnpm dev"
        );
    }

    #[test]
    fn service_selection_label_handles_root_and_missing_command() {
        let mut service = test_service("server", "/repo", None);
        service.command = None;

        assert_eq!(
            service_selection_label(&service),
            "server (.) - no command configured"
        );
    }

    #[test]
    fn prepare_local_dev_command_adds_vite_runtime_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev:bare":"ALCHEMY=false vite dev"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev:bare".into()],
            temp_dir.path(),
            58522,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev:bare",
                "--host",
                "127.0.0.1",
                "--port",
                "58522"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_adds_next_runtime_flags_with_npm_separator() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["npm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "npm",
                "run",
                "dev",
                "--",
                "--hostname",
                "127.0.0.1",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_skips_matching_next_runtime_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev --hostname 127.0.0.1 --port 4010"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(command, vec!["pnpm", "run", "dev"]);
    }

    #[test]
    fn prepare_local_dev_command_overrides_stale_next_runtime_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev --hostname 0.0.0.0 --port=3000"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev",
                "--hostname",
                "127.0.0.1",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_adds_email_runtime_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"email dev --port 3003"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            52168,
        )
        .unwrap();

        assert_eq!(command, vec!["pnpm", "run", "dev", "--port", "52168"]);
    }

    #[test]
    fn ensure_next_allowed_dev_origin_updates_existing_next_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("next.config.js");
        std::fs::write(
            &config_path,
            "module.exports = {\n  reactStrictMode: true,\n};\n",
        )
        .unwrap();

        ensure_next_allowed_dev_origin(temp_dir.path(), "fix-branch.demo.localify");
        ensure_next_allowed_dev_origin(temp_dir.path(), "fix-branch.demo.localify");

        let updated = std::fs::read_to_string(config_path).unwrap();
        assert!(updated.contains("allowedDevOrigins: ['fix-branch.demo.localify']"));
        assert_eq!(updated.matches("fix-branch.demo.localify").count(), 1);
    }

    #[test]
    fn ensure_vite_allowed_host_updates_existing_vite_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        std::fs::write(
            &config_path,
            "import { defineConfig } from 'vite';\n\nexport default defineConfig({\n  plugins: [],\n});\n",
        )
        .unwrap();

        ensure_vite_allowed_host(temp_dir.path(), "fix-branch.demo.localify");
        ensure_vite_allowed_host(temp_dir.path(), "fix-branch.demo.localify");

        let updated = std::fs::read_to_string(config_path).unwrap();
        assert!(updated.contains("server: { allowedHosts: ['fix-branch.demo.localify'] }"));
        assert_eq!(updated.matches("fix-branch.demo.localify").count(), 1);
    }

    #[test]
    fn ensure_vite_allowed_host_skips_unsupported_vite_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let original = "import { defineConfig } from 'vite';\nconst config = loadConfig();\nexport default defineConfig(config);\n";
        std::fs::write(&config_path, original).unwrap();

        ensure_vite_allowed_host(temp_dir.path(), "fix-branch.demo.localify");

        let updated = std::fs::read_to_string(config_path).unwrap();
        assert_eq!(updated, original);
    }

    #[test]
    fn url_environment_overrides_include_default_runtime_keys() {
        let temp_dir = tempfile::tempdir().unwrap();
        let url = "https://feat.demo.localify.test:8443";
        let domain = "feat.demo.localify.test:8443";

        let envs = url_environment_overrides(temp_dir.path(), url, domain).unwrap();

        assert_eq!(env_value(&envs, "LOCALIFY_URL"), Some(url));
        assert_eq!(env_value(&envs, "NEXT_PUBLIC_APP_URL"), Some(url));
        assert_eq!(env_value(&envs, "AUTH_URL"), Some(url));
        assert_eq!(env_value(&envs, "VITE_APP_URL"), Some(url));
        assert_eq!(env_value(&envs, "LOCALIFY_DOMAIN"), Some(domain));
        assert_eq!(env_value(&envs, "NEXT_PUBLIC_APP_DOMAIN"), Some(domain));
    }

    #[test]
    fn url_environment_overrides_discovers_safe_env_file_keys_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join(".env"),
            "\
# ignored
VITE_SITE_URL=https://old.example
PUBLIC_DOMAIN=old.example
export WEB_ORIGIN=https://old.example
DATABASE_URL=postgres://secret
STRIPE_PUBLIC_URL=https://stripe.example
NEXT_PUBLIC_API_KEY=secret
DIRECTUS_URL=https://directus.example
INVALID-NAME=https://old.example
",
        )
        .unwrap();

        let url = "https://feat.demo.localify.test:8443";
        let domain = "feat.demo.localify.test:8443";
        let envs = url_environment_overrides(temp_dir.path(), url, domain).unwrap();

        assert_eq!(env_value(&envs, "VITE_SITE_URL"), Some(url));
        assert_eq!(env_value(&envs, "WEB_ORIGIN"), Some(url));
        assert_eq!(env_value(&envs, "PUBLIC_DOMAIN"), Some(domain));
        assert_eq!(env_value(&envs, "DATABASE_URL"), None);
        assert_eq!(env_value(&envs, "STRIPE_PUBLIC_URL"), None);
        assert_eq!(env_value(&envs, "NEXT_PUBLIC_API_KEY"), None);
        assert_eq!(env_value(&envs, "DIRECTUS_URL"), None);
        assert_eq!(env_value(&envs, "INVALID-NAME"), None);
    }

    #[test]
    fn parse_env_key_accepts_exported_dotenv_assignments() {
        assert_eq!(
            parse_env_key("export NEXT_PUBLIC_APP_URL=https://old.example").as_deref(),
            Some("NEXT_PUBLIC_APP_URL")
        );
        assert_eq!(parse_env_key(" # comment"), None);
        assert_eq!(parse_env_key("BAD-NAME=value"), None);
    }

    fn env_value<'a>(envs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        envs.iter()
            .find(|(env_key, _)| env_key == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn service_for_cwd_detects_nested_service_path() {
        let services = vec![
            test_service("api", "/repo", Some("apps/api")),
            test_service("web", "/repo", Some("apps/web")),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/apps/web/src")).unwrap();

        assert_eq!(service.id, "web");
    }

    #[test]
    fn service_for_cwd_does_not_pick_root_service_when_at_project_root() {
        let services = vec![
            test_service("api", "/repo", Some("apps/api")),
            test_service("web", "/repo", None),
        ];

        let service = service_for_cwd(&services, Path::new("/repo"));

        assert!(service.is_none());
    }

    #[test]
    fn service_for_cwd_uses_most_specific_matching_service() {
        let services = vec![
            test_service("root", "/repo", Some(".")),
            test_service("web", "/repo", Some("apps/web")),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/apps/web")).unwrap();

        assert_eq!(service.id, "web");
    }

    #[test]
    fn service_for_cwd_detects_pathless_service_from_folder_name() {
        let services = vec![
            test_service("api", "/repo", None),
            test_service("web", "/repo", None),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/web")).unwrap();

        assert_eq!(service.id, "web");
        assert_eq!(service.path.as_deref(), Some("web"));
    }

    #[test]
    fn service_for_cwd_prefers_pathless_folder_match_over_root_service() {
        let services = vec![
            test_service("root", "/repo", Some(".")),
            test_service("web", "/repo", None),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/web")).unwrap();

        assert_eq!(service.id, "web");
        assert_eq!(service.path.as_deref(), Some("web"));
    }

    #[test]
    fn service_with_project_root_uses_local_metadata_fallback() {
        let roots = HashMap::from([("demo".into(), PathBuf::from("/repo"))]);

        let service = service_with_project_root(test_service_without_root("web", None), &roots);

        assert_eq!(service.project_root_path.as_deref(), Some("/repo"));
    }

    #[test]
    fn run_context_has_configured_project_or_service_when_cwd_is_inside_project() {
        let services = vec![
            test_service("api", "/repo", Some("apps/api")),
            test_service("web", "/repo", Some("apps/web")),
        ];

        assert!(run_context_has_configured_project_or_service(
            &services,
            Path::new("/repo/apps/web")
        ));
        assert!(!run_context_has_configured_project_or_service(
            &services,
            Path::new("/other/apps/web")
        ));
    }

    #[test]
    fn auto_init_root_for_cwd_prefers_workspace_root_for_nested_app() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        let nested_dir = app_dir.join("src/routes");
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();

        let init_root = auto_init_root_for_cwd(&nested_dir).unwrap();

        assert_eq!(init_root, root);
        assert!(auto_init_root_has_runnable_service(&init_root).unwrap());
    }

    #[test]
    fn auto_init_root_has_runnable_service_rejects_plain_folder() {
        let temp_dir = tempfile::tempdir().unwrap();

        assert!(!auto_init_root_has_runnable_service(temp_dir.path()).unwrap());
    }

    #[test]
    fn service_for_cwd_detects_pathless_service_from_matching_local_dev_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_for_cwd(&[service], &app_dir).unwrap();

        assert_eq!(service.id, "acme");
        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(service.command, None);
    }

    #[test]
    fn service_with_local_dev_launch_keeps_legacy_dev_localify_path_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_with_local_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(service.command, None);
    }

    #[test]
    fn service_with_local_dev_launch_replaces_persisted_dev_localify_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165","dev":"next dev"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]);
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_with_local_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_local_dev_launch_uses_portable_script_for_worktree_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{
  "private": true,
  "workspaces": ["apps/*"],
  "packageManager": "pnpm@10.0.0",
  "scripts": {
    "dev": "turbo run dev"
  }
}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{
  "name": "web",
  "scripts": {
    "dev-localify": "next dev --hostname 127.0.0.1 --port 61092",
    "dev": "next dev"
  }
}"#,
        )
        .unwrap();
        let mut service = test_service("store", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("63612").unwrap()));

        let service = service_with_local_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_dev_launch_uses_dev_script_from_legacy_localify_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165","dev":"next dev"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_dev_launch_replaces_persisted_dev_localify_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"echo Localify URL: https://store.localify && AUTH_URL=https://store.localify NEXT_PUBLIC_APP_URL=https://store.localify NEXT_PUBLIC_APP_DOMAIN=store.localify next dev --hostname 127.0.0.1 --port 61092","dev":"next dev"}}"#,
        )
        .unwrap();
        let mut service = test_service("store", &root.display().to_string(), Some("apps/web"));
        service.command = Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]);
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52199").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn command_invokes_dev_localify_detects_legacy_package_runner_forms() {
        assert!(command_invokes_dev_localify(&[
            "pnpm".into(),
            "dev-localify".into()
        ]));
        assert!(command_invokes_dev_localify(&[
            "pnpm".into(),
            "run".into(),
            "dev-localify".into()
        ]));
        assert!(command_invokes_dev_localify(&[
            "yarn".into(),
            "dev-localify".into()
        ]));
        assert!(!command_invokes_dev_localify(&[
            "pnpm".into(),
            "run".into(),
            "dev".into()
        ]));
    }

    #[test]
    fn worktree_dev_launch_reuses_inferred_package_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let primary_root = temp_dir.path().join("primary");
        let worktree_root = temp_dir.path().join("worktree");
        let primary_app_dir = primary_root.join("apps/org-space");
        let worktree_app_dir = worktree_root.join("apps/org-space");
        std::fs::create_dir_all(&primary_app_dir).unwrap();
        std::fs::create_dir_all(&worktree_app_dir).unwrap();
        for root in [&primary_root, &worktree_root] {
            std::fs::write(
                root.join("package.json"),
                r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
            )
            .unwrap();
        }
        std::fs::write(
            primary_app_dir.join("package.json"),
            r#"{"name":"org-space","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52164","dev":"next dev"}}"#,
        )
        .unwrap();
        std::fs::write(
            worktree_app_dir.join("package.json"),
            r#"{"name":"org-space","scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();

        let mut service = test_service("acteur", &primary_root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52164").unwrap()));
        let inferred_path = inferred_path_from_configured_route_launch(&service);
        service.project_root_path = Some(worktree_root.display().to_string());
        service.path = inferred_path;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("54481").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/org-space"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_dev_launch_uses_matching_service_folder_without_legacy_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/email");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"email","scripts":{"dev":"email dev --port 3003"}}"#,
        )
        .unwrap();
        let mut service = test_service("email", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52168").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/email"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }
}
