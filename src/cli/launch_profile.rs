//! `localify dev --profile`: picks a launch profile, guards dangerous ones,
//! and reports what was loaded without ever printing a value.

use std::io::{self, IsTerminal};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use localify::infrastructure::launch_sessions::{LaunchSessionDto, unix_now};

use super::service_supervisor::SessionReporter;
use crate::branding::BRANDING;
use crate::domain::launch_profile::RuntimeVariables;
use crate::infrastructure::launch_profiles::{
    self, InfisicalCli, LaunchProfiles, LaunchRequest, ProtonPassCli, ResolvedProfile,
    SourceStatus, launch_profiles_file_name,
};
use crate::infrastructure::output_masking::secret_patterns;

/// Set on the service to the launch profile name.
pub const PROFILE_ENV: &str = "LOCALIFY_LAUNCH_PROFILE";
/// Set to `1` on the service when the launch profile is dangerous.
pub const DANGER_ENV: &str = "LOCALIFY_DANGER";

#[derive(Debug, Default, Clone)]
pub struct LaunchProfileArgs {
    pub profile: Option<String>,
    pub env_files: Vec<PathBuf>,
    pub confirm_profile: Option<String>,
    pub no_masking: bool,
}

/// A resolved launch profile, ready to apply to the service process.
pub struct PreparedProfile {
    pub resolved: ResolvedProfile,
    mask_output: bool,
}

impl PreparedProfile {
    pub fn name(&self) -> Option<&str> {
        self.resolved.name.as_ref().map(|name| name.as_str())
    }

    pub fn is_dangerous(&self) -> bool {
        self.resolved.danger
    }

    pub fn max_duration(&self) -> Option<Duration> {
        self.resolved.max_duration
    }

    /// Byte patterns to hide in service output, or `None` to pass the output
    /// through untouched (no Proton Pass or Infisical value, or
    /// `--no-masking`).
    pub fn mask_patterns(&self) -> Option<Vec<Vec<u8>>> {
        self.mask_output
            .then(|| secret_patterns(self.resolved.secret_values()))
            .filter(|patterns| !patterns.is_empty())
    }

    /// Source labels with key counts, for the terminal and the audit log.
    pub fn source_summaries(&self) -> Vec<String> {
        self.resolved
            .sources
            .iter()
            .map(|source| match &source.status {
                SourceStatus::Loaded { keys } => format!("{}: {}", source.label, key_count(*keys)),
                SourceStatus::Missing { reason } => format!("{}: missing ({reason})", source.label),
            })
            .collect()
    }
}

/// Variables set only when nothing else defines them. Output piped through
/// the masker is not a terminal, so ask tools to keep their colors.
pub fn output_defaults(profile: Option<&PreparedProfile>) -> Vec<(String, String)> {
    let masked = profile.is_some_and(|profile| profile.mask_patterns().is_some());
    if masked && io::stdout().is_terminal() {
        vec![("FORCE_COLOR".to_string(), "1".to_string())]
    } else {
        Vec::new()
    }
}

/// Profile markers Localify owns on the service, set or cleared on every
/// launch so a value inherited from a parent launch never leaks through.
pub fn profile_runtime_variables(profile: Option<&PreparedProfile>) -> RuntimeVariables {
    let mut variables = RuntimeVariables::default();
    match profile.and_then(PreparedProfile::name) {
        Some(name) => variables.set.push((PROFILE_ENV.into(), name.into())),
        None => variables.unset.push(PROFILE_ENV.into()),
    }
    if profile.is_some_and(PreparedProfile::is_dangerous) {
        variables.set.push((DANGER_ENV.into(), "1".into()));
    } else {
        variables.unset.push(DANGER_ENV.into());
    }
    variables
}

/// Loads the launch profile for a service, checks that a dangerous one was
/// confirmed when no terminal is attached, and resolves its sources.
///
/// Returns `None` when neither a profile nor `--env-file` applies, so the
/// service launches exactly as it did before launch profiles existed.
pub fn prepare(
    args: &LaunchProfileArgs,
    project_root: &Path,
    service_dir: &Path,
    current_dir: &Path,
) -> Result<Option<PreparedProfile>> {
    let profiles = LaunchProfiles::load(project_root)?;
    let profile = match (&profiles, args.profile.as_deref()) {
        (Some(profiles), requested) => profiles.select(requested)?,
        (None, Some(requested)) => bail!(
            "--profile {requested} needs launch profiles, but '{}' does not exist.\n\
             Declare profiles in that file, for example:\n\n\
             [profiles.{requested}]\n\
             sources = [{{ file = \".env.{requested}\" }}]\n\n\
             Or, if the project's variables live in Infisical, run 'infisical init' in that folder: default, staging and prod then read its environments.",
            project_root.join(launch_profiles_file_name()).display()
        ),
        (None, None) => None,
    };
    if profile.is_none() && args.env_files.is_empty() {
        return Ok(None);
    }

    if let Some(profile) = profile.filter(|profile| profile.danger) {
        check_dangerous_launch(
            profile.name.as_str(),
            args.confirm_profile.as_deref(),
            is_interactive(),
        )?;
    }

    let env_files: Vec<PathBuf> = args
        .env_files
        .iter()
        .map(|path| current_dir.join(path))
        .collect();
    let service_path = service_path(project_root, service_dir);
    let resolved = launch_profiles::resolve(
        &LaunchRequest {
            profile,
            env_files: &env_files,
            project_root,
            service_dir,
            service_path: &service_path,
        },
        &ProtonPassCli,
        &InfisicalCli,
    )?;

    let mask_output = !args.no_masking && !resolved.secret_values().is_empty();
    Ok(Some(PreparedProfile {
        resolved,
        mask_output,
    }))
}

/// Service folder relative to the project root with `/` separators, empty
/// for a service at the root. Used to expand `{service_path}` in note titles.
fn service_path(project_root: &Path, service_dir: &Path) -> String {
    let Ok(relative) = service_dir.strip_prefix(project_root) else {
        return String::new();
    };
    relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// In a terminal, `--profile <name>` is explicit enough: the red banner, tab
/// prefix and badge follow. Without a terminal (agents, scripts, CI),
/// `--confirm-profile <name>` is required so automation cannot start a
/// dangerous profile by accident.
fn check_dangerous_launch(name: &str, confirm_flag: Option<&str>, interactive: bool) -> Result<()> {
    match confirm_flag {
        Some(confirmation) if confirmation == name => Ok(()),
        Some(confirmation) => bail!(
            "--confirm-profile must repeat the profile name '{name}', got '{confirmation}'. The service was not started."
        ),
        None if interactive => Ok(()),
        None => bail!(
            "Profile '{name}' is marked danger = true and no terminal is attached.\n\
             Re-run with --confirm-profile {name} to start it anyway."
        ),
    }
}

fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// What a launch session is about, for the daemon and the audit log.
pub struct SessionTarget {
    pub project_id: String,
    pub environment_slug: Option<String>,
    /// Service id, or a label such as `packages/database: drizzle-kit` for
    /// `localify exec`.
    pub service_id: String,
    /// Loopback port the service listens on; 0 for a command without one.
    pub target_port: u16,
    pub browser_indicator: bool,
}

/// Registers the launch with the daemon for the `X-Localify-Profile` header,
/// the desktop badge and the audit log. Mandatory for dangerous profiles.
pub fn start_launch_session(
    profile: &PreparedProfile,
    target: SessionTarget,
    socket_path: &Path,
) -> Result<Option<SessionReporter>> {
    let Some(name) = profile.name() else {
        return Ok(None);
    };
    let started_at = unix_now();
    let session = LaunchSessionDto {
        id: format!("{}-{started_at}", std::process::id()),
        project_id: target.project_id,
        environment_slug: target.environment_slug,
        service_id: target.service_id,
        profile: name.to_string(),
        danger: profile.is_dangerous(),
        target_port: target.target_port,
        started_at,
        expires_at: profile
            .max_duration()
            .map(|duration| started_at + duration.as_secs()),
        sources: profile.source_summaries(),
        browser_indicator: target.browser_indicator,
    };

    match SessionReporter::start(session, socket_path) {
        Ok(reporter) => Ok(Some(reporter)),
        Err(error) if profile.is_dangerous() => bail!(
            "Profile '{name}' is marked danger = true, which needs the {app} daemon for the X-Localify-Profile header, the desktop badge and the audit log.\n\
             Start it with '{cli} start' or open the desktop app, then try again. If it is already running an older version, run '{cli} restart'.\n\
             Details: {error:#}",
            app = BRANDING.app_name,
            cli = BRANDING.cli_command(),
        ),
        Err(error) => {
            let reason = error.to_string();
            let reason = reason.lines().next().unwrap_or_default();
            eprintln!(
                "  Note: the X-Localify-Profile header and desktop badge are unavailable: {reason}"
            );
            Ok(None)
        }
    }
}

/// Prints the profile, its sources with key counts and every warning.
pub fn print_summary(profile: &PreparedProfile) {
    let heading = match profile.name() {
        Some(name) if profile.is_dangerous() => format!("  launch profile: {name} (danger)"),
        Some(name) => format!("  launch profile: {name}"),
        None => "  launch profile: none (--env-file only)".into(),
    };
    eprintln!("{heading}");
    for source in &profile.resolved.sources {
        if let SourceStatus::Loaded { keys } = source.status {
            eprintln!("    {}: {}", source.label, key_count(keys));
        }
        for warning in &source.warnings {
            eprintln!("    Warning: {}: {warning}", source.label);
        }
    }
    for problem in profile.resolved.problems() {
        eprintln!(
            "  Warning: {problem}. The service starts without it; the framework's own env files still apply."
        );
    }
    if profile.mask_output {
        eprintln!(
            "  output: Proton Pass and Infisical values are masked (use --no-masking to disable)"
        );
    }
}

/// Keys the user may be surprised to see ignored. Names only.
pub fn print_conflicts(shadowed_by_shell: &[String], replaced_by_runtime: &[String]) {
    if !shadowed_by_shell.is_empty() {
        eprintln!(
            "  Warning: your shell already exports {}; the shell value wins over the profile.",
            shadowed_by_shell.join(", ")
        );
    }
    if !replaced_by_runtime.is_empty() {
        eprintln!(
            "  {} rewrites {} so the service keeps its local domain.",
            BRANDING.app_name,
            replaced_by_runtime.join(", ")
        );
    }
}

/// Red banner shown before a dangerous profile starts.
pub fn print_danger_banner(profile: &PreparedProfile, service_label: &str) {
    let name = profile.name().unwrap_or_default();
    let expiry = match profile.max_duration() {
        Some(duration) => format!(
            "stops automatically after {}",
            humantime::format_duration(duration)
        ),
        None => "no time limit".into(),
    };
    let lines = [
        format!(" DANGER  profile '{name}' on {service_label} "),
        format!(" {expiry}; the app sees {DANGER_ENV}=1 "),
    ];
    let width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    eprintln!();
    for line in lines {
        eprintln!("{}", red_banner_line(&format!("{line:<width$}")));
    }
    eprintln!();
}

/// Terminal title such as `[PROD] web`, so the tab stays recognizable.
pub fn set_danger_title(profile_name: &str, service_label: &str) {
    if io::stderr().is_terminal() {
        let title = format!("[{}] {service_label}", profile_name.to_uppercase());
        eprint!("\x1b]0;{title}\x07");
    }
}

pub fn reset_title() {
    if io::stderr().is_terminal() {
        eprint!("\x1b]0;\x07");
    }
}

fn red_banner_line(text: &str) -> String {
    if io::stderr().is_terminal() {
        format!("\x1b[1;97;41m{text}\x1b[0m")
    } else {
        format!("!!{text}!!")
    }
}

fn key_count(keys: usize) -> String {
    if keys == 1 {
        "1 key".into()
    } else {
        format!("{keys} keys")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_interactive_dangerous_launch_requires_the_confirm_flag() {
        let error = check_dangerous_launch("prod", None, false)
            .unwrap_err()
            .to_string();

        assert!(error.contains("--confirm-profile prod"));
    }

    #[test]
    fn confirm_flag_must_repeat_the_profile_name() {
        assert!(check_dangerous_launch("prod", Some("prod"), false).is_ok());

        let error = check_dangerous_launch("prod", Some("yes"), false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must repeat the profile name 'prod'"));
    }

    #[test]
    fn interactive_dangerous_launch_starts_without_a_prompt() {
        assert!(check_dangerous_launch("prod", None, true).is_ok());
    }

    #[test]
    fn profile_markers_are_set_or_cleared() {
        let none = profile_runtime_variables(None);

        assert!(none.set.is_empty());
        assert_eq!(
            none.unset,
            vec![PROFILE_ENV.to_string(), DANGER_ENV.to_string()]
        );
    }

    #[test]
    fn service_path_is_relative_to_the_project_root() {
        let root = Path::new("/repo");

        assert_eq!(service_path(root, Path::new("/repo/apps/web")), "apps/web");
        assert_eq!(service_path(root, Path::new("/repo")), "");
        assert_eq!(service_path(root, Path::new("/elsewhere/web")), "");
    }

    #[test]
    fn requesting_a_profile_without_localify_toml_explains_how_to_add_one() {
        let dir = tempfile::tempdir().unwrap();
        let args = LaunchProfileArgs {
            profile: Some("prod".into()),
            ..LaunchProfileArgs::default()
        };

        let error = match prepare(&args, dir.path(), dir.path(), dir.path()) {
            Ok(_) => panic!("expected an error"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("localify.toml"));
        assert!(error.contains("[profiles.prod]"));
    }

    #[test]
    fn no_profile_file_and_no_flags_keeps_the_previous_behavior() {
        let dir = tempfile::tempdir().unwrap();

        let prepared = prepare(
            &LaunchProfileArgs::default(),
            dir.path(),
            dir.path(),
            dir.path(),
        )
        .unwrap();

        assert!(prepared.is_none());
    }

    #[test]
    fn summaries_show_sources_and_counts_but_never_values() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        std::fs::write(
            dir.path().join("localify.toml"),
            "[profiles.staging]\nsources = [{ file = \".env.staging\" }, { file = \".env.absent\" }]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("apps/web/.env.staging"),
            "DATABASE_URL=postgres://user:hunter2-secret@db\nBROKEN hunter2-secret\n",
        )
        .unwrap();
        let args = LaunchProfileArgs {
            profile: Some("staging".into()),
            ..LaunchProfileArgs::default()
        };

        let prepared = prepare(&args, dir.path(), &dir.path().join("apps/web"), dir.path())
            .unwrap()
            .unwrap();
        let rendered = format!(
            "{:?}\n{:?}\n{:?}",
            prepared.source_summaries(),
            prepared.resolved.problems(),
            prepared.resolved.sources
        );

        assert!(rendered.contains("file .env.staging: 1 key"));
        assert!(rendered.contains("file .env.absent: missing"));
        assert!(!rendered.contains("hunter2"));
        assert!(prepared.mask_patterns().is_none());
    }

    #[test]
    fn dangerous_profile_is_refused_without_confirmation_in_non_interactive_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("localify.toml"),
            "[profiles.prod]\ndanger = true\n",
        )
        .unwrap();
        let args = LaunchProfileArgs {
            profile: Some("prod".into()),
            ..LaunchProfileArgs::default()
        };

        // Test processes have no terminal on stdin, like agents and CI.
        if is_interactive() {
            return;
        }
        let error = match prepare(&args, dir.path(), dir.path(), dir.path()) {
            Ok(_) => panic!("expected the launch to be refused"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("--confirm-profile prod"));

        let confirmed = LaunchProfileArgs {
            confirm_profile: Some("prod".into()),
            ..args
        };
        let prepared = prepare(&confirmed, dir.path(), dir.path(), dir.path())
            .unwrap()
            .unwrap();
        assert!(prepared.is_dangerous());
    }
}
