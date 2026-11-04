use std::collections::HashSet;
use std::path::PathBuf;

use crate::branding::BRANDING;

const FIRST_UNPRIVILEGED_PORT: u16 = 1024;

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
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            http_port: default_http_port(),
            https_port: default_https_port(),
            dns_port: default_dns_port(),
            log_level: default_log_level(),
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

    /// Limits what a root process will bind for the user. The config can be
    /// changed through the helper socket with no password, so it must not be a
    /// way to take over another low port. Anything else stays unprivileged.
    pub fn validate_root_ports(&self) -> Result<(), String> {
        for (name, port, allowed) in [
            ("http_port", self.http_port, Some(80)),
            ("https_port", self.https_port, Some(443)),
            ("dns_port", self.dns_port, None),
        ] {
            if port < FIRST_UNPRIVILEGED_PORT && Some(port) != allowed {
                return Err(format!(
                    "{name} {port} is a privileged port that {} does not use. \
                     Choose a port of {FIRST_UNPRIVILEGED_PORT} or above{}.",
                    BRANDING.app_name,
                    allowed.map_or(String::new(), |port| format!(", or {port}"))
                ));
            }
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

fn default_agent_install_skill() -> bool {
    true
}

fn default_agent_promote_dev_script() -> bool {
    false
}

/// Agent workflow preferences shared by CLI and desktop flows.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// Offer/install the bundled Localify Agent Skill for detected coding agents.
    #[serde(default = "default_agent_install_skill")]
    pub install_skill: bool,

    /// Make Localify write and promote package dev scripts during project bootstrap.
    #[serde(default = "default_agent_promote_dev_script")]
    pub promote_dev_script: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            install_skill: default_agent_install_skill(),
            promote_dev_script: default_agent_promote_dev_script(),
        }
    }
}

/// All resolved paths needed by Localify components.
/// Loaded once from config, then passed to components via DI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon(http_port: u16, https_port: u16, dns_port: u16) -> DaemonConfig {
        DaemonConfig {
            http_port,
            https_port,
            dns_port,
            log_level: default_log_level(),
        }
    }

    #[test]
    fn root_binds_the_standard_ports_and_unprivileged_ones() {
        assert!(daemon(80, 443, 1053).validate_root_ports().is_ok());
        assert!(daemon(8080, 8443, 1054).validate_root_ports().is_ok());
    }

    #[test]
    fn root_refuses_other_privileged_ports() {
        for config in [
            daemon(22, 443, 1053),
            daemon(80, 631, 1053),
            daemon(443, 80, 1053),
            daemon(80, 443, 53),
        ] {
            assert!(config.validate_root_ports().is_err(), "{config:?}");
        }
    }
}
