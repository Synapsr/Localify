use std::collections::HashSet;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::branding::BRANDING;

const SYNTHETIC_PRIMARY_SERVICE_ID: &str = "__primary__";

#[derive(Debug, Clone)]
pub struct ServicePlan {
    pub domain: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct InitPlan {
    pub services: Vec<ServicePlan>,
}

#[derive(Debug, Clone)]
pub struct FolderInitServicePlan {
    pub id: String,
    pub label: String,
    pub display_path: String,
    pub domain: String,
    pub target: String,
    pub enabled: bool,
    pub can_update_dev_script: bool,
    pub update_dev_script: bool,
}

#[derive(Debug, Clone)]
pub struct FolderInitPlan {
    pub name: String,
    pub services: Vec<FolderInitServicePlan>,
}

#[derive(Debug, Clone)]
pub struct FolderInitSelection {
    pub id: String,
    pub domain: String,
    pub target: String,
    pub enabled: bool,
    pub update_dev_script: bool,
}

#[derive(Debug, Clone)]
pub struct PackageScriptUpdate {
    pub package_json_path: PathBuf,
    pub display_path: String,
    pub script: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedFolderInitSelection {
    pub services: Vec<ServicePlan>,
    pub package_updates: Vec<PackageScriptUpdate>,
}

#[derive(Debug, Clone)]
pub struct PackageUpdateResults {
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
struct FolderServiceBlueprint {
    id: String,
    label: String,
    display_path: String,
    domain: String,
    target: String,
    enabled: bool,
    package_json_path: Option<PathBuf>,
    dev_script: Option<String>,
}

#[derive(Debug, Clone)]
struct WorkspaceAppCandidate {
    id: String,
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

/// Builds the service plan used by `localify init-project` when explicit flags
/// are provided instead of an interactive workspace flow.
pub fn build_init_plan(
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

/// Detects the default desktop init-project plan for a selected local folder.
///
/// The returned plan mirrors the CLI defaults: workspace apps get suggested
/// domains and free ports, and supported `package.json` dev scripts are marked
/// as editable.
pub fn detect_folder_init_plan(root: &Path, project_name: Option<&str>) -> Result<FolderInitPlan> {
    ensure_root_directory(root)?;
    let name = match project_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(name) => name.to_string(),
        None => current_directory_name(root)?,
    };
    let services = folder_service_blueprints(root, &name)?
        .into_iter()
        .map(|service| {
            let can_update_dev_script = service
                .dev_script
                .as_ref()
                .and_then(|script| plan_dev_script_update(script, &service.target))
                .is_some();

            FolderInitServicePlan {
                id: service.id,
                label: service.label,
                display_path: service.display_path,
                domain: service.domain,
                target: service.target,
                enabled: service.enabled,
                can_update_dev_script,
                update_dev_script: can_update_dev_script,
            }
        })
        .collect();

    Ok(FolderInitPlan { name, services })
}

/// Resolves a user-edited folder init plan into concrete Localify services and
/// optional `package.json` script rewrites.
pub fn resolve_folder_init_selection(
    root: &Path,
    project_name: &str,
    selections: &[FolderInitSelection],
) -> Result<ResolvedFolderInitSelection> {
    ensure_root_directory(root)?;
    let blueprints = folder_service_blueprints(root, project_name)?;
    let mut services = Vec::new();
    let mut package_updates = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut seen_domains = HashSet::new();

    for selection in selections {
        if !seen_ids.insert(selection.id.as_str()) {
            bail!(
                "Duplicate workspace service in init-project selection: {}",
                selection.id
            );
        }

        let Some(blueprint) = blueprints
            .iter()
            .find(|candidate| candidate.id == selection.id)
        else {
            bail!(
                "Unknown workspace service in init-project selection: {}",
                selection.id
            );
        };

        if !selection.enabled {
            continue;
        }

        if selection.domain.trim().is_empty() {
            bail!("Domain is required for '{}'", blueprint.display_path);
        }

        let domain = normalize_domain(selection.domain.clone());
        let target = selection.target.trim().to_string();

        if target.is_empty() {
            bail!("Target is required for '{}'", blueprint.display_path);
        }

        if !seen_domains.insert(domain.clone()) {
            bail!("Duplicate domain in init-project plan: {domain}");
        }

        services.push(ServicePlan {
            domain,
            target: target.clone(),
        });

        if !selection.update_dev_script {
            continue;
        }

        let update = build_package_update(blueprint, &target)?;
        if let Some(update) = update {
            package_updates.push(update);
        }
    }

    if services.is_empty() {
        bail!("Select at least one app to initialize");
    }

    Ok(ResolvedFolderInitSelection {
        services,
        package_updates,
    })
}

/// Applies planned `package.json` dev-script rewrites after a project has been
/// initialized so local apps start on the selected Localify ports.
pub fn apply_package_updates(updates: &[PackageScriptUpdate]) -> PackageUpdateResults {
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

fn ensure_root_directory(root: &Path) -> Result<()> {
    if root.is_dir() {
        return Ok(());
    }

    bail!("Selected folder does not exist: {}", root.display());
}

fn current_directory_name(current_dir: &Path) -> Result<String> {
    let name = current_dir
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Could not infer a project name from the selected folder"))?;

    Ok(name.to_string())
}

fn folder_service_blueprints(
    root: &Path,
    project_name: &str,
) -> Result<Vec<FolderServiceBlueprint>> {
    let workspace_apps = detect_workspace_apps(root)?;
    if workspace_apps.is_empty() {
        return synthetic_primary_service(project_name);
    }

    workspace_service_blueprints(project_name, &workspace_apps)
}

fn synthetic_primary_service(project_name: &str) -> Result<Vec<FolderServiceBlueprint>> {
    let slug = slugify(project_name)?;
    let target = reserve_auto_targets(1)?
        .into_iter()
        .next()
        .unwrap_or_else(|| "3000".into());

    Ok(vec![FolderServiceBlueprint {
        id: SYNTHETIC_PRIMARY_SERVICE_ID.into(),
        label: "Primary app".into(),
        display_path: "Primary app".into(),
        domain: normalize_domain(slug),
        target,
        enabled: true,
        package_json_path: None,
        dev_script: None,
    }])
}

fn workspace_service_blueprints(
    project_name: &str,
    workspace_apps: &[WorkspaceAppCandidate],
) -> Result<Vec<FolderServiceBlueprint>> {
    let slug = slugify(project_name)?;
    let default_targets = reserve_auto_targets(workspace_apps.len())?;
    let mut services = Vec::with_capacity(workspace_apps.len());

    for (index, app) in workspace_apps.iter().enumerate() {
        let enabled = if app.is_root && workspace_apps.len() > 1 {
            false
        } else {
            index == 0 || matches!(app.label.as_str(), "web" | "app")
        };
        let domain = default_workspace_domain(&slug, &app.label, services.is_empty(), app.is_root);
        let target = default_targets
            .get(index)
            .cloned()
            .unwrap_or_else(|| "3000".into());

        services.push(FolderServiceBlueprint {
            id: app.id.clone(),
            label: app.label.clone(),
            display_path: app.display_path.clone(),
            domain,
            target,
            enabled,
            package_json_path: Some(app.package_json_path.clone()),
            dev_script: Some(app.dev_script.clone()),
        });
    }

    Ok(services)
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
    let manifests = candidate_manifests(root)?;
    let root_manifest = root.join("package.json");
    let root_is_workspace =
        root_manifest.exists() && package_json_declares_workspaces(&root_manifest)?;
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

fn candidate_manifests(root: &Path) -> Result<Vec<PathBuf>> {
    let mut manifests = Vec::new();
    let root_manifest = root.join("package.json");
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

    Ok(manifests)
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
    let relative_manifest = package_json_path
        .strip_prefix(root)
        .unwrap_or(package_json_path)
        .display()
        .to_string();

    Ok(Some(WorkspaceAppCandidate {
        id: relative_manifest,
        label,
        display_path,
        package_json_path: package_json_path.to_path_buf(),
        dev_script: dev_script.to_string(),
        is_root: package_dir == root,
    }))
}

fn build_package_update(
    blueprint: &FolderServiceBlueprint,
    target: &str,
) -> Result<Option<PackageScriptUpdate>> {
    let Some(package_json_path) = blueprint.package_json_path.as_ref() else {
        bail!(
            "Cannot update the dev script for '{}': no package.json was detected",
            blueprint.display_path
        );
    };
    let Some(dev_script) = blueprint.dev_script.as_deref() else {
        bail!(
            "Cannot update the dev script for '{}': no dev script was detected",
            blueprint.display_path
        );
    };

    if !is_port_target(target) {
        return Ok(None);
    }

    let Some(script) = plan_dev_script_update(dev_script, target) else {
        bail!(
            "Cannot update the dev script for '{}': the current dev command is not supported",
            blueprint.display_path
        );
    };

    Ok(Some(PackageScriptUpdate {
        package_json_path: package_json_path.clone(),
        display_path: blueprint.display_path.clone(),
        script,
    }))
}

fn is_port_target(target: &str) -> bool {
    target.parse::<u16>().is_ok()
}

fn plan_dev_script_update(script: &str, port: &str) -> Option<String> {
    let trimmed = script.trim();
    if trimmed.contains("next dev") {
        let normalized = strip_script_flags(
            trimmed,
            &["--hostname", "--host", "-H", "--port", "-p"],
            &[],
        );
        let updated = normalized.replacen(
            "next dev",
            &format!("next dev --hostname 127.0.0.1 --port {port}"),
            1,
        );
        return Some(updated);
    }
    if trimmed == "vite" || trimmed.starts_with("vite ") {
        let normalized = strip_script_flags(trimmed, &["--host", "-H", "--port", "-p"], &[]);
        return Some(format!("{normalized} --host 127.0.0.1 --port {port}"));
    }
    if trimmed.contains("astro dev") {
        let normalized = strip_script_flags(trimmed, &["--host", "-H", "--port", "-p"], &[]);
        let updated = normalized.replacen(
            "astro dev",
            &format!("astro dev --host 127.0.0.1 --port {port}"),
            1,
        );
        return Some(updated);
    }
    if trimmed.contains("nuxt dev") {
        let normalized = strip_script_flags(trimmed, &["--host", "-H", "--port", "-p"], &[]);
        let updated = normalized.replacen(
            "nuxt dev",
            &format!("nuxt dev --host 127.0.0.1 --port {port}"),
            1,
        );
        return Some(updated);
    }
    if trimmed.contains("react-scripts start") {
        let normalized = strip_script_flags(trimmed, &[], &["HOST", "PORT"]);
        return Some(format!("HOST=127.0.0.1 PORT={port} {normalized}"));
    }

    None
}

fn strip_script_flags(script: &str, flags: &[&str], env_vars: &[&str]) -> String {
    let tokens = script.split_whitespace().collect::<Vec<_>>();
    let mut cleaned = Vec::with_capacity(tokens.len());
    let mut index = 0;

    while index < tokens.len() {
        let token = tokens[index];

        if env_vars.iter().any(|name| {
            token
                .strip_prefix(name)
                .is_some_and(|suffix| suffix.starts_with('='))
        }) {
            index += 1;
            continue;
        }

        if flags.contains(&token) {
            index += 2;
            continue;
        }

        if flags.iter().any(|flag| {
            token
                .strip_prefix(flag)
                .is_some_and(|suffix| suffix.starts_with('='))
        }) {
            index += 1;
            continue;
        }

        cleaned.push(token);
        index += 1;
    }

    cleaned.join(" ")
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
    fn detect_folder_init_plan_creates_single_primary_service_without_workspace_apps() {
        let temp_dir = tempdir().unwrap();

        let plan = detect_folder_init_plan(temp_dir.path(), Some("Demo App")).unwrap();

        assert_eq!(plan.services.len(), 1);
        assert_eq!(plan.services[0].id, SYNTHETIC_PRIMARY_SERVICE_ID);
        assert_eq!(
            plan.services[0].domain,
            format!("demo-app{}", BRANDING.domain_suffix)
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
    fn plan_dev_script_update_rewrites_existing_port_flags() {
        assert_eq!(
            plan_dev_script_update("next dev --port 3000", "4010").as_deref(),
            Some("next dev --hostname 127.0.0.1 --port 4010")
        );
        assert_eq!(
            plan_dev_script_update("vite dev --host 127.0.0.1 --port 55190", "61342").as_deref(),
            Some("vite dev --host 127.0.0.1 --port 61342")
        );
        assert_eq!(
            plan_dev_script_update("PORT=3000 react-scripts start", "4010").as_deref(),
            Some("HOST=127.0.0.1 PORT=4010 react-scripts start")
        );
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

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();
        let labels = plan
            .services
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

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();

        assert_eq!(plan.services.len(), 1);
        assert_eq!(
            plan.services[0].display_path,
            root.file_name().unwrap().to_string_lossy()
        );
        assert!(plan.services[0].can_update_dev_script);
    }

    #[test]
    fn apply_package_update_rewrites_scripts_dev() {
        let temp_dir = tempdir().unwrap();
        let package_json_path = temp_dir.path().join("package.json");
        write_package_json(&package_json_path, "vite");

        let results = apply_package_updates(&[PackageScriptUpdate {
            package_json_path: package_json_path.clone(),
            display_path: "app".into(),
            script: "vite --port 4010".into(),
        }]);

        assert_eq!(results.updated.len(), 1);
        let updated = fs::read_to_string(package_json_path).unwrap();
        assert!(updated.contains(r#""dev": "vite --port 4010""#));
    }

    #[test]
    fn resolve_folder_init_selection_builds_package_update_for_supported_app() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        write_package_json(&root.join("apps/web/package.json"), "next dev");

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();
        let selection = FolderInitSelection {
            id: plan.services[0].id.clone(),
            domain: plan.services[0].domain.clone(),
            target: "4010".into(),
            enabled: true,
            update_dev_script: true,
        };

        let resolved = resolve_folder_init_selection(root, "Demo", &[selection]).unwrap();

        assert_eq!(resolved.services.len(), 1);
        assert_eq!(resolved.package_updates.len(), 1);
        assert_eq!(
            resolved.package_updates[0].script,
            "next dev --hostname 127.0.0.1 --port 4010"
        );
    }
}
