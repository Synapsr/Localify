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

fn default_keygen_api_url() -> String {
    KEYGEN_API_URL.to_string()
}

fn default_keygen_account() -> String {
    KEYGEN_ACCOUNT_ID.to_string()
}

fn default_keygen_product() -> Option<String> {
    Some(KEYGEN_PRODUCT_ID.to_string())
}

fn default_keygen_public_key() -> Option<String> {
    Some(KEYGEN_PUBLIC_KEY.to_string())
}

pub const KEYGEN_API_URL: &str = "https://licences.localify.dev";
pub const KEYGEN_ACCOUNT_ID: &str = "00000000-0000-0000-0000-000000000000";
pub const KEYGEN_PRODUCT_ID: &str = "00000000-0000-0000-0000-000000000000";
pub const KEYGEN_PUBLIC_KEY: &str =
    "552d01b1542040fab21a49cd6e363991d863a711938061cc04ee703a4fa6e068";

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

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeygenConfig {
    #[serde(default = "default_keygen_api_url")]
    pub api_url: String,

    #[serde(default = "default_keygen_account")]
    pub account: String,

    #[serde(default = "default_keygen_product")]
    pub product: Option<String>,

    #[serde(default = "default_keygen_public_key")]
    pub public_key: Option<String>,
}

impl KeygenConfig {
    pub fn effective_api_url(&self) -> &str {
        let api_url = self.api_url.trim();
        if api_url.is_empty() {
            KEYGEN_API_URL
        } else {
            api_url
        }
    }

    pub fn effective_account(&self) -> &str {
        let account = self.account.trim();
        if account.is_empty() {
            KEYGEN_ACCOUNT_ID
        } else {
            account
        }
    }

    pub fn effective_product(&self) -> Option<&str> {
        self.product
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(Some(KEYGEN_PRODUCT_ID))
    }

    pub fn effective_public_key(&self) -> Option<&str> {
        self.public_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(Some(KEYGEN_PUBLIC_KEY))
    }

    pub fn is_configured(&self) -> bool {
        !self.effective_account().is_empty() && !self.effective_api_url().is_empty()
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredLicense {
    pub key: String,
    pub license_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub expiry: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub max_projects: Option<usize>,
    #[serde(default)]
    pub max_domains_per_project: Option<usize>,
    #[serde(default)]
    pub validation_code: Option<String>,
    #[serde(default)]
    pub validation_detail: Option<String>,
    #[serde(default)]
    pub last_validated_at: Option<String>,
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub machine_fingerprint: Option<String>,
    #[serde(default)]
    pub machine_name: Option<String>,
    #[serde(default)]
    pub machine_hostname: Option<String>,
    #[serde(default)]
    pub machine_platform: Option<String>,
    #[serde(default)]
    pub response_signature_verified: bool,
    #[serde(default)]
    pub response_signature_date: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineLicenseFile {
    pub certificate: String,
    pub algorithm: String,
    #[serde(default)]
    pub issued: Option<String>,
    #[serde(default)]
    pub expiry: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicensingConfig {
    #[serde(default)]
    pub keygen: KeygenConfig,

    #[serde(default)]
    pub license: Option<StoredLicense>,

    #[serde(default)]
    pub offline_license: Option<OfflineLicenseFile>,
}

/// All resolved paths needed by Roxy components.
/// Loaded once from config, then passed to components via DI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoxyPaths {
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

impl Default for RoxyPaths {
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
