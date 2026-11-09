//! What `localify dev` runs for a service: the launch plan. A configured
//! service becomes one by detection (the package manager and script its
//! folder's `package.json` names) and by preparation (the host and port
//! arguments its framework needs). `localify dev` and the inventory both go
//! through here, so the plan a revision covers is the plan that runs
//! (docs/remote-machines.md, "Inventory").

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::domain::RouteTarget;
use crate::infrastructure::config::{Config, ConfiguredProjectEnvironment, ConfiguredService};
use crate::infrastructure::local_dev::{
    LocalDevLaunchPlan, resolve_local_dev_launch, resolve_localify_dev_launch,
    resolve_portable_local_dev_launch,
};

const LOOPBACK_HOST: &str = "127.0.0.1";

/// The command a launch starts with, before preparation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchCommand {
    /// The service's own command, configured or detected: Localify adds the
    /// arguments its framework needs.
    Detected(Vec<String>),
    /// A command after `--`, run exactly as given.
    Override(Vec<String>),
}

/// The effective launch of one service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// What runs, the arguments Localify added included.
    pub argv: Vec<String>,
    /// The arguments Localify added to the service's command.
    pub injected: Vec<String>,
    pub workdir: PathBuf,
    /// Where its launch profiles are read.
    pub project_root: PathBuf,
}

/// The plan of `service`, its launch detection already applied (see
/// [`service_with_dev_launch`]), listening on `port`. Fails when its folder
/// is missing, or a `package.json` it reads does not parse.
pub fn resolve(
    service: &ConfiguredService,
    command: LaunchCommand,
    port: u16,
) -> Result<LaunchPlan> {
    let workdir = service_workdir(service)?;
    let project_root = service_project_root(service, &workdir);
    let (argv, injected) = match command {
        LaunchCommand::Detected(command) => {
            let base = command.len();
            let argv = prepare_local_dev_command(command, &workdir, &project_root, port)?;
            let injected = argv[base.min(argv.len())..].to_vec();
            (argv, injected)
        }
        LaunchCommand::Override(command) => (command, Vec::new()),
    };
    Ok(LaunchPlan {
        argv,
        injected,
        workdir,
        project_root,
    })
}

/// One service as `localify dev` launches it with its own command. The
/// entry the config gives for its environment, which its revision covers, is
/// kept apart from the plan resolved once its launch is detected, which is
/// what runs: detection may infer its command and its folder, and drop a
/// legacy `dev-localify` command. The inventory and the executor both build
/// it here, so a revision one computed is the revision the other checks.
pub struct DevLaunch {
    pub configured: ConfiguredService,
    pub plan: Result<LaunchPlan>,
}

impl DevLaunch {
    /// `configured` as `localify dev` runs it, listening on `port`.
    pub fn resolve(configured: ConfiguredService, port: u16) -> Self {
        let plan = dev_plan(configured.clone(), port);
        Self { configured, plan }
    }
}

/// The plan of `localify dev <service> --project <id> [--env <slug>]`, from
/// the service as its environment gives it: what a remote start would run.
pub fn dev_plan(service: ConfiguredService, port: u16) -> Result<LaunchPlan> {
    let service = service_with_dev_launch(service);
    let command = service.command.clone().with_context(|| {
        format!(
            "Service '{}' has no detected command: its folder has no dev script. Run it with a command, for example: {} dev {} -- pnpm dev",
            service.id,
            crate::branding::BRANDING.cli_command(),
            service.id
        )
    })?;
    resolve(&service, LaunchCommand::Detected(command), port)
}

/// A service as `localify dev --env` runs it in another environment: that
/// environment's domain, checkout and target, and the folder the primary
/// checkout's route detects when the config names none. The primary
/// environment, named or not, runs the service as the config gives it, every
/// domain and no inferred folder: one launch, so one `configRev`, whether
/// `--env` names it or not.
pub fn environment_service(
    config: &Config,
    service: ConfiguredService,
    environment: &ConfiguredProjectEnvironment,
) -> Option<ConfiguredService> {
    if environment.primary {
        return Some(service);
    }
    let inferred_service_path = service
        .path
        .clone()
        .or_else(|| inferred_path_from_configured_route_launch(&service));
    let mut service = config.environment_service(service, environment)?;
    if service
        .path
        .as_deref()
        .is_none_or(|path| path.trim().is_empty())
    {
        service.path = inferred_service_path;
    }
    Some(service)
}

pub fn service_project_root(service: &ConfiguredService, workdir: &Path) -> PathBuf {
    service
        .project_root_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| workdir.to_path_buf())
}

pub fn service_cwd_match_path(service: &ConfiguredService, root_path: &Path) -> Option<PathBuf> {
    let path = service.path.as_deref()?.trim();
    if path.is_empty() {
        return None;
    }
    let path = Path::new(path);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root_path.join(path)
    })
}

pub fn service_with_local_dev_launch(service: ConfiguredService) -> ConfiguredService {
    let service = service_without_legacy_dev_localify_command(service);
    let Some(root_path) = service.project_root_path.as_ref().map(PathBuf::from) else {
        return service;
    };

    if let Some(launch) = local_dev_launch_for_service(&service, &root_path) {
        let service = apply_local_dev_launch(service, &root_path, &launch);
        if !is_legacy_dev_localify_launch(&launch) {
            return service;
        }

        return portable_local_dev_launch_for_service(&service, &root_path)
            .map(|launch| apply_local_dev_launch(service.clone(), &root_path, &launch))
            .unwrap_or(service);
    }

    portable_local_dev_launch_for_service(&service, &root_path)
        .map(|launch| apply_local_dev_launch(service.clone(), &root_path, &launch))
        .unwrap_or(service)
}

pub fn service_with_dev_launch(service: ConfiguredService) -> ConfiguredService {
    let service = service_without_legacy_dev_localify_command(service);
    let Some(root_path) = service.project_root_path.as_ref().map(PathBuf::from) else {
        return service;
    };
    let launch = localify_dev_launch_for_service(&service, &root_path);
    let Some(launch) = launch else {
        return service;
    };

    let mut service = service;
    service.command = None;
    apply_local_dev_launch(service, &root_path, &launch)
}

fn localify_dev_launch_for_service(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<LocalDevLaunchPlan> {
    if let Some(package_scope) = service_cwd_match_path(service, root_path) {
        return resolve_localify_dev_launch(root_path, Some(&package_scope)).ok();
    }

    if let Some(legacy_launch) = local_dev_launch_for_service(service, root_path)
        && let Ok(launch) = resolve_localify_dev_launch(root_path, Some(&legacy_launch.package_dir))
    {
        return Some(launch);
    }

    if let Some(package_scope) = package_scope_for_service_name(service, root_path)
        && let Ok(launch) = resolve_localify_dev_launch(root_path, Some(&package_scope))
    {
        return Some(launch);
    }

    resolve_localify_dev_launch(root_path, None).ok()
}

fn package_scope_for_service_name(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<PathBuf> {
    let labels = [
        service.id.trim().to_ascii_lowercase(),
        service.name.trim().to_ascii_lowercase(),
    ];

    for directory in ["apps", "packages", "services"] {
        let parent = root_path.join(directory);
        let Ok(entries) = fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.join("package.json").is_file() {
                continue;
            }

            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            if labels
                .iter()
                .any(|label| !label.is_empty() && label == &name)
            {
                return Some(path);
            }
        }
    }

    None
}

pub fn local_dev_launch_for_service(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<LocalDevLaunchPlan> {
    if service
        .path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
        && service.command.is_some()
    {
        return None;
    }

    let target = match service.primary_target.as_ref()? {
        RouteTarget::Proxy(target) => target.to_string(),
        RouteTarget::StaticFiles(_) => return None,
    };
    resolve_local_dev_launch(root_path, &target).ok()
}

pub fn inferred_path_from_configured_route_launch(service: &ConfiguredService) -> Option<String> {
    let root_path = service.project_root_path.as_ref().map(PathBuf::from)?;
    let launch = local_dev_launch_for_service(service, &root_path)?;
    let relative_path = launch
        .package_dir
        .strip_prefix(&root_path)
        .unwrap_or(&launch.package_dir);
    if relative_path.as_os_str().is_empty() {
        return None;
    }

    Some(relative_path.display().to_string())
}

fn portable_local_dev_launch_for_service(
    service: &ConfiguredService,
    root_path: &Path,
) -> Option<LocalDevLaunchPlan> {
    if service.command.is_some() {
        return None;
    }

    let package_scope = service_cwd_match_path(service, root_path);
    resolve_portable_local_dev_launch(root_path, package_scope.as_deref()).ok()
}

pub fn apply_local_dev_launch(
    mut service: ConfiguredService,
    root_path: &Path,
    launch: &LocalDevLaunchPlan,
) -> ConfiguredService {
    if service
        .path
        .as_deref()
        .is_none_or(|path| path.trim().is_empty())
    {
        let relative_path = launch
            .package_dir
            .strip_prefix(root_path)
            .unwrap_or(&launch.package_dir);
        if !relative_path.as_os_str().is_empty() {
            service.path = Some(relative_path.display().to_string());
        }
    }
    // `dev-localify` is legacy metadata only. Localify can use it to recover
    // old service paths, but should never choose it as a launch command.
    if service.command.is_none() && !is_legacy_dev_localify_launch(launch) {
        service.command = Some(launch.command_parts());
    }

    service
}

fn service_without_legacy_dev_localify_command(
    mut service: ConfiguredService,
) -> ConfiguredService {
    if service
        .command
        .as_deref()
        .is_some_and(command_invokes_dev_localify)
    {
        service.command = None;
    }
    service
}

fn is_legacy_dev_localify_launch(launch: &LocalDevLaunchPlan) -> bool {
    launch.script_name == "dev-localify"
}

pub fn command_invokes_dev_localify(command: &[String]) -> bool {
    match command {
        [binary, script] if binary == "yarn" && script == "dev-localify" => true,
        [binary, script]
            if matches!(binary.as_str(), "pnpm" | "bun") && script == "dev-localify" =>
        {
            true
        }
        [binary, run, script, ..]
            if matches!(binary.as_str(), "npm" | "pnpm" | "bun")
                && run == "run"
                && script == "dev-localify" =>
        {
            true
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageScriptRunner {
    Npm,
    DirectArgs,
    Yarn,
}

pub fn prepare_local_dev_command(
    command: Vec<String>,
    workdir: &Path,
    project_root: &Path,
    port: u16,
) -> Result<Vec<String>> {
    let Some((runner, script_name)) = package_script_invocation(&command) else {
        return Ok(command);
    };
    let Some(script) = read_package_script(workdir, script_name)? else {
        return Ok(command);
    };
    let next_host = if script.contains("next dev") {
        next_dev_host(workdir, project_root)
    } else {
        "127.0.0.1"
    };
    let args = localify_dev_args_for_script(&script, port, next_host);
    if args.is_empty() {
        return Ok(command);
    }

    Ok(append_package_script_args(command, runner, args))
}

fn package_script_invocation(command: &[String]) -> Option<(PackageScriptRunner, &str)> {
    match command {
        [binary, script] if binary == "yarn" => Some((PackageScriptRunner::Yarn, script.as_str())),
        [binary, run, script, ..] if binary == "npm" && run == "run" => {
            Some((PackageScriptRunner::Npm, script.as_str()))
        }
        [binary, run, script, ..] if matches!(binary.as_str(), "pnpm" | "bun") && run == "run" => {
            Some((PackageScriptRunner::DirectArgs, script.as_str()))
        }
        _ => None,
    }
}

fn read_package_script(workdir: &Path, script_name: &str) -> Result<Option<String>> {
    let package_json_path = workdir.join("package.json");
    if !package_json_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;

    Ok(manifest
        .get("scripts")
        .and_then(|scripts| scripts.get(script_name))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
}

fn localify_dev_args_for_script(script: &str, port: u16, next_host: &str) -> Vec<String> {
    let trimmed = script.trim();
    if trimmed.contains("next dev") {
        return host_and_port_args(
            trimmed,
            "--hostname",
            &["--hostname", "-H"],
            next_host,
            port,
        );
    }
    if is_vite_dev_script(trimmed) || contains_dev_command(trimmed, "astro dev") {
        return host_and_port_args(trimmed, "--host", &["--host"], "127.0.0.1", port);
    }
    if contains_dev_command(trimmed, "nuxt dev") {
        return host_and_port_args(trimmed, "--host", &["--host"], "127.0.0.1", port);
    }
    if contains_dev_command(trimmed, "email dev") {
        return arg_if_needed(trimmed, "--port", &["--port", "-p"], &port.to_string());
    }

    Vec::new()
}

fn host_and_port_args(
    script: &str,
    host_flag: &str,
    host_flags: &[&str],
    host: &str,
    port: u16,
) -> Vec<String> {
    let mut args = arg_if_needed(script, host_flag, host_flags, host);
    args.extend(arg_if_needed(
        script,
        "--port",
        &["--port", "-p"],
        &port.to_string(),
    ));
    args
}

fn arg_if_needed(script: &str, flag: &str, aliases: &[&str], desired: &str) -> Vec<String> {
    if script_has_flag_value(script, aliases, desired) {
        Vec::new()
    } else {
        vec![flag.into(), desired.into()]
    }
}

fn script_has_flag_value(script: &str, flags: &[&str], desired: &str) -> bool {
    let parts = script.split_whitespace().collect::<Vec<_>>();

    for (index, part) in parts.iter().enumerate() {
        for flag in flags {
            if part == flag && parts.get(index + 1).is_some_and(|value| *value == desired) {
                return true;
            }

            let prefix = format!("{flag}=");
            if part
                .strip_prefix(&prefix)
                .is_some_and(|value| value == desired)
            {
                return true;
            }
        }
    }

    false
}

fn is_vite_dev_script(script: &str) -> bool {
    script == "vite" || script.starts_with("vite ") || script.contains(" vite ")
}

fn contains_dev_command(script: &str, command: &str) -> bool {
    script == command
        || script.starts_with(&format!("{command} "))
        || script.contains(&format!(" {command}"))
}

fn append_package_script_args(
    mut command: Vec<String>,
    runner: PackageScriptRunner,
    args: Vec<String>,
) -> Vec<String> {
    match runner {
        PackageScriptRunner::Npm => {
            if !command.iter().any(|part| part == "--") {
                command.push("--".into());
            }
            command.extend(args);
        }
        PackageScriptRunner::DirectArgs | PackageScriptRunner::Yarn => command.extend(args),
    }
    command
}

pub fn service_with_project_root(
    mut service: ConfiguredService,
    project_roots: &HashMap<String, PathBuf>,
) -> ConfiguredService {
    let should_use_local_root = service
        .project_root_path
        .as_deref()
        .map(|root_path| !Path::new(root_path).exists())
        .unwrap_or(true);
    if should_use_local_root && let Some(root_path) = project_roots.get(&service.project_id) {
        service.project_root_path = Some(root_path.display().to_string());
    }
    service
}

pub fn service_workdir(service: &ConfiguredService) -> Result<PathBuf> {
    let root_path = service.project_root_path.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "Service '{}' is unavailable: project '{}' has no root path metadata.",
            service.id,
            service.project_id
        )
    })?;

    let root_path = PathBuf::from(root_path);
    let workdir = service
        .path
        .as_deref()
        .map(Path::new)
        .map(|path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root_path.join(path)
            }
        })
        .unwrap_or(root_path);

    if !workdir.exists() {
        bail!(
            "Service '{}' is unavailable: '{}' does not exist.",
            service.id,
            workdir.display()
        );
    }
    if !workdir.is_dir() {
        bail!(
            "Service '{}' is unavailable: '{}' is not a directory.",
            service.id,
            workdir.display()
        );
    }

    Ok(workdir)
}

/// The host a Next.js dev server should listen on behind Localify.
///
/// Next.js listening on 127.0.0.1 sends next-intl's locale rewrites back to
/// itself over the network, as https, which fails with EPROTO
/// (vercel/next.js#94745). On 0.0.0.0 the rewrite stays inside Next.js, but the
/// dev server is open to the local network, so only next-intl apps get it.
/// A monorepo can list next-intl in a parent package.json, up to the project
/// root. Go back to 127.0.0.1 for every app once Next.js ships a fix.
pub fn next_dev_host(package_dir: &Path, project_root: &Path) -> &'static str {
    if uses_next_intl(package_dir, project_root) {
        "0.0.0.0"
    } else {
        LOOPBACK_HOST
    }
}

/// Detection only: a folder or package.json Localify cannot read counts as
/// no next-intl, so it never stops a launch.
fn uses_next_intl(package_dir: &Path, project_root: &Path) -> bool {
    let (Ok(package_dir), Ok(project_root)) =
        (package_dir.canonicalize(), project_root.canonicalize())
    else {
        return false;
    };
    if !package_dir.starts_with(&project_root) {
        return lists_next_intl(&package_dir);
    }
    package_dir
        .ancestors()
        .take_while(|dir| dir.starts_with(&project_root))
        .any(lists_next_intl)
}

fn lists_next_intl(dir: &Path) -> bool {
    let Ok(content) = fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    ["dependencies", "devDependencies"].iter().any(|field| {
        manifest
            .get(field)
            .and_then(|deps| deps.get("next-intl"))
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ProxyTarget;

    const PORT: u16 = 4321;

    /// A service with a configured command, in its own checkout, served on
    /// [`PORT`]: what `localify dev` detects there replaces the command.
    fn service(root: &Path) -> ConfiguredService {
        ConfiguredService {
            id: "web".into(),
            name: "web".into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: Some(root.display().to_string()),
            primary_domain: "web.demo.localify".into(),
            domains: vec!["web.demo.localify".into()],
            path: None,
            command: Some(vec!["pnpm".into(), "dev".into()]),
            primary_target: Some(RouteTarget::Proxy(
                ProxyTarget::parse(&PORT.to_string()).unwrap(),
            )),
            inferred: false,
        }
    }

    fn plan(lockfile: &str, manifest: &str) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), manifest).unwrap();
        fs::write(dir.path().join(lockfile), "").unwrap();
        dev_plan(service(dir.path()), PORT).unwrap().argv
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    /// The argv `localify dev` ran before the plan had its own module, for
    /// each package manager and each framework it prepares.
    #[test]
    fn each_package_manager_and_framework_gets_the_argv_it_always_had() {
        let next = r#"{"scripts":{"dev":"next dev"}}"#;
        let next_intl = r#"{"scripts":{"dev":"next dev"},"dependencies":{"next-intl":"^4.0.0"}}"#;
        let vite = r#"{"scripts":{"dev":"vite"}}"#;
        let astro = r#"{"scripts":{"dev":"astro dev"}}"#;
        let nuxt = r#"{"scripts":{"dev":"nuxt dev"}}"#;
        let email = r#"{"scripts":{"dev":"email dev"}}"#;
        let pinned = r#"{"scripts":{"dev":"next dev --hostname 127.0.0.1 --port 4321"}}"#;
        let other = r#"{"scripts":{"dev":"node server.js"}}"#;
        let next_args = ["--hostname", "127.0.0.1", "--port", "4321"];
        let host = ["--host", "127.0.0.1", "--port", "4321"];
        let with = |command: &[&str], args: &[&str]| argv(&[command, args].concat());
        let cases = [
            (
                "package-lock.json",
                next,
                with(&["npm", "run", "dev", "--"], &next_args),
            ),
            (
                "pnpm-lock.yaml",
                next,
                with(&["pnpm", "run", "dev"], &next_args),
            ),
            ("bun.lock", next, with(&["bun", "run", "dev"], &next_args)),
            ("yarn.lock", next, with(&["yarn", "dev"], &next_args)),
            (
                "pnpm-lock.yaml",
                next_intl,
                with(
                    &["pnpm", "run", "dev"],
                    &["--hostname", "0.0.0.0", "--port", "4321"],
                ),
            ),
            (
                "package-lock.json",
                vite,
                with(&["npm", "run", "dev", "--"], &host),
            ),
            ("pnpm-lock.yaml", vite, with(&["pnpm", "run", "dev"], &host)),
            ("bun.lockb", astro, with(&["bun", "run", "dev"], &host)),
            ("yarn.lock", nuxt, with(&["yarn", "dev"], &host)),
            (
                "pnpm-lock.yaml",
                email,
                with(&["pnpm", "run", "dev"], &["--port", "4321"]),
            ),
            ("pnpm-lock.yaml", pinned, argv(&["pnpm", "run", "dev"])),
            ("package-lock.json", other, argv(&["npm", "run", "dev"])),
        ];

        for (lockfile, manifest, expected) in cases {
            assert_eq!(plan(lockfile, manifest), expected, "{lockfile} {manifest}");
        }
    }

    #[test]
    fn the_plan_names_what_localify_added() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"packageManager":"npm@10.0.0","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();

        let plan = dev_plan(service(dir.path()), PORT).unwrap();

        assert_eq!(
            plan.injected,
            ["--", "--host", "127.0.0.1", "--port", "4321"]
        );
        assert_eq!(plan.workdir, dir.path());
        assert_eq!(plan.project_root, dir.path());
    }

    #[test]
    fn an_override_runs_as_given() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        let command = argv(&["npm", "run", "dev"]);

        let plan = resolve(
            &service(dir.path()),
            LaunchCommand::Override(command.clone()),
            PORT,
        )
        .unwrap();

        assert_eq!(plan.argv, command);
        assert!(plan.injected.is_empty());
    }

    #[test]
    fn a_missing_folder_has_no_plan() {
        let dir = tempfile::tempdir().unwrap();

        assert!(dev_plan(service(&dir.path().join("gone")), PORT).is_err());
    }
}
