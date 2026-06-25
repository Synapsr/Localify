use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use super::context::{AppContext, effective_current_dir};
use crate::application::ports::DaemonConnection;
use crate::application::register_domain::RegisterDomain;
use crate::application::unregister_domain::UnregisterDomain;
use crate::branding::BRANDING;
use crate::cli::project_env;
use crate::domain::{DomainPattern, DomainRegistration, Route};
use crate::infrastructure::certs::CertificateService;
use crate::infrastructure::config::{
    ConfigStore, ConfiguredProject, ConfiguredService, ProjectServiceConfig,
    ProjectServiceDomainUpdate,
};
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use roxy::project_init::{
    FolderInitSelection, FolderInitServicePlan, LocalAppUpdatePolicy, ServicePlan,
    apply_local_app_updates, detect_current_dir_localify_target, detect_folder_init_plan,
    resolve_folder_init_selection,
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
    let detected_services = detected.services.clone();
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
    let materialized_count = materialize_inferred_project_services(
        &config_store,
        &project.id,
        &current_dir,
        &detected_services,
    )?;
    let update_results = apply_local_app_updates(
        &resolved.package_updates,
        &resolved.next_config_updates,
        &resolved.vite_config_updates,
    );

    if materialized_count > 0 {
        println!("  Materialized project services: {materialized_count}");
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

pub fn set_domain(args: SetDomainArgs, ctx: &AppContext) -> Result<()> {
    let implicit_environment = if args.environment.is_none() {
        project_env::ensure_implicit_worktree_environment(args.project.as_deref(), ctx)?
    } else {
        None
    };
    let environment = args.environment.as_deref().or_else(|| {
        implicit_environment
            .as_ref()
            .map(|environment| environment.slug.as_str())
    });

    if let Some(environment) = environment {
        if let Some(environment) = implicit_environment.as_ref() {
            eprintln!(
                "Using project environment '{}' [{}] for worktree.",
                environment.name, environment.slug
            );
        }
        project_env::set_environment_service_domain(
            args.project.as_deref(),
            environment,
            &args.service,
            &args.domain,
            ctx,
        )?;
        return Ok(());
    }

    let current_dir = effective_current_dir()?;
    let project = resolve_project(&ctx.config_store, args.project.as_deref(), &current_dir)?;
    let service = ctx
        .config_store
        .list_services()?
        .into_iter()
        .filter(|service| service.project_id == project.id)
        .find(|service| service.id == args.service || service.name.eq_ignore_ascii_case(&args.service))
        .ok_or_else(|| {
            anyhow!(
                "Service '{}' was not found in project '{}'. Run '{} services list' to inspect configured services.",
                args.service,
                project.id,
                BRANDING.cli_command()
            )
        })?;

    let current_pattern = domain_pattern_from_display(&service.primary_domain)?;
    let current_registration = ctx
        .config_store
        .get_domain(&current_pattern)?
        .ok_or_else(|| anyhow!("Domain '{}' is not registered.", service.primary_domain))?;
    let new_pattern = domain_pattern_from_display(&args.domain)?;
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
    let mut new_registration = DomainRegistration::with_source(
        new_pattern,
        current_registration.routes().to_vec(),
        current_registration.source(),
    );
    if cert_installed {
        new_registration.enable_https();
    }
    let new_domain = new_registration.display_pattern();

    ctx.config_store.replace_project_service_domain(
        &project.id,
        &service.id,
        ProjectServiceDomainUpdate {
            current_domain: service.primary_domain.clone(),
            new_registration,
        },
    )?;
    reload_daemon(ctx)?;

    println!(
        "Updated service '{}' domain: {} -> {}",
        service.id, service.primary_domain, new_domain
    );
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

fn materialize_inferred_project_services(
    config_store: &ConfigStore,
    project_id: &str,
    root: &Path,
    detected_services: &[FolderInitServicePlan],
) -> Result<usize> {
    let services = config_store
        .list_services()?
        .into_iter()
        .filter(|service| service.project_id == project_id)
        .collect::<Vec<_>>();
    if services.iter().all(|service| !service.inferred) {
        return Ok(0);
    }

    let (project_services, materialized_count) =
        materialized_project_services(&services, root, detected_services)?;
    if materialized_count > 0 {
        config_store.set_project_services(project_id, project_services)?;
    }

    Ok(materialized_count)
}

fn materialized_project_services(
    services: &[ConfiguredService],
    root: &Path,
    detected_services: &[FolderInitServicePlan],
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
    service: &'a FolderInitServicePlan,
    legacy_target: Option<String>,
}

fn detected_service_matches<'a>(
    root: &Path,
    services: &'a [FolderInitServicePlan],
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
                legacy_target: detect_current_dir_localify_target(&package_dir)?,
            })
        })
        .collect()
}

fn detected_service_for_existing<'a>(
    service: &ConfiguredService,
    detected: &'a [DetectedServiceMatch<'a>],
) -> Option<&'a FolderInitServicePlan> {
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
        crate::domain::RouteTarget::Proxy(target) => Some(target.port().value().to_string()),
        crate::domain::RouteTarget::StaticFiles(_) => None,
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
    config_store
        .update_domain_with_project(result.registration.clone(), Some(project_id.to_string()))?;

    if let Some(project_service) = project_service_config(service)
        && let Err(error) = config_store.add_project_service(project_id, project_service)
    {
        let unregister_domain = UnregisterDomain::new(config_store, &certs);
        let _ = unregister_domain.execute(result.registration.pattern());
        return Err(error.into());
    }

    match MgmtSocketClient::new(&config.paths.socket_path).reload() {
        Ok(()) => println!("  Daemon: reloaded"),
        Err(crate::application::ports::DaemonConnectionError::NotRunning) => {
            println!("  Daemon: not running");
            println!("  Start it with: {} start", BRANDING.cli_command());
        }
        Err(error) => bail!("Failed to reload the running daemon: {error}"),
    }

    Ok(())
}

fn reload_daemon(ctx: &AppContext) -> Result<()> {
    match ctx.mgmt_client.reload() {
        Ok(()) => println!("  Daemon: reloaded"),
        Err(crate::application::ports::DaemonConnectionError::NotRunning) => {
            println!("  Daemon: not running");
            println!("  Start it with: {} start", BRANDING.cli_command());
        }
        Err(error) => bail!("Failed to reload the running daemon: {error}"),
    }
    Ok(())
}

fn project_service_config(service: &ServicePlan) -> Option<ProjectServiceConfig> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::config::{Config, RoxyPaths};

    #[test]
    fn register_project_service_persists_explicit_service_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let config_store = ConfigStore::new(config_path);
        let mut config = Config::default();
        config.paths = RoxyPaths {
            data_dir: temp_dir.path().join("data"),
            pid_file: temp_dir.path().join("localify.pid"),
            log_file: temp_dir.path().join("localify.log"),
            certs_dir: temp_dir.path().join("certs"),
            socket_path: temp_dir.path().join("localify.sock"),
        };
        config_store.save(&config).unwrap();
        let project = config_store
            .create_project_with_root("Client App", Some(temp_dir.path().display().to_string()))
            .unwrap();

        register_project_service(
            &config_store,
            &project.id,
            &ServicePlan {
                domain: "web.client-app.localify".into(),
                target: "3000".into(),
                service_name: Some("web".into()),
                service_path: Some("apps/web".into()),
                command: Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]),
            },
        )
        .unwrap();

        let services = config_store.list_services().unwrap();

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "web");
        assert_eq!(services[0].primary_domain, "web.client-app.localify");
        assert_eq!(services[0].path.as_deref(), Some("apps/web"));
        assert_eq!(
            services[0].command,
            Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()])
        );
        assert!(!services[0].inferred);
    }

    #[test]
    fn materialize_inferred_services_reuses_legacy_target_mapping() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let config_store = ConfigStore::new(config_path);
        let mut config = Config::default();
        config.paths = RoxyPaths {
            data_dir: temp_dir.path().join("data"),
            pid_file: temp_dir.path().join("localify.pid"),
            log_file: temp_dir.path().join("localify.log"),
            certs_dir: temp_dir.path().join("certs"),
            socket_path: temp_dir.path().join("localify.sock"),
        };
        config_store.save(&config).unwrap();

        let root = temp_dir.path().join("ACME");
        let app_dir = root.join("apps/org-space");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"org-space","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52164","dev":"next dev"}}"#,
        )
        .unwrap();

        let project = config_store
            .create_project_with_root("ACME", Some(root.display().to_string()))
            .unwrap();
        config_store
            .add_domain_with_project(
                test_proxy_registration(&format!("acteur.acme{}", BRANDING.domain_suffix()), 52164),
                Some(project.id.clone()),
            )
            .unwrap();
        let detected = detect_folder_init_plan(&root, Some(&project.name)).unwrap();

        let materialized_count = materialize_inferred_project_services(
            &config_store,
            &project.id,
            &root,
            &detected.services,
        )
        .unwrap();
        let services = config_store.list_services().unwrap();

        assert_eq!(materialized_count, 1);
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "acteur");
        assert_eq!(services[0].path.as_deref(), Some("apps/org-space"));
        assert_eq!(
            services[0].command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
        assert!(!services[0].inferred);
    }

    fn test_proxy_registration(domain: &str, target: u16) -> DomainRegistration {
        DomainRegistration::new(
            DomainPattern::from_name(domain, false).unwrap(),
            vec![Route::parse(&format!("/={target}")).unwrap()],
        )
    }
}
