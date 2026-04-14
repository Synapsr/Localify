use std::collections::HashSet;
use std::net::TcpListener;

use anyhow::{Context, Result, anyhow, bail};

use super::context::AppContext;
use crate::application::ports::DaemonConnection;
use crate::application::register_domain::RegisterDomain;
use crate::application::unregister_domain::UnregisterDomain;
use crate::branding::BRANDING;
use crate::domain::{DomainPattern, Route};

pub struct InitProjectArgs {
    pub name: String,
    pub domain: Option<String>,
    pub target: Option<String>,
    pub with_api: bool,
    pub api_domain: Option<String>,
    pub api_target: Option<String>,
    pub project_only: bool,
}

pub fn execute(args: InitProjectArgs, ctx: &AppContext) -> Result<()> {
    validate_init_project_flags(
        args.project_only,
        args.with_api,
        args.domain.as_ref(),
        args.target.as_ref(),
    )?;
    if args.project_only && (args.api_domain.is_some() || args.api_target.is_some()) {
        bail!("--project-only cannot be combined with API initialization options");
    }

    let project = ctx.config_store.create_project(&args.name)?;
    let init_plan = build_init_plan(
        &args.name,
        args.domain,
        args.target,
        args.with_api || args.api_domain.is_some() || args.api_target.is_some(),
        args.api_domain,
        args.api_target,
        args.project_only,
    )?;

    let register_domain = RegisterDomain::new(&ctx.config_store, &ctx.cert_service);
    let unregister_domain = UnregisterDomain::new(&ctx.config_store, &ctx.cert_service);
    let mut created_patterns = Vec::new();

    for service in &init_plan.services {
        let pattern = DomainPattern::from_name(&service.domain, false)?;
        let route = Route::parse(&format!("/={}", service.target))
            .map_err(|error| anyhow!("Invalid target for '{}': {error}", service.domain))?;

        let result = register_domain.execute(pattern.clone(), vec![route])?;
        ctx.config_store
            .update_domain_with_project(result.registration, Some(project.id.clone()))?;
        created_patterns.push(pattern);
    }

    let reloaded_daemon = match reload_if_running(ctx) {
        Ok(reloaded) => reloaded,
        Err(error) => {
            rollback_init_project(&project.id, &created_patterns, &unregister_domain, ctx)?;
            return Err(error);
        }
    };

    println!("Initialized project: {}", project.name);
    println!("  Project ID: {}", project.id);

    if init_plan.services.is_empty() {
        println!("  Domains: none");
    } else {
        println!("  Domains:");
        for service in &init_plan.services {
            println!("    {} -> {}", service.domain, service.target);
        }
    }

    if reloaded_daemon {
        println!("  Daemon: reloaded");
    } else {
        println!("  Daemon: not running");
        println!("  Start it with: {} start", BRANDING.cli_command());
    }

    Ok(())
}

fn rollback_init_project(
    project_id: &str,
    created_patterns: &[DomainPattern],
    unregister_domain: &UnregisterDomain<'_>,
    ctx: &AppContext,
) -> Result<()> {
    for pattern in created_patterns.iter().rev() {
        unregister_domain.execute(pattern).with_context(|| {
            format!("Failed to roll back domain '{}'", pattern.display_pattern())
        })?;
    }

    ctx.config_store
        .remove_project(project_id)
        .with_context(|| format!("Failed to roll back project '{project_id}'"))?;

    Ok(())
}

fn reload_if_running(ctx: &AppContext) -> Result<bool> {
    match ctx.mgmt_client.reload() {
        Ok(()) => Ok(true),
        Err(crate::application::ports::DaemonConnectionError::NotRunning) => Ok(false),
        Err(error) => Err(anyhow!("Failed to reload the running daemon: {error}")),
    }
}

fn validate_init_project_flags(
    project_only: bool,
    with_api: bool,
    domain: Option<&String>,
    target: Option<&String>,
) -> Result<()> {
    if project_only && (with_api || domain.is_some() || target.is_some()) {
        bail!(
            "--project-only cannot be combined with domain, target, or API initialization options"
        );
    }

    Ok(())
}

#[derive(Debug)]
struct ServicePlan {
    domain: String,
    target: String,
}

fn build_init_plan(
    project_name: &str,
    domain: Option<String>,
    target: Option<String>,
    with_api: bool,
    api_domain: Option<String>,
    api_target: Option<String>,
    project_only: bool,
) -> Result<InitPlan> {
    if project_only {
        return Ok(InitPlan {
            services: Vec::new(),
        });
    }

    let slug = slugify(project_name)?;
    let domain = normalize_domain(domain.unwrap_or_else(|| slug.clone()));
    let auto_target_count =
        usize::from(target.is_none()) + usize::from(with_api && api_target.is_none());
    let api_domain = if with_api {
        Some(normalize_domain(
            api_domain.unwrap_or_else(|| format!("api-{slug}")),
        ))
    } else {
        None
    };

    let auto_targets = reserve_auto_targets(auto_target_count)?;
    let mut next_auto_target = auto_targets.into_iter();
    let target = target.unwrap_or_else(|| next_auto_target.next().unwrap());
    let api_target = if with_api {
        Some(api_target.unwrap_or_else(|| next_auto_target.next().unwrap()))
    } else {
        None
    };

    let mut seen_domains = HashSet::new();
    if !seen_domains.insert(domain.clone()) {
        bail!("Duplicate domain in init-project plan: {domain}");
    }

    let mut services = vec![ServicePlan { domain, target }];
    if let (Some(domain), Some(target)) = (api_domain, api_target) {
        if !seen_domains.insert(domain.clone()) {
            bail!("Duplicate domain in init-project plan: {domain}");
        }
        services.push(ServicePlan { domain, target });
    }

    Ok(InitPlan { services })
}

#[derive(Debug)]
struct InitPlan {
    services: Vec<ServicePlan>,
}

fn reserve_auto_targets(count: usize) -> Result<Vec<String>> {
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
        ports.push(port.to_string());
    }

    drop(listeners);
    Ok(ports)
}

fn normalize_domain(value: String) -> String {
    let trimmed = value.trim();
    if trimmed.ends_with(BRANDING.domain_suffix) {
        return trimmed.to_string();
    }

    format!("{trimmed}{}", BRANDING.domain_suffix)
}

fn slugify(value: &str) -> Result<String> {
    let slug = value
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if slug.is_empty() {
        bail!("Project name must contain letters or numbers");
    }

    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_init_plan_defaults_to_slugged_primary_domain() {
        let plan = build_init_plan("Test 1", None, None, false, None, None, false).unwrap();

        assert_eq!(plan.services.len(), 1);
        assert_eq!(
            plan.services[0].domain,
            format!("test-1{}", BRANDING.domain_suffix)
        );
        assert!(!plan.services[0].target.is_empty());
    }

    #[test]
    fn build_init_plan_adds_default_api_domain() {
        let plan = build_init_plan("Test 1", None, None, true, None, None, false).unwrap();

        assert_eq!(plan.services.len(), 2);
        assert_eq!(
            plan.services[1].domain,
            format!("api-test-1{}", BRANDING.domain_suffix)
        );
    }

    #[test]
    fn normalize_domain_appends_suffix_once() {
        assert_eq!(
            normalize_domain("demo".into()),
            format!("demo{}", BRANDING.domain_suffix)
        );
        assert_eq!(
            normalize_domain(format!("demo{}", BRANDING.domain_suffix)),
            format!("demo{}", BRANDING.domain_suffix)
        );
    }
}
