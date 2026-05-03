use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

use crate::branding::BRANDING;
use roxy::desktop::dto::{InitProjectRequest, InitProjectServiceRequest};
use roxy::desktop::{DesktopApi, ProjectRootStore};
use roxy::project_init::{
    FolderInitSelection, FolderInitServicePlan, NextConfigUpdate, PackageScriptUpdate, ServicePlan,
    apply_local_app_updates, build_init_plan, detect_current_dir_localify_target,
    detect_folder_init_plan, plan_current_dir_local_app_updates, resolve_folder_init_selection,
};

pub struct InitProjectArgs {
    pub name: Option<String>,
    pub domain: Option<String>,
    pub target: Option<String>,
    pub with_api: bool,
    pub api_domain: Option<String>,
    pub api_target: Option<String>,
    pub project_only: bool,
}

struct ResolvedInitProjectArgs {
    name: String,
    services: Vec<ServicePlan>,
    package_updates: Vec<PackageScriptUpdate>,
    next_config_updates: Vec<NextConfigUpdate>,
}

pub fn execute(args: InitProjectArgs, config_path: &Path) -> Result<()> {
    let resolved = resolve_args(args)?;
    let current_dir = env::current_dir().context("Failed to determine the current directory")?;
    let binary_path = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.cli_command()
    ))?;
    let api = DesktopApi::new(config_path.to_path_buf(), binary_path);
    let init_result = api.init_project(InitProjectRequest {
        name: resolved.name,
        root_path: Some(current_dir.display().to_string()),
        services: resolved
            .services
            .iter()
            .map(|service| InitProjectServiceRequest {
                domain: service.domain.clone(),
                target: service.target.clone(),
            })
            .collect(),
    })?;
    ProjectRootStore::new_default()?.set(&init_result.project.id, &current_dir)?;

    println!("Initialized project: {}", init_result.project.name);
    println!("  Project ID: {}", init_result.project.id);

    if init_result.services.is_empty() {
        println!("  Domains: none");
    } else {
        println!("  Domains:");
        for service in &init_result.services {
            println!("    {} -> {}", service.domain, service.target);
        }
    }

    let update_results =
        apply_local_app_updates(&resolved.package_updates, &resolved.next_config_updates);
    if !update_results.updated.is_empty() {
        println!("  Updated local app files:");
        for updated in &update_results.updated {
            println!("    {updated}");
        }
    }
    if !update_results.skipped.is_empty() {
        println!("  Skipped local app file updates:");
        for skipped in &update_results.skipped {
            println!("    {skipped}");
        }
    }

    if init_result.daemon_reloaded {
        println!("  Daemon: reloaded");
    } else {
        println!("  Daemon: not running");
        println!("  Start it with: {} start", BRANDING.cli_command());
    }

    Ok(())
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

fn resolve_args(args: InitProjectArgs) -> Result<ResolvedInitProjectArgs> {
    let current_dir = env::current_dir().context("Failed to determine the current directory")?;
    let default_name = args
        .name
        .clone()
        .unwrap_or(current_directory_name(&current_dir)?);
    validate_init_project_flags(
        args.project_only,
        args.with_api,
        args.domain.as_ref(),
        args.target.as_ref(),
    )?;
    if args.project_only && (args.api_domain.is_some() || args.api_target.is_some()) {
        bail!("--project-only cannot be combined with API initialization options");
    }

    if !is_interactive_terminal() {
        return resolve_non_interactive_args(default_name, args);
    }

    let name = prompt_string("Project name", &default_name)?;
    if args.project_only {
        return Ok(ResolvedInitProjectArgs {
            name,
            services: Vec::new(),
            package_updates: Vec::new(),
            next_config_updates: Vec::new(),
        });
    }

    if has_explicit_service_args(&args) {
        return resolve_non_interactive_args(name, args);
    }

    let detected = detect_folder_init_plan(&current_dir, Some(&name))?;
    let services = detected.services;

    if services.len() == 1 && services[0].id == "__primary__" {
        return resolve_single_service_interactive(name, &current_dir, &services[0]);
    }

    resolve_workspace_services_interactive(name, &current_dir, &services)
}

fn resolve_non_interactive_args(
    name: String,
    mut args: InitProjectArgs,
) -> Result<ResolvedInitProjectArgs> {
    let current_dir = env::current_dir().context("Failed to determine the current directory")?;
    if !args.project_only && args.target.is_none() {
        args.target = detect_current_dir_localify_target(&current_dir)?;
    }

    let init_plan = build_init_plan(
        &name,
        args.domain,
        args.target,
        args.with_api || args.api_domain.is_some() || args.api_target.is_some(),
        args.api_domain,
        args.api_target,
        args.project_only,
    )?;
    let next_config_updates =
        plan_current_dir_local_app_updates(&current_dir, &name, &init_plan.services)?;

    Ok(ResolvedInitProjectArgs {
        name,
        services: init_plan.services,
        package_updates: Vec::new(),
        next_config_updates,
    })
}

fn has_explicit_service_args(args: &InitProjectArgs) -> bool {
    args.domain.is_some()
        || args.target.is_some()
        || args.with_api
        || args.api_domain.is_some()
        || args.api_target.is_some()
}

fn resolve_single_service_interactive(
    name: String,
    current_dir: &Path,
    service: &FolderInitServicePlan,
) -> Result<ResolvedInitProjectArgs> {
    let domain = prompt_string("Primary domain", &service.domain)?;
    let target = prompt_string("Primary target", &service.target)?;
    let resolved = resolve_folder_init_selection(
        current_dir,
        &name,
        &[FolderInitSelection {
            id: service.id.clone(),
            domain,
            target,
            enabled: true,
            update_dev_script: false,
        }],
    )?;

    Ok(ResolvedInitProjectArgs {
        name,
        services: resolved.services,
        package_updates: resolved.package_updates,
        next_config_updates: resolved.next_config_updates,
    })
}

fn resolve_workspace_services_interactive(
    name: String,
    current_dir: &Path,
    services: &[FolderInitServicePlan],
) -> Result<ResolvedInitProjectArgs> {
    let mut selections = Vec::with_capacity(services.len());

    for service in services {
        let create_prompt = format!("Create domain for {}?", service.display_path);
        if !prompt_bool(&create_prompt, service.enabled)? {
            selections.push(FolderInitSelection {
                id: service.id.clone(),
                domain: service.domain.clone(),
                target: service.target.clone(),
                enabled: false,
                update_dev_script: false,
            });
            continue;
        }

        let domain = prompt_string(
            &format!("Domain for {}", service.display_path),
            &service.domain,
        )?;
        let target = prompt_string(
            &format!("Target for {}", service.display_path),
            &service.target,
        )?;
        let update_dev_script = prompt_update_dev_script(service, &target)?;

        selections.push(FolderInitSelection {
            id: service.id.clone(),
            domain,
            target,
            enabled: true,
            update_dev_script,
        });
    }

    let resolved = resolve_folder_init_selection(current_dir, &name, &selections)?;
    Ok(ResolvedInitProjectArgs {
        name,
        services: resolved.services,
        package_updates: resolved.package_updates,
        next_config_updates: resolved.next_config_updates,
    })
}

fn prompt_update_dev_script(service: &FolderInitServicePlan, target: &str) -> Result<bool> {
    if !service.can_update_dev_script || target.parse::<u16>().is_err() {
        return Ok(false);
    }

    prompt_bool(
        &format!(
            "Update {} dev script to use port {}?",
            service.display_path, target
        ),
        service.update_dev_script,
    )
}

fn is_interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn current_directory_name(current_dir: &Path) -> Result<String> {
    let name = current_dir
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Could not infer a project name from the current directory"))?;

    Ok(name.to_string())
}

fn prompt_string(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    io::stdout().flush()?;

    let mut buffer = String::new();
    io::stdin().read_line(&mut buffer)?;
    let value = buffer.trim();

    if value.is_empty() {
        return Ok(default.to_string());
    }

    Ok(value.to_string())
}

fn prompt_bool(label: &str, default: bool) -> Result<bool> {
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    print!("{label} {hint}: ");
    io::stdout().flush()?;

    let mut buffer = String::new();
    io::stdin().read_line(&mut buffer)?;
    let value = buffer.trim().to_lowercase();

    if value.is_empty() {
        return Ok(default);
    }

    match value.as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => bail!("Please answer yes or no"),
    }
}
