use std::collections::HashMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use super::context::{AppContext, effective_current_dir};
use crate::application::StepOutcome;
use crate::application::ports::{CertificateManager, DaemonConnection};
use crate::application::register_domain::RegisterDomain;
use crate::application::unregister_domain::UnregisterDomain;
use crate::branding::BRANDING;
use crate::domain::{DomainPattern, Route};
use crate::infrastructure::config::{
    ConfigStore, ConfiguredProject, ConfiguredProjectEnvironment, ConfiguredService,
    ProjectEnvironmentConfig, ProjectEnvironmentServiceConfig,
    ProjectEnvironmentServiceDomainUpdate,
};
use roxy::desktop::ProjectRootStore;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnsureOutput {
    Cli,
    Quiet,
}

#[derive(Debug, Clone)]
struct GitMetadata {
    worktree_root: Option<PathBuf>,
    common_dir: Option<PathBuf>,
    branch: Option<String>,
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

pub fn ensure(args: EnsureEnvArgs, ctx: &AppContext) -> Result<()> {
    let persisted = ensure_project_environment(args, ctx, EnsureOutput::Cli)?;

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

pub(crate) fn ensure_implicit_worktree_environment(
    project: Option<&str>,
    ctx: &AppContext,
) -> Result<Option<ConfiguredProjectEnvironment>> {
    let current_dir = effective_current_dir()?;
    let root_path = absolute_existing_path(&current_dir, &current_dir);
    let git = detect_git_metadata(&root_path);
    let environment_root = git
        .worktree_root
        .clone()
        .unwrap_or_else(|| root_path.clone());
    let project = match resolve_project(&ctx.config_store, project, &environment_root) {
        Ok(project) => project,
        Err(_) if project.is_none() => return Ok(None),
        Err(error) => return Err(error),
    };

    if !is_non_primary_worktree(&project, &environment_root, &git) {
        return Ok(None);
    }

    if let Some(existing) =
        existing_environment_for_root(&ctx.config_store, &project.id, &environment_root)?
    {
        return Ok(Some(existing));
    }

    let environment = ensure_project_environment_for_root(
        project,
        None,
        environment_root,
        git,
        false,
        ctx,
        EnsureOutput::Quiet,
    )?;
    Ok(Some(environment))
}

fn ensure_project_environment(
    args: EnsureEnvArgs,
    ctx: &AppContext,
    output: EnsureOutput,
) -> Result<ConfiguredProjectEnvironment> {
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

    ensure_project_environment_for_root(
        project,
        args.name,
        environment_root,
        git,
        args.primary,
        ctx,
        output,
    )
}

fn ensure_project_environment_for_root(
    project: ConfiguredProject,
    name: Option<String>,
    environment_root: PathBuf,
    git: GitMetadata,
    primary: bool,
    ctx: &AppContext,
    output: EnsureOutput,
) -> Result<ConfiguredProjectEnvironment> {
    let services = project_services(&ctx.config_store, &project.id)?;
    if services.is_empty() {
        bail!(
            "Project '{}' has no services. Add one with '{} project add-service --path <app>'.",
            project.id,
            BRANDING.cli_command()
        );
    }

    let name = environment_name(name, primary, &git, &environment_root)?;
    let slug = if primary {
        "primary".to_string()
    } else {
        slugify(&name).ok_or_else(|| anyhow!("Environment name must contain letters or numbers"))?
    };

    let domain_mappings = environment_service_domains(&project.id, &services, &slug, primary);
    let created_domains = if primary {
        Vec::new()
    } else {
        ensure_environment_domains(&project.id, &domain_mappings, ctx)?
    };

    let environment = ProjectEnvironmentConfig {
        name,
        slug: slug.clone(),
        primary,
        root_path: Some(environment_root.display().to_string()),
        git_common_dir: git.common_dir.map(|path| path.display().to_string()),
        git_branch: git.branch,
        services: domain_mappings,
    };

    let persisted = match ctx
        .config_store
        .upsert_project_environment(&project.id, environment)
    {
        Ok(environment) => environment,
        Err(error) => {
            rollback_domains(&created_domains, ctx);
            return Err(error.into());
        }
    };

    reload_daemon(ctx, output)?;
    Ok(persisted)
}

pub(crate) fn set_environment_service_domain(
    project: Option<&str>,
    environment: &str,
    service_id: &str,
    domain: &str,
    ctx: &AppContext,
) -> Result<()> {
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
    let mapping = environment
        .services
        .iter()
        .find(|mapping| {
            mapping.service_id == service_id || mapping.service_id.eq_ignore_ascii_case(service_id)
        })
        .ok_or_else(|| {
            anyhow!(
                "Service '{}' is unavailable in environment '{}'. Re-run '{} project env ensure --project {} --name {}'.",
                service_id,
                environment.slug,
                BRANDING.cli_command(),
                project.id,
                environment.slug
            )
        })?;

    let current_pattern = domain_pattern_from_display(&mapping.domain)?;
    let current_registration = ctx
        .config_store
        .get_domain(&current_pattern)?
        .ok_or_else(|| anyhow!("Domain '{}' is not registered.", mapping.domain))?;
    let new_pattern = domain_pattern_from_display(domain)?;
    let cert_installed = match ctx.cert_service.create_and_install(&new_pattern) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "  Certificate warning: failed to generate certificate for '{}': {error}",
                new_pattern
            );
            false
        }
    };
    let mut new_registration = crate::domain::DomainRegistration::with_source(
        new_pattern,
        current_registration.routes().to_vec(),
        current_registration.source(),
    );
    if cert_installed {
        new_registration.enable_https();
    }
    let new_domain = new_registration.display_pattern();

    ctx.config_store
        .replace_project_environment_service_domain(
            &project.id,
            ProjectEnvironmentServiceDomainUpdate {
                environment_slug: environment.slug.clone(),
                service_id: mapping.service_id.clone(),
                current_domain: mapping.domain.clone(),
                new_registration,
            },
        )?;
    reload_daemon(ctx, EnsureOutput::Cli)?;
    println!(
        "Updated environment '{}' service '{}' domain: {} -> {}",
        environment.slug, mapping.service_id, mapping.domain, new_domain
    );
    Ok(())
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

fn project_services(
    config_store: &ConfigStore,
    project_id: &str,
) -> Result<Vec<ConfiguredService>> {
    Ok(config_store
        .list_services()?
        .into_iter()
        .filter(|service| service.project_id == project_id)
        .collect())
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
    ctx: &AppContext,
) -> Result<Vec<DomainPattern>> {
    let mut missing = Vec::new();
    for service in domains {
        let pattern = DomainPattern::from_name(&service.domain, false)?;
        if let Some(registration) = ctx.config_store.get_domain(&pattern)? {
            let owner = ctx.config_store.domain_project_id(&pattern)?;
            if owner.as_deref() != Some(project_id) {
                bail!(
                    "Domain '{}' already exists outside project '{}'. Choose another environment name.",
                    service.domain,
                    project_id
                );
            }
            ensure_existing_domain_https(
                project_id,
                registration,
                &ctx.config_store,
                &ctx.cert_service,
            )?;
            continue;
        }
        missing.push((service.service_id.clone(), pattern));
    }

    let ports = reserve_ports(missing.len())?;
    let use_case = RegisterDomain::new(&ctx.config_store, &ctx.cert_service);
    let mut created = Vec::new();
    for ((service_id, pattern), port) in missing.into_iter().zip(ports.into_iter()) {
        let route = Route::parse(&format!("/={port}"))
            .with_context(|| format!("Failed to create route for service '{service_id}'"))?;
        match use_case.execute(pattern.clone(), vec![route]) {
            Ok(result) => {
                if let StepOutcome::Warning(message) = &result.cert_outcome {
                    eprintln!(
                        "  Certificate warning: failed to enable HTTPS for '{}': {message}",
                        result.registration.display_pattern()
                    );
                }
                if let Err(error) = ctx
                    .config_store
                    .update_domain_with_project(result.registration, Some(project_id.to_string()))
                {
                    rollback_domains(&created, ctx);
                    let _ = UnregisterDomain::new(&ctx.config_store, &ctx.cert_service)
                        .execute(&pattern);
                    return Err(error.into());
                }
                created.push(pattern);
            }
            Err(error) => {
                rollback_domains(&created, ctx);
                return Err(error);
            }
        }
    }

    Ok(created)
}

fn ensure_existing_domain_https(
    project_id: &str,
    mut registration: crate::domain::DomainRegistration,
    config_store: &ConfigStore,
    cert_service: &dyn CertificateManager,
) -> Result<()> {
    if registration.is_https_enabled() {
        return Ok(());
    }

    match cert_service.create_and_install(registration.pattern()) {
        Ok(()) => {
            registration.enable_https();
            config_store.update_domain_with_project(registration, Some(project_id.to_string()))?;
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

fn domain_pattern_from_display(value: &str) -> Result<DomainPattern> {
    let value = value.trim();
    let (domain, wildcard) = value
        .strip_prefix("*.")
        .map(|domain| (domain, true))
        .unwrap_or((value, false));
    Ok(DomainPattern::from_name(domain, wildcard)?)
}

fn environment_matches(environment: &ConfiguredProjectEnvironment, value: &str) -> bool {
    environment.slug == value || environment.name.eq_ignore_ascii_case(value)
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

fn rollback_domains(patterns: &[DomainPattern], ctx: &AppContext) {
    let unregister = UnregisterDomain::new(&ctx.config_store, &ctx.cert_service);
    for pattern in patterns {
        let _ = unregister.execute(pattern);
    }
}

fn reload_daemon(ctx: &AppContext, output: EnsureOutput) -> Result<()> {
    match ctx.mgmt_client.reload() {
        Ok(()) => {
            if output == EnsureOutput::Cli {
                println!("  Daemon: reloaded");
            }
        }
        Err(crate::application::ports::DaemonConnectionError::NotRunning) => {
            if output == EnsureOutput::Cli {
                println!("  Daemon: not running");
                println!("  Start it with: {} start", BRANDING.cli_command());
            }
        }
        Err(error) => bail!("Failed to reload the running daemon: {error}"),
    }
    Ok(())
}

fn environment_name(
    requested: Option<String>,
    primary: bool,
    git: &GitMetadata,
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

    if let Some(branch) = git
        .branch
        .as_ref()
        .filter(|branch| !branch.trim().is_empty())
    {
        return Ok(branch.clone());
    }

    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("Could not infer an environment name. Pass --name <label>."))?;
    Ok(name.to_string())
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
    let branch = git_output(root, &["branch", "--show-current"]);

    GitMetadata {
        worktree_root,
        common_dir,
        branch,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::testkit::InMemoryCertificateManager;
    use crate::domain::DomainRegistration;

    fn service(id: &str, primary_domain: &str) -> ConfiguredService {
        ConfiguredService {
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

    #[test]
    fn non_primary_single_service_domain_uses_environment_label() {
        let domains = environment_service_domains(
            "demo",
            &[service("web", &format!("demo{}", BRANDING.domain_suffix()))],
            "feat-auth",
            false,
        );

        assert_eq!(
            domains[0].domain,
            format!("feat-auth.demo{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn non_primary_multi_service_domain_uses_service_and_environment_label() {
        let domains = environment_service_domains(
            "demo",
            &[
                service("web", &format!("demo{}", BRANDING.domain_suffix())),
                service("api", &format!("api.demo{}", BRANDING.domain_suffix())),
            ],
            "feat-auth",
            false,
        );

        assert_eq!(
            domains[0].domain,
            format!("web--feat-auth.demo{}", BRANDING.domain_suffix())
        );
        assert_eq!(
            domains[1].domain,
            format!("api--feat-auth.demo{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn primary_environment_keeps_service_domains() {
        let primary = format!("demo{}", BRANDING.domain_suffix());
        let domains =
            environment_service_domains("demo", &[service("web", &primary)], "primary", true);

        assert_eq!(domains[0].domain, primary);
    }

    #[test]
    fn ensure_existing_domain_https_upgrades_http_domain_when_cert_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.toml"));
        let project = store.create_project("Demo").unwrap();
        let domain = format!("feat-auth.demo{}", BRANDING.domain_suffix());
        let pattern = DomainPattern::from_name(&domain, false).unwrap();
        store
            .add_domain_with_project(proxy_registration(&domain, 3000), Some(project.id.clone()))
            .unwrap();
        let registration = store.get_domain(&pattern).unwrap().unwrap();

        ensure_existing_domain_https(
            &project.id,
            registration,
            &store,
            &InMemoryCertificateManager::new(),
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
        let domain = format!("feat-auth.demo{}", BRANDING.domain_suffix());
        let pattern = DomainPattern::from_name(&domain, false).unwrap();
        store
            .add_domain_with_project(proxy_registration(&domain, 3000), Some(project.id.clone()))
            .unwrap();
        let registration = store.get_domain(&pattern).unwrap().unwrap();

        ensure_existing_domain_https(
            &project.id,
            registration,
            &store,
            &InMemoryCertificateManager::always_failing(),
        )
        .unwrap();

        let registration = store.get_domain(&pattern).unwrap().unwrap();
        assert!(!registration.is_https_enabled());
    }
}
