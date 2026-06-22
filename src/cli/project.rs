use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::branding::BRANDING;
use roxy::application::ports::DaemonConnection;
use roxy::application::register_domain::RegisterDomain;
use roxy::domain::{DomainPattern, Route};
use roxy::infrastructure::certs::CertificateService;
use roxy::infrastructure::config::{ConfigStore, ConfiguredProject};
use roxy::infrastructure::mgmt_client::MgmtSocketClient;
use roxy::project_init::{
    FolderInitSelection, LocalAppUpdatePolicy, ServicePlan, apply_local_app_updates,
    detect_folder_init_plan, resolve_folder_init_selection,
};

pub struct ConfigureDevArgs {
    pub project: Option<String>,
}

pub struct AddServiceArgs {
    pub path: PathBuf,
    pub project: Option<String>,
    pub domain: Option<String>,
    pub target: Option<String>,
}

pub fn configure_dev(
    args: ConfigureDevArgs,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
) -> Result<()> {
    let current_dir = env::current_dir().context("Failed to determine the current directory")?;
    let config_store = ConfigStore::new(config_path.to_path_buf());
    let project = resolve_project(&config_store, args.project.as_deref(), &current_dir)?;
    let detected = detect_folder_init_plan(&current_dir, Some(&project.name))?;
    let selections = detected
        .services
        .into_iter()
        .map(|service| FolderInitSelection {
            id: service.id,
            domain: service.domain,
            target: service.target,
            enabled: service.enabled,
            update_dev_script: service.can_update_dev_script,
        })
        .collect::<Vec<_>>();
    let resolved = resolve_folder_init_selection(&current_dir, &project.name, &selections, policy)?;
    let update_results = apply_local_app_updates(
        &resolved.package_updates,
        &resolved.next_config_updates,
        &resolved.vite_config_updates,
    );

    print_update_results(&update_results.updated, &update_results.skipped);
    Ok(())
}

pub fn add_service(
    args: AddServiceArgs,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
) -> Result<()> {
    let current_dir = env::current_dir().context("Failed to determine the current directory")?;
    let config_store = ConfigStore::new(config_path.to_path_buf());
    let project = resolve_project(&config_store, args.project.as_deref(), &current_dir)?;
    let service_path = normalize_service_path(&current_dir, &args.path)?;
    let detected = detect_folder_init_plan(&current_dir, Some(&project.name))?;
    let service = detected
        .services
        .into_iter()
        .find(|service| normalize_display_path(&service.display_path) == service_path)
        .ok_or_else(|| {
            anyhow!(
                "Could not find a detected app at '{}'. Run '{} init-project --yes' first or pass a path to a folder with a package.json dev script.",
                args.path.display(),
                BRANDING.cli_command()
            )
        })?;
    let domain = args.domain.unwrap_or(service.domain);
    let target = args.target.unwrap_or(service.target);
    let selection = FolderInitSelection {
        id: service.id,
        domain,
        target,
        enabled: true,
        update_dev_script: service.can_update_dev_script,
    };
    let resolved =
        resolve_folder_init_selection(&current_dir, &project.name, &[selection], policy)?;
    let Some(service) = resolved.services.first() else {
        bail!("No service was resolved for '{}'", args.path.display());
    };

    register_project_service(&config_store, &project.id, service)?;
    let update_results = apply_local_app_updates(
        &resolved.package_updates,
        &resolved.next_config_updates,
        &resolved.vite_config_updates,
    );

    println!("Added service: {} -> {}", service.domain, service.target);
    print_update_results(&update_results.updated, &update_results.skipped);
    Ok(())
}

fn resolve_project(
    config_store: &ConfigStore,
    project_id: Option<&str>,
    current_dir: &Path,
) -> Result<ConfiguredProject> {
    let projects = config_store.list_projects()?;
    if let Some(project_id) = project_id {
        return projects
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| anyhow!("Project not found: {project_id}"));
    }

    let current = current_dir.display().to_string();
    let matches = projects
        .into_iter()
        .filter(|project| project.root_path.as_deref() == Some(current.as_str()))
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [project] => Ok(project.clone()),
        [] => bail!(
            "No Localify project is linked to '{}'. Run '{} init-project --yes' first.",
            current_dir.display(),
            BRANDING.cli_command()
        ),
        _ => bail!(
            "Multiple Localify projects are linked to '{}'. Pass --project <project-id>.",
            current_dir.display()
        ),
    }
}

fn normalize_service_path(root: &Path, path: &Path) -> Result<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let relative = absolute
        .strip_prefix(root)
        .with_context(|| format!("'{}' is not inside '{}'", path.display(), root.display()))?;
    Ok(normalize_display_path(&relative.display().to_string()))
}

fn normalize_display_path(path: &str) -> String {
    path.trim_matches('/').replace('\\', "/")
}

fn register_project_service(
    config_store: &ConfigStore,
    project_id: &str,
    service: &ServicePlan,
) -> Result<()> {
    let config = config_store.load()?;
    let certs = CertificateService::new(&config.paths);
    let pattern = DomainPattern::from_name(&service.domain, false)?;
    let route = Route::parse(&format!("/={}", service.target))
        .map_err(|error| anyhow!("Invalid target for '{}': {error}", service.domain))?;
    let result = RegisterDomain::new(config_store, &certs).execute(pattern, vec![route])?;
    config_store.update_domain_with_project(result.registration, Some(project_id.to_string()))?;

    match MgmtSocketClient::new(&config.paths.socket_path).reload() {
        Ok(()) => println!("  Daemon: reloaded"),
        Err(roxy::application::ports::DaemonConnectionError::NotRunning) => {
            println!("  Daemon: not running");
            println!("  Start it with: {} start", BRANDING.cli_command());
        }
        Err(error) => bail!("Failed to reload the running daemon: {error}"),
    }

    Ok(())
}

fn print_update_results(updated: &[String], skipped: &[String]) {
    if !updated.is_empty() {
        println!("  Updated local app files:");
        for updated in updated {
            println!("    {updated}");
        }
    }
    if !skipped.is_empty() {
        println!("  Skipped local app file updates:");
        for skipped in skipped {
            println!("    {skipped}");
        }
    }
}
