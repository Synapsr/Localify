use std::collections::HashSet;
use std::path::PathBuf;

use crate::branding::BRANDING;

fn default_http_port() -> u16 {
    if BRANDING.is_dev_profile() {
        return 8080;
    }

    80
}

fn default_https_port() -> u16 {
    if BRANDING.is_dev_profile() {
        return 8443;
    }

    443
}

fn default_dns_port() -> u16 {
    if BRANDING.is_dev_profile() {
        return 1054;
    }

    1053
}

fn default_log_level() -> String {
    "info".to_string()
}

/// A service for a normal user listens on loopback unless they opt in. A daemon run as
/// root on Linux keeps listening on all interfaces, as it does in a container.
fn default_lan_access() -> bool {
    !BRANDING.installs_with_one_approval()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_http_port")]
    pub http_port: u16,

    #[serde(default = "default_https_port")]
    pub https_port: u16,

    #[serde(default = "default_dns_port")]
    pub dns_port: u16,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Answer other devices on the network, not only this machine. On macOS
    /// changing it rewrites the launchd plist, which takes one approval.
    #[serde(default = "default_lan_access")]
    pub lan_access: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            http_port: default_http_port(),
            https_port: default_https_port(),
            dns_port: default_dns_port(),
            log_level: default_log_level(),
            lan_access: default_lan_access(),
        }
    }
}

impl DaemonConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.http_port == 0 {
            return Err("http_port cannot be 0".into());
        }
        if self.https_port == 0 {
            return Err("https_port cannot be 0".into());
        }
        if self.dns_port == 0 {
            return Err("dns_port cannot be 0".into());
        }
        if self.http_port == self.https_port {
            return Err("http_port and https_port must be different".into());
        }

        let ports = [self.http_port, self.https_port, self.dns_port];
        let unique_ports: HashSet<_> = ports.iter().collect();
        if unique_ports.len() != ports.len() {
            return Err("http_port, https_port, and dns_port must all be different".into());
        }

        let valid_levels = ["error", "warn", "info", "debug"];
        if !valid_levels.contains(&self.log_level.as_str()) {
            return Err(format!(
                "Invalid log_level '{}'. Must be one of: {}",
                self.log_level,
                valid_levels.join(", ")
            ));
        }

        Ok(())
    }
}

fn default_data_dir() -> PathBuf {
    BRANDING.data_dir()
}

fn default_pid_file() -> PathBuf {
    BRANDING.pid_file()
}

fn default_log_file() -> PathBuf {
    BRANDING.log_file()
}

fn default_certs_dir() -> PathBuf {
    BRANDING.certs_dir()
}

fn default_socket_path() -> PathBuf {
    BRANDING.socket_path()
}

/// Docker integration configuration.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DockerConfig {
    /// Enable Docker auto-discovery of containers.
    #[serde(default)]
    pub enabled: bool,
}

fn default_agent_connect_agents() -> bool {
    true
}

fn default_agent_promote_dev_script() -> bool {
    false
}

/// Agent workflow preferences shared by CLI and desktop flows.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// Connect detected coding agents to `localify mcp` during setup. Read from
    /// `installSkill` too, its name when agents got a skill instead.
    #[serde(default = "default_agent_connect_agents", alias = "installSkill")]
    pub connect_agents: bool,

    /// Make Localify write and promote package dev scripts during project bootstrap.
    #[serde(default = "default_agent_promote_dev_script")]
    pub promote_dev_script: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            connect_agents: default_agent_connect_agents(),
            promote_dev_script: default_agent_promote_dev_script(),
        }
    }
}

/// Automatic updates (docs/desktop.md, "Automatic updates").
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UpdatesConfig {
    /// Install new versions without asking. A config without the key
    /// predates the feature and keeps updating by hand: `localify install`
    /// writes it on for a new install.
    #[serde(default)]
    pub automatic: bool,
}

/// All resolved paths needed by Localify components.
/// Loaded once from config, then passed to components via DI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocalifyPaths {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    #[serde(default = "default_pid_file")]
    pub pid_file: PathBuf,

    #[serde(default = "default_log_file")]
    pub log_file: PathBuf,

    #[serde(default = "default_certs_dir")]
    pub certs_dir: PathBuf,

    #[serde(default = "default_socket_path")]
    pub socket_path: PathBuf,
}

impl Default for LocalifyPaths {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            pid_file: default_pid_file(),
            log_file: default_log_file(),
            certs_dir: default_certs_dir(),
            socket_path: default_socket_path(),
        }
    }
}

impl LocalifyPaths {
    /// Present while the user wants the launchd service stopped. launchd would
    /// start the job again on the next connection, so the daemon checks this
    /// and goes back to sleep.
    pub fn stop_marker(&self) -> PathBuf {
        self.pid_file.with_extension("stopped")
    }
}
