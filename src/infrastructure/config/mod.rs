mod dto;
pub mod watcher;

// Re-export shared config types so existing CLI/daemon code keeps compiling.
pub use crate::config::{
    DaemonConfig, DockerConfig, KeygenConfig, OfflineLicenseFile, RoxyPaths, StoredLicense,
};

use crate::application::ports::{ConfigLoadError, ConfigLoader, DomainRepository, RepositoryError};
use crate::branding::BRANDING;
use crate::config::LicensingConfig as RootLicensingConfig;
use crate::domain::value_objects::{RouteTarget, RouteTargetError};
use crate::domain::{DomainPattern, DomainRegistration};
use crate::infrastructure::licensing::plan_limits_for_license;
use dto::{ProjectDto, RegistrationDto};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(std::io::Error),

    #[error("Failed to write config file: {0}")]
    WriteError(std::io::Error),

    #[error("Failed to parse config file: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Failed to serialize config: {0}")]
    SerializeError(#[from] toml::ser::Error),

    #[error("Domain already registered: {0}")]
    DomainExists(String),

    #[error("Domain not found: {0}")]
    DomainNotFound(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Invalid domain '{0}': {1}")]
    InvalidDomain(String, String),

    #[allow(dead_code)]
    #[error("Project already exists: {0}")]
    ProjectExists(String),

    #[error("Project not found: {0}")]
    ProjectNotFound(String),

    #[error("Project is still referenced by registered domains: {0}")]
    ProjectInUse(String),

    #[error("Invalid project '{0}': {1}")]
    InvalidProject(String, String),

    #[error("{0}")]
    LicenseLimitExceeded(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConfiguredDomain {
    pub registration: DomainRegistration,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub project_enabled: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConfiguredProject {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub root_path: Option<String>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub paths: RoxyPaths,

    #[serde(default)]
    pub docker: DockerConfig,

    #[serde(default)]
    pub licensing: RootLicensingConfig,

    #[serde(default)]
    domains: HashMap<String, RegistrationDto>,

    #[serde(default)]
    projects: HashMap<String, ProjectDto>,
}

impl Config {
    /// Convert all stored DTOs to domain registrations.
    pub fn registrations(&self) -> Vec<DomainRegistration> {
        self.domains
            .values()
            .filter(|dto| self.project_is_enabled(dto.project.as_deref()))
            .cloned()
            .map(DomainRegistration::from)
            .collect()
    }

    #[allow(dead_code)]
    pub fn configured_domains(&self) -> Vec<ConfiguredDomain> {
        self.domains
            .values()
            .cloned()
            .map(|dto| {
                let project_name = dto
                    .project
                    .as_ref()
                    .and_then(|project_id| self.projects.get(project_id))
                    .map(|project| project.name.clone());
                let project_enabled = self.project_is_enabled(dto.project.as_deref());

                ConfiguredDomain {
                    registration: DomainRegistration::from(dto.clone()),
                    project_id: dto.project,
                    project_name,
                    project_enabled,
                }
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn configured_projects(&self) -> Vec<ConfiguredProject> {
        let mut projects = self
            .projects
            .iter()
            .map(|(id, project)| ConfiguredProject {
                id: id.clone(),
                name: project.name.clone(),
                enabled: project.enabled,
                root_path: project.root_path.clone(),
            })
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        projects
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.daemon.validate().map_err(ConfigError::InvalidConfig)?;

        for (project_id, project) in &self.projects {
            let trimmed_name = project.name.trim();
            if trimmed_name.is_empty() {
                return Err(ConfigError::InvalidProject(
                    project_id.clone(),
                    "Project name cannot be empty".into(),
                ));
            }
        }

        for (name, dto) in &self.domains {
            if let Some(project_id) = &dto.project
                && !self.projects.contains_key(project_id)
            {
                return Err(ConfigError::InvalidDomain(
                    name.clone(),
                    format!("Unknown project '{project_id}'"),
                ));
            }

            let registration = DomainRegistration::from(dto.clone());
            // Structural validation (invariants enforced at construction)
            registration
                .validate()
                .map_err(|e| ConfigError::InvalidDomain(name.clone(), e.to_string()))?;
            // Infrastructure-level validation: check filesystem paths exist
            for route in registration.routes() {
                validate_route_path(route.target()).map_err(|e: RouteTargetError| {
                    ConfigError::InvalidDomain(name.clone(), e.to_string())
                })?;
            }
        }

        Ok(())
    }

    fn project_is_enabled(&self, project_id: Option<&str>) -> bool {
        project_id
            .and_then(|id| self.projects.get(id))
            .map(|project| project.enabled)
            .unwrap_or(true)
    }

    fn license_limits(&self) -> LicenseLimits {
        let derived = plan_limits_for_license(&self.licensing);
        LicenseLimits {
            max_projects: derived.max_projects,
            max_domains_per_project: derived.max_domains_per_project,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LicenseLimits {
    max_projects: Option<usize>,
    max_domains_per_project: Option<usize>,
}

/// Validate that a static files target points to an existing directory.
///
/// This is an infrastructure concern (filesystem I/O), kept out of the domain layer.
fn validate_route_path(target: &RouteTarget) -> Result<(), RouteTargetError> {
    if let RouteTarget::StaticFiles(path) = target {
        if !path.exists() {
            return Err(RouteTargetError::PathNotFound(path.clone()));
        }
        if !path.is_dir() {
            return Err(RouteTargetError::NotADirectory(path.clone()));
        }
    }
    Ok(())
}

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Create a new ConfigStore pointing at the given config file path
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn ensure_config_dir(&self) -> Result<(), ConfigError> {
        if let Some(dir) = self.path.parent()
            && !dir.exists()
        {
            fs::create_dir_all(dir).map_err(ConfigError::WriteError)?;
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Config, ConfigError> {
        if !self.path.exists() {
            return Ok(Config::default());
        }

        let content = fs::read_to_string(&self.path).map_err(ConfigError::ReadError)?;
        let mut config: Config = toml::from_str(&content)?;
        if apply_runtime_profile_defaults(&mut config) {
            self.save(&config)?;
        }
        Ok(config)
    }

    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        self.ensure_config_dir()?;

        let content = toml::to_string_pretty(config)?;
        fs::write(&self.path, content).map_err(ConfigError::WriteError)?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn licensing_config(&self) -> Result<RootLicensingConfig, ConfigError> {
        Ok(self.load()?.licensing)
    }

    #[allow(dead_code)]
    pub fn update_keygen_config(
        &self,
        keygen: KeygenConfig,
    ) -> Result<RootLicensingConfig, ConfigError> {
        let mut config = self.load()?;
        config.licensing.keygen = keygen;
        config.licensing.license = None;
        config.licensing.offline_license = None;
        self.save(&config)?;
        Ok(config.licensing)
    }

    pub fn store_licensing(
        &self,
        license: Option<StoredLicense>,
        offline_license: Option<OfflineLicenseFile>,
    ) -> Result<RootLicensingConfig, ConfigError> {
        let mut config = self.load()?;
        config.licensing.license = license;
        config.licensing.offline_license = offline_license;
        self.save(&config)?;
        Ok(config.licensing)
    }

    #[allow(dead_code)]
    pub fn store_license(
        &self,
        license: Option<StoredLicense>,
    ) -> Result<RootLicensingConfig, ConfigError> {
        let offline_license = if license.is_some() {
            self.load()?.licensing.offline_license
        } else {
            None
        };
        self.store_licensing(license, offline_license)
    }

    pub fn add_domain(&self, registration: DomainRegistration) -> Result<(), ConfigError> {
        self.add_domain_with_project(registration, None)
    }

    pub fn add_domain_with_project(
        &self,
        registration: DomainRegistration,
        project_id: Option<String>,
    ) -> Result<(), ConfigError> {
        let mut config = self.load()?;

        let key = registration.display_pattern();
        if config.domains.contains_key(&key) {
            return Err(ConfigError::DomainExists(key));
        }

        if let Some(project_id) = project_id.as_ref()
            && !config.projects.contains_key(project_id)
        {
            return Err(ConfigError::ProjectNotFound(project_id.clone()));
        }

        enforce_domain_limits(&config, project_id.as_deref(), None)?;

        let mut dto = RegistrationDto::from(registration);
        dto.project = project_id;
        config.domains.insert(key, dto);
        self.save(&config)
    }

    pub fn remove_domain(
        &self,
        pattern: &DomainPattern,
    ) -> Result<DomainRegistration, ConfigError> {
        let mut config = self.load()?;

        let key = pattern.display_pattern();
        let dto = config
            .domains
            .remove(&key)
            .ok_or(ConfigError::DomainNotFound(key))?;

        self.save(&config)?;
        Ok(dto.into())
    }

    pub fn get_domain(
        &self,
        pattern: &DomainPattern,
    ) -> Result<Option<DomainRegistration>, ConfigError> {
        let config = self.load()?;
        let key = pattern.display_pattern();
        Ok(config
            .domains
            .get(&key)
            .cloned()
            .map(DomainRegistration::from))
    }

    pub fn update_domain(&self, registration: DomainRegistration) -> Result<(), ConfigError> {
        self.update_domain_with_project(registration, None)
    }

    pub fn update_domain_with_project(
        &self,
        registration: DomainRegistration,
        project_id: Option<String>,
    ) -> Result<(), ConfigError> {
        let mut config = self.load()?;

        let key = registration.display_pattern();
        if !config.domains.contains_key(&key) {
            return Err(ConfigError::DomainNotFound(key));
        }

        let project_id =
            project_id.or_else(|| config.domains.get(&key).and_then(|dto| dto.project.clone()));

        if let Some(project_id) = project_id.as_ref()
            && !config.projects.contains_key(project_id)
        {
            return Err(ConfigError::ProjectNotFound(project_id.clone()));
        }

        enforce_domain_limits(&config, project_id.as_deref(), Some(&key))?;

        let mut dto = RegistrationDto::from(registration);
        dto.project = project_id;
        config.domains.insert(key, dto);
        self.save(&config)
    }

    #[allow(dead_code)]
    pub fn replace_domain_with_project(
        &self,
        current_pattern: &DomainPattern,
        registration: DomainRegistration,
        project_id: Option<String>,
    ) -> Result<(), ConfigError> {
        let mut config = self.load()?;

        let current_key = current_pattern.display_pattern();
        let new_key = registration.display_pattern();
        let existing = config
            .domains
            .get(&current_key)
            .ok_or_else(|| ConfigError::DomainNotFound(current_key.clone()))?;

        if current_key != new_key && config.domains.contains_key(&new_key) {
            return Err(ConfigError::DomainExists(new_key));
        }

        let project_id = project_id.or_else(|| existing.project.clone());

        if let Some(project_id) = project_id.as_ref()
            && !config.projects.contains_key(project_id)
        {
            return Err(ConfigError::ProjectNotFound(project_id.clone()));
        }

        enforce_domain_limits(&config, project_id.as_deref(), Some(&current_key))?;

        let mut dto = RegistrationDto::from(registration);
        dto.project = project_id;
        config.domains.remove(&current_key);
        config.domains.insert(new_key, dto);
        self.save(&config)
    }

    /// Check whether the config file exists on disk.
    pub fn config_exists(&self) -> bool {
        self.path.exists()
    }

    pub fn list_domains(&self) -> Result<Vec<DomainRegistration>, ConfigError> {
        let config = self.load()?;
        Ok(config
            .domains
            .into_values()
            .map(DomainRegistration::from)
            .collect())
    }

    #[allow(dead_code)]
    pub fn list_configured_domains(&self) -> Result<Vec<ConfiguredDomain>, ConfigError> {
        Ok(self.load()?.configured_domains())
    }

    #[allow(dead_code)]
    pub fn list_projects(&self) -> Result<Vec<ConfiguredProject>, ConfigError> {
        Ok(self.load()?.configured_projects())
    }

    #[allow(dead_code)]
    pub fn create_project(&self, name: &str) -> Result<ConfiguredProject, ConfigError> {
        self.create_project_with_root(name, None)
    }

    #[allow(dead_code)]
    pub fn create_project_with_root(
        &self,
        name: &str,
        root_path: Option<String>,
    ) -> Result<ConfiguredProject, ConfigError> {
        let mut config = self.load()?;
        let project_name = name.trim();
        if project_name.is_empty() {
            return Err(ConfigError::InvalidProject(
                "new".into(),
                "Project name cannot be empty".into(),
            ));
        }

        let project_id = normalize_project_id(project_name)?;
        if config.projects.contains_key(&project_id) {
            return Err(ConfigError::ProjectExists(project_id));
        }

        enforce_project_limit(&config)?;

        config.projects.insert(
            project_id.clone(),
            ProjectDto {
                name: project_name.to_string(),
                enabled: true,
                root_path: root_path.clone(),
            },
        );
        self.save(&config)?;

        Ok(ConfiguredProject {
            id: project_id,
            name: project_name.to_string(),
            enabled: true,
            root_path,
        })
    }

    #[allow(dead_code)]
    pub fn update_project(
        &self,
        project_id: &str,
        name: Option<String>,
        enabled: Option<bool>,
        root_path: Option<String>,
    ) -> Result<ConfiguredProject, ConfigError> {
        let mut config = self.load()?;
        let project = config
            .projects
            .get_mut(project_id)
            .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;

        if let Some(name) = name {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Err(ConfigError::InvalidProject(
                    project_id.to_string(),
                    "Project name cannot be empty".into(),
                ));
            }
            project.name = trimmed.to_string();
        }

        if let Some(enabled) = enabled {
            project.enabled = enabled;
        }

        if let Some(root_path) = root_path {
            project.root_path = Some(root_path);
        }

        let result = ConfiguredProject {
            id: project_id.to_string(),
            name: project.name.clone(),
            enabled: project.enabled,
            root_path: project.root_path.clone(),
        };
        self.save(&config)?;
        Ok(result)
    }

    #[allow(dead_code)]
    pub fn remove_project(&self, project_id: &str) -> Result<(), ConfigError> {
        let mut config = self.load()?;

        if !config.projects.contains_key(project_id) {
            return Err(ConfigError::ProjectNotFound(project_id.to_string()));
        }

        if config
            .domains
            .values()
            .any(|domain| domain.project.as_deref() == Some(project_id))
        {
            return Err(ConfigError::ProjectInUse(project_id.to_string()));
        }

        config.projects.remove(project_id);
        self.save(&config)
    }
}

fn apply_runtime_profile_defaults(config: &mut Config) -> bool {
    if !BRANDING.is_dev_profile() {
        return false;
    }

    if config.daemon.http_port == 80
        && config.daemon.https_port == 443
        && config.daemon.dns_port == 1053
    {
        config.daemon = DaemonConfig::default();
        return true;
    }

    false
}

fn enforce_project_limit(config: &Config) -> Result<(), ConfigError> {
    let limits = config.license_limits();
    let Some(max_projects) = limits.max_projects else {
        return Ok(());
    };

    if config.projects.len() < max_projects {
        return Ok(());
    }

    Err(ConfigError::LicenseLimitExceeded(format!(
        "Free plan limit reached: up to {max_projects} projects are included. Activate a Keygen license to add more."
    )))
}

fn enforce_domain_limits(
    config: &Config,
    project_id: Option<&str>,
    excluding_domain_key: Option<&str>,
) -> Result<(), ConfigError> {
    let limits = config.license_limits();

    if let Some(project_id) = project_id
        && let Some(max_domains_per_project) = limits.max_domains_per_project
    {
        let current = config
            .domains
            .values()
            .filter(|domain| domain.project.as_deref() == Some(project_id))
            .filter(|domain| {
                excluding_domain_key
                    .map(|key| domain.pattern.display_pattern() != key)
                    .unwrap_or(true)
            })
            .count();

        if current >= max_domains_per_project {
            return Err(ConfigError::LicenseLimitExceeded(format!(
                "Plan limit reached: project '{project_id}' already has {max_domains_per_project} domains. Activate a Keygen license to add more."
            )));
        }
    }

    if project_id.is_none() {
        let Some(max_projects) = limits.max_projects else {
            return Ok(());
        };
        let Some(max_domains_per_project) = limits.max_domains_per_project else {
            return Ok(());
        };
        let Some(max_total_domains) = max_projects.checked_mul(max_domains_per_project) else {
            return Ok(());
        };

        let existing_domain_count = config
            .domains
            .keys()
            .filter(|key| {
                excluding_domain_key
                    .map(|excluded| excluded != key.as_str())
                    .unwrap_or(true)
            })
            .count();

        if existing_domain_count >= max_total_domains {
            return Err(ConfigError::LicenseLimitExceeded(format!(
                "Free plan limit reached: up to {max_projects} projects with {max_domains_per_project} domains each are included. Activate a Keygen license to add more."
            )));
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn normalize_project_id(name: &str) -> Result<String, ConfigError> {
    let mut slug = String::new();
    let mut previous_was_dash = false;

    for ch in name.trim().chars().flat_map(|ch| ch.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_was_dash = false;
            continue;
        }

        if slug.is_empty() || previous_was_dash {
            continue;
        }

        slug.push('-');
        previous_was_dash = true;
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return Err(ConfigError::InvalidProject(
            "new".into(),
            "Project name must contain letters or numbers".into(),
        ));
    }

    Ok(slug)
}

fn map_config_error(e: ConfigError) -> ConfigLoadError {
    match e {
        ConfigError::InvalidConfig(msg) => ConfigLoadError::Invalid(msg),
        other => ConfigLoadError::IoFailed(other.into()),
    }
}

impl ConfigLoader for ConfigStore {
    fn load(&self) -> Result<(DaemonConfig, RoxyPaths), ConfigLoadError> {
        let config = ConfigStore::load(self).map_err(map_config_error)?;
        Ok((config.daemon, config.paths))
    }

    fn save_defaults(&self) -> Result<(), ConfigLoadError> {
        let config = Config::default();
        self.save(&config).map_err(map_config_error)
    }

    fn exists(&self) -> bool {
        self.config_exists()
    }
}

impl DomainRepository for ConfigStore {
    fn get(&self, pattern: &DomainPattern) -> Result<Option<DomainRegistration>, RepositoryError> {
        self.get_domain(pattern)
            .map_err(|e| RepositoryError::StorageFailed(e.into()))
    }

    fn list(&self) -> Result<Vec<DomainRegistration>, RepositoryError> {
        self.list_domains()
            .map_err(|e| RepositoryError::StorageFailed(e.into()))
    }

    fn add(&self, registration: DomainRegistration) -> Result<(), RepositoryError> {
        self.add_domain(registration).map_err(|e| match e {
            ConfigError::DomainExists(d) => RepositoryError::DomainExists(d),
            other => RepositoryError::StorageFailed(other.into()),
        })
    }

    fn update(&self, registration: DomainRegistration) -> Result<(), RepositoryError> {
        self.update_domain(registration).map_err(|e| match e {
            ConfigError::DomainNotFound(d) => RepositoryError::DomainNotFound(d),
            other => RepositoryError::StorageFailed(other.into()),
        })
    }

    fn remove(&self, pattern: &DomainPattern) -> Result<(), RepositoryError> {
        self.remove_domain(pattern).map_err(|e| match e {
            ConfigError::DomainNotFound(d) => RepositoryError::DomainNotFound(d),
            other => RepositoryError::StorageFailed(other.into()),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branding::BRANDING;
    use crate::domain::Route;

    fn temp_store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.toml"));
        (dir, store)
    }

    fn proxy_registration(domain: &str, port: u16) -> DomainRegistration {
        let normalized = domain.replace(".roxy", BRANDING.domain_suffix());
        let pattern = DomainPattern::from_name(&normalized, false).unwrap();
        let route = Route::parse(&format!("/={port}")).unwrap();
        DomainRegistration::new(pattern, vec![route])
    }

    // --- DaemonConfig::validate ---

    #[test]
    fn default_config_is_valid() {
        let config = DaemonConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn custom_valid_config() {
        let config = DaemonConfig {
            http_port: 8080,
            https_port: 8443,
            dns_port: 5353,
            log_level: "debug".to_string(),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn zero_http_port_is_invalid() {
        let config = DaemonConfig {
            http_port: 0,
            ..DaemonConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("http_port cannot be 0"));
    }

    #[test]
    fn zero_https_port_is_invalid() {
        let config = DaemonConfig {
            https_port: 0,
            ..DaemonConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("https_port cannot be 0"));
    }

    #[test]
    fn zero_dns_port_is_invalid() {
        let config = DaemonConfig {
            dns_port: 0,
            ..DaemonConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("dns_port cannot be 0"));
    }

    #[test]
    fn duplicate_http_and_https_ports_is_invalid() {
        let config = DaemonConfig {
            http_port: 8080,
            https_port: 8080,
            ..DaemonConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must be different"));
    }

    #[test]
    fn duplicate_http_and_dns_ports_is_invalid() {
        let config = DaemonConfig {
            http_port: 1053,
            https_port: 443,
            dns_port: 1053,
            ..DaemonConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must all be different"));
    }

    #[test]
    fn duplicate_https_and_dns_ports_is_invalid() {
        let config = DaemonConfig {
            http_port: 80,
            https_port: 1053,
            dns_port: 1053,
            ..DaemonConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must all be different"));
    }

    #[test]
    fn invalid_log_level_is_rejected() {
        let config = DaemonConfig {
            log_level: "verbose".to_string(),
            ..DaemonConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("Invalid log_level"));
    }

    #[test]
    fn valid_log_levels_are_accepted() {
        for level in ["error", "warn", "info", "debug"] {
            let config = DaemonConfig {
                log_level: level.to_string(),
                ..DaemonConfig::default()
            };
            assert!(
                config.validate().is_ok(),
                "level '{}' should be valid",
                level
            );
        }
    }

    // --- Config::validate ---

    #[test]
    fn config_validate_delegates_to_daemon() {
        let config = Config {
            daemon: DaemonConfig {
                http_port: 0,
                ..DaemonConfig::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn create_project_persists_slug_and_enabled_state() {
        let (_dir, store) = temp_store();

        let project = store.create_project("Test 1").unwrap();

        assert_eq!(project.id, "test-1");
        assert_eq!(project.name, "Test 1");
        assert!(project.enabled);

        let persisted = store.list_projects().unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].id, "test-1");
        assert!(persisted[0].enabled);
    }

    #[test]
    fn create_project_with_root_persists_root_path() {
        let (_dir, store) = temp_store();

        let project = store
            .create_project_with_root("Test 1", Some("/tmp/test-1".into()))
            .unwrap();

        assert_eq!(project.root_path.as_deref(), Some("/tmp/test-1"));

        let persisted = store.list_projects().unwrap();
        assert_eq!(persisted[0].root_path.as_deref(), Some("/tmp/test-1"));
    }

    #[test]
    fn update_project_root_path_persists_root_path() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Test 1").unwrap();

        let updated = store
            .update_project(&project.id, None, None, Some("/tmp/test-1".into()))
            .unwrap();

        assert_eq!(updated.root_path.as_deref(), Some("/tmp/test-1"));

        let persisted = store.list_projects().unwrap();
        assert_eq!(persisted[0].root_path.as_deref(), Some("/tmp/test-1"));
    }

    #[test]
    fn create_project_rejects_duplicate_normalized_slug() {
        let (_dir, store) = temp_store();
        store.create_project("Test 1").unwrap();

        let error = store.create_project("test-1").unwrap_err();

        assert!(matches!(error, ConfigError::ProjectExists(id) if id == "test-1"));
    }

    #[test]
    fn registrations_skip_domains_from_disabled_projects() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Demo").unwrap();

        store
            .add_domain_with_project(
                proxy_registration("app.roxy", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain(proxy_registration("plain.roxy", 4000))
            .unwrap();

        store
            .update_project(&project.id, None, Some(false), None)
            .unwrap();

        let registrations = store.load().unwrap().registrations();
        let patterns = registrations
            .into_iter()
            .map(|registration| registration.display_pattern())
            .collect::<Vec<_>>();

        assert_eq!(patterns, vec![format!("plain{}", BRANDING.domain_suffix())]);
    }

    #[test]
    fn configured_domains_include_project_metadata() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();

        store
            .add_domain_with_project(
                proxy_registration("app.roxy", 3000),
                Some(project.id.clone()),
            )
            .unwrap();

        let configured_domains = store.list_configured_domains().unwrap();

        assert_eq!(configured_domains.len(), 1);
        let configured = &configured_domains[0];
        assert_eq!(configured.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(configured.project_name.as_deref(), Some("Client App"));
        assert!(configured.project_enabled);
        assert_eq!(
            configured.registration.display_pattern(),
            format!("app{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn replace_domain_updates_key_and_preserves_project() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        let current = proxy_registration("old.roxy", 3000);
        let current_pattern = current.pattern().clone();

        store
            .add_domain_with_project(current, Some(project.id.clone()))
            .unwrap();

        store
            .replace_domain_with_project(
                &current_pattern,
                proxy_registration("new.roxy", 3001),
                None,
            )
            .unwrap();

        let domains = store.list_configured_domains().unwrap();
        assert_eq!(domains.len(), 1);
        assert_eq!(
            domains[0].registration.display_pattern(),
            format!("new{}", BRANDING.domain_suffix())
        );
        assert_eq!(domains[0].project_id.as_deref(), Some(project.id.as_str()));
    }

    #[test]
    fn replace_domain_rejects_duplicate_without_removing_current() {
        let (_dir, store) = temp_store();
        let current = proxy_registration("old.roxy", 3000);
        let current_pattern = current.pattern().clone();

        store.add_domain(current).unwrap();
        store
            .add_domain(proxy_registration("existing.roxy", 3001))
            .unwrap();

        let error = store
            .replace_domain_with_project(
                &current_pattern,
                proxy_registration("existing.roxy", 3002),
                None,
            )
            .unwrap_err();

        assert!(matches!(error, ConfigError::DomainExists(_)));
        assert!(store.get_domain(&current_pattern).unwrap().is_some());
    }

    #[test]
    fn remove_project_rejects_projects_still_in_use() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Demo").unwrap();

        store
            .add_domain_with_project(
                proxy_registration("app.roxy", 3000),
                Some(project.id.clone()),
            )
            .unwrap();

        let error = store.remove_project(&project.id).unwrap_err();

        assert!(matches!(error, ConfigError::ProjectInUse(id) if id == project.id));
    }

    #[test]
    fn remove_project_deletes_unused_project() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Demo").unwrap();

        store.remove_project(&project.id).unwrap();

        assert!(store.list_projects().unwrap().is_empty());
    }

    #[test]
    fn free_plan_limits_projects() {
        let (_dir, store) = temp_store();

        store.create_project("One").unwrap();
        store.create_project("Two").unwrap();
        store.create_project("Three").unwrap();
        store.create_project("Four").unwrap();
        store.create_project("Five").unwrap();

        let error = store.create_project("Six").unwrap_err();

        assert!(matches!(error, ConfigError::LicenseLimitExceeded(_)));
    }

    #[test]
    fn free_plan_allows_unlimited_domains_per_project() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Demo").unwrap();

        store
            .add_domain_with_project(
                proxy_registration("one.roxy", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration("two.roxy", 3001),
                Some(project.id.clone()),
            )
            .unwrap();

        store
            .add_domain_with_project(
                proxy_registration("three.roxy", 3002),
                Some(project.id.clone()),
            )
            .unwrap();
    }

    #[test]
    fn valid_keygen_license_overrides_free_limits() {
        let (_dir, store) = temp_store();

        store
            .store_license(Some(StoredLicense {
                key: "TEST-KEY-1234".into(),
                license_id: "lic_123".into(),
                name: Some("Pro".into()),
                expiry: None,
                plan: Some("Pro".into()),
                max_projects: Some(5),
                max_domains_per_project: Some(4),
                validation_code: Some("VALID".into()),
                validation_detail: Some("ok".into()),
                last_validated_at: Some("now".into()),
                machine_id: None,
                machine_fingerprint: None,
                machine_name: None,
                machine_hostname: None,
                machine_platform: None,
                response_signature_verified: false,
                response_signature_date: None,
            }))
            .unwrap();

        store.create_project("One").unwrap();
        store.create_project("Two").unwrap();
        store.create_project("Three").unwrap();
        store.create_project("Four").unwrap();

        assert_eq!(store.list_projects().unwrap().len(), 4);
    }

    #[test]
    fn license_save_omits_raw_json_metadata() {
        let (_dir, store) = temp_store();

        store
            .store_license(Some(StoredLicense {
                key: "TEST-KEY-1234".into(),
                license_id: "lic_123".into(),
                ..StoredLicense::default()
            }))
            .unwrap();

        let content = std::fs::read_to_string(&store.path).unwrap();

        assert!(!content.contains("metadata"));
        assert!(store.load().unwrap().licensing.license.is_some());
    }

    #[test]
    fn config_loads_external_wildcard_domains() {
        let (dir, store) = temp_store();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[domains."*.store.example.com"]
pattern = "*.store.example.com"
https_enabled = true

[[domains."*.store.example.com".routes]]
path = "/"
target = "127.0.0.1:61092"
"#,
        )
        .unwrap();

        let config = store.load().unwrap();
        let registration = config.registrations().into_iter().next().unwrap();

        assert_eq!(
            registration.pattern().display_pattern(),
            "*.store.example.com"
        );
    }
}
