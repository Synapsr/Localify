use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::branding::BRANDING;

const LOCALIFY_SKILL: &str = include_str!("../skills/localify/SKILL.md");

#[derive(Debug, Clone)]
struct ResolvedCommand {
    program: PathBuf,
    path_env: OsString,
}

#[derive(Debug, Clone)]
pub struct AgentSkillAgent {
    pub id: &'static str,
    pub label: &'static str,
    pub detected: bool,
    pub installed: bool,
    pub skills_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AgentSkillStatus {
    pub skill_dir: PathBuf,
    pub npx_available: bool,
    pub detected_agents: Vec<AgentSkillAgent>,
    pub install_command: String,
}

#[derive(Debug, Clone)]
pub struct AgentSkillInstallResult {
    pub status: AgentSkillStatus,
    pub installed_agents: Vec<String>,
    pub stdout: String,
    pub stderr: String,
}

pub fn agent_skill_status() -> Result<AgentSkillStatus> {
    let skill_dir = bundled_skill_dir()?;
    write_bundled_skill(&skill_dir)?;
    let npx_command = resolve_command("npx");
    let detected_agents = detect_agent_skills_dirs()?;
    let selected_agents = detected_agents
        .iter()
        .filter(|agent| agent.detected)
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    let install_command = skill_install_command(&skill_dir, &selected_agents);

    Ok(AgentSkillStatus {
        skill_dir,
        npx_available: npx_command.is_some(),
        detected_agents,
        install_command,
    })
}

pub fn install_agent_skill() -> Result<AgentSkillInstallResult> {
    let status = agent_skill_status()?;
    let agents = status
        .detected_agents
        .iter()
        .filter(|agent| agent.detected)
        .collect::<Vec<_>>();
    let selected_agent_ids = agents.iter().map(|agent| agent.id).collect::<Vec<_>>();

    if agents.is_empty() {
        bail!(
            "No supported coding agents were detected. Install Codex, Claude Code, or Cursor first, then retry."
        );
    }
    if !status.npx_available {
        bail!(
            "Node.js npx is required to install the Localify Agent Skill.\nCommand: {}",
            status.install_command
        );
    }

    write_bundled_skill(&status.skill_dir)?;
    for agent in &agents {
        fs::create_dir_all(&agent.skills_dir).with_context(|| {
            format!(
                "Failed to create {} skills directory '{}'",
                agent.label,
                agent.skills_dir.display()
            )
        })?;
    }

    let Some(npx_command) = resolve_command("npx") else {
        bail!(
            "Node.js npx is required to install the Localify Agent Skill.\nCommand: {}",
            status.install_command
        );
    };

    let output = Command::new(&npx_command.program)
        .env("PATH", &npx_command.path_env)
        .arg("--yes")
        .arg("skills")
        .arg("add")
        .arg(&status.skill_dir)
        .arg("--global")
        .arg("--skill")
        .arg("localify")
        .arg("--yes")
        .arg("--agent")
        .args(&selected_agent_ids)
        .output()
        .context("Failed to run npx skills add")?;

    if !output.status.success() {
        bail!(
            "Failed to install the Localify Agent Skill.\n{}",
            command_failure_message(&output)
        );
    }

    let updated_status = agent_skill_status()?;
    let missing_agents = updated_status
        .detected_agents
        .iter()
        .filter(|agent| selected_agent_ids.contains(&agent.id) && !agent.installed)
        .map(|agent| agent.label)
        .collect::<Vec<_>>();
    if !missing_agents.is_empty() {
        bail!(
            "The Localify Agent Skill could not be verified for {}.",
            missing_agents.join(", ")
        );
    }

    let installed_agents = updated_status
        .detected_agents
        .iter()
        .filter(|agent| selected_agent_ids.contains(&agent.id) && agent.installed)
        .map(|agent| agent.id.to_string())
        .collect::<Vec<_>>();

    Ok(AgentSkillInstallResult {
        status: updated_status,
        installed_agents,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn write_bundled_skill(skill_dir: &Path) -> Result<()> {
    fs::create_dir_all(skill_dir).with_context(|| {
        format!(
            "Failed to create bundled skill directory '{}'",
            skill_dir.display()
        )
    })?;
    fs::write(skill_dir.join("SKILL.md"), LOCALIFY_SKILL).with_context(|| {
        format!(
            "Failed to write bundled Localify skill to '{}'",
            skill_dir.display()
        )
    })
}

fn bundled_skill_dir() -> Result<PathBuf> {
    Ok(local_state_dir()?.join("skills").join("localify"))
}

fn local_state_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(user_home_dir()?
            .join("Library/Application Support")
            .join(BRANDING.desktop_state_dir_name()))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let base = match env::var_os("XDG_STATE_HOME") {
            Some(path) => PathBuf::from(path),
            None => user_home_dir()?.join(".local/state"),
        };
        Ok(base.join(BRANDING.slug()))
    }
}

fn detect_agent_skills_dirs() -> Result<Vec<AgentSkillAgent>> {
    let home = user_home_dir()?;
    let shared_skill_installed = skill_installed(&home.join(".agents/skills"));
    let candidates = [
        (
            "codex",
            "Codex",
            home.join(".codex"),
            home.join(".codex/skills"),
        ),
        (
            "claude-code",
            "Claude Code",
            home.join(".claude"),
            home.join(".claude/skills"),
        ),
        (
            "cursor",
            "Cursor",
            home.join(".cursor"),
            home.join(".cursor/skills"),
        ),
    ];

    Ok(candidates
        .into_iter()
        .map(|(id, label, root_dir, skills_dir)| AgentSkillAgent {
            id,
            label,
            detected: root_dir.exists() || skills_dir.exists(),
            installed: skill_installed(&skills_dir) || shared_skill_installed,
            skills_dir,
        })
        .collect())
}

fn skill_installed(skills_dir: &Path) -> bool {
    skills_dir.join("localify").join("SKILL.md").exists()
}

fn user_home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("HOME is not set for the current user."))
}

fn resolve_command(command: &str) -> Option<ResolvedCommand> {
    for path_env in command_path_candidates() {
        let Some(program) = find_executable_in_path(command, &path_env) else {
            continue;
        };
        if command_runs(&program, &path_env) {
            return Some(ResolvedCommand { program, path_env });
        }
    }
    None
}

fn command_runs(program: &Path, path_env: &OsStr) -> bool {
    Command::new(program)
        .env("PATH", path_env)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command_path_candidates() -> Vec<OsString> {
    let mut candidates = Vec::new();
    push_path_candidate(&mut candidates, env::var_os("PATH"));

    #[cfg(target_os = "macos")]
    push_path_candidate(&mut candidates, login_shell_path_env());

    push_path_candidate(
        &mut candidates,
        Some(OsString::from(
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )),
    );

    candidates
}

fn push_path_candidate(candidates: &mut Vec<OsString>, path_env: Option<OsString>) {
    let Some(path_env) = path_env else {
        return;
    };
    if path_env.is_empty() || candidates.iter().any(|candidate| candidate == &path_env) {
        return;
    }
    candidates.push(path_env);
}

/// An interactive shell takes over the terminal it finds. Run from a terminal, that
/// stops whatever else is using it, so this shell gets a session with no terminal.
#[cfg(target_os = "macos")]
fn detach_from_terminal(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: the closure runs in the child between fork and exec. `setsid` is
    // async-signal-safe, and the closure allocates nothing and shares no state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(target_os = "macos")]
fn login_shell_path_env() -> Option<OsString> {
    const PATH_START: &str = "__LOCALIFY_PATH_START__";
    const PATH_END: &str = "__LOCALIFY_PATH_END__";

    let shell = env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    let mut command = Command::new(shell);
    command
        .arg("-lic")
        .arg(format!("printf '{}%s{}' \"$PATH\"", PATH_START, PATH_END));
    detach_from_terminal(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let start = stdout.find(PATH_START)? + PATH_START.len();
    let end = stdout[start..].find(PATH_END)? + start;
    let path_env = stdout[start..end].trim().to_string();
    if path_env.is_empty() {
        None
    } else {
        Some(OsString::from(path_env))
    }
}

fn find_executable_in_path(command: &str, path_env: &OsStr) -> Option<PathBuf> {
    env::split_paths(path_env)
        .map(|directory| directory.join(command))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn skill_install_command(skill_dir: &Path, agents: &[&str]) -> String {
    let mut parts = vec![
        "npx".to_string(),
        "--yes".into(),
        "skills".into(),
        "add".into(),
        shell_escape(&skill_dir.display().to_string()),
        "--global".into(),
        "--skill".into(),
        "localify".into(),
        "--yes".into(),
    ];
    if !agents.is_empty() {
        parts.push("--agent".into());
        for agent in agents {
            parts.push((*agent).into());
        }
    }
    parts.join(" ")
}

fn shell_escape(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '='))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn command_failure_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    format!("Command exited with status {}", output.status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn a_detached_shell_gets_its_own_session() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo $$; ps -o pgid= -p $$"]);
        detach_from_terminal(&mut command);

        let output = command.output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let ids: Vec<&str> = text.split_whitespace().collect();

        assert_eq!(ids.len(), 2, "{text}");
        assert_eq!(ids[0], ids[1]);
    }

    #[test]
    fn skill_install_command_accepts_npx_package_prompt() {
        let command = skill_install_command(
            Path::new("/Users/test/Library/Application Support/Localify/skills/localify"),
            &["codex", "claude-code"],
        );

        assert!(command.starts_with("npx --yes skills add "));
        assert!(command.contains("--agent codex claude-code"));
        assert_eq!(command.matches("--agent").count(), 1);
    }

    #[test]
    fn find_executable_in_path_returns_executable_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let command_path = temp_dir.path().join("npx");
        std::fs::write(&command_path, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&command_path);

        let path_env = temp_dir.path().as_os_str().to_os_string();

        assert_eq!(
            find_executable_in_path("npx", &path_env),
            Some(command_path)
        );
    }

    #[test]
    fn command_runs_with_resolved_path_environment() {
        let temp_dir = tempfile::tempdir().unwrap();
        let node_path = temp_dir.path().join("node");
        let npx_path = temp_dir.path().join("npx");
        std::fs::write(&node_path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(&npx_path, "#!/bin/sh\ncommand -v node >/dev/null\n").unwrap();
        make_executable(&node_path);
        make_executable(&npx_path);

        assert!(command_runs(&npx_path, temp_dir.path().as_os_str()));
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}
