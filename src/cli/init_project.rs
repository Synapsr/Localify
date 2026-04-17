use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::branding::BRANDING;
use roxy::desktop::DesktopApi;
use roxy::desktop::dto::{InitProjectRequest, InitProjectServiceRequest};

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
}

pub fn execute(args: InitProjectArgs, config_path: &Path) -> Result<()> {
    let resolved = resolve_args(args)?;
    let binary_path = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.cli_command()
    ))?;
    let api = DesktopApi::new(config_path.to_path_buf(), binary_path);
    let init_result = api.init_project(InitProjectRequest {
        name: resolved.name,
        services: resolved
            .services
            .iter()
            .map(|service| InitProjectServiceRequest {
                domain: service.domain.clone(),
                target: service.target.clone(),
            })
            .collect(),
    })?;

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

    let update_results = apply_package_updates(&resolved.package_updates);
    if !update_results.updated.is_empty() {
        println!("  Updated dev scripts:");
        for updated in &update_results.updated {
            println!("    {updated}");
        }
    }
    if !update_results.skipped.is_empty() {
        println!("  Skipped dev script updates:");
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
    let explicit_service_args = has_explicit_service_args(&args);

    if args.project_only {
        return Ok(ResolvedInitProjectArgs {
            name,
            services: Vec::new(),
            package_updates: Vec::new(),
        });
    }

    if explicit_service_args {
        return resolve_non_interactive_args(name, args);
    }

    let workspace_apps = detect_workspace_apps(&current_dir)?;
    if workspace_apps.is_empty() {
        return resolve_single_service_interactive(name);
    }

    resolve_workspace_services_interactive(name, &workspace_apps)
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

#[derive(Debug)]
struct ServicePlan {
    domain: String,
    target: String,
}

struct PackageScriptUpdate {
    package_json_path: PathBuf,
    display_path: String,
    script: String,
}

struct PackageUpdateResults {
    updated: Vec<String>,
    skipped: Vec<String>,
}

#[derive(Debug)]
struct WorkspaceAppCandidate {
    label: String,
    display_path: String,
    package_json_path: PathBuf,
    dev_script: String,
    is_root: bool,
}

#[derive(Deserialize)]
struct PackageJsonManifest {
    scripts: Option<Map<String, Value>>,
    workspaces: Option<Value>,
}

fn resolve_non_interactive_args(
    name: String,
    args: InitProjectArgs,
) -> Result<ResolvedInitProjectArgs> {
    let init_plan = build_init_plan(
        &name,
        args.domain,
        args.target,
        args.with_api || args.api_domain.is_some() || args.api_target.is_some(),
        args.api_domain,
        args.api_target,
        args.project_only,
    )?;

    Ok(ResolvedInitProjectArgs {
        name,
        services: init_plan.services,
        package_updates: Vec::new(),
    })
}

fn has_explicit_service_args(args: &InitProjectArgs) -> bool {
    args.domain.is_some()
        || args.target.is_some()
        || args.with_api
        || args.api_domain.is_some()
        || args.api_target.is_some()
}

fn resolve_single_service_interactive(name: String) -> Result<ResolvedInitProjectArgs> {
    let slug = slugify(&name)?;
    let default_domain = normalize_domain(slug);
    let default_target = reserve_auto_targets(1)?
        .into_iter()
        .next()
        .unwrap_or_else(|| "3000".into());

    let domain = prompt_string("Primary domain", &default_domain)?;
    let target = prompt_string("Primary target", &default_target)?;

    Ok(ResolvedInitProjectArgs {
        name,
        services: vec![ServicePlan { domain, target }],
        package_updates: Vec::new(),
    })
}

fn resolve_workspace_services_interactive(
    name: String,
    workspace_apps: &[WorkspaceAppCandidate],
) -> Result<ResolvedInitProjectArgs> {
    let slug = slugify(&name)?;
    let default_targets = reserve_auto_targets(workspace_apps.len())?;
    let mut services = Vec::new();
    let mut package_updates = Vec::new();

    for (index, app) in workspace_apps.iter().enumerate() {
        let default_enabled = if app.is_root && workspace_apps.len() > 1 {
            false
        } else {
            index == 0 || matches!(app.label.as_str(), "web" | "app")
        };
        let create_prompt = format!("Create domain for {}?", app.display_path);
        if !prompt_bool(&create_prompt, default_enabled)? {
            continue;
        }

        let default_domain =
            default_workspace_domain(&slug, &app.label, services.is_empty(), app.is_root);
        let default_target = default_targets
            .get(index)
            .cloned()
            .unwrap_or_else(|| "3000".into());

        let domain = prompt_string(&format!("Domain for {}", app.display_path), &default_domain)?;
        let target = prompt_string(&format!("Target for {}", app.display_path), &default_target)?;

        if let Some(update) = prompt_package_script_update(app, &target)? {
            package_updates.push(update);
        }

        services.push(ServicePlan { domain, target });
    }

    Ok(ResolvedInitProjectArgs {
        name,
        services,
        package_updates,
    })
}

fn default_workspace_domain(
    project_slug: &str,
    label: &str,
    is_primary: bool,
    is_root: bool,
) -> String {
    if is_primary && (is_root || matches!(label, "web" | "app")) {
        return normalize_domain(project_slug.to_string());
    }

    normalize_domain(format!("{label}-{project_slug}"))
}

fn detect_workspace_apps(root: &Path) -> Result<Vec<WorkspaceAppCandidate>> {
    let mut manifests = Vec::new();
    let root_manifest = root.join("package.json");
    let root_is_workspace =
        root_manifest.exists() && package_json_declares_workspaces(&root_manifest)?;
    if root_manifest.exists() {
        manifests.push(root_manifest);
    }

    for directory in ["apps", "packages", "services"] {
        let parent = root.join(directory);
        if !parent.is_dir() {
            continue;
        }

        for entry in fs::read_dir(&parent)
            .with_context(|| format!("Failed to read workspace directory '{}'", parent.display()))?
        {
            let entry = entry?;
            let package_json_path = entry.path().join("package.json");
            if package_json_path.exists() {
                manifests.push(package_json_path);
            }
        }
    }

    let mut apps = Vec::new();
    for manifest_path in manifests {
        if let Some(app) = read_workspace_app(root, &manifest_path)? {
            apps.push(app);
        }
    }

    if root_is_workspace && apps.iter().any(|app| !app.is_root) {
        apps.retain(|app| !app.is_root);
    }

    Ok(apps)
}

fn package_json_declares_workspaces(package_json_path: &Path) -> Result<bool> {
    let content = fs::read_to_string(package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: PackageJsonManifest = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;
    Ok(manifest.workspaces.is_some())
}

fn read_workspace_app(
    root: &Path,
    package_json_path: &Path,
) -> Result<Option<WorkspaceAppCandidate>> {
    let content = fs::read_to_string(package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: PackageJsonManifest = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;
    let Some(dev_script) = manifest
        .scripts
        .as_ref()
        .and_then(|scripts| scripts.get("dev"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let package_dir = package_json_path.parent().ok_or_else(|| {
        anyhow!(
            "Invalid package.json path '{}'",
            package_json_path.display()
        )
    })?;
    let relative_dir = package_dir
        .strip_prefix(root)
        .unwrap_or(package_dir)
        .display()
        .to_string();
    let label = package_dir
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "Could not infer an app name from '{}'",
                package_dir.display()
            )
        })?
        .to_string();
    let display_path = if relative_dir.is_empty() || relative_dir == "." {
        label.clone()
    } else {
        relative_dir
    };

    Ok(Some(WorkspaceAppCandidate {
        label,
        display_path,
        package_json_path: package_json_path.to_path_buf(),
        dev_script: dev_script.to_string(),
        is_root: package_dir == root,
    }))
}

fn prompt_package_script_update(
    app: &WorkspaceAppCandidate,
    target: &str,
) -> Result<Option<PackageScriptUpdate>> {
    if !is_port_target(target) {
        return Ok(None);
    }

    let Some(script) = plan_dev_script_update(&app.dev_script, target) else {
        return Ok(None);
    };

    let prompt = format!(
        "Update {} dev script to use port {}?",
        app.display_path, target
    );
    if !prompt_bool(&prompt, true)? {
        return Ok(None);
    }

    Ok(Some(PackageScriptUpdate {
        package_json_path: app.package_json_path.clone(),
        display_path: app.display_path.clone(),
        script,
    }))
}

fn is_port_target(target: &str) -> bool {
    target.parse::<u16>().is_ok()
}

fn plan_dev_script_update(script: &str, port: &str) -> Option<String> {
    let trimmed = script.trim();
    if script_has_explicit_port(trimmed) {
        return None;
    }

    if trimmed.contains("next dev") {
        let updated = trimmed.replacen(
            "next dev",
            &format!("next dev --hostname 127.0.0.1 --port {port}"),
            1,
        );
        return Some(updated);
    }
    if trimmed == "vite" || trimmed.starts_with("vite ") {
        return Some(format!("{trimmed} --host 127.0.0.1 --port {port}"));
    }
    if trimmed.contains("astro dev") {
        let updated = trimmed.replacen(
            "astro dev",
            &format!("astro dev --host 127.0.0.1 --port {port}"),
            1,
        );
        return Some(updated);
    }
    if trimmed.contains("nuxt dev") {
        let updated = trimmed.replacen(
            "nuxt dev",
            &format!("nuxt dev --host 127.0.0.1 --port {port}"),
            1,
        );
        return Some(updated);
    }
    if trimmed.contains("react-scripts start") {
        return Some(format!("HOST=127.0.0.1 PORT={port} {trimmed}"));
    }

    None
}

fn script_has_explicit_port(script: &str) -> bool {
    script.contains("--port")
        || script.contains("--host")
        || script.contains("--hostname")
        || script.contains(" -p ")
        || script.starts_with("HOST=")
        || script.contains(" HOST=")
        || script.starts_with("PORT=")
        || script.contains(" PORT=")
}

fn apply_package_updates(updates: &[PackageScriptUpdate]) -> PackageUpdateResults {
    let mut results = PackageUpdateResults {
        updated: Vec::new(),
        skipped: Vec::new(),
    };

    for update in updates {
        match apply_package_update(update) {
            Ok(()) => results.updated.push(format!(
                "{} ({})",
                update.display_path,
                update.package_json_path.display()
            )),
            Err(error) => results.skipped.push(format!(
                "{} ({}) - {}",
                update.display_path,
                update.package_json_path.display(),
                error
            )),
        }
    }

    results
}

fn apply_package_update(update: &PackageScriptUpdate) -> Result<()> {
    let content = fs::read_to_string(&update.package_json_path)
        .with_context(|| format!("Failed to read '{}'", update.package_json_path.display()))?;
    let mut package_json: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", update.package_json_path.display()))?;
    let scripts = package_json
        .get_mut("scripts")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("Missing scripts object"))?;
    let current_script = scripts
        .get("dev")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing scripts.dev"))?;

    if current_script.trim() == update.script.trim() {
        return Ok(());
    }

    scripts.insert("dev".into(), Value::String(update.script.clone()));
    let formatted = serde_json::to_string_pretty(&package_json).with_context(|| {
        format!(
            "Failed to serialize '{}'",
            update.package_json_path.display()
        )
    })?;
    fs::write(&update.package_json_path, format!("{formatted}\n"))
        .with_context(|| format!("Failed to write '{}'", update.package_json_path.display()))?;

    Ok(())
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
    use tempfile::tempdir;

    fn write_package_json(path: &Path, dev_script: &str) {
        let package_json = format!(
            r#"{{
  "name": "test-app",
  "scripts": {{
    "dev": "{dev_script}"
  }}
}}"#
        );
        fs::write(path, package_json).unwrap();
    }

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

    #[test]
    fn plan_dev_script_update_supports_next_vite_and_react_scripts() {
        assert_eq!(
            plan_dev_script_update("next dev", "4010").as_deref(),
            Some("next dev --hostname 127.0.0.1 --port 4010")
        );
        assert_eq!(
            plan_dev_script_update("vite", "4011").as_deref(),
            Some("vite --host 127.0.0.1 --port 4011")
        );
        assert_eq!(
            plan_dev_script_update("react-scripts start", "4012").as_deref(),
            Some("HOST=127.0.0.1 PORT=4012 react-scripts start")
        );
    }

    #[test]
    fn plan_dev_script_update_skips_scripts_that_already_set_a_port() {
        assert!(plan_dev_script_update("next dev --port 3000", "4010").is_none());
        assert!(plan_dev_script_update("vite --host 127.0.0.1", "4010").is_none());
        assert!(plan_dev_script_update("PORT=3000 react-scripts start", "4010").is_none());
    }

    #[test]
    fn detect_workspace_apps_reads_root_and_apps_directories() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::create_dir_all(root.join("apps/admin")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "name": "repo-root",
  "workspaces": ["apps/*"],
  "scripts": {
    "dev": "turbo dev"
  }
}"#,
        )
        .unwrap();
        write_package_json(&root.join("apps/web/package.json"), "next dev");
        write_package_json(&root.join("apps/admin/package.json"), "astro dev");

        let apps = detect_workspace_apps(root).unwrap();
        let labels = apps
            .iter()
            .map(|app| app.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["web", "admin"]);
    }

    #[test]
    fn detect_workspace_apps_keeps_root_for_single_package_project() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        write_package_json(&root.join("package.json"), "vite");

        let apps = detect_workspace_apps(root).unwrap();

        assert_eq!(apps.len(), 1);
        assert!(apps[0].is_root);
    }

    #[test]
    fn apply_package_update_rewrites_scripts_dev() {
        let temp_dir = tempdir().unwrap();
        let package_json_path = temp_dir.path().join("package.json");
        write_package_json(&package_json_path, "vite");

        apply_package_update(&PackageScriptUpdate {
            package_json_path: package_json_path.clone(),
            display_path: "app".into(),
            script: "vite --port 4010".into(),
        })
        .unwrap();

        let updated = fs::read_to_string(package_json_path).unwrap();
        assert!(updated.contains(r#""dev": "vite --port 4010""#));
    }
}
