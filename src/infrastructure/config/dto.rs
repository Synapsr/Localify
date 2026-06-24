//! Persistence DTO for `DomainRegistration`.
//!
//! Decouples the on-disk TOML format from the domain entity so that
//! adding or removing domain fields doesn't accidentally change the
//! config file layout, and deserialization can't bypass domain
//! invariants enforced by `DomainRegistration` methods.

use serde::{Deserialize, Serialize};

use crate::domain::{DomainPattern, DomainRegistration, Route};

fn default_project_enabled() -> bool {
    true
}

/// Serializable representation of a domain registration in the config
/// file. Converted to/from `DomainRegistration` at the `ConfigStore`
/// boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationDto {
    pub pattern: DomainPattern,
    pub routes: Vec<Route>,
    #[serde(default)]
    pub https_enabled: bool,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDto {
    pub name: String,
    #[serde(default = "default_project_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ProjectServiceDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environments: Vec<ProjectEnvironmentDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectServiceDto {
    pub name: String,
    pub primary_domain: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEnvironmentDto {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub git_common_dir: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ProjectEnvironmentServiceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEnvironmentServiceDto {
    pub service: String,
    pub domain: String,
}

impl From<DomainRegistration> for RegistrationDto {
    fn from(reg: DomainRegistration) -> Self {
        Self {
            pattern: reg.pattern().clone(),
            routes: reg.routes().to_vec(),
            https_enabled: reg.is_https_enabled(),
            project: None,
        }
    }
}

impl From<RegistrationDto> for DomainRegistration {
    fn from(dto: RegistrationDto) -> Self {
        let mut reg = DomainRegistration::new(dto.pattern, dto.routes);
        if dto.https_enabled {
            reg.enable_https();
        }
        reg
    }
}
