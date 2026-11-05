//! Connects coding agents to `localify mcp`, the MCP server that gives them every
//! Localify action. The server is the installed binary itself, so agents always
//! get the actions of the version that is installed: nothing to sync on update.
//!
//! Claude Code and Codex are configured through their own CLIs, which own their
//! config files. Cursor has no such CLI, so its `~/.cursor/mcp.json` is edited.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::branding::BRANDING;

const PROFILE_ENV: &str = "LOCALIFY_PROFILE";

#[derive(Debug, Clone)]
struct ResolvedCommand {
    program: PathBuf,
    path_env: OsString,
}

/// The server as agents start it.
#[derive(Debug, Clone)]
pub struct McpServer {
    /// Name agents list the server under, and prefix its tools with.
    pub name: String,
    pub binary: PathBuf,
    /// The dev build keeps its own config and domains, chosen by `LOCALIFY_PROFILE`.
    pub dev_profile: bool,
}

impl McpServer {
    pub fn new(binary: PathBuf) -> Self {
        Self {
            name: BRANDING.cli_command().to_string(),
            binary,
            dev_profile: BRANDING.is_dev_profile(),
        }
    }

    fn env_args(&self, flag: &str) -> Vec<String> {
        if self.dev_profile {
            vec![flag.to_string(), format!("{PROFILE_ENV}=dev")]
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::ClaudeCode, Agent::Codex, Agent::Cursor];

    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
        }
    }

    fn home_dir_name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => ".claude",
            Agent::Codex => ".codex",
            Agent::Cursor => ".cursor",
        }
    }

    fn cli(self) -> Option<&'static str> {
        match self {
            Agent::ClaudeCode => Some("claude"),
            Agent::Codex => Some("codex"),
            Agent::Cursor => None,
        }
    }

    /// `mcp add` arguments before `--`. The name comes before `-e`: Claude Code
    /// reads every word after `-e` as another variable.
    fn add_args(self, server: &McpServer) -> Vec<String> {
        let (mut args, env_flag) = match self {
            Agent::ClaudeCode => (argv(&["mcp", "add", "--scope", "user"]), "-e"),
            Agent::Codex | Agent::Cursor => (argv(&["mcp", "add"]), "--env"),
        };
        args.push(server.name.clone());
        args.extend(server.env_args(env_flag));
        args
    }

    /// The command a person can run to connect this agent by hand.
    pub fn manual_command(self, server: &McpServer) -> String {
        let binary = shell_escape(&server.binary.display().to_string());
        match self.cli() {
            Some(cli) => format!("{cli} {} -- {binary} mcp", self.add_args(server).join(" ")),
            None => format!(
                "Add \"{}\": {{ \"command\": \"{}\", \"args\": [\"mcp\"] }} under \"mcpServers\" in ~/.cursor/mcp.json",
                server.name,
                server.binary.display()
            ),
        }
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

#[derive(Debug, Clone)]
pub struct AgentMcpAgent {
    pub agent: Agent,
    pub detected: bool,
    pub installed: bool,
    pub manual_command: String,
}

#[derive(Debug, Clone)]
pub struct AgentMcpStatus {
    pub server: McpServer,
    pub agents: Vec<AgentMcpAgent>,
}

#[derive(Debug, Clone)]
pub struct AgentMcpInstallResult {
    pub status: AgentMcpStatus,
    pub installed_agents: Vec<Agent>,
}

/// Which agents are installed here and which already have the server.
pub fn agent_mcp_status(binary: &Path) -> Result<AgentMcpStatus> {
    let server = McpServer::new(binary.to_path_buf());
    let home = user_home_dir()?;
    let agents = Agent::ALL
        .into_iter()
        .map(|agent| AgentMcpAgent {
            agent,
            detected: home.join(agent.home_dir_name()).exists()
                || agent.cli().and_then(resolve_command).is_some(),
            installed: server_installed(agent, &home, &server.name),
            manual_command: agent.manual_command(&server),
        })
        .collect();
    Ok(AgentMcpStatus { server, agents })
}

/// Connects every detected agent to the server, replacing an older entry of the
/// same name so it points at this binary.
pub fn install_agent_mcp(binary: &Path) -> Result<AgentMcpInstallResult> {
    let status = agent_mcp_status(binary)?;
    let detected = status
        .agents
        .iter()
        .filter(|agent| agent.detected)
        .collect::<Vec<_>>();
    if detected.is_empty() {
        bail!(
            "No supported coding agent was found. Install Claude Code, Codex or Cursor, then retry."
        );
    }

    let home = user_home_dir()?;
    let mut installed = Vec::new();
    let mut failures = Vec::new();
    for agent in detected {
        match install_for(agent.agent, &home, &status.server) {
            Ok(()) => installed.push(agent.agent),
            Err(error) => failures.push(format!(
                "{}: {error:#}\n  Connect it by hand: {}",
                agent.agent.label(),
                agent.manual_command
            )),
        }
    }
    if !failures.is_empty() {
        let done = installed
            .iter()
            .map(|agent| agent.label())
            .collect::<Vec<_>>();
        bail!(
            "{}{}",
            if done.is_empty() {
                String::new()
            } else {
                format!("Connected {}. ", done.join(", "))
            },
            failures.join("\n")
        );
    }

    Ok(AgentMcpInstallResult {
        status: agent_mcp_status(binary)?,
        installed_agents: installed,
    })
}

fn install_for(agent: Agent, home: &Path, server: &McpServer) -> Result<()> {
    match agent {
        Agent::ClaudeCode => run_agent_cli(
            agent,
            &["mcp", "remove", "--scope", "user", &server.name],
            server,
        ),
        Agent::Codex => run_agent_cli(agent, &["mcp", "remove", &server.name], server),
        Agent::Cursor => write_cursor_server(&home.join(".cursor/mcp.json"), server),
    }
}

/// Removes the server's previous entry, if any, then adds it again.
fn run_agent_cli(agent: Agent, remove_args: &[&str], server: &McpServer) -> Result<()> {
    let cli = agent.cli().context("This agent has no CLI")?;
    let resolved =
        resolve_command(cli).ok_or_else(|| anyhow!("its `{cli}` command was not found in PATH"))?;
    let _ = Command::new(&resolved.program)
        .env("PATH", &resolved.path_env)
        .args(remove_args)
        .output();
    let output = Command::new(&resolved.program)
        .env("PATH", &resolved.path_env)
        .args(agent.add_args(server))
        .arg("--")
        .arg(&server.binary)
        .arg("mcp")
        .output()
        .with_context(|| format!("Failed to run {cli}"))?;
    if !output.status.success() {
        bail!("{cli} mcp add failed: {}", command_failure_message(&output));
    }
    Ok(())
}

fn write_cursor_server(path: &Path, server: &McpServer) -> Result<()> {
    let mut config = match fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => serde_json::from_str(&content)
            .with_context(|| format!("'{}' is not valid JSON", path.display()))?,
        Ok(_) => serde_json::json!({}),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read '{}'", path.display()));
        }
    };
    let root = config
        .as_object_mut()
        .with_context(|| format!("'{}' does not hold a JSON object", path.display()))?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .with_context(|| format!("mcpServers in '{}' is not an object", path.display()))?;
    let mut entry = serde_json::json!({
        "command": server.binary.display().to_string(),
        "args": ["mcp"],
    });
    if server.dev_profile {
        entry["env"] = serde_json::json!({ PROFILE_ENV: "dev" });
    }
    servers.insert(server.name.clone(), entry);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create '{}'", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(path, format!("{content}\n"))
        .with_context(|| format!("Failed to write '{}'", path.display()))
}

/// Reads the file each agent's own CLI writes to, which moves with the same
/// variables the CLI follows.
fn server_installed(agent: Agent, home: &Path, name: &str) -> bool {
    match agent {
        Agent::ClaudeCode => {
            let dir = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.to_path_buf());
            json_has_server(&dir.join(".claude.json"), name)
        }
        Agent::Codex => {
            let dir = env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
            toml_has_server(&dir.join("config.toml"), name)
        }
        Agent::Cursor => json_has_server(&home.join(".cursor/mcp.json"), name),
    }
}

fn env_dir(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn json_has_server(path: &Path, name: &str) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .is_some_and(|config| config["mcpServers"].get(name).is_some())
}

fn toml_has_server(path: &Path, name: &str) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| content.parse::<toml::Table>().ok())
        .and_then(|config| config.get("mcp_servers")?.as_table()?.get(name).cloned())
        .is_some()
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
    fn find_executable_in_path_returns_executable_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let command_path = temp_dir.path().join("claude");
        std::fs::write(&command_path, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&command_path);

        let path_env = temp_dir.path().as_os_str().to_os_string();

        assert_eq!(
            find_executable_in_path("claude", &path_env),
            Some(command_path)
        );
    }

    #[test]
    fn cursor_config_keeps_other_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"other"}},"theme":"dark"}"#,
        )
        .unwrap();
        let server = McpServer {
            name: "localify".into(),
            binary: PathBuf::from("/Applications/Localify.app/Contents/MacOS/localify"),
            dev_profile: false,
        };

        write_cursor_server(&path, &server).unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["mcpServers"]["other"]["command"], "other");
        assert_eq!(config["theme"], "dark");
        assert_eq!(
            config["mcpServers"]["localify"]["args"],
            serde_json::json!(["mcp"])
        );
        assert!(json_has_server(&path, "localify"));
    }

    #[test]
    fn codex_config_lists_its_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "model = \"o4\"\n[mcp_servers.localify]\ncommand = \"localify\"\nargs = [\"mcp\"]\n",
        )
        .unwrap();

        assert!(toml_has_server(&path, "localify"));
        assert!(!toml_has_server(&path, "other"));
    }

    #[test]
    fn manual_commands_name_the_server_and_binary() {
        let server = McpServer {
            name: "localify".into(),
            binary: PathBuf::from("/Applications/Localify.app/Contents/MacOS/localify"),
            dev_profile: false,
        };

        assert_eq!(
            Agent::ClaudeCode.manual_command(&server),
            "claude mcp add --scope user localify -- /Applications/Localify.app/Contents/MacOS/localify mcp"
        );
        assert_eq!(
            Agent::Codex.manual_command(&server),
            "codex mcp add localify -- /Applications/Localify.app/Contents/MacOS/localify mcp"
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
