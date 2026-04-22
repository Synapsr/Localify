use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalPreference {
    #[default]
    Terminal,
    Ghostty,
    ITerm,
    WezTerm,
}

const MACOS_APP_DIRS: [&str; 3] = [
    "/Applications",
    "/System/Applications",
    "/System/Applications/Utilities",
];

#[allow(dead_code)]
pub fn available_terminal_preferences() -> Vec<TerminalPreference> {
    [
        TerminalPreference::Terminal,
        TerminalPreference::Ghostty,
        TerminalPreference::ITerm,
        TerminalPreference::WezTerm,
    ]
    .into_iter()
    .filter(|preference| terminal_available(*preference))
    .collect()
}

#[allow(dead_code)]
pub fn terminal_label(preference: TerminalPreference) -> &'static str {
    match preference {
        TerminalPreference::Terminal => "Terminal",
        TerminalPreference::Ghostty => "Ghostty",
        TerminalPreference::ITerm => "iTerm",
        TerminalPreference::WezTerm => "WezTerm",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDevLaunchPlan {
    pub display_path: String,
    pub package_dir: PathBuf,
    pub package_manager: PackageManager,
    pub script_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManager {
    fn command(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    fn command_parts(self, script_name: &str) -> Vec<String> {
        match self {
            Self::Yarn => vec![self.command().into(), script_name.into()],
            Self::Npm | Self::Pnpm | Self::Bun => {
                vec![self.command().into(), "run".into(), script_name.into()]
            }
        }
    }

    fn label(self) -> &'static str {
        self.command()
    }
}

#[derive(Debug, Clone)]
struct LocalDevCandidate {
    display_path: String,
    package_dir: PathBuf,
    package_manager: PackageManager,
    script_name: String,
    script_priority: usize,
    port: Option<u16>,
    is_root: bool,
}

#[derive(Debug, Deserialize)]
struct PackageJsonManifest {
    scripts: Option<Map<String, Value>>,
    workspaces: Option<Value>,
    #[serde(rename = "packageManager")]
    package_manager: Option<String>,
}

pub fn open_local_dev_server(project_root: &Path, target: &str) -> Result<()> {
    open_local_dev_server_with_terminal(project_root, target, TerminalPreference::Terminal)
}

pub fn open_local_dev_server_with_terminal(
    project_root: &Path,
    target: &str,
    terminal: TerminalPreference,
) -> Result<()> {
    if !project_root.is_dir() {
        bail!(
            "Project folder '{}' does not exist.",
            project_root.display()
        );
    }

    let launch = resolve_local_dev_launch(project_root, target)?;
    launch_in_terminal(&launch, terminal)
}

fn resolve_local_dev_launch(project_root: &Path, target: &str) -> Result<LocalDevLaunchPlan> {
    let target_port = extract_target_port(target)?;
    let mut candidates = detect_local_dev_candidates(project_root)?;

    if candidates.is_empty() {
        bail!(
            "No workspace package in '{}' exposes a launchable local dev script with an explicit port yet.",
            project_root.display()
        );
    }

    let matching_indexes = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (candidate.port == Some(target_port)).then_some(index))
        .collect::<Vec<_>>();

    let selected = if matching_indexes.is_empty() {
        if candidates.len() == 1 {
            let candidate = candidates.swap_remove(0);
            if let Some(candidate_port) = candidate.port
                && candidate_port != target_port
            {
                bail!(
                    "The only detected script '{}' runs on port {}, but this Localify domain points to {}.",
                    candidate.script_name,
                    candidate_port,
                    target_port
                );
            }
        }

        bail!(
            "No workspace package matches Localify target port {}.\n{}",
            target_port,
            format_candidates(&candidates)
        );
    } else {
        let best_priority = matching_indexes
            .iter()
            .map(|index| candidates[*index].script_priority)
            .min()
            .unwrap_or(usize::MAX);

        let best_indexes = matching_indexes
            .into_iter()
            .filter(|index| candidates[*index].script_priority == best_priority)
            .collect::<Vec<_>>();

        if best_indexes.len() == 1 {
            candidates.swap_remove(best_indexes[0])
        } else {
            let matches = best_indexes
                .iter()
                .map(|index| candidates[*index].clone())
                .collect::<Vec<_>>();
            bail!(
                "Multiple workspace scripts match port {}.\n{}",
                target_port,
                format_candidates(&matches)
            );
        }
    };

    Ok(LocalDevLaunchPlan {
        display_path: selected.display_path,
        package_dir: selected.package_dir,
        package_manager: selected.package_manager,
        script_name: selected.script_name,
    })
}

fn detect_local_dev_candidates(project_root: &Path) -> Result<Vec<LocalDevCandidate>> {
    let manifests = candidate_manifests(project_root)?;
    let root_manifest = project_root.join("package.json");
    let root_is_workspace =
        root_manifest.exists() && package_json_declares_workspaces(&root_manifest)?;
    let mut candidates = Vec::new();

    for package_json_path in manifests {
        let content = fs::read_to_string(&package_json_path)
            .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
        let manifest: PackageJsonManifest = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;
        let Some(scripts) = manifest.scripts.as_ref() else {
            continue;
        };

        let package_dir = package_json_path.parent().ok_or_else(|| {
            anyhow!(
                "Invalid package.json path '{}'",
                package_json_path.display()
            )
        })?;
        let display_path = display_path(project_root, package_dir)?;
        let package_manager = detect_package_manager(project_root, package_dir, &manifest)?;

        for (script_name, script_value) in scripts {
            let Some(script_priority) = script_priority(script_name) else {
                continue;
            };
            let Some(script) = script_value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };

            candidates.push(LocalDevCandidate {
                display_path: display_path.clone(),
                package_dir: package_dir.to_path_buf(),
                package_manager,
                script_name: script_name.clone(),
                script_priority,
                port: extract_script_port(script),
                is_root: package_dir == project_root,
            });
        }
    }

    if root_is_workspace && candidates.iter().any(|candidate| !candidate.is_root) {
        candidates.retain(|candidate| !candidate.is_root);
    }

    Ok(candidates)
}

fn script_priority(script_name: &str) -> Option<usize> {
    match script_name {
        "dev-localify" => Some(0),
        "dev" => Some(1),
        _ if script_name.starts_with("dev:") => Some(2),
        _ if script_name.ends_with(":dev") => Some(3),
        _ => None,
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

fn display_path(root: &Path, package_dir: &Path) -> Result<String> {
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
        })?;

    if relative_dir.is_empty() || relative_dir == "." {
        Ok(label.to_string())
    } else {
        Ok(relative_dir)
    }
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

fn extract_target_port(target: &str) -> Result<u16> {
    let trimmed = target.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        bail!("Localify target is empty.");
    }

    if let Ok(port) = trimmed.parse::<u16>() {
        return Ok(port);
    }

    let candidate = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let (_, port) = candidate
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("Localify target '{}' is not a proxy port.", target))?;

    port.parse::<u16>().with_context(|| {
        format!(
            "Localify target '{}' does not end with a valid port.",
            target
        )
    })
}

fn extract_script_port(script: &str) -> Option<u16> {
    let tokens = script.split_whitespace().collect::<Vec<_>>();
    let mut index = 0;

    while index < tokens.len() {
        let token = tokens[index];

        if let Some(port) = token.strip_prefix("PORT=").and_then(parse_port_token) {
            return Some(port);
        }
        if let Some(port) = token
            .strip_prefix("--port=")
            .and_then(parse_port_token)
            .or_else(|| token.strip_prefix("-p=").and_then(parse_port_token))
        {
            return Some(port);
        }
        if matches!(token, "--port" | "-p") {
            let next = tokens
                .get(index + 1)
                .and_then(|value| parse_port_token(value));
            if next.is_some() {
                return next;
            }
            index += 2;
            continue;
        }

        index += 1;
    }

    None
}

fn parse_port_token(value: &str) -> Option<u16> {
    value
        .trim_matches(|char| char == '"' || char == '\'')
        .parse::<u16>()
        .ok()
}

fn format_candidates(candidates: &[LocalDevCandidate]) -> String {
    let mut lines = String::from("Detected candidate scripts:");
    for candidate in candidates {
        let port = candidate
            .port
            .map(|port| port.to_string())
            .unwrap_or_else(|| "unknown".into());
        lines.push_str(&format!(
            "\n- {}#{} ({}, port {})",
            candidate.display_path,
            candidate.script_name,
            candidate.package_manager.label(),
            port,
        ));
    }
    lines
}

fn launch_command(launch: &LocalDevLaunchPlan) -> String {
    let mut parts = vec![
        "cd".into(),
        shell_escape(&launch.package_dir.to_string_lossy()),
        "&&".into(),
    ];
    parts.extend(
        launch
            .package_manager
            .command_parts(&launch.script_name)
            .into_iter()
            .map(|part| shell_escape(&part)),
    );
    parts.join(" ")
}

fn run_script_command(launch: &LocalDevLaunchPlan) -> String {
    launch
        .package_manager
        .command_parts(&launch.script_name)
        .into_iter()
        .map(|part| shell_escape(&part))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(target_os = "macos")]
fn launch_in_terminal(launch: &LocalDevLaunchPlan, terminal: TerminalPreference) -> Result<()> {
    match terminal {
        TerminalPreference::Terminal => launch_with_terminal_app(launch),
        TerminalPreference::Ghostty => launch_with_ghostty(launch),
        TerminalPreference::ITerm => launch_with_iterm(launch),
        TerminalPreference::WezTerm => launch_with_wezterm(launch),
    }
}

#[cfg(not(target_os = "macos"))]
fn launch_in_terminal(_launch: &LocalDevLaunchPlan, _terminal: TerminalPreference) -> Result<()> {
    bail!("Launching a local dev server from Localify currently supports macOS only.");
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

#[cfg(target_os = "macos")]
fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn find_app_bundle(app_name: &str) -> Option<PathBuf> {
    MACOS_APP_DIRS
        .iter()
        .map(|directory| PathBuf::from(directory).join(format!("{app_name}.app")))
        .find(|candidate| candidate.exists())
}

#[cfg(not(target_os = "macos"))]
fn find_app_bundle(_app_name: &str) -> Option<PathBuf> {
    None
}

fn find_command(command_name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(command_name))
            .find(|candidate| candidate.is_file())
    })
}

#[allow(dead_code)]
fn terminal_available(preference: TerminalPreference) -> bool {
    match preference {
        TerminalPreference::Terminal => find_app_bundle("Terminal").is_some(),
        TerminalPreference::Ghostty => find_app_bundle("Ghostty").is_some(),
        TerminalPreference::ITerm => {
            find_app_bundle("iTerm").is_some() || find_app_bundle("iTerm2").is_some()
        }
        TerminalPreference::WezTerm => {
            find_command("wezterm").is_some() || wezterm_bundle_binary().is_some()
        }
    }
}

#[cfg(target_os = "macos")]
fn launch_with_terminal_app(launch: &LocalDevLaunchPlan) -> Result<()> {
    let command = launch_command(launch);
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
        escape_applescript_string(&command)
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .with_context(|| format!("Failed to launch Terminal for '{}'.", launch.display_path))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        bail!("Terminal could not start the linked dev server.");
    }

    bail!("{stderr}")
}

#[cfg(target_os = "macos")]
fn launch_with_ghostty(launch: &LocalDevLaunchPlan) -> Result<()> {
    let command = run_script_command(launch);
    let script = format!(
        "tell application \"Ghostty\"\nactivate\nset cfg to new surface configuration\nset initial working directory of cfg to \"{}\"\nset win to new window with configuration cfg\nset term to focused terminal of selected tab of win\ninput text \"{}\" to term\nsend key \"enter\" to term\nend tell",
        escape_applescript_string(&launch.package_dir.to_string_lossy()),
        escape_applescript_string(&command)
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .with_context(|| format!("Failed to launch Ghostty for '{}'.", launch.display_path))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        bail!("Ghostty could not start the linked dev server.");
    }

    bail!("{stderr}")
}

#[cfg(target_os = "macos")]
fn launch_with_iterm(launch: &LocalDevLaunchPlan) -> Result<()> {
    let command = launch_command(launch);
    let script = format!(
        "tell application \"iTerm2\"\nactivate\ncreate window with default profile command \"{}\"\nend tell",
        escape_applescript_string(&command)
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .with_context(|| format!("Failed to launch iTerm for '{}'.", launch.display_path))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        bail!("iTerm could not start the linked dev server.");
    }

    bail!("{stderr}")
}

#[cfg(target_os = "macos")]
fn launch_with_wezterm(launch: &LocalDevLaunchPlan) -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let command = run_script_command(launch);
    let wezterm = wezterm_binary().ok_or_else(|| anyhow!("WezTerm is not installed."))?;
    let output = Command::new(wezterm)
        .arg("start")
        .arg("--cwd")
        .arg(&launch.package_dir)
        .arg("--")
        .arg(shell)
        .arg("-l")
        .arg("-c")
        .arg(command)
        .output()
        .with_context(|| format!("Failed to launch WezTerm for '{}'.", launch.display_path))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        bail!("WezTerm could not start the linked dev server.");
    }

    bail!("{stderr}")
}

#[cfg(target_os = "macos")]
fn wezterm_binary() -> Option<PathBuf> {
    find_command("wezterm").or_else(wezterm_bundle_binary)
}

#[cfg(target_os = "macos")]
fn wezterm_bundle_binary() -> Option<PathBuf> {
    find_app_bundle("WezTerm").map(|bundle| bundle.join("Contents/MacOS/wezterm"))
}

#[cfg(not(target_os = "macos"))]
fn wezterm_bundle_binary() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        PackageManager, detect_local_dev_candidates, extract_script_port, resolve_local_dev_launch,
    };

    #[test]
    fn extract_script_port_reads_common_patterns() {
        assert_eq!(extract_script_port("next dev --port 4010"), Some(4010));
        assert_eq!(
            extract_script_port("vite --host 127.0.0.1 --port=4020"),
            Some(4020)
        );
        assert_eq!(
            extract_script_port("PORT=4030 react-scripts start"),
            Some(4030)
        );
        assert_eq!(extract_script_port("yarn dev-localify"), None);
    }

    #[test]
    fn resolve_local_dev_launch_prefers_dev_localify_over_other_dev_scripts() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("package.json"),
            r#"{
  "name": "demo",
  "packageManager": "pnpm@10.0.0",
  "scripts": {
    "dev-localify": "next dev --hostname 127.0.0.1 --port 4010",
    "dev:web": "next dev --hostname 127.0.0.1 --port 4010"
  }
}"#,
        )
        .unwrap();

        let launch = resolve_local_dev_launch(root, "127.0.0.1:4010").unwrap();

        assert_eq!(launch.script_name, "dev-localify");
        assert_eq!(launch.package_manager, PackageManager::Pnpm);
    }

    #[test]
    fn resolve_local_dev_launch_accepts_dev_scoped_script_fallback() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::create_dir_all(root.join("apps/admin")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "private": true,
  "packageManager": "pnpm@10.0.0",
  "workspaces": ["apps/*"]
}"#,
        )
        .unwrap();
        fs::write(
            root.join("apps/web/package.json"),
            r#"{
  "name": "web",
  "scripts": {
    "dev:web": "next dev --hostname 127.0.0.1 --port 4010"
  }
}"#,
        )
        .unwrap();
        fs::write(
            root.join("apps/admin/package.json"),
            r#"{
  "name": "admin",
  "scripts": {
    "dev:admin": "vite --host 127.0.0.1 --port 4020"
  }
}"#,
        )
        .unwrap();

        let launch = resolve_local_dev_launch(root, "127.0.0.1:4020").unwrap();

        assert_eq!(launch.display_path, "apps/admin");
        assert_eq!(launch.script_name, "dev:admin");
        assert_eq!(launch.package_manager, PackageManager::Pnpm);
        assert_eq!(launch.package_dir, root.join("apps/admin"));
    }

    #[test]
    fn detect_local_dev_candidates_ignores_non_dev_like_scripts() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(
            root.join("package.json"),
            r#"{
  "name": "demo",
  "scripts": {
    "preview": "next start --port 4010",
    "serve:web": "vite --port 4010",
    "dev:web": "vite --port 4010"
  }
}"#,
        )
        .unwrap();

        let candidates = detect_local_dev_candidates(root).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].script_name, "dev:web");
    }

    #[test]
    fn detect_local_dev_candidates_ignores_workspace_root_when_apps_exist() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{
  "private": true,
  "workspaces": ["apps/*"],
  "scripts": {
    "dev-localify": "next dev --hostname 127.0.0.1 --port 4000"
  }
}"#,
        )
        .unwrap();
        fs::write(
            root.join("apps/web/package.json"),
            r#"{
  "name": "web",
  "scripts": {
    "dev-localify": "next dev --hostname 127.0.0.1 --port 4010"
  }
}"#,
        )
        .unwrap();

        let candidates = detect_local_dev_candidates(root).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].display_path, "apps/web");
    }
}
