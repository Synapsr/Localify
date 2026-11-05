use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use super::context::{AppContext, direct_api, effective_current_dir, require_config_writable};
use crate::branding::BRANDING;
use crate::infrastructure::config::{ConfigStore, ConfiguredProject, ConfiguredProjectEnvironment};
use crate::infrastructure::running_services::RunningServiceStore;
use localify::desktop::DesktopApi;
use localify::desktop::ProjectRootStore;
use localify::desktop::dto::{
    DiscoverProjectWorktreesRequest, EnsureProjectEnvironmentRequest,
    EnsureProjectEnvironmentResultDto, EnvironmentFileCopyRequest, EnvironmentFileCopyStatusDto,
    ProjectEnvironmentInfoDto, ProjectEnvironmentServiceInfoDto, RemoveProjectEnvironmentRequest,
};

pub struct ListEnvArgs {
    pub project: Option<String>,
    pub json: bool,
}

pub struct EnsureEnvArgs {
    pub project: Option<String>,
    pub name: Option<String>,
    pub root: Option<PathBuf>,
    pub primary: bool,
}

pub struct DiscoverEnvArgs {
    pub project: String,
    pub json: bool,
}

pub struct RemoveEnvArgs {
    pub environment: String,
    pub project: Option<String>,
    pub delete_worktree: bool,
    pub force: bool,
}

pub struct PruneEnvArgs {
    pub project: Option<String>,
    pub dry_run: bool,
}

pub struct CopyEnvArgs {
    pub project: String,
    pub environment: String,
    pub from: String,
    pub paths: Vec<String>,
    pub all: bool,
    pub overwrite: bool,
    pub json: bool,
}

#[derive(Debug, Clone)]
struct GitMetadata {
    worktree_root: Option<PathBuf>,
    common_dir: Option<PathBuf>,
}

pub fn list(args: ListEnvArgs, ctx: &AppContext) -> Result<()> {
    let environments = ctx
        .config_store
        .list_project_environments(args.project.as_deref())?;

    if args.json {
        let output = environments
            .iter()
            .map(EnvironmentOutput::from)
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if environments.is_empty() {
        println!("No project environments configured.");
        println!(
            "\nCreate one with: {} project env ensure --name feat-auth",
            BRANDING.cli_command()
        );
        return Ok(());
    }

    println!("Project environments:\n");
    for environment in environments {
        println!(
            "  {} [{}]{}",
            environment.name,
            environment.slug,
            if environment.primary {
                " (primary)"
            } else {
                ""
            }
        );
        println!(
            "    project: {} [{}]",
            environment.project_name, environment.project_id
        );
        if let Some(root_path) = &environment.root_path {
            println!("    root: {root_path}");
        }
        if let Some(branch) = &environment.git_branch {
            println!("    branch: {branch}");
        }
        if let Some(common_dir) = &environment.git_common_dir {
            println!("    git common-dir: {common_dir}");
        }
        if !environment.services.is_empty() {
            println!("    service domains:");
            for service in &environment.services {
                println!("      {} -> {}", service.service_id, service.domain);
            }
        }
        println!();
    }

    Ok(())
}

pub fn discover(args: DiscoverEnvArgs, config_path: &Path, _ctx: &AppContext) -> Result<()> {
    let api = DesktopApi::for_cli(config_path.to_path_buf(), current_binary_path()?);
    let discovery = api.discover_project_worktrees(DiscoverProjectWorktreesRequest {
        project_id: args.project,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&discovery)?);
        return Ok(());
    }

    println!(
        "Git worktrees for {} [{}]\n",
        discovery.project_name, discovery.project_id
    );
    for worktree in discovery.worktrees {
        println!("  {} [{}]", worktree.name, worktree.slug);
        println!("    status: {:?}", worktree.status);
        println!("    root: {}", worktree.root_path);
        if let Some(branch) = worktree.git_branch {
            println!("    branch: {branch}");
        }
        if let Some(environment) = worktree.attached_environment_slug {
            println!("    environment: {environment}");
        }
        if worktree.existing_environment_file_count > 0 {
            println!(
                "    env files: {} existing",
                worktree.existing_environment_file_count
            );
        }
        if !worktree.service_candidates.is_empty() {
            println!("    new service candidates:");
            for service in worktree.service_candidates {
                println!("      {} ({})", service.label, service.display_path);
            }
        }
        if let Some(reason) = worktree.unavailable_reason {
            println!("    unavailable: {reason}");
        }
        println!();
    }

    Ok(())
}

pub fn ensure(args: EnsureEnvArgs, ctx: &AppContext) -> Result<()> {
    let current_dir = effective_current_dir()?;
    let requested_root = args.root.as_deref().unwrap_or(&current_dir);
    let root_path = absolute_existing_path(requested_root, &current_dir);
    let git = detect_git_metadata(&root_path);
    let environment_root = git
        .worktree_root
        .clone()
        .unwrap_or_else(|| root_path.clone());
    let project = resolve_project(
        &ctx.config_store,
        args.project.as_deref(),
        &environment_root,
    )?;

    let result = ensure_environment_backend(
        ctx,
        EnsureProjectEnvironmentRequest {
            project_id: project.id,
            name: args.name,
            root_path: environment_root.display().to_string(),
            primary: args.primary,
        },
    )?;
    crate::cli::project::print_daemon_state(result.daemon_reloaded);
    let persisted = result.environment;

    println!(
        "Ensured project environment: {} [{}]{}",
        persisted.name,
        persisted.slug,
        if persisted.primary { " (primary)" } else { "" }
    );
    println!(
        "  project: {} [{}]",
        persisted.project_name, persisted.project_id
    );
    if let Some(root_path) = &persisted.root_path {
        println!("  root: {root_path}");
    }
    if !persisted.services.is_empty() {
        println!("  service domains:");
        for service in persisted.services {
            println!("    {} -> {}", service.service_id, service.domain);
        }
    }

    Ok(())
}

pub fn copy_env(args: CopyEnvArgs, config_path: &Path) -> Result<()> {
    if args.all && !args.paths.is_empty() {
        bail!("Pass --all or --path, not both.");
    }
    if !args.all && args.paths.is_empty() {
        bail!("Pass one or more --path values, or use --all.");
    }

    let api = DesktopApi::for_cli(config_path.to_path_buf(), current_binary_path()?);
    let result = api.copy_environment_files(EnvironmentFileCopyRequest {
        project_id: args.project,
        source_environment_slug: args.from,
        target_environment_slug: args.environment,
        relative_paths: if args.all { Vec::new() } else { args.paths },
        overwrite: args.overwrite,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!("Environment file copy result:\n");
    for item in result.items {
        let status = match item.status {
            EnvironmentFileCopyStatusDto::Create => "create",
            EnvironmentFileCopyStatusDto::Overwrite => "overwrite",
            EnvironmentFileCopyStatusDto::SkipExists => "skip",
            EnvironmentFileCopyStatusDto::Blocked => "blocked",
            EnvironmentFileCopyStatusDto::Copied => "copied",
            EnvironmentFileCopyStatusDto::Skipped => "skipped",
        };
        println!("  {status}: {}", item.relative_path);
        if let Some(message) = item.message {
            println!("    {message}");
        }
    }

    Ok(())
}

/// Removes a worktree environment: stops its services, unregisters its domains and,
/// with `delete_worktree`, deletes the Git worktree too. Uncommitted changes are
/// checked first, so when they block the deletion nothing has changed yet.
pub fn remove(args: RemoveEnvArgs, config_path: &Path, ctx: &AppContext) -> Result<()> {
    require_config_writable(config_path)?;
    let (project, slug) = resolve_environment(args.project.as_deref(), &args.environment, ctx)?;
    let environment = ctx
        .config_store
        .list_project_environments(Some(&project.id))?
        .into_iter()
        .find(|environment| environment.slug == slug)
        .ok_or_else(|| anyhow!("Project environment '{slug}' was not found."))?;
    if environment.primary {
        bail!(
            "'{}' is the primary environment of '{}' and cannot be removed. To remove the whole project, use: {} remove-project {}",
            environment.name,
            project.name,
            BRANDING.cli_command(),
            project.id
        );
    }

    let worktree = if args.delete_worktree {
        deletable_worktree(&environment, &project, args.force)?
    } else {
        None
    };
    stop_environment_services(&project.id, &slug)?;
    match worktree {
        Some(root) => delete_worktree(&root, &environment, args.force)?,
        None if args.delete_worktree => forget_missing_worktree(&environment, &project),
        None => {}
    }
    remove_environment_config(&project.id, &environment, config_path)
}

/// Removes the environments whose folder no longer exists, such as a worktree an
/// agent deleted with `git worktree remove`.
pub fn prune(args: PruneEnvArgs, config_path: &Path, ctx: &AppContext) -> Result<()> {
    require_config_writable(config_path)?;
    let project_id = match args.project.as_deref() {
        Some(project) => {
            Some(resolve_project(&ctx.config_store, Some(project), &effective_current_dir()?)?.id)
        }
        None => None,
    };
    let missing = ctx
        .config_store
        .list_project_environments(project_id.as_deref())?
        .into_iter()
        .filter(|environment| !environment.primary)
        .filter(|environment| {
            environment
                .root_path
                .as_deref()
                .is_some_and(|root| !Path::new(root).exists())
        })
        .collect::<Vec<_>>();

    if missing.is_empty() {
        println!("Every project environment still has its folder. Nothing to remove.");
        return Ok(());
    }
    for environment in missing {
        let root = environment.root_path.clone().unwrap_or_default();
        if args.dry_run {
            println!(
                "Would remove '{}' [{}] of '{}': {root} is gone.",
                environment.name, environment.slug, environment.project_id
            );
            continue;
        }
        stop_environment_services(&environment.project_id, &environment.slug)?;
        remove_environment_config(&environment.project_id, &environment, config_path)?;
    }
    Ok(())
}

fn stop_environment_services(project_id: &str, slug: &str) -> Result<()> {
    let store = RunningServiceStore::new_default()?;
    for service in store.list()? {
        let key = &service.key;
        if key.project_id != project_id || key.environment.as_deref() != Some(slug) {
            continue;
        }
        if store.stop_and_forget(key)? {
            println!("Stopped '{}' (PID {}).", key.service_id, service.pid);
        }
    }
    Ok(())
}

/// The worktree folder `--delete-worktree` will delete, checked before anything
/// changes. `None` when the folder is already gone.
fn deletable_worktree(
    environment: &ConfiguredProjectEnvironment,
    project: &ConfiguredProject,
    force: bool,
) -> Result<Option<PathBuf>> {
    let Some(root) = environment.root_path.as_deref().map(PathBuf::from) else {
        bail!(
            "Environment '{}' has no folder, so there is no worktree to delete. Run the command again without --delete-worktree.",
            environment.slug
        );
    };
    if !root.exists() {
        return Ok(None);
    }
    if !is_non_primary_worktree(project, &root, &detect_git_metadata(&root)) {
        bail!(
            "{} is not a Git worktree of '{}', so {} does not delete it. Delete the folder yourself, or run the command again without --delete-worktree.",
            root.display(),
            project.name,
            BRANDING.app_name
        );
    }
    if !force {
        let changes = git_output(&root, &["status", "--porcelain"]).unwrap_or_default();
        if !changes.is_empty() {
            bail!(
                "The worktree {} has uncommitted changes:\n{changes}\nCommit or discard them first, or pass --force to discard them. Nothing was removed.",
                root.display()
            );
        }
    }
    Ok(Some(root))
}

fn delete_worktree(
    root: &Path,
    environment: &ConfiguredProjectEnvironment,
    force: bool,
) -> Result<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(["worktree", "remove"]);
    if force {
        command.arg("--force");
    }
    let output = command
        .arg(root)
        .output()
        .context("Failed to run git worktree remove")?;
    if !output.status.success() {
        bail!(
            "Git did not delete the worktree {}: {}\nIts services are stopped; the environment and its domains are kept.",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    println!("Deleted the worktree {}.", root.display());
    if let Some(branch) = &environment.git_branch {
        println!("Kept its branch '{branch}'. Delete it with: git branch -d {branch}");
    }
    Ok(())
}

/// Git still lists a worktree whose folder was deleted by hand until it is pruned.
fn forget_missing_worktree(
    environment: &ConfiguredProjectEnvironment,
    project: &ConfiguredProject,
) {
    if let Some(root) = &environment.root_path {
        println!("The worktree folder {root} is already gone.");
    }
    if let Some(project_root) = project.root_path.as_deref() {
        let _ = git_output(Path::new(project_root), &["worktree", "prune"]);
    }
}

fn remove_environment_config(
    project_id: &str,
    environment: &ConfiguredProjectEnvironment,
    config_path: &Path,
) -> Result<()> {
    let api = DesktopApi::for_cli(config_path.to_path_buf(), current_binary_path()?);
    let result = api.remove_project_environment(RemoveProjectEnvironmentRequest {
        project_id: project_id.to_string(),
        environment_slug: environment.slug.clone(),
    })?;
    println!(
        "Removed project environment '{}' [{}] of '{}'.",
        environment.name, environment.slug, environment.project_name
    );
    for domain in result.removed_domains {
        println!("  unregistered {domain}");
    }
    crate::cli::project::print_daemon_state(result.daemon_reloaded);
    Ok(())
}

pub(crate) fn ensure_implicit_worktree_environment(
    project: Option<&str>,
    ctx: &AppContext,
) -> Result<Option<ProjectEnvironmentInfoDto>> {
    let Some(worktree) = current_worktree(project, ctx)? else {
        return Ok(None);
    };
    if let Some(existing) = worktree.environment {
        return Ok(Some(environment_info(existing)));
    }
    let (project, environment_root) = (worktree.project, worktree.root);

    let result = ensure_environment_backend(
        ctx,
        EnsureProjectEnvironmentRequest {
            project_id: project.id,
            name: None,
            root_path: environment_root.display().to_string(),
            primary: false,
        },
    )?;
    Ok(Some(result.environment))
}

/// The environment of the worktree the command runs in, without creating one.
pub(crate) fn current_worktree_environment(
    project: Option<&str>,
    ctx: &AppContext,
) -> Result<Option<ProjectEnvironmentInfoDto>> {
    Ok(current_worktree(project, ctx)?
        .and_then(|worktree| worktree.environment)
        .map(environment_info))
}

struct CurrentWorktree {
    project: ConfiguredProject,
    root: PathBuf,
    environment: Option<ConfiguredProjectEnvironment>,
}

/// The project worktree, other than the primary checkout, that the current folder is in.
fn current_worktree(project: Option<&str>, ctx: &AppContext) -> Result<Option<CurrentWorktree>> {
    let current_dir = effective_current_dir()?;
    let root_path = absolute_existing_path(&current_dir, &current_dir);
    let git = detect_git_metadata(&root_path);
    let root = git
        .worktree_root
        .clone()
        .unwrap_or_else(|| root_path.clone());
    let project = match resolve_project(&ctx.config_store, project, &root) {
        Ok(project) => project,
        Err(_) if project.is_none() => return Ok(None),
        Err(error) => return Err(error),
    };
    if !is_non_primary_worktree(&project, &root, &git) {
        return Ok(None);
    }
    let environment = existing_environment_for_root(&ctx.config_store, &project.id, &root)?;
    Ok(Some(CurrentWorktree {
        project,
        root,
        environment,
    }))
}

/// Resolves the project and the exact environment slug for a `--env` value,
/// accepting either the slug or the display name.
pub(crate) fn resolve_environment(
    project: Option<&str>,
    environment: &str,
    ctx: &AppContext,
) -> Result<(ConfiguredProject, String)> {
    let current_dir = effective_current_dir()?;
    let root_path = absolute_existing_path(&current_dir, &current_dir);
    let git = detect_git_metadata(&root_path);
    let environment_root = git
        .worktree_root
        .clone()
        .unwrap_or_else(|| root_path.clone());
    let project = resolve_project(&ctx.config_store, project, &environment_root)?;
    let environments = ctx
        .config_store
        .list_project_environments(Some(&project.id))?;
    let matches = environments
        .into_iter()
        .filter(|candidate| environment_matches(candidate, environment))
        .collect::<Vec<_>>();
    let environment = match matches.as_slice() {
        [environment] => environment.clone(),
        [] => bail!(
            "Project environment '{environment}' was not found for project '{}'. Create it with: {} project env ensure --project {} --name {}",
            project.id,
            BRANDING.cli_command(),
            project.id,
            environment
        ),
        _ => bail!(
            "Project environment '{environment}' is ambiguous in project '{}'. Use its slug.",
            project.id
        ),
    };

    Ok((project, environment.slug))
}

fn ensure_environment_backend(
    ctx: &AppContext,
    request: EnsureProjectEnvironmentRequest,
) -> Result<EnsureProjectEnvironmentResultDto> {
    require_config_writable(&ctx.config_path)?;
    direct_api(&ctx.config_path)?.ensure_project_environment(request)
}

fn environment_info(environment: ConfiguredProjectEnvironment) -> ProjectEnvironmentInfoDto {
    let (launch_profiles, launch_profiles_error) =
        localify::desktop::environment_launch_profiles(environment.root_path.as_deref());
    ProjectEnvironmentInfoDto {
        launch_profiles,
        launch_profiles_error,
        project_id: environment.project_id,
        project_name: environment.project_name,
        name: environment.name,
        slug: environment.slug,
        primary: environment.primary,
        root_path: environment.root_path,
        git_common_dir: environment.git_common_dir,
        git_branch: environment.git_branch,
        services: environment
            .services
            .into_iter()
            .map(|service| ProjectEnvironmentServiceInfoDto {
                service_id: service.service_id,
                domain: service.domain,
            })
            .collect(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentOutput {
    project_id: String,
    project_name: String,
    name: String,
    slug: String,
    primary: bool,
    root_path: Option<String>,
    git_common_dir: Option<String>,
    git_branch: Option<String>,
    services: Vec<EnvironmentServiceOutput>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentServiceOutput {
    service_id: String,
    domain: String,
}

impl From<&ConfiguredProjectEnvironment> for EnvironmentOutput {
    fn from(environment: &ConfiguredProjectEnvironment) -> Self {
        Self {
            project_id: environment.project_id.clone(),
            project_name: environment.project_name.clone(),
            name: environment.name.clone(),
            slug: environment.slug.clone(),
            primary: environment.primary,
            root_path: environment.root_path.clone(),
            git_common_dir: environment.git_common_dir.clone(),
            git_branch: environment.git_branch.clone(),
            services: environment
                .services
                .iter()
                .map(|service| EnvironmentServiceOutput {
                    service_id: service.service_id.clone(),
                    domain: service.domain.clone(),
                })
                .collect(),
        }
    }
}

fn resolve_project(
    config_store: &ConfigStore,
    project: Option<&str>,
    environment_root: &Path,
) -> Result<ConfiguredProject> {
    let projects = projects_with_local_roots(config_store.list_projects()?);
    if let Some(project) = project {
        return projects
            .into_iter()
            .find(|candidate| {
                candidate.id == project || candidate.name.eq_ignore_ascii_case(project)
            })
            .ok_or_else(|| anyhow!("Project not found: {project}"));
    }

    let environment_git = detect_git_metadata(environment_root);
    let mut matches = projects
        .into_iter()
        .filter(|project| project_matches_root_or_git(project, environment_root, &environment_git))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.id.cmp(&right.id));

    match matches.as_slice() {
        [project] => Ok(project.clone()),
        [] => bail!(
            "No Localify project matches '{}'. Pass --project <project-id> or run '{} init-project --yes' first.",
            environment_root.display(),
            BRANDING.cli_command()
        ),
        _ => bail!(
            "Multiple Localify projects match '{}'. Pass --project <project-id>.",
            environment_root.display()
        ),
    }
}

fn projects_with_local_roots(projects: Vec<ConfiguredProject>) -> Vec<ConfiguredProject> {
    let project_roots = load_project_roots();
    projects
        .into_iter()
        .map(|mut project| {
            let should_use_local_root = project
                .root_path
                .as_deref()
                .map(|root_path| !Path::new(root_path).exists())
                .unwrap_or(true);
            if should_use_local_root && let Some(root_path) = project_roots.get(&project.id) {
                project.root_path = Some(root_path.display().to_string());
            }
            project
        })
        .collect()
}

fn load_project_roots() -> HashMap<String, PathBuf> {
    ProjectRootStore::new_default()
        .and_then(|store| store.load())
        .unwrap_or_default()
        .into_iter()
        .map(|(project_id, root_path)| (project_id, PathBuf::from(root_path)))
        .collect()
}

fn current_binary_path() -> Result<PathBuf> {
    std::env::current_exe().context("Failed to determine the Localify CLI path")
}

fn project_matches_root_or_git(
    project: &ConfiguredProject,
    environment_root: &Path,
    environment_git: &GitMetadata,
) -> bool {
    let Some(project_root) = project.root_path.as_ref().map(PathBuf::from) else {
        return false;
    };

    if environment_root == project_root || environment_root.starts_with(&project_root) {
        return true;
    }

    let Some(environment_common_dir) = environment_git.common_dir.as_ref() else {
        return false;
    };
    let project_git = detect_git_metadata(&project_root);
    project_git
        .common_dir
        .as_ref()
        .map(|project_common_dir| project_common_dir == environment_common_dir)
        .unwrap_or(false)
}

fn is_non_primary_worktree(
    project: &ConfiguredProject,
    environment_root: &Path,
    environment_git: &GitMetadata,
) -> bool {
    let Some(project_root) = project.root_path.as_ref().map(PathBuf::from) else {
        return false;
    };
    let project_root = fs::canonicalize(&project_root).unwrap_or(project_root);

    if environment_root == project_root
        || environment_root.starts_with(&project_root)
        || project_root.starts_with(environment_root)
    {
        return false;
    }

    let Some(environment_common_dir) = environment_git.common_dir.as_ref() else {
        return false;
    };
    let project_git = detect_git_metadata(&project_root);
    project_git
        .common_dir
        .as_ref()
        .map(|project_common_dir| project_common_dir == environment_common_dir)
        .unwrap_or(false)
}

fn existing_environment_for_root(
    config_store: &ConfigStore,
    project_id: &str,
    environment_root: &Path,
) -> Result<Option<ConfiguredProjectEnvironment>> {
    let environment_root =
        fs::canonicalize(environment_root).unwrap_or_else(|_| environment_root.to_path_buf());
    Ok(config_store
        .list_project_environments(Some(project_id))?
        .into_iter()
        .find(|environment| {
            environment
                .root_path
                .as_ref()
                .map(PathBuf::from)
                .map(|path| fs::canonicalize(&path).unwrap_or(path))
                .map(|root_path| root_path == environment_root)
                .unwrap_or(false)
        }))
}

fn environment_matches(environment: &ConfiguredProjectEnvironment, value: &str) -> bool {
    environment.slug == value || environment.name.eq_ignore_ascii_case(value)
}

fn absolute_existing_path(path: &Path, current_dir: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    };

    fs::canonicalize(&absolute).unwrap_or(absolute)
}

fn detect_git_metadata(root: &Path) -> GitMetadata {
    let worktree_root = git_output(root, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
    let common_dir = git_output(root, &["rev-parse", "--git-common-dir"]).map(|value| {
        let path = PathBuf::from(value);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        fs::canonicalize(&path).unwrap_or(path)
    });
    GitMetadata {
        worktree_root,
        common_dir,
    }
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
