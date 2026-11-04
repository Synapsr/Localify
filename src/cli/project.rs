use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use super::context::{AppContext, direct_api, effective_current_dir, require_config_writable};
use crate::branding::BRANDING;
use crate::cli::project_env;
use crate::infrastructure::config::{ConfigStore, ConfiguredProject};
use crate::infrastructure::project_marker::{find_project_marker, write_project_marker};
use localify::desktop::ProjectRootStore;
use localify::desktop::dto::{
    AddProjectServiceRequest, AddProjectServiceResultDto, MaterializeProjectServicesRequest,
    SetProjectServiceDomainRequest,
};
use localify::project_init::{
    FolderInitSelection, LocalAppUpdatePolicy, ServicePlan, apply_local_app_updates,
    detect_folder_init_plan, resolve_folder_init_selection,
};

pub struct ConfigureDevArgs {
    pub project: Option<String>,
    pub write_scripts: bool,
}

pub struct AddServiceArgs {
    pub path: PathBuf,
    pub project: Option<String>,
    pub domain: Option<String>,
    pub target: Option<String>,
    pub write_scripts: bool,
}

pub struct SetDomainArgs {
    pub service: String,
    pub domain: String,
    pub project: Option<String>,
    pub environment: Option<String>,
}

pub fn configure_dev(
    args: ConfigureDevArgs,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
) -> Result<()> {
    let current_dir = effective_current_dir()?;
    let config_store = ConfigStore::new(config_path.to_path_buf());
    let project = resolve_project(&config_store, args.project.as_deref(), &current_dir)?;
    let policy = policy.with_package_scripts(args.write_scripts || policy.write_package_scripts);
    let detected = detect_folder_init_plan(&current_dir, Some(&project.name))?;
    let selections = detected
        .services
        .into_iter()
        .map(|service| FolderInitSelection {
            id: service.id,
            domain: service.domain,
            target: service.target,
            enabled: service.enabled,
            update_dev_script: policy.write_package_scripts && service.can_update_dev_script,
            service_name: None,
            service_path: None,
            command: None,
        })
        .collect::<Vec<_>>();
    let resolved = resolve_folder_init_selection(&current_dir, &project.name, &selections, policy)?;
    let request = MaterializeProjectServicesRequest {
        project_id: project.id.clone(),
        root_path: current_dir.display().to_string(),
    };
    require_config_writable(config_path)?;
    let materialized = direct_api(config_path)?
        .materialize_project_services(request)?
        .materialized;
    // Best-effort: lets a moved folder be re-linked to this project later.
    let _ = write_project_marker(&current_dir, &project.id);
    let update_results = apply_local_app_updates(
        &resolved.package_updates,
        &resolved.next_config_updates,
        &resolved.vite_config_updates,
    );

    if materialized > 0 {
        println!("  Materialized project services: {materialized}");
    }
    print_update_results(&update_results.updated, &update_results.skipped);
    Ok(())
}

pub fn add_service(
    args: AddServiceArgs,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
) -> Result<()> {
    let current_dir = effective_current_dir()?;
    let config_store = ConfigStore::new(config_path.to_path_buf());
    let project = resolve_project(&config_store, args.project.as_deref(), &current_dir)?;
    let policy = policy.with_package_scripts(args.write_scripts || policy.write_package_scripts);
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
        update_dev_script: policy.write_package_scripts && service.can_update_dev_script,
        service_name: None,
        service_path: None,
        command: None,
    };
    let resolved =
        resolve_folder_init_selection(&current_dir, &project.name, &[selection], policy)?;
    let Some(service) = resolved.services.first() else {
        bail!("No service was resolved for '{}'", args.path.display());
    };

    let result = register_project_service(config_path, &project.id, service)?;
    let update_results = apply_local_app_updates(
        &resolved.package_updates,
        &resolved.next_config_updates,
        &resolved.vite_config_updates,
    );

    print_daemon_state(result.daemon_reloaded);
    println!("Added service: {} -> {}", result.domain, result.target);
    print_update_results(&update_results.updated, &update_results.skipped);
    Ok(())
}

pub fn set_domain(args: SetDomainArgs, ctx: &AppContext) -> Result<()> {
    let implicit_environment = if args.environment.is_none() {
        project_env::ensure_implicit_worktree_environment(args.project.as_deref(), ctx)?
    } else {
        None
    };
    let environment = args.environment.clone().or_else(|| {
        implicit_environment
            .as_ref()
            .map(|environment| environment.slug.clone())
    });

    let (project, environment_slug) = match environment.as_deref() {
        Some(environment_value) => {
            if let Some(environment) = implicit_environment.as_ref() {
                eprintln!(
                    "Using project environment '{}' [{}] for worktree.",
                    environment.name, environment.slug
                );
            }
            let (project, slug) =
                project_env::resolve_environment(args.project.as_deref(), environment_value, ctx)?;
            (project, Some(slug))
        }
        None => {
            let current_dir = effective_current_dir()?;
            let project =
                resolve_project(&ctx.config_store, args.project.as_deref(), &current_dir)?;
            (project, None)
        }
    };

    let request = SetProjectServiceDomainRequest {
        project_id: project.id.clone(),
        service_id: args.service.clone(),
        domain: args.domain.clone(),
        environment_slug: environment_slug.clone(),
    };
    require_config_writable(&ctx.config_path)?;
    let result = direct_api(&ctx.config_path)?.set_project_service_domain(request)?;

    print_daemon_state(result.daemon_reloaded);
    match environment_slug {
        Some(slug) => println!(
            "Updated environment '{}' service '{}' domain: {} -> {}",
            slug, result.service_id, result.previous_domain, result.domain
        ),
        None => println!(
            "Updated service '{}' domain: {} -> {}",
            result.service_id, result.previous_domain, result.domain
        ),
    }
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
    let local_roots = ProjectRootStore::new_default()
        .and_then(|store| store.load())
        .unwrap_or_default();
    let matches = projects
        .into_iter()
        .filter(|project| {
            project.root_path.as_deref() == Some(current.as_str())
                || local_roots.get(&project.id).map(String::as_str) == Some(current.as_str())
        })
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [project] => Ok(project.clone()),
        [] => {
            let canonical_current =
                std::fs::canonicalize(current_dir).unwrap_or_else(|_| current_dir.to_path_buf());
            if let Some(healed) = heal_moved_project_root(config_store, current_dir)
                && healed.root_path == canonical_current
            {
                eprintln!(
                    "Re-linked project '{}' [{}] to '{}' (folder moved).",
                    healed.project_name,
                    healed.project_id,
                    healed.root_path.display()
                );
                return Ok(healed.project);
            }
            bail!(
                "No Localify project is linked to '{}'. Run '{} init-project --yes' first.",
                current_dir.display(),
                BRANDING.cli_command()
            )
        }
        _ => bail!(
            "Multiple Localify projects are linked to '{}'. Pass --project <project-id>.",
            current_dir.display()
        ),
    }
}

/// A moved project folder that was re-linked through its in-project marker.
pub(crate) struct HealedProjectRoot {
    pub project_id: String,
    pub project_name: String,
    pub root_path: PathBuf,
    pub project: ConfiguredProject,
}

/// Re-links a project whose stored root path went stale because the folder
/// was moved on disk.
///
/// Walks up from `current_dir` to the nearest project marker. When the marker
/// names a known project and none of its stored roots still exists, the local
/// project-roots metadata is pointed at the marker folder and the shared
/// config root is updated best-effort (root owns it on Linux). A folder
/// *copy* — the stored root still exists — is left untouched, and a marker
/// naming an unknown project (e.g. committed and cloned from another machine)
/// is ignored.
pub(crate) fn heal_moved_project_root(
    config_store: &ConfigStore,
    current_dir: &Path,
) -> Option<HealedProjectRoot> {
    let (marker_root, project_id) = find_project_marker(current_dir)?;
    let marker_root = std::fs::canonicalize(&marker_root).unwrap_or(marker_root);
    let mut project = config_store
        .list_projects()
        .ok()?
        .into_iter()
        .find(|project| project.id == project_id)?;

    let local_roots = ProjectRootStore::new_default()
        .and_then(|store| store.load())
        .unwrap_or_default();
    let known_roots = [
        project.root_path.as_deref(),
        local_roots.get(&project.id).map(String::as_str),
    ]
    .into_iter()
    .flatten()
    .map(PathBuf::from)
    .collect::<Vec<_>>();

    // A known root that still exists means either nothing moved or the folder
    // was copied — never steal the root in those cases.
    if known_roots.iter().any(|root| root.is_dir()) {
        return None;
    }

    ProjectRootStore::new_default()
        .ok()?
        .set(&project.id, &marker_root)
        .ok()?;
    let root_path = marker_root.display().to_string();
    // Best-effort: the marker above is what matters for finding the folder.
    let _ = config_store.update_project(&project.id, None, None, Some(root_path.clone()));
    project.root_path = Some(root_path);

    Some(HealedProjectRoot {
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        root_path: marker_root,
        project,
    })
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
    config_path: &Path,
    project_id: &str,
    service: &ServicePlan,
) -> Result<AddProjectServiceResultDto> {
    let request = AddProjectServiceRequest {
        project_id: project_id.to_string(),
        domain: service.domain.clone(),
        target: service.target.clone(),
        service_name: service.service_name.clone(),
        service_path: service.service_path.clone(),
        command: service.command.clone(),
    };

    require_config_writable(config_path)?;
    direct_api(config_path)?.add_project_service(request)
}

pub(crate) fn print_daemon_state(reloaded: bool) {
    if reloaded {
        println!("  Daemon: reloaded");
    } else {
        println!("  Daemon: not running");
        println!("  Start it with: {} start", BRANDING.cli_command());
    }
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
