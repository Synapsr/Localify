//! `localify exec`: runs a command that is not a service (a database studio,
//! a migration, a worker with its own port) with the variables of a launch
//! profile, from the current folder.

use std::env;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use super::context::effective_current_dir;
use super::launch_profile::{self, LaunchProfileArgs, PreparedProfile, SessionTarget};
use super::service_supervisor::{self, LaunchOutcome, SupervisedLaunch};
use crate::domain::launch_profile::{LaunchEnvironment, layer_environment};
use crate::infrastructure::launch_profiles::{infisical_project_file, launch_profiles_file_name};
use crate::infrastructure::paths::LocalifyPaths;
use crate::infrastructure::project_marker::read_project_marker;

pub struct ExecArgs {
    pub launch_profile: LaunchProfileArgs,
    pub command: Vec<String>,
}

/// Runs the command and returns its exit code, so `localify exec` can be
/// used in scripts exactly like the command itself.
pub fn run(args: ExecArgs, paths: &LocalifyPaths) -> Result<i32> {
    let current_dir = effective_current_dir()?;
    let project_root = find_profiles_root(&current_dir).unwrap_or_else(|| current_dir.clone());
    let profile = launch_profile::prepare(
        &args.launch_profile,
        &project_root,
        &current_dir,
        &current_dir,
    )?;

    let command_label = command_label(&args.command, &project_root, &current_dir);
    eprintln!("Running {command_label}");
    if let Some(profile) = &profile {
        launch_profile::print_summary(profile);
    }
    let environment = command_environment(profile.as_ref());
    if profile.is_some() {
        launch_profile::print_conflicts(
            &environment.shadowed_by_shell,
            &environment.replaced_by_runtime,
        );
    }

    let Some(profile) = profile else {
        return run_plain(&args.command, &current_dir, &environment);
    };
    // A command has no domain to mark or badge to show: only a dangerous
    // profile needs the daemon, for its audit log.
    let session = if profile.is_dangerous() {
        launch_profile::start_launch_session(
            &profile,
            SessionTarget {
                project_id: project_id(&project_root),
                environment_slug: None,
                service_id: command_label.clone(),
                target_port: 0,
                browser_indicator: false,
            },
            &paths.socket_path,
        )?
    } else {
        None
    };
    run_with_profile(
        &profile,
        &args.command,
        &current_dir,
        &environment,
        &command_label,
        session,
    )
}

fn run_with_profile(
    profile: &PreparedProfile,
    command: &[String],
    current_dir: &Path,
    environment: &LaunchEnvironment,
    command_label: &str,
    session: Option<service_supervisor::SessionReporter>,
) -> Result<i32> {
    let dangerous_name = profile.name().filter(|_| profile.is_dangerous());
    if let Some(name) = dangerous_name {
        launch_profile::print_danger_banner(profile, command_label);
        launch_profile::set_danger_title(name, command_label);
    }

    let outcome = service_supervisor::run(SupervisedLaunch {
        command,
        workdir: current_dir,
        environment,
        mask_patterns: profile.mask_patterns(),
        stream: None,
        max_duration: profile.max_duration(),
        session,
        heartbeat: &|| {},
        leads_session: false,
    });
    if dangerous_name.is_some() {
        launch_profile::reset_title();
    }

    Ok(match outcome?.outcome {
        LaunchOutcome::Exited(status) => exit_code(status),
        LaunchOutcome::Interrupted => 130,
        LaunchOutcome::Expired(duration) => {
            eprintln!(
                "Stopped: launch profile '{}' reached its max_duration of {}.",
                profile.name().unwrap_or_default(),
                humantime::format_duration(duration)
            );
            1
        }
    })
}

fn run_plain(
    command: &[String],
    current_dir: &Path,
    environment: &LaunchEnvironment,
) -> Result<i32> {
    let program = &command[0];
    let mut child = Command::new(program);
    child.args(&command[1..]).current_dir(current_dir);
    for key in &environment.unset {
        child.env_remove(key);
    }
    child.envs(environment.set.iter().map(|(key, value)| (key, value)));
    let status = child
        .status()
        .with_context(|| format!("Failed to start command '{program}'"))?;
    Ok(exit_code(status))
}

/// Shell priority without Localify routing: the command is not a service, so
/// only the profile markers are owned by Localify.
fn command_environment(profile: Option<&PreparedProfile>) -> LaunchEnvironment {
    let profile_variables = profile
        .map(|profile| profile.resolved.pairs())
        .unwrap_or_default();
    layer_environment(
        &launch_profile::profile_runtime_variables(profile),
        |key| env::var_os(key).is_some(),
        &profile_variables,
        &launch_profile::output_defaults(profile),
    )
}

/// Nearest folder, from the current one upwards, that holds `localify.toml`.
/// Only without one anywhere does an Infisical project file count, for
/// implicit profiles: a nested `.infisical.json` must not replace declared
/// profiles and their safeguards.
fn find_profiles_root(current_dir: &Path) -> Option<PathBuf> {
    let file_name = launch_profiles_file_name();
    current_dir
        .ancestors()
        .find(|dir| dir.join(&file_name).is_file())
        .or_else(|| {
            current_dir
                .ancestors()
                .find(|dir| infisical_project_file(dir).is_some())
        })
        .map(Path::to_path_buf)
}

fn project_id(project_root: &Path) -> String {
    read_project_marker(project_root).unwrap_or_else(|| {
        project_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
            .to_string()
    })
}

/// `packages/database: drizzle-kit`, used in the terminal title and the
/// audit log.
fn command_label(command: &[String], project_root: &Path, current_dir: &Path) -> String {
    let program = command
        .first()
        .map(|program| {
            Path::new(program)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(program)
                .to_string()
        })
        .unwrap_or_default();
    match current_dir.strip_prefix(project_root) {
        Ok(folder) if !folder.as_os_str().is_empty() => format!("{}: {program}", folder.display()),
        _ => program,
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn finds_localify_toml_in_a_parent_folder() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("packages/database");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("localify.toml"), "").unwrap();

        assert_eq!(find_profiles_root(&nested).as_deref(), Some(dir.path()));
        assert_eq!(find_profiles_root(Path::new("/")), None);
    }

    #[test]
    fn declared_profiles_win_over_a_nested_infisical_link() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("packages/database");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(".infisical.json"), "{}").unwrap();
        assert_eq!(
            find_profiles_root(&nested).as_deref(),
            Some(nested.as_path())
        );

        fs::write(dir.path().join("localify.toml"), "").unwrap();
        assert_eq!(find_profiles_root(&nested).as_deref(), Some(dir.path()));
    }

    #[test]
    fn labels_commands_with_their_folder() {
        let root = Path::new("/repo");
        let command = vec!["/usr/bin/drizzle-kit".to_string(), "studio".to_string()];

        assert_eq!(
            command_label(&command, root, Path::new("/repo/packages/database")),
            "packages/database: drizzle-kit"
        );
        assert_eq!(command_label(&command, root, root), "drizzle-kit");
    }

    #[test]
    fn profile_variables_reach_the_command_below_the_shell() {
        let environment = command_environment(None);

        assert!(environment.set.is_empty());
        assert!(
            environment
                .unset
                .contains(&launch_profile::PROFILE_ENV.to_string())
        );
    }
}
