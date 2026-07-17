use std::collections::HashSet;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::branding::BRANDING;
use crate::domain::DomainName;
use crate::infrastructure::local_dev::extract_script_port;

const SYNTHETIC_PRIMARY_SERVICE_ID: &str = "__primary__";

#[derive(Debug, Clone)]
pub struct ServicePlan {
    pub domain: String,
    pub target: String,
    pub service_name: Option<String>,
    pub service_path: Option<String>,
    pub command: Option<Vec<String>>,
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
    pub service_name: String,
    pub service_path: Option<String>,
    pub command: Option<Vec<String>>,
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
    pub service_name: Option<String>,
    pub service_path: Option<String>,
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct PackageScriptUpdate {
    pub package_json_path: PathBuf,
    pub display_path: String,
    pub script_name: String,
    pub script: String,
}

#[derive(Debug, Clone)]
pub struct NextConfigUpdate {
    pub next_config_path: PathBuf,
    pub display_path: String,
    pub allowed_dev_origin: String,
}

#[derive(Debug, Clone)]
pub struct ViteConfigUpdate {
    pub vite_config_path: PathBuf,
    pub display_path: String,
    pub allowed_host: String,
}

#[derive(Debug, Clone, Copy)]
pub struct LocalAppUpdatePolicy {
    pub write_package_scripts: bool,
    pub promote_dev_script: bool,
}

impl LocalAppUpdatePolicy {
    pub fn runtime_only() -> Self {
        Self {
            write_package_scripts: false,
            promote_dev_script: false,
        }
    }

    pub fn from_dev_script_preference(promote_dev_script: bool) -> Self {
        Self {
            write_package_scripts: promote_dev_script,
            promote_dev_script,
        }
    }

    pub fn with_package_scripts(self, write_package_scripts: bool) -> Self {
        Self {
            write_package_scripts,
            ..self
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedFolderInitSelection {
    pub services: Vec<ServicePlan>,
    pub package_updates: Vec<PackageScriptUpdate>,
    pub next_config_updates: Vec<NextConfigUpdate>,
    pub vite_config_updates: Vec<ViteConfigUpdate>,
}

#[derive(Debug, Clone)]
pub struct LocalAppUpdateResults {
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
struct FolderServiceBlueprint {
    id: String,
    label: String,
    display_path: String,
    service_name: String,
    service_path: Option<String>,
    package_command: Option<Vec<String>>,
    script_name: String,
    domain: String,
    target: String,
    enabled: bool,
    package_json_path: Option<PathBuf>,
    dev_script: Option<String>,
    scripts: Option<Map<String, Value>>,
    next_config_path: Option<PathBuf>,
    vite_config_path: Option<PathBuf>,
}

impl FolderServiceBlueprint {
    fn package_command_for(&self, update_dev_script: bool) -> Option<Vec<String>> {
        let mut command = self.package_command.clone()?;
        let script_name = if update_dev_script {
            "dev-localify"
        } else {
            &self.script_name
        };
        if command.len() == 2 && command[0] == "yarn" {
            command[1] = script_name.into();
        } else if command.len() >= 3 {
            command[2] = script_name.into();
        }
        Some(command)
    }
}

#[derive(Debug, Clone)]
struct WorkspaceAppCandidate {
    id: String,
    label: String,
    display_path: String,
    service_path: Option<String>,
    package_command: Vec<String>,
    script_name: String,
    package_json_path: PathBuf,
    dev_script: String,
    scripts: Map<String, Value>,
    is_root: bool,
    next_config_path: Option<PathBuf>,
    vite_config_path: Option<PathBuf>,
}

#[derive(Deserialize)]
struct PackageJsonManifest {
    scripts: Option<Map<String, Value>>,
    workspaces: Option<Value>,
    #[serde(rename = "packageManager")]
    package_manager: Option<String>,
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
    let target = match target {
        Some(target) => target,
        None => next_auto_target
            .next()
            .ok_or_else(|| anyhow!("Failed to reserve a target port for the primary service"))?,
    };
    let api_target = if with_api {
        Some(match api_target {
            Some(target) => target,
            None => next_auto_target
                .next()
                .ok_or_else(|| anyhow!("Failed to reserve a target port for the API service"))?,
        })
    } else {
        None
    };

    let mut seen_domains = HashSet::new();
    if !seen_domains.insert(domain.clone()) {
        bail!("Duplicate domain in init-project plan: {domain}");
    }

    let mut services = vec![ServicePlan {
        domain,
        target,
        service_name: None,
        service_path: None,
        command: None,
    }];
    if let (Some(domain), Some(target)) = (api_domain, api_target) {
        if !seen_domains.insert(domain.clone()) {
            bail!("Duplicate domain in init-project plan: {domain}");
        }
        services.push(ServicePlan {
            domain,
            target,
            service_name: None,
            service_path: None,
            command: None,
        });
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
                service_name: service.service_name,
                service_path: service.service_path,
                command: service.package_command,
                domain: service.domain,
                target: service.target,
                enabled: service.enabled,
                can_update_dev_script,
                update_dev_script: false,
            }
        })
        .collect();

    Ok(FolderInitPlan { name, services })
}

/// Resolves a user-edited folder init plan into concrete Localify services and
/// any local app file rewrites needed to run cleanly behind Localify.
pub fn resolve_folder_init_selection(
    root: &Path,
    project_name: &str,
    selections: &[FolderInitSelection],
    policy: LocalAppUpdatePolicy,
) -> Result<ResolvedFolderInitSelection> {
    ensure_root_directory(root)?;
    let blueprints = folder_service_blueprints(root, project_name)?;
    let mut services = Vec::new();
    let mut package_updates = Vec::new();
    let mut next_config_updates = Vec::new();
    let mut vite_config_updates = Vec::new();
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

        let package_update = if selection.update_dev_script {
            build_package_updates(blueprint, &target, &domain, policy)?
        } else {
            Vec::new()
        };

        if let Some(update) = build_next_config_update(blueprint, &domain) {
            next_config_updates.push(update);
        }
        if let Some(update) = build_vite_config_update(blueprint, &domain) {
            vite_config_updates.push(update);
        }
        package_updates.extend(package_update);

        let command = selection
            .command
            .clone()
            .or_else(|| blueprint.package_command_for(selection.update_dev_script));
        let service_name = selection
            .service_name
            .as_deref()
            .and_then(non_empty_string)
            .or_else(|| command.as_ref().map(|_| blueprint.service_name.clone()));

        services.push(ServicePlan {
            domain,
            target: target.clone(),
            service_name,
            service_path: selection
                .service_path
                .clone()
                .or_else(|| blueprint.service_path.clone()),
            command,
        });
    }

    if services.is_empty() {
        bail!("Select at least one app to initialize");
    }

    Ok(ResolvedFolderInitSelection {
        services,
        package_updates,
        next_config_updates,
        vite_config_updates,
    })
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Applies planned local app file rewrites after a project has been
/// initialized so the selected apps run cleanly behind Localify.
pub fn apply_local_app_updates(
    package_updates: &[PackageScriptUpdate],
    next_config_updates: &[NextConfigUpdate],
    vite_config_updates: &[ViteConfigUpdate],
) -> LocalAppUpdateResults {
    let mut results = LocalAppUpdateResults {
        updated: Vec::new(),
        skipped: Vec::new(),
    };

    for update in package_updates {
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

    for update in next_config_updates {
        match apply_next_config_update(update) {
            Ok(()) => results.updated.push(format!(
                "{} ({})",
                update.display_path,
                update.next_config_path.display()
            )),
            Err(error) => results.skipped.push(format!(
                "{} ({}) - {}",
                update.display_path,
                update.next_config_path.display(),
                error
            )),
        }
    }

    for update in vite_config_updates {
        match apply_vite_config_update(update) {
            Ok(()) => results.updated.push(format!(
                "{} ({})",
                update.display_path,
                update.vite_config_path.display()
            )),
            Err(error) => results.skipped.push(format!(
                "{} ({}) - {}",
                update.display_path,
                update.vite_config_path.display(),
                error
            )),
        }
    }

    dedup_preserve_order(&mut results.updated);
    dedup_preserve_order(&mut results.skipped);

    results
}

fn dedup_preserve_order(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

/// Plans local app file updates for `localify init-project` in the current
/// directory when the folder maps cleanly to a single detected app.
pub fn plan_current_dir_local_app_updates(
    root: &Path,
    project_name: &str,
    services: &[ServicePlan],
    policy: LocalAppUpdatePolicy,
) -> Result<(
    Vec<PackageScriptUpdate>,
    Vec<NextConfigUpdate>,
    Vec<ViteConfigUpdate>,
)> {
    if services.is_empty() {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }

    let blueprints = folder_service_blueprints(root, project_name)?;
    if blueprints.len() != 1 {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }

    let package_updates = build_package_updates(
        &blueprints[0],
        &services[0].target,
        &services[0].domain,
        policy,
    )?;
    let next_config_updates = build_next_config_update(&blueprints[0], &services[0].domain)
        .into_iter()
        .collect();
    let vite_config_updates = build_vite_config_update(&blueprints[0], &services[0].domain)
        .into_iter()
        .collect();

    Ok((package_updates, next_config_updates, vite_config_updates))
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
        service_name: "web".into(),
        service_path: None,
        package_command: None,
        script_name: "dev".into(),
        domain: normalize_domain(slug),
        target,
        enabled: true,
        package_json_path: None,
        dev_script: None,
        scripts: None,
        next_config_path: None,
        vite_config_path: None,
    }])
}

fn workspace_service_blueprints(
    project_name: &str,
    workspace_apps: &[WorkspaceAppCandidate],
) -> Result<Vec<FolderServiceBlueprint>> {
    let slug = slugify(project_name)?;
    let default_targets = reserve_auto_targets(workspace_apps.len())?;
    let mut services = Vec::with_capacity(workspace_apps.len());
    let has_app_service = workspace_apps.iter().any(is_apps_service);

    for (index, app) in workspace_apps.iter().enumerate() {
        let enabled =
            default_workspace_service_enabled(app, index, workspace_apps.len(), has_app_service);
        let domain = default_workspace_domain(&slug, &app.label, services.is_empty(), app.is_root);
        let target = default_targets
            .get(index)
            .cloned()
            .unwrap_or_else(|| "3000".into());
        services.push(FolderServiceBlueprint {
            id: app.id.clone(),
            label: app.label.clone(),
            display_path: app.display_path.clone(),
            service_name: default_service_name(app),
            service_path: app.service_path.clone(),
            package_command: Some(app.package_command.clone()),
            script_name: app.script_name.clone(),
            domain,
            target,
            enabled,
            package_json_path: Some(app.package_json_path.clone()),
            dev_script: Some(app.dev_script.clone()),
            scripts: Some(app.scripts.clone()),
            next_config_path: app.next_config_path.clone(),
            vite_config_path: app.vite_config_path.clone(),
        });
    }

    Ok(services)
}

fn default_workspace_service_enabled(
    app: &WorkspaceAppCandidate,
    index: usize,
    app_count: usize,
    has_app_service: bool,
) -> bool {
    if app.is_root && app_count > 1 {
        return false;
    }
    if has_app_service && !is_apps_service(app) {
        return false;
    }

    index == 0 || matches!(app.label.as_str(), "web" | "app" | "server" | "api")
}

fn is_apps_service(app: &WorkspaceAppCandidate) -> bool {
    app.service_path
        .as_deref()
        .is_some_and(|path| path.starts_with("apps/"))
}

fn default_service_name(app: &WorkspaceAppCandidate) -> String {
    if app.is_root {
        "web".into()
    } else {
        app.label.clone()
    }
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

    normalize_domain(format!("{label}.{project_slug}"))
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
    apps.sort_by(|left, right| {
        workspace_app_rank(left)
            .cmp(&workspace_app_rank(right))
            .then_with(|| left.display_path.cmp(&right.display_path))
    });

    Ok(apps)
}

fn workspace_app_rank(app: &WorkspaceAppCandidate) -> usize {
    match app.service_path.as_deref() {
        Some("apps/web") | Some("apps/app") => 0,
        Some("apps/server") | Some("apps/api") => 1,
        Some(path) if path.starts_with("apps/") => 2,
        Some(path) if path.starts_with("services/") => 3,
        Some(path) if path.starts_with("packages/") => 4,
        _ if app.is_root => 5,
        _ => 6,
    }
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

fn package_script_command(
    root: &Path,
    package_dir: &Path,
    manifest: &PackageJsonManifest,
    script_name: &str,
) -> Result<Vec<String>> {
    let package_manager = detect_package_manager(root, package_dir, manifest)?;
    Ok(match package_manager {
        PackageManager::Yarn => vec!["yarn".into(), script_name.into()],
        PackageManager::Npm => vec!["npm".into(), "run".into(), script_name.into()],
        PackageManager::Pnpm => vec!["pnpm".into(), "run".into(), script_name.into()],
        PackageManager::Bun => vec!["bun".into(), "run".into(), script_name.into()],
    })
}

#[derive(Debug, Clone, Copy)]
enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

fn detect_package_manager(
    root: &Path,
    package_dir: &Path,
    manifest: &PackageJsonManifest,
) -> Result<PackageManager> {
    if let Some(package_manager) = manifest
        .package_manager
        .as_deref()
        .and_then(parse_package_manager)
    {
        return Ok(package_manager);
    }

    let root_manifest_path = root.join("package.json");
    if package_dir != root && root_manifest_path.exists() {
        let content = fs::read_to_string(&root_manifest_path)
            .with_context(|| format!("Failed to read '{}'", root_manifest_path.display()))?;
        let root_manifest: PackageJsonManifest = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse '{}'", root_manifest_path.display()))?;
        if let Some(package_manager) = root_manifest
            .package_manager
            .as_deref()
            .and_then(parse_package_manager)
        {
            return Ok(package_manager);
        }
    }

    for directory in package_dir.ancestors() {
        if !directory.starts_with(root) {
            break;
        }

        if directory.join("bun.lockb").exists() || directory.join("bun.lock").exists() {
            return Ok(PackageManager::Bun);
        }
        if directory.join("pnpm-lock.yaml").exists() {
            return Ok(PackageManager::Pnpm);
        }
        if directory.join("yarn.lock").exists() {
            return Ok(PackageManager::Yarn);
        }
        if directory.join("package-lock.json").exists()
            || directory.join("npm-shrinkwrap.json").exists()
        {
            return Ok(PackageManager::Npm);
        }
    }

    Ok(PackageManager::Npm)
}

fn parse_package_manager(value: &str) -> Option<PackageManager> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized == "pnpm" || normalized.starts_with("pnpm@") {
        return Some(PackageManager::Pnpm);
    }
    if normalized == "yarn" || normalized.starts_with("yarn@") {
        return Some(PackageManager::Yarn);
    }
    if normalized == "bun" || normalized.starts_with("bun@") {
        return Some(PackageManager::Bun);
    }
    if normalized == "npm" || normalized.starts_with("npm@") {
        return Some(PackageManager::Npm);
    }
    None
}

fn read_workspace_app(
    root: &Path,
    package_json_path: &Path,
) -> Result<Option<WorkspaceAppCandidate>> {
    let content = fs::read_to_string(package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: PackageJsonManifest = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;
    let Some(scripts) = manifest.scripts.as_ref() else {
        return Ok(None);
    };
    let Some((script_name, dev_script)) = select_dev_script(scripts, package_json_path)? else {
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
        relative_dir.clone()
    };
    let service_path = if package_dir == root {
        None
    } else {
        Some(relative_dir)
    };
    let package_command = package_script_command(root, package_dir, &manifest, &script_name)?;
    let relative_manifest = package_json_path
        .strip_prefix(root)
        .unwrap_or(package_json_path)
        .display()
        .to_string();
    let next_config_path = resolve_next_config_path(package_dir, &dev_script);
    let vite_config_path = resolve_vite_config_path(package_dir, &dev_script);

    Ok(Some(WorkspaceAppCandidate {
        id: relative_manifest,
        label,
        display_path,
        service_path,
        package_command,
        script_name,
        package_json_path: package_json_path.to_path_buf(),
        dev_script,
        scripts: scripts.clone(),
        is_root: package_dir == root,
        next_config_path,
        vite_config_path,
    }))
}

fn select_dev_script(
    scripts: &Map<String, Value>,
    package_json_path: &Path,
) -> Result<Option<(String, String)>> {
    let package_label = package_json_path
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let mut candidates = scripts
        .iter()
        .filter_map(|(script_name, script_value)| {
            let script = script_value.as_str()?.trim();
            if script.is_empty() {
                return None;
            }
            Some((
                dev_script_priority(script_name, package_label)?,
                script_name.clone(),
                script.to_string(),
            ))
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(candidates
        .into_iter()
        .next()
        .map(|(_, script_name, script)| (script_name, script)))
}

fn dev_script_priority(script_name: &str, package_label: &str) -> Option<usize> {
    if script_name == "dev" {
        return Some(0);
    }
    if script_name == "dev:bare" {
        return Some(1);
    }
    if !package_label.is_empty() && script_name == format!("dev:{package_label}") {
        return Some(2);
    }
    if !package_label.is_empty() && script_name == format!("{package_label}:dev") {
        return Some(3);
    }
    if script_name.starts_with("dev:") {
        return Some(4);
    }
    if script_name.ends_with(":dev") {
        return Some(5);
    }
    None
}

/// Reads the checked-in `dev-localify` port for explicit CLI compatibility.
/// Normal project bootstrap intentionally uses fresh available ports instead.
pub fn detect_current_dir_localify_target(root: &Path) -> Result<Option<String>> {
    let package_json_path = root.join("package.json");
    if !package_json_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: PackageJsonManifest = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;

    Ok(manifest
        .scripts
        .as_ref()
        .and_then(|scripts| scripts.get("dev-localify"))
        .and_then(Value::as_str)
        .and_then(detect_localify_target))
}

fn build_package_updates(
    blueprint: &FolderServiceBlueprint,
    target: &str,
    domain: &str,
    policy: LocalAppUpdatePolicy,
) -> Result<Vec<PackageScriptUpdate>> {
    if !policy.write_package_scripts {
        return Ok(Vec::new());
    }

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
    let Some(scripts) = blueprint.scripts.as_ref() else {
        bail!(
            "Cannot update the dev script for '{}': no scripts object was detected",
            blueprint.display_path
        );
    };

    if !is_port_target(target) {
        return Ok(Vec::new());
    }

    let Some(script) = plan_dev_script_update(dev_script, target) else {
        bail!(
            "Cannot update the dev script for '{}': the current dev command is not supported",
            blueprint.display_path
        );
    };
    let script = prepend_localify_url_banner(script, domain)?;

    let mut updates = Vec::new();

    if policy.promote_dev_script && scripts.get("dev-localhost").is_none() {
        updates.push(PackageScriptUpdate {
            package_json_path: package_json_path.clone(),
            display_path: blueprint.display_path.clone(),
            script_name: "dev-localhost".into(),
            script: dev_script.to_string(),
        });
    }

    updates.push(PackageScriptUpdate {
        package_json_path: package_json_path.clone(),
        display_path: blueprint.display_path.clone(),
        script_name: "dev-localify".into(),
        script: script.clone(),
    });

    if policy.promote_dev_script && scripts.get("dev").is_some() {
        updates.push(PackageScriptUpdate {
            package_json_path: package_json_path.clone(),
            display_path: blueprint.display_path.clone(),
            script_name: "dev".into(),
            script,
        });
    }

    Ok(updates)
}

fn is_port_target(target: &str) -> bool {
    target.parse::<u16>().is_ok()
}

fn detect_localify_target(script: &str) -> Option<String> {
    extract_script_port(script).map(|port| port.to_string())
}

fn prepend_localify_url_banner(script: String, domain: &str) -> Result<String> {
    let domain = DomainName::new(domain).with_context(|| {
        format!(
            "Cannot update the dev script: '{}' is not a valid Localify domain",
            domain
        )
    })?;

    Ok(format!(
        "echo Localify URL: https://{} && {script}",
        domain.as_str()
    ))
}

fn build_next_config_update(
    blueprint: &FolderServiceBlueprint,
    domain: &str,
) -> Option<NextConfigUpdate> {
    blueprint
        .next_config_path
        .as_ref()
        .map(|next_config_path| NextConfigUpdate {
            next_config_path: next_config_path.clone(),
            display_path: blueprint.display_path.clone(),
            allowed_dev_origin: normalize_domain(domain.to_string()),
        })
}

fn build_vite_config_update(
    blueprint: &FolderServiceBlueprint,
    domain: &str,
) -> Option<ViteConfigUpdate> {
    blueprint
        .vite_config_path
        .as_ref()
        .map(|vite_config_path| ViteConfigUpdate {
            vite_config_path: vite_config_path.clone(),
            display_path: blueprint.display_path.clone(),
            allowed_host: normalize_domain(domain.to_string()),
        })
}

fn plan_dev_script_update(script: &str, port: &str) -> Option<String> {
    let trimmed = script.trim();
    if is_next_dev_script(trimmed) {
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
    if is_vite_dev_script(trimmed) {
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

fn is_next_dev_script(script: &str) -> bool {
    script.trim().contains("next dev")
}

fn is_vite_dev_script(script: &str) -> bool {
    let trimmed = script.trim();
    trimmed == "vite" || trimmed.starts_with("vite ") || trimmed.contains(" vite ")
}

/// Finds an existing Next.js config without creating one.
///
/// Runtime commands use this to repair framework allowlists only when a
/// project has already opted into a checked-in Next.js config.
pub fn find_existing_next_config_path(package_dir: &Path) -> Option<PathBuf> {
    for file_name in [
        "next.config.ts",
        "next.config.mts",
        "next.config.js",
        "next.config.mjs",
        "next.config.cjs",
        "next.config.cts",
    ] {
        let candidate = package_dir.join(file_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

fn resolve_next_config_path(package_dir: &Path, dev_script: &str) -> Option<PathBuf> {
    if !is_next_dev_script(dev_script) {
        return None;
    }

    find_existing_next_config_path(package_dir)
}

/// Finds an existing Vite config without creating one.
///
/// Runtime commands use this to repair framework allowlists only when a
/// project has already opted into a checked-in Vite config.
pub fn find_existing_vite_config_path(package_dir: &Path) -> Option<PathBuf> {
    for file_name in [
        "vite.config.ts",
        "vite.config.mts",
        "vite.config.js",
        "vite.config.mjs",
        "vite.config.cjs",
        "vite.config.cts",
    ] {
        let candidate = package_dir.join(file_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

fn resolve_vite_config_path(package_dir: &Path, dev_script: &str) -> Option<PathBuf> {
    if !is_vite_dev_script(dev_script) {
        return None;
    }

    find_existing_vite_config_path(package_dir)
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
    let updated = update_package_script_content(&content, &update.script_name, &update.script)?;

    if updated == content {
        return Ok(());
    }

    fs::write(&update.package_json_path, updated)
        .with_context(|| format!("Failed to write '{}'", update.package_json_path.display()))?;

    Ok(())
}

fn update_package_script_content(content: &str, script_name: &str, script: &str) -> Result<String> {
    let Some((object_open, object_close)) = find_json_property_object_bounds(content, "scripts")
    else {
        bail!("Missing scripts object");
    };
    let object_body = &content[object_open + 1..object_close];

    if let Some(property_offset) = object_body.find(&format!("\"{script_name}\"")) {
        let property_start = object_open + 1 + property_offset;
        return update_existing_package_script(
            content,
            object_close,
            property_start,
            script_name,
            script,
        );
    }

    Ok(insert_package_script_property(
        content,
        object_open,
        object_close,
        script_name,
        script,
    ))
}

fn find_json_property_object_bounds(content: &str, property_name: &str) -> Option<(usize, usize)> {
    let property = format!("\"{property_name}\"");
    let property_start = content.find(&property)?;
    let tail = &content[property_start + property.len()..];
    let colon_offset = tail.find(':')?;
    let object_offset = tail[colon_offset + 1..].find('{')?;
    let object_open = property_start + property.len() + colon_offset + 1 + object_offset;
    let object_close = find_matching_delimiter(content, object_open, b'{', b'}')?;

    Some((object_open, object_close))
}

fn update_existing_package_script(
    content: &str,
    object_close: usize,
    property_start: usize,
    script_name: &str,
    script: &str,
) -> Result<String> {
    let property_body = &content[property_start..object_close];
    let property = format!("\"{script_name}\"");
    let after_property = &property_body[property.len()..];
    let colon_offset = after_property
        .find(':')
        .ok_or_else(|| anyhow!("Found script '{script_name}' but could not locate its value"))?;
    let value_start = property_start + property.len() + colon_offset + 1;
    let quote_start = content[value_start..]
        .find('"')
        .map(|offset| value_start + offset)
        .ok_or_else(|| anyhow!("Found script '{script_name}' but could not parse its string"))?;
    let quote_end = find_json_string_end(content, quote_start)
        .ok_or_else(|| anyhow!("Found script '{script_name}' but could not parse its string"))?;
    let serialized_value =
        serde_json::to_string(script).context("Failed to serialize the updated script command")?;

    if content[quote_start..=quote_end] == serialized_value {
        return Ok(content.to_string());
    }

    Ok(format!(
        "{}{}{}",
        &content[..quote_start],
        serialized_value,
        &content[quote_end + 1..]
    ))
}

fn insert_package_script_property(
    content: &str,
    object_open: usize,
    object_close: usize,
    script_name: &str,
    script: &str,
) -> String {
    let inner = &content[object_open + 1..object_close];
    let trimmed = inner.trim_end_matches(char::is_whitespace);
    let trailing = &inner[trimmed.len()..];
    let closing_indent = line_indent_at(content, object_close);
    let property_indent = format!("{closing_indent}  ");
    let serialized_key =
        serde_json::to_string(script_name).unwrap_or_else(|_| format!("\"{script_name}\""));
    let serialized_value =
        serde_json::to_string(script).unwrap_or_else(|_| format!("\"{script}\""));

    let inserted = if trimmed.trim().is_empty() {
        format!("\n{property_indent}{serialized_key}: {serialized_value}\n{closing_indent}")
    } else {
        let mut updated_inner = String::from(trimmed);
        if !trimmed.ends_with(',') {
            updated_inner.push(',');
        }
        updated_inner.push('\n');
        updated_inner.push_str(&property_indent);
        updated_inner.push_str(&format!("{serialized_key}: {serialized_value}"));
        updated_inner.push_str(trailing);
        updated_inner
    };

    format!(
        "{}{}{}",
        &content[..object_open + 1],
        inserted,
        &content[object_close..]
    )
}

fn find_json_string_end(content: &str, quote_start: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut index = quote_start + 1;

    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }

        if bytes[index] == b'"' {
            return Some(index);
        }

        index += 1;
    }

    None
}

fn apply_next_config_update(update: &NextConfigUpdate) -> Result<()> {
    let content = fs::read_to_string(&update.next_config_path)
        .with_context(|| format!("Failed to read '{}'", update.next_config_path.display()))?;
    let updated = update_next_config_content(&content, &update.allowed_dev_origin)?;

    if updated == content {
        return Ok(());
    }

    fs::write(&update.next_config_path, updated)
        .with_context(|| format!("Failed to write '{}'", update.next_config_path.display()))?;

    Ok(())
}

/// Adds a Localify host to Next.js `allowedDevOrigins` without touching images.
///
/// Full project bootstrap also updates `images.remotePatterns`; `localify run`
/// only needs to satisfy the Next.js dev-server origin check for the current
/// ephemeral worktree domain.
pub fn apply_next_allowed_dev_origin(
    next_config_path: &Path,
    allowed_dev_origin: &str,
) -> Result<bool> {
    let content = fs::read_to_string(next_config_path)
        .with_context(|| format!("Failed to read '{}'", next_config_path.display()))?;
    let origin = normalize_domain(allowed_dev_origin.to_string());
    let updated = update_next_allowed_dev_origins_content(&content, &origin)?;

    if updated == content {
        return Ok(false);
    }

    fs::write(next_config_path, updated)
        .with_context(|| format!("Failed to write '{}'", next_config_path.display()))?;

    Ok(true)
}

fn apply_vite_config_update(update: &ViteConfigUpdate) -> Result<()> {
    let content = fs::read_to_string(&update.vite_config_path)
        .with_context(|| format!("Failed to read '{}'", update.vite_config_path.display()))?;
    let updated = update_vite_config_content(&content, &update.allowed_host)?;

    if updated == content {
        return Ok(());
    }

    fs::write(&update.vite_config_path, updated)
        .with_context(|| format!("Failed to write '{}'", update.vite_config_path.display()))?;

    Ok(())
}

/// Adds a Localify host to Vite `server.allowedHosts`.
///
/// Localify keeps Vite strict by adding the concrete worktree host instead of
/// using `allowedHosts: true`, which would disable Vite's host allowlist.
pub fn apply_vite_allowed_host(vite_config_path: &Path, allowed_host: &str) -> Result<bool> {
    let content = fs::read_to_string(vite_config_path)
        .with_context(|| format!("Failed to read '{}'", vite_config_path.display()))?;
    let host = normalize_domain(allowed_host.to_string());
    let updated = update_vite_config_content(&content, &host)?;

    if updated == content {
        return Ok(false);
    }

    fs::write(vite_config_path, updated)
        .with_context(|| format!("Failed to write '{}'", vite_config_path.display()))?;

    Ok(true)
}

fn update_vite_config_content(content: &str, host: &str) -> Result<String> {
    let Some((object_open, object_close)) = find_vite_config_object_bounds(content) else {
        bail!("Could not find a supported Vite config object");
    };
    if let Some(property_start) = find_object_property(content, object_open, object_close, "server")
    {
        return update_existing_vite_server(content, object_close, property_start, host);
    }

    Ok(insert_object_property(
        content,
        object_open,
        object_close,
        &format!("server: {{ allowedHosts: ['{host}'] }},"),
    ))
}

fn find_vite_config_object_bounds(content: &str) -> Option<(usize, usize)> {
    let mut search_start = 0;
    while let Some(paren_open) =
        find_identifier_call_open(content, "defineConfig", search_start, content.len())
    {
        if let Some(paren_close) = find_matching_delimiter(content, paren_open, b'(', b')')
            && let Some(bounds) =
                find_vite_config_object_in_define_config_args(content, paren_open + 1, paren_close)
        {
            return Some(bounds);
        }
        search_start = paren_open + 1;
    }

    if let Some(anchor_start) = content.find("export default") {
        let value_start = anchor_start + "export default".len();
        if let Some(open) = next_non_whitespace_index(content, value_start, content.len())
            && content.as_bytes()[open] == b'{'
            && let Some(close) = find_matching_delimiter(content, open, b'{', b'}')
        {
            return Some((open, close));
        }
    }

    None
}

fn find_identifier_call_open(
    content: &str,
    identifier: &str,
    start: usize,
    end: usize,
) -> Option<usize> {
    let bytes = content.as_bytes();
    let identifier_bytes = identifier.as_bytes();
    let mut index = start;

    while index < end {
        if let Some(next_index) = skip_comment_or_string(content, index, end) {
            index = next_index;
            continue;
        }

        let identifier_end = index + identifier_bytes.len();
        if identifier_end <= end
            && bytes[index..identifier_end] == *identifier_bytes
            && bytes
                .get(index.wrapping_sub(1))
                .is_none_or(|byte| !is_identifier_byte(*byte))
            && bytes
                .get(identifier_end)
                .is_none_or(|byte| !is_identifier_byte(*byte))
            && let Some(paren_open) = next_non_whitespace_index(content, identifier_end, end)
            && bytes[paren_open] == b'('
        {
            return Some(paren_open);
        }

        index += 1;
    }

    None
}

fn find_vite_config_object_in_define_config_args(
    content: &str,
    args_start: usize,
    args_end: usize,
) -> Option<(usize, usize)> {
    if let Some(return_start) = find_keyword_in_range(content, "return", args_start, args_end)
        && let Some(bounds) =
            find_first_object_after(content, return_start + "return".len(), args_end)
    {
        return Some(bounds);
    }

    if let Some(arrow_start) = content[args_start..args_end].find("=>")
        && let Some(bounds) =
            find_first_object_after(content, args_start + arrow_start + 2, args_end)
    {
        return Some(bounds);
    }

    find_first_object_after(content, args_start, args_end)
}

fn find_first_object_after(content: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let open_offset = content[start..end].find('{')?;
    let open = start + open_offset;
    let close = find_matching_delimiter(content, open, b'{', b'}')?;
    if close <= end {
        Some((open, close))
    } else {
        None
    }
}

fn find_keyword_in_range(content: &str, keyword: &str, start: usize, end: usize) -> Option<usize> {
    let mut offset = 0;
    let haystack = &content[start..end];
    while let Some(found) = haystack[offset..].find(keyword) {
        let absolute = start + offset + found;
        let before = content[..absolute].chars().next_back();
        let after = content[absolute + keyword.len()..].chars().next();
        if before.is_none_or(|ch| !is_identifier_char(ch))
            && after.is_none_or(|ch| !is_identifier_char(ch))
        {
            return Some(absolute);
        }
        offset += found + keyword.len();
    }
    None
}

fn find_object_property(
    content: &str,
    object_open: usize,
    object_close: usize,
    property: &str,
) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut depth = 0usize;
    let mut index = object_open + 1;

    while index < object_close {
        let byte = bytes[index];

        if depth == 0 {
            if quoted_property_key_end(content, index, object_close, property).is_some() {
                return Some(index);
            }
            if matches_identifier_property(content, index, object_close, property) {
                return Some(index);
            }
        }

        if let Some(next_index) = skip_comment_or_string(content, index, object_close) {
            index = next_index;
        } else if matches!(byte, b'{' | b'[' | b'(') {
            depth += 1;
            index += 1;
        } else if matches!(byte, b'}' | b']' | b')') {
            depth = depth.checked_sub(1)?;
            index += 1;
        } else {
            index += 1;
        }
    }

    None
}

fn skip_comment_or_string(content: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let byte = *bytes.get(start)?;

    if byte == b'/' && bytes.get(start + 1) == Some(&b'/') {
        return Some(
            content[start + 2..end]
                .find('\n')
                .map_or(end, |newline_offset| start + 2 + newline_offset + 1),
        );
    }

    if byte == b'/' && bytes.get(start + 1) == Some(&b'*') {
        let close_offset = content[start + 2..end].find("*/")?;
        return Some(start + 2 + close_offset + 2);
    }

    if matches!(byte, b'\'' | b'"' | b'`') {
        return Some(find_string_end(content, start)? + 1);
    }

    None
}

fn quoted_property_key_end(
    content: &str,
    start: usize,
    end: usize,
    property: &str,
) -> Option<usize> {
    if !matches!(content.as_bytes().get(start), Some(b'\'' | b'"' | b'`')) {
        return None;
    }

    let string_end = find_string_end(content, start)?;
    if content[start + 1..string_end] == *property
        && next_non_whitespace_is(content, string_end + 1, end, b':')
    {
        Some(string_end)
    } else {
        None
    }
}

fn find_string_end(content: &str, quote_start: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let quote = *bytes.get(quote_start)?;
    let mut index = quote_start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
        } else if bytes[index] == quote {
            return Some(index);
        } else {
            index += 1;
        }
    }
    None
}

fn matches_identifier_property(content: &str, start: usize, end: usize, property: &str) -> bool {
    if !content[start..end].starts_with(property) {
        return false;
    }

    let before = content[..start].chars().next_back();
    let after_index = start + property.len();
    let after = content[after_index..].chars().next();
    before.is_none_or(|ch| !is_identifier_char(ch))
        && after.is_none_or(|ch| !is_identifier_char(ch))
        && next_non_whitespace_is(content, after_index, end, b':')
}

fn next_non_whitespace_is(content: &str, start: usize, end: usize, expected: u8) -> bool {
    next_non_whitespace_index(content, start, end)
        .is_some_and(|index| content.as_bytes()[index] == expected)
}

fn next_non_whitespace_index(content: &str, start: usize, end: usize) -> Option<usize> {
    content.as_bytes()[start..end]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map(|offset| start + offset)
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}

fn is_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte == b'$' || byte.is_ascii_alphanumeric()
}

fn update_existing_vite_server(
    content: &str,
    object_close: usize,
    property_start: usize,
    host: &str,
) -> Result<String> {
    let property_body = &content[property_start..object_close];
    let Some(server_object_offset) = property_body.find('{') else {
        bail!("Found server config but could not locate its object value");
    };
    let server_object_open = property_start + server_object_offset;
    let Some(server_object_close) =
        find_matching_delimiter(content, server_object_open, b'{', b'}')
    else {
        bail!("Found server config but could not parse its object value");
    };

    if server_object_close > object_close {
        bail!("Found server config outside the Vite config object");
    }

    if let Some(property_start) = find_object_property(
        content,
        server_object_open,
        server_object_close,
        "allowedHosts",
    ) {
        return update_existing_allowed_hosts(content, server_object_close, property_start, host);
    }

    Ok(insert_object_property(
        content,
        server_object_open,
        server_object_close,
        &format!("allowedHosts: ['{host}'],"),
    ))
}

fn update_existing_allowed_hosts(
    content: &str,
    server_object_close: usize,
    property_start: usize,
    host: &str,
) -> Result<String> {
    let property_body = &content[property_start..server_object_close];
    let Some(array_offset) = property_body.find('[') else {
        bail!("Found server.allowedHosts but could not locate its array value");
    };
    let array_open = property_start + array_offset;
    let Some(array_close) = find_matching_delimiter(content, array_open, b'[', b']') else {
        bail!("Found server.allowedHosts but could not parse its array value");
    };

    if array_close > server_object_close {
        bail!("Found server.allowedHosts outside the server config object");
    }

    let inner = &content[array_open + 1..array_close];
    if array_contains_origin(inner, host) {
        return Ok(content.to_string());
    }

    let replacement = if inner.contains('\n') {
        let trimmed = inner.trim_end_matches(char::is_whitespace);
        let trailing = &inner[trimmed.len()..];
        let closing_indent = line_indent_at(content, array_close);
        let item_indent = format!("{closing_indent}  ");
        let mut updated_inner = String::from(trimmed);
        if !trimmed.trim().is_empty() && !trimmed.ends_with(',') {
            updated_inner.push(',');
        }
        if !trimmed.trim().is_empty() {
            updated_inner.push('\n');
        }
        updated_inner.push_str(&item_indent);
        updated_inner.push_str(&format!("'{host}',"));
        updated_inner.push_str(trailing);
        format!("[{updated_inner}]")
    } else {
        let trimmed = inner.trim();
        if trimmed.is_empty() {
            format!("['{host}']")
        } else if trimmed.ends_with(',') {
            format!("[{trimmed} '{host}']")
        } else {
            format!("[{trimmed}, '{host}']")
        }
    };

    Ok(format!(
        "{}{}{}",
        &content[..array_open],
        replacement,
        &content[array_close + 1..]
    ))
}
fn update_next_config_content(content: &str, origin: &str) -> Result<String> {
    let with_origin = update_next_allowed_dev_origins_content(content, origin)?;

    let Some((updated_object_open, updated_object_close)) =
        find_next_config_object_bounds(&with_origin)
    else {
        bail!("Could not find a supported Next.js config object after update");
    };

    update_next_images_remote_patterns(
        &with_origin,
        updated_object_open,
        updated_object_close,
        origin,
    )
}

fn update_next_allowed_dev_origins_content(content: &str, origin: &str) -> Result<String> {
    let Some((object_open, object_close)) = find_next_config_object_bounds(content) else {
        bail!("Could not find a supported Next.js config object");
    };

    let object_body = &content[object_open + 1..object_close];
    if let Some(property_offset) = object_body.find("allowedDevOrigins") {
        let property_start = object_open + 1 + property_offset;
        update_existing_allowed_dev_origins(
            content,
            object_open,
            object_close,
            property_start,
            origin,
        )
    } else {
        Ok(insert_allowed_dev_origins_property(
            content,
            object_open,
            object_close,
            origin,
        ))
    }
}

fn update_next_images_remote_patterns(
    content: &str,
    object_open: usize,
    object_close: usize,
    origin: &str,
) -> Result<String> {
    let object_body = &content[object_open + 1..object_close];
    if let Some(property_offset) = object_body.find("images") {
        let property_start = object_open + 1 + property_offset;
        return update_existing_images_config(
            content,
            object_open,
            object_close,
            property_start,
            origin,
        );
    }

    Ok(insert_images_property(
        content,
        object_open,
        object_close,
        origin,
    ))
}

fn find_next_config_object_bounds(content: &str) -> Option<(usize, usize)> {
    let mut matches = Vec::new();

    for anchor in ["const nextConfig", "let nextConfig", "var nextConfig"] {
        if let Some(anchor_start) = content.find(anchor) {
            let tail = &content[anchor_start + anchor.len()..];
            if let Some(equals_offset) = tail.find('=')
                && let Some(open_offset) = tail[equals_offset + 1..].find('{')
            {
                let open = anchor_start + anchor.len() + equals_offset + 1 + open_offset;
                if let Some(close) = find_matching_delimiter(content, open, b'{', b'}') {
                    matches.push((open, close));
                }
            }
        }
    }

    if let Some(anchor_start) = content.find("module.exports") {
        let tail = &content[anchor_start + "module.exports".len()..];
        if let Some(equals_offset) = tail.find('=')
            && let Some(open_offset) = tail[equals_offset + 1..].find('{')
        {
            let open = anchor_start + "module.exports".len() + equals_offset + 1 + open_offset;
            if let Some(close) = find_matching_delimiter(content, open, b'{', b'}') {
                matches.push((open, close));
            }
        }
    }

    if let Some(anchor_start) = content.find("export default") {
        let tail = &content[anchor_start + "export default".len()..];
        if let Some(open_offset) = tail.find('{') {
            let open = anchor_start + "export default".len() + open_offset;
            if let Some(close) = find_matching_delimiter(content, open, b'{', b'}') {
                matches.push((open, close));
            }
        }
    }

    matches.into_iter().min_by_key(|(open, _)| *open)
}

fn update_existing_allowed_dev_origins(
    content: &str,
    _object_open: usize,
    object_close: usize,
    property_start: usize,
    origin: &str,
) -> Result<String> {
    let property_body = &content[property_start..object_close];
    let Some(array_offset) = property_body.find('[') else {
        bail!("Found allowedDevOrigins but could not locate its array value");
    };
    let array_open = property_start + array_offset;
    let Some(array_close) = find_matching_delimiter(content, array_open, b'[', b']') else {
        bail!("Found allowedDevOrigins but could not parse its array value");
    };

    if array_close > object_close {
        bail!("Found allowedDevOrigins outside the Next.js config object");
    }

    let inner = &content[array_open + 1..array_close];
    if array_contains_origin(inner, origin) {
        return Ok(content.to_string());
    }

    let replacement = if inner.contains('\n') {
        let trimmed = inner.trim_end_matches(char::is_whitespace);
        let trailing = &inner[trimmed.len()..];
        let closing_indent = line_indent_at(content, array_close);
        let item_indent = format!("{closing_indent}  ");
        let mut updated_inner = String::from(trimmed);
        if !trimmed.trim().is_empty() && !trimmed.ends_with(',') {
            updated_inner.push(',');
        }
        if !trimmed.trim().is_empty() {
            updated_inner.push('\n');
        }
        updated_inner.push_str(&item_indent);
        updated_inner.push_str(&format!("'{origin}',"));
        updated_inner.push_str(trailing);
        format!("[{updated_inner}]")
    } else {
        let trimmed = inner.trim();
        if trimmed.is_empty() {
            format!("['{origin}']")
        } else if trimmed.ends_with(',') {
            format!("[{trimmed} '{origin}']")
        } else {
            format!("[{trimmed}, '{origin}']")
        }
    };

    Ok(format!(
        "{}{}{}",
        &content[..array_open],
        replacement,
        &content[array_close + 1..]
    ))
}

fn insert_allowed_dev_origins_property(
    content: &str,
    object_open: usize,
    object_close: usize,
    origin: &str,
) -> String {
    let inner = &content[object_open + 1..object_close];
    let trimmed = inner.trim_end_matches(char::is_whitespace);
    let trailing = &inner[trimmed.len()..];
    let closing_indent = line_indent_at(content, object_close);
    let property_indent = format!("{closing_indent}  ");

    let inserted = if trimmed.trim().is_empty() {
        format!("\n{property_indent}allowedDevOrigins: ['{origin}'],\n{closing_indent}")
    } else {
        let mut updated_inner = String::from(trimmed);
        if !trimmed.ends_with(',') {
            updated_inner.push(',');
        }
        updated_inner.push('\n');
        updated_inner.push_str(&property_indent);
        updated_inner.push_str(&format!("allowedDevOrigins: ['{origin}'],"));
        updated_inner.push_str(trailing);
        updated_inner
    };

    format!(
        "{}{}{}",
        &content[..object_open + 1],
        inserted,
        &content[object_close..]
    )
}

fn update_existing_images_config(
    content: &str,
    _object_open: usize,
    object_close: usize,
    property_start: usize,
    origin: &str,
) -> Result<String> {
    let property_body = &content[property_start..object_close];
    let Some(images_object_offset) = property_body.find('{') else {
        bail!("Found images config but could not locate its object value");
    };
    let images_object_open = property_start + images_object_offset;
    let Some(images_object_close) =
        find_matching_delimiter(content, images_object_open, b'{', b'}')
    else {
        bail!("Found images config but could not parse its object value");
    };

    if images_object_close > object_close {
        bail!("Found images config outside the Next.js config object");
    }

    let images_body = &content[images_object_open + 1..images_object_close];
    if let Some(remote_patterns_offset) = images_body.find("remotePatterns") {
        let property_start = images_object_open + 1 + remote_patterns_offset;
        return update_existing_remote_patterns(
            content,
            images_object_close,
            property_start,
            origin,
        );
    }

    Ok(insert_remote_patterns_property(
        content,
        images_object_open,
        images_object_close,
        origin,
    ))
}

fn update_existing_remote_patterns(
    content: &str,
    images_object_close: usize,
    property_start: usize,
    origin: &str,
) -> Result<String> {
    let property_body = &content[property_start..images_object_close];
    let Some(array_offset) = property_body.find('[') else {
        bail!("Found images.remotePatterns but could not locate its array value");
    };
    let array_open = property_start + array_offset;
    let Some(array_close) = find_matching_delimiter(content, array_open, b'[', b']') else {
        bail!("Found images.remotePatterns but could not parse its array value");
    };

    if array_close > images_object_close {
        bail!("Found images.remotePatterns outside the images config object");
    }

    let inner = &content[array_open + 1..array_close];
    if remote_patterns_contains_origin(inner, origin) {
        return Ok(content.to_string());
    }

    let pattern = remote_pattern_entry(origin);
    let replacement = if inner.contains('\n') {
        let trimmed = inner.trim_end_matches(char::is_whitespace);
        let trailing = &inner[trimmed.len()..];
        let closing_indent = line_indent_at(content, array_close);
        let item_indent = format!("{closing_indent}  ");
        let mut updated_inner = String::from(trimmed);
        if !trimmed.trim().is_empty() && !trimmed.ends_with(',') {
            updated_inner.push(',');
        }
        if !trimmed.trim().is_empty() {
            updated_inner.push('\n');
        }
        updated_inner.push_str(&item_indent);
        updated_inner.push_str(&pattern);
        updated_inner.push(',');
        updated_inner.push_str(trailing);
        format!("[{updated_inner}]")
    } else {
        let trimmed = inner.trim();
        if trimmed.is_empty() {
            format!("[{pattern}]")
        } else if trimmed.ends_with(',') {
            format!("[{trimmed} {pattern}]")
        } else {
            format!("[{trimmed}, {pattern}]")
        }
    };

    Ok(format!(
        "{}{}{}",
        &content[..array_open],
        replacement,
        &content[array_close + 1..]
    ))
}

fn insert_remote_patterns_property(
    content: &str,
    object_open: usize,
    object_close: usize,
    origin: &str,
) -> String {
    insert_object_property(
        content,
        object_open,
        object_close,
        &format!("remotePatterns: [{}],", remote_pattern_entry(origin)),
    )
}

fn insert_images_property(
    content: &str,
    object_open: usize,
    object_close: usize,
    origin: &str,
) -> String {
    let property_indent = format!("{}  ", line_indent_at(content, object_close));
    let nested_indent = format!("{property_indent}  ");

    insert_object_property(
        content,
        object_open,
        object_close,
        &format!(
            "images: {{\n{}remotePatterns: [{}],\n{}}},",
            nested_indent,
            remote_pattern_entry(origin),
            property_indent
        ),
    )
}

fn insert_object_property(
    content: &str,
    object_open: usize,
    object_close: usize,
    property: &str,
) -> String {
    let inner = &content[object_open + 1..object_close];
    let trimmed = inner.trim_end_matches(char::is_whitespace);
    let trailing = &inner[trimmed.len()..];
    let closing_indent = line_indent_at(content, object_close);
    let property_indent = format!("{closing_indent}  ");

    let inserted = if trimmed.trim().is_empty() {
        format!("\n{property_indent}{property}\n{closing_indent}")
    } else {
        let mut updated_inner = String::from(trimmed);
        if !trimmed.ends_with(',') {
            updated_inner.push(',');
        }
        updated_inner.push('\n');
        updated_inner.push_str(&property_indent);
        updated_inner.push_str(property);
        updated_inner.push_str(trailing);
        updated_inner
    };

    format!(
        "{}{}{}",
        &content[..object_open + 1],
        inserted,
        &content[object_close..]
    )
}

fn remote_pattern_entry(origin: &str) -> String {
    format!("{{ protocol: 'https', hostname: '{origin}', pathname: '/**' }}")
}

fn remote_patterns_contains_origin(remote_patterns_content: &str, origin: &str) -> bool {
    remote_patterns_content.contains(&format!("hostname: '{origin}'"))
        || remote_patterns_content.contains(&format!("hostname: \"{origin}\""))
        || remote_patterns_content.contains(&format!("\"hostname\": \"{origin}\""))
        || remote_patterns_content.contains(&format!("'hostname': '{origin}'"))
}

fn array_contains_origin(array_content: &str, origin: &str) -> bool {
    array_content.contains(&format!("'{origin}'"))
        || array_content.contains(&format!("\"{origin}\""))
}

fn line_indent_at(content: &str, index: usize) -> String {
    let line_start = content[..index].rfind('\n').map_or(0, |value| value + 1);
    content[line_start..index]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect()
}

fn find_matching_delimiter(content: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut depth = 0usize;
    let mut index = start;
    let mut quote = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while index < bytes.len() {
        let byte = bytes[index];

        if in_line_comment {
            if byte == b'\n' {
                in_line_comment = false;
            }
            index += 1;
            continue;
        }

        if in_block_comment {
            if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                in_block_comment = false;
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }

        if let Some(active_quote) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }

        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            in_line_comment = true;
            index += 2;
            continue;
        }

        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            in_block_comment = true;
            index += 2;
            continue;
        }

        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            index += 1;
            continue;
        }

        if byte == open {
            depth += 1;
        } else if byte == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }

        index += 1;
    }

    None
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
    if trimmed.ends_with(BRANDING.domain_suffix())
        || trimmed.ends_with(BRANDING.domain_suffix)
        || trimmed.ends_with(BRANDING.dev_domain_suffix)
    {
        return trimmed.to_string();
    }

    format!("{trimmed}{}", BRANDING.domain_suffix())
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

    fn write_next_config(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    fn write_package_json_with_localify_script(
        path: &Path,
        dev_script: &str,
        localify_script: &str,
    ) {
        let package_json = format!(
            r#"{{
  "name": "test-app",
  "scripts": {{
    "dev": "{dev_script}",
    "dev-localify": "{localify_script}"
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
            format!("test-1{}", BRANDING.domain_suffix())
        );
        assert!(!plan.services[0].target.is_empty());
    }

    #[test]
    fn build_init_plan_adds_default_api_domain() {
        let plan = build_init_plan("Test 1", None, None, true, None, None, false).unwrap();

        assert_eq!(plan.services.len(), 2);
        assert_eq!(
            plan.services[1].domain,
            format!("api-test-1{}", BRANDING.domain_suffix())
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
            format!("demo-app{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn detect_folder_init_plan_ignores_existing_dev_localify_port() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        write_package_json_with_localify_script(
            &root.join("package.json"),
            "vite",
            "echo Localify URL: https://demo.localify && vite --host 127.0.0.1 --port 4217",
        );

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();

        assert_eq!(plan.services.len(), 1);
        assert_ne!(plan.services[0].target, "4217");
        assert!(plan.services[0].target.parse::<u16>().is_ok());
    }

    #[test]
    fn detect_folder_init_plan_reserves_free_port_for_next_default_dev_script() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        write_package_json(&root.join("package.json"), "next dev");

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();

        assert_eq!(plan.services.len(), 1);
        assert_ne!(plan.services[0].target, "3000");
        assert!(plan.services[0].target.parse::<u16>().is_ok());
        assert!(plan.services[0].can_update_dev_script);
        assert!(!plan.services[0].update_dev_script);
        assert_eq!(plan.services[0].service_name, "web");
        assert_eq!(plan.services[0].service_path, None);
        assert_eq!(
            plan.services[0].command,
            Some(vec!["npm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn detect_current_dir_localify_target_reads_existing_script_port() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        write_package_json_with_localify_script(
            &root.join("package.json"),
            "react-scripts start",
            "echo Localify URL: https://demo.localify && HOST=127.0.0.1 PORT=4318 react-scripts start",
        );

        let target = detect_current_dir_localify_target(root).unwrap();

        assert_eq!(target.as_deref(), Some("4318"));
    }

    #[test]
    fn normalize_domain_appends_suffix_once() {
        assert_eq!(
            normalize_domain("demo".into()),
            format!("demo{}", BRANDING.domain_suffix())
        );
        assert_eq!(
            normalize_domain(format!("demo{}", BRANDING.domain_suffix())),
            format!("demo{}", BRANDING.domain_suffix())
        );
        assert_eq!(normalize_domain("demo.localify".into()), "demo.localify");
        assert_eq!(
            normalize_domain("demo.localify.test".into()),
            "demo.localify.test"
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
    fn detect_workspace_apps_enables_web_and_server_by_default() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::create_dir_all(root.join("apps/server")).unwrap();
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
        write_package_json(
            &root.join("apps/server/package.json"),
            "bun run --hot src/index.ts",
        );

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();

        assert_eq!(plan.services.len(), 2);
        assert_eq!(plan.services[0].display_path, "apps/web");
        assert_eq!(plan.services[0].service_name, "web");
        assert_eq!(plan.services[0].service_path.as_deref(), Some("apps/web"));
        assert_eq!(
            plan.services[0].command,
            Some(vec!["npm".into(), "run".into(), "dev".into()])
        );
        assert_eq!(
            plan.services[0].domain,
            format!("demo{}", BRANDING.domain_suffix())
        );
        assert!(plan.services[0].enabled);
        assert_eq!(plan.services[1].display_path, "apps/server");
        assert_eq!(plan.services[1].service_name, "server");
        assert_eq!(
            plan.services[1].service_path.as_deref(),
            Some("apps/server")
        );
        assert_eq!(
            plan.services[1].domain,
            format!("server.demo{}", BRANDING.domain_suffix())
        );
        assert!(plan.services[1].enabled);
    }

    #[test]
    fn detect_workspace_apps_prioritizes_apps_over_packages() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::create_dir_all(root.join("packages/infra")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "name": "dev-playground-2",
  "workspaces": ["apps/*", "packages/*"],
  "packageManager": "pnpm@11.4.0",
  "scripts": {
    "dev": "turbo dev",
    "dev:web": "turbo -F web dev"
  }
}"#,
        )
        .unwrap();
        fs::write(
            root.join("apps/web/package.json"),
            r#"{
  "name": "web",
  "scripts": {
    "dev:bare": "ALCHEMY=false vite dev"
  }
}"#,
        )
        .unwrap();
        fs::write(
            root.join("apps/web/vite.config.ts"),
            "import { defineConfig } from 'vite';\nexport default defineConfig({ plugins: [] });\n",
        )
        .unwrap();
        fs::write(
            root.join("packages/infra/package.json"),
            r#"{
  "name": "@demo/infra",
  "scripts": {
    "dev": "alchemy dev"
  }
}"#,
        )
        .unwrap();

        let plan = detect_folder_init_plan(root, Some("dev-playground-2")).unwrap();

        assert_eq!(plan.services.len(), 2);
        assert_eq!(plan.services[0].display_path, "apps/web");
        assert_eq!(plan.services[0].service_name, "web");
        assert_eq!(
            plan.services[0].domain,
            format!("dev-playground-2{}", BRANDING.domain_suffix())
        );
        assert!(plan.services[0].enabled);
        assert_eq!(
            plan.services[0].command,
            Some(vec!["pnpm".into(), "run".into(), "dev:bare".into()])
        );
        assert_eq!(plan.services[1].display_path, "packages/infra");
        assert_eq!(
            plan.services[1].domain,
            format!("infra.dev-playground-2{}", BRANDING.domain_suffix())
        );
        assert!(!plan.services[1].enabled);

        let selections = plan
            .services
            .iter()
            .map(|service| FolderInitSelection {
                id: service.id.clone(),
                domain: service.domain.clone(),
                target: service.target.clone(),
                enabled: service.enabled,
                update_dev_script: service.update_dev_script,
                service_name: None,
                service_path: None,
                command: None,
            })
            .collect::<Vec<_>>();
        let resolved = resolve_folder_init_selection(
            root,
            "dev-playground-2",
            &selections,
            LocalAppUpdatePolicy {
                write_package_scripts: true,
                promote_dev_script: true,
            },
        )
        .unwrap();

        assert_eq!(resolved.services.len(), 1);
        assert_eq!(resolved.services[0].service_name.as_deref(), Some("web"));
        assert_eq!(
            resolved.services[0].service_path.as_deref(),
            Some("apps/web")
        );
        assert_eq!(
            resolved.services[0].domain,
            format!("dev-playground-2{}", BRANDING.domain_suffix())
        );
        assert_eq!(
            resolved.services[0].command,
            Some(vec!["pnpm".into(), "run".into(), "dev:bare".into()])
        );
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
        fs::write(
            &package_json_path,
            r#"{
  "name": "test-app",
  "version": "0.1.0",
  "private": true,
  "scripts": {
    "dev": "vite",
    "build": "vite build"
  },
  "dependencies": {
    "react": "^19.0.0"
  }
}
"#,
        )
        .unwrap();

        let results = apply_local_app_updates(
            &[PackageScriptUpdate {
                package_json_path: package_json_path.clone(),
                display_path: "app".into(),
                script_name: "dev-localify".into(),
                script: "vite --port 4010".into(),
            }],
            &[],
            &[],
        );

        assert_eq!(results.updated.len(), 1);
        let updated = fs::read_to_string(package_json_path).unwrap();
        assert!(updated.contains(r#""dev": "vite""#));
        assert!(updated.contains(r#""dev-localify": "vite --port 4010""#));
        assert!(!updated.contains(r#""dev-localify": "vite --port 4010","#));
        assert!(updated.contains(r#""dependencies": {"#));
        assert!(updated.find(r#""name": "test-app""#) < updated.find(r#""scripts": {"#));
        assert!(updated.find(r#""scripts": {"#) < updated.find(r#""dependencies": {"#));
    }

    #[test]
    fn resolve_folder_init_selection_builds_package_update_for_supported_app() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        write_package_json(&root.join("apps/web/package.json"), "next dev");
        write_next_config(
            &root.join("apps/web/next.config.ts"),
            "export default { reactStrictMode: true };\n",
        );

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();
        let selection = FolderInitSelection {
            id: plan.services[0].id.clone(),
            domain: plan.services[0].domain.clone(),
            target: "4010".into(),
            enabled: true,
            update_dev_script: true,
            service_name: None,
            service_path: None,
            command: None,
        };

        let resolved = resolve_folder_init_selection(
            root,
            "Demo",
            &[selection],
            LocalAppUpdatePolicy {
                write_package_scripts: true,
                promote_dev_script: true,
            },
        )
        .unwrap();

        assert_eq!(resolved.services.len(), 1);
        assert_eq!(resolved.services[0].service_name.as_deref(), Some("web"));
        assert_eq!(
            resolved.services[0].command,
            Some(vec!["npm".into(), "run".into(), "dev-localify".into()])
        );
        assert_eq!(resolved.package_updates.len(), 3);
        assert_eq!(resolved.next_config_updates.len(), 1);
        assert_eq!(
            resolved.package_updates[1].script,
            "echo Localify URL: https://demo.localify && next dev --hostname 127.0.0.1 --port 4010"
        );
        assert_eq!(
            resolved.next_config_updates[0].allowed_dev_origin,
            format!("demo{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn resolve_folder_init_selection_rejects_unsafe_domain_for_package_update() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        write_package_json(&root.join("apps/web/package.json"), "next dev");

        let plan = detect_folder_init_plan(root, Some("Demo")).unwrap();
        let selection = FolderInitSelection {
            id: plan.services[0].id.clone(),
            domain: "demo;rm".into(),
            target: "4010".into(),
            enabled: true,
            update_dev_script: true,
            service_name: None,
            service_path: None,
            command: None,
        };

        let error = resolve_folder_init_selection(
            root,
            "Demo",
            &[selection],
            LocalAppUpdatePolicy {
                write_package_scripts: true,
                promote_dev_script: true,
            },
        )
        .expect_err("unsafe shell characters should not reach dev-localify");

        assert!(
            error.to_string().contains("is not a valid Localify domain"),
            "{error}"
        );
    }

    #[test]
    fn plan_current_dir_local_app_updates_detects_single_next_app() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        write_package_json(&root.join("package.json"), "next dev");
        write_next_config(
            &root.join("next.config.ts"),
            "export default { reactStrictMode: true };\n",
        );

        let updates = plan_current_dir_local_app_updates(
            root,
            "Demo",
            &[ServicePlan {
                domain: format!("demo{}", BRANDING.domain_suffix()),
                target: "3000".into(),
                service_name: None,
                service_path: None,
                command: None,
            }],
            LocalAppUpdatePolicy {
                write_package_scripts: true,
                promote_dev_script: false,
            },
        )
        .unwrap();

        assert_eq!(updates.0.len(), 1);
        assert_eq!(updates.1.len(), 1);
        assert_eq!(updates.1[0].next_config_path, root.join("next.config.ts"));
    }

    #[test]
    fn plan_current_dir_local_app_updates_skips_next_without_config_file() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        write_package_json(&root.join("package.json"), "next dev");

        let updates = plan_current_dir_local_app_updates(
            root,
            "Demo",
            &[ServicePlan {
                domain: format!("demo{}", BRANDING.domain_suffix()),
                target: "3000".into(),
                service_name: None,
                service_path: None,
                command: None,
            }],
            LocalAppUpdatePolicy {
                write_package_scripts: true,
                promote_dev_script: false,
            },
        )
        .unwrap();

        assert_eq!(updates.0.len(), 1);
        assert!(updates.1.is_empty());
        assert!(updates.2.is_empty());
    }

    #[test]
    fn apply_next_config_update_inserts_allowed_dev_origins_into_existing_config() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("next.config.ts");
        let origin = format!("demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "import type { NextConfig } from \"next\";\n\nconst nextConfig: NextConfig = {\n  reactStrictMode: true,\n};\n\nexport default nextConfig;\n",
        );

        apply_next_config_update(&NextConfigUpdate {
            next_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_dev_origin: origin.clone(),
        })
        .unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains(&format!("allowedDevOrigins: ['{origin}']")));
        assert!(updated.contains(&format!("hostname: '{origin}'")));
    }

    #[test]
    fn apply_next_config_update_appends_missing_origin_without_duplication() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("next.config.js");
        let existing_origin = format!("existing{}", BRANDING.domain_suffix());
        let origin = format!("demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            &format!("module.exports = {{\n  allowedDevOrigins: ['{existing_origin}'],\n}};\n"),
        );

        let update = NextConfigUpdate {
            next_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_dev_origin: origin.clone(),
        };

        apply_next_config_update(&update).unwrap();
        apply_next_config_update(&update).unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains(&format!("'{existing_origin}', '{origin}'")));
        assert!(updated.contains(&format!("hostname: '{origin}'")));
        assert_eq!(updated.matches(&format!("hostname: '{origin}'")).count(), 1);
    }

    #[test]
    fn apply_next_allowed_dev_origin_does_not_add_image_remote_patterns() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("next.config.js");
        let origin = format!("worktree.demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "module.exports = {\n  reactStrictMode: true,\n};\n",
        );

        assert!(apply_next_allowed_dev_origin(&config_path, &origin).unwrap());
        assert!(!apply_next_allowed_dev_origin(&config_path, &origin).unwrap());

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains(&format!("allowedDevOrigins: ['{origin}']")));
        assert!(!updated.contains("remotePatterns"));
    }

    #[test]
    fn apply_next_config_update_appends_remote_pattern_to_existing_images_config() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("next.config.ts");
        let origin = format!("demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "const nextConfig = {\n  images: {\n    unoptimized: true,\n  },\n};\n\nexport default nextConfig;\n",
        );

        apply_next_config_update(&NextConfigUpdate {
            next_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_dev_origin: origin.clone(),
        })
        .unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains("images: {"));
        assert!(updated.contains("unoptimized: true"));
        assert!(updated.contains("remotePatterns: ["));
        assert!(updated.contains(&format!("hostname: '{origin}'")));
    }

    #[test]
    fn apply_vite_config_update_inserts_allowed_hosts() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let host = format!("demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "import { defineConfig } from 'vite';\n\nexport default defineConfig({\n  plugins: [],\n});\n",
        );

        apply_vite_config_update(&ViteConfigUpdate {
            vite_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_host: host.clone(),
        })
        .unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains(&format!("server: {{ allowedHosts: ['{host}'] }}")));
    }

    #[test]
    fn apply_vite_config_update_ignores_server_inside_string_values() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let host = format!("cinav{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "import { tanstackStart } from '@tanstack/react-start/plugin/vite';\nimport { defineConfig } from 'vite';\n\nexport default defineConfig({\n  define: {\n    'process.env.DEBUG': JSON.stringify(''),\n  },\n  plugins: [\n    tanstackStart({ customViteReactPlugin: true, target: 'node-server' }),\n  ],\n});\n",
        );

        apply_vite_config_update(&ViteConfigUpdate {
            vite_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_host: host.clone(),
        })
        .unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains("target: 'node-server'"));
        assert!(updated.contains(&format!("server: {{ allowedHosts: ['{host}'] }}")));
    }

    #[test]
    fn apply_vite_allowed_host_is_idempotent() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let host = format!("worktree.demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "import { defineConfig } from 'vite';\n\nexport default defineConfig({\n  plugins: [],\n});\n",
        );

        assert!(apply_vite_allowed_host(&config_path, &host).unwrap());
        assert!(!apply_vite_allowed_host(&config_path, &host).unwrap());

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains(&format!("server: {{ allowedHosts: ['{host}'] }}")));
        assert_eq!(updated.matches(&host).count(), 1);
    }

    #[test]
    fn apply_vite_config_update_targets_returned_define_config_object() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let host = format!("demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "import { defineConfig } from 'vite';\n\nconst alias = true ? {\n  server: { ignored: true },\n} : {};\n\nexport default defineConfig(async () => {\n  return {\n    server: {\n      port: 3001,\n    },\n    resolve: {\n      alias,\n    },\n  };\n});\n",
        );

        apply_vite_config_update(&ViteConfigUpdate {
            vite_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_host: host.clone(),
        })
        .unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains("server: { ignored: true }"));
        assert!(updated.contains(&format!("port: 3001,\n      allowedHosts: ['{host}']")));
    }

    #[test]
    fn apply_vite_config_update_skips_import_and_prior_function_calls() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let host = format!("playground{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            "import { fileURLToPath } from 'node:url';\nimport { defineConfig } from 'vite';\n\nconst configPath = fileURLToPath(new URL('./config.json', import.meta.url));\n\nexport default defineConfig(async () => {\n  return {\n    server: {\n      host: process.env.HOST,\n    },\n    plugins: [],\n  };\n});\n",
        );

        apply_vite_config_update(&ViteConfigUpdate {
            vite_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_host: host.clone(),
        })
        .unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains(&format!(
            "host: process.env.HOST,\n      allowedHosts: ['{host}']"
        )));
        assert_eq!(updated.matches("server:").count(), 1);
        assert!(!updated.contains("};,"));
    }

    #[test]
    fn apply_vite_config_update_appends_missing_allowed_host_without_duplication() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let existing_host = format!("existing{}", BRANDING.domain_suffix());
        let host = format!("demo{}", BRANDING.domain_suffix());
        write_next_config(
            &config_path,
            &format!(
                "export default {{\n  server: {{\n    allowedHosts: ['{existing_host}'],\n  }},\n}};\n"
            ),
        );

        let update = ViteConfigUpdate {
            vite_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_host: host.clone(),
        };
        apply_vite_config_update(&update).unwrap();
        apply_vite_config_update(&update).unwrap();

        let updated = fs::read_to_string(config_path).unwrap();
        assert!(updated.contains(&format!("'{existing_host}', '{host}'")));
        assert_eq!(updated.matches(&host).count(), 1);
    }

    #[test]
    fn package_updates_promote_dev_and_preserve_localhost_script() {
        let temp_dir = tempdir().unwrap();
        let package_json_path = temp_dir.path().join("package.json");
        write_package_json(&package_json_path, "vite");
        let content = fs::read_to_string(&package_json_path).unwrap();
        let manifest: PackageJsonManifest = serde_json::from_str(&content).unwrap();
        let scripts = manifest.scripts.unwrap();
        let blueprint = FolderServiceBlueprint {
            id: "app".into(),
            label: "app".into(),
            display_path: "app".into(),
            service_name: "app".into(),
            service_path: Some("app".into()),
            package_command: Some(vec!["npm".into(), "run".into(), "dev".into()]),
            script_name: "dev".into(),
            domain: format!("demo{}", BRANDING.domain_suffix()),
            target: "4010".into(),
            enabled: true,
            package_json_path: Some(package_json_path.clone()),
            dev_script: Some("vite".into()),
            scripts: Some(scripts),
            next_config_path: None,
            vite_config_path: None,
        };

        let updates = build_package_updates(
            &blueprint,
            "4010",
            &format!("demo{}", BRANDING.domain_suffix()),
            LocalAppUpdatePolicy {
                write_package_scripts: true,
                promote_dev_script: true,
            },
        )
        .unwrap();
        let results = apply_local_app_updates(&updates, &[], &[]);

        let updated = fs::read_to_string(package_json_path).unwrap();
        assert_eq!(results.updated.len(), 1);
        assert!(updated.contains(r#""dev-localhost": "vite""#));
        assert!(updated.contains(r#""dev-localify": "echo Localify URL: https://demo.localify && vite --host 127.0.0.1 --port 4010""#));
        assert!(updated.contains(r#""dev": "echo Localify URL: https://demo.localify && vite --host 127.0.0.1 --port 4010""#));
    }

    #[test]
    fn apply_next_config_update_requires_existing_file() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("next.config.mjs");
        let origin = format!("demo{}", BRANDING.domain_suffix());

        let error = apply_next_config_update(&NextConfigUpdate {
            next_config_path: config_path.clone(),
            display_path: "web".into(),
            allowed_dev_origin: origin.clone(),
        })
        .unwrap_err();

        assert!(error.to_string().contains("Failed to read"));
        assert!(!config_path.exists());
    }
}
