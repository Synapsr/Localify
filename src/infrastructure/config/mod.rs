mod dto;
pub mod watcher;

// Re-export shared config types so existing CLI/daemon code keeps compiling.
pub use crate::config::{AgentConfig, DaemonConfig, DockerConfig, LocalifyPaths};

use crate::application::ports::{ConfigLoadError, ConfigLoader, DomainRepository, RepositoryError};
use crate::branding::BRANDING;
use crate::domain::value_objects::{RouteTarget, RouteTargetError};
use crate::domain::{DomainPattern, DomainRegistration};
use dto::{
    ProjectDto, ProjectEnvironmentDto, ProjectEnvironmentServiceDto, ProjectServiceDto,
    RegistrationDto,
};
use std::collections::{HashMap, HashSet};
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

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConfiguredService {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub project_name: String,
    pub project_enabled: bool,
    pub project_root_path: Option<String>,
    pub primary_domain: String,
    pub domains: Vec<String>,
    pub path: Option<String>,
    pub command: Option<Vec<String>>,
    pub primary_target: Option<RouteTarget>,
    pub inferred: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConfiguredProjectEnvironment {
    pub project_id: String,
    pub project_name: String,
    pub name: String,
    pub slug: String,
    pub primary: bool,
    pub root_path: Option<String>,
    pub git_common_dir: Option<String>,
    pub git_branch: Option<String>,
    pub services: Vec<ConfiguredEnvironmentService>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConfiguredEnvironmentService {
    pub service_id: String,
    pub domain: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProjectServiceConfig {
    pub name: String,
    pub primary_domain: String,
    pub domains: Vec<String>,
    pub path: Option<String>,
    pub command: Option<Vec<String>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProjectEnvironmentConfig {
    pub name: String,
    pub slug: String,
    pub primary: bool,
    pub root_path: Option<String>,
    pub git_common_dir: Option<String>,
    pub git_branch: Option<String>,
    pub services: Vec<ProjectEnvironmentServiceConfig>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProjectEnvironmentServiceConfig {
    pub service_id: String,
    pub domain: String,
}

#[allow(dead_code)]
pub struct ProjectServiceDomainUpdate {
    pub current_domain: String,
    pub new_registration: DomainRegistration,
}

#[allow(dead_code)]
pub struct ProjectEnvironmentServiceDomainUpdate {
    pub environment_slug: String,
    pub service_id: String,
    pub current_domain: String,
    pub new_registration: DomainRegistration,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub paths: LocalifyPaths,

    #[serde(default)]
    pub docker: DockerConfig,

    #[serde(default)]
    pub agent: AgentConfig,

    #[serde(default)]
    domains: HashMap<String, RegistrationDto>,

    #[serde(default)]
    projects: HashMap<String, ProjectDto>,
}

impl Config {
    /// Adds the domains and projects of an older config that this one lacks, and returns
    /// how many. This config's version wins where both have one.
    pub fn adopt_missing_from(&mut self, older: Config) -> usize {
        let mut added = 0;
        for (key, registration) in older.domains {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.domains.entry(key) {
                entry.insert(registration);
                added += 1;
            }
        }
        for (key, project) in older.projects {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.projects.entry(key) {
                entry.insert(project);
                added += 1;
            }
        }
        added
    }

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

    #[allow(dead_code)]
    pub fn configured_services(&self) -> Vec<ConfiguredService> {
        let mut services = Vec::new();
        let mut projects = self.projects.iter().collect::<Vec<_>>();
        projects.sort_by(|(left_id, left), (right_id, right)| {
            left.name
                .cmp(&right.name)
                .then_with(|| left_id.cmp(right_id))
        });

        for (project_id, project) in projects {
            if project.services.is_empty() {
                services.extend(self.inferred_services_for_project(project_id, project));
                continue;
            }

            services.extend(project.services.iter().enumerate().map(|(index, service)| {
                self.configured_explicit_service(project_id, project, service, index)
            }));
        }

        services.sort_by(|left, right| {
            left.project_name
                .cmp(&right.project_name)
                .then_with(|| left.project_id.cmp(&right.project_id))
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| left.primary_domain.cmp(&right.primary_domain))
        });
        services
    }

    #[allow(dead_code)]
    pub fn configured_project_environments(
        &self,
        project_filter: Option<&str>,
    ) -> Vec<ConfiguredProjectEnvironment> {
        let services = self.configured_services();
        let mut projects = self.projects.iter().collect::<Vec<_>>();
        projects.sort_by(|(left_id, left), (right_id, right)| {
            left.name
                .cmp(&right.name)
                .then_with(|| left_id.cmp(right_id))
        });

        let mut environments = Vec::new();
        for (project_id, project) in projects {
            if let Some(filter) = project_filter
                && project_id != filter
                && !project.name.eq_ignore_ascii_case(filter)
            {
                continue;
            }

            let project_services = services
                .iter()
                .filter(|service| service.project_id == *project_id)
                .collect::<Vec<_>>();
            let has_primary = project
                .environments
                .iter()
                .any(|environment| environment.primary);
            if !has_primary {
                environments.push(implicit_primary_environment(
                    project_id,
                    project,
                    &project_services,
                ));
            }

            environments.extend(
                project
                    .environments
                    .iter()
                    .map(|environment| configured_environment(project_id, project, environment)),
            );
        }

        environments.sort_by(|left, right| {
            left.project_name
                .cmp(&right.project_name)
                .then_with(|| left.project_id.cmp(&right.project_id))
                .then_with(|| right.primary.cmp(&left.primary))
                .then_with(|| left.slug.cmp(&right.slug))
        });
        environments
    }

    /// A service as it runs in one environment of its project: that
    /// environment's domain, checkout and route target in place of the
    /// primary ones. `None` when the environment does not map the service.
    /// `launch_plan::environment_service` adds the folder the route detects.
    pub fn environment_service(
        &self,
        mut service: ConfiguredService,
        environment: &ConfiguredProjectEnvironment,
    ) -> Option<ConfiguredService> {
        let mapping = environment
            .services
            .iter()
            .find(|mapping| mapping.service_id == service.id)?;
        service.primary_domain = mapping.domain.clone();
        service.domains = vec![mapping.domain.clone()];
        service.project_root_path = environment
            .root_path
            .clone()
            .or_else(|| service.project_root_path.clone());
        service.primary_target = self
            .domains
            .values()
            .find(|domain| domain.pattern.display_pattern() == mapping.domain)
            .and_then(root_route_target);
        Some(service)
    }

    /// Whether the domain is served over HTTPS, as a service URL says.
    pub fn https_enabled_for(&self, domain: &str) -> bool {
        self.registration_for_domain(domain)
            .is_some_and(|registration| registration.https_enabled)
    }

    fn configured_explicit_service(
        &self,
        project_id: &str,
        project: &ProjectDto,
        service: &ProjectServiceDto,
        index: usize,
    ) -> ConfiguredService {
        let primary_domain = service.primary_domain.trim().to_string();
        let id =
            normalized_label(&service.name).unwrap_or_else(|| format!("service-{}", index + 1));
        let domains = service_domains(&primary_domain, &service.domains);

        ConfiguredService {
            id,
            name: service.name.trim().to_string(),
            project_id: project_id.to_string(),
            project_name: project.name.clone(),
            project_enabled: project.enabled,
            project_root_path: project.root_path.clone(),
            primary_target: self
                .registration_for_domain(&primary_domain)
                .and_then(root_route_target),
            primary_domain,
            domains,
            path: service.path.clone(),
            command: service.command.clone(),
            inferred: false,
        }
    }

    fn inferred_services_for_project(
        &self,
        project_id: &str,
        project: &ProjectDto,
    ) -> Vec<ConfiguredService> {
        let environment_domains = project
            .environments
            .iter()
            .filter(|environment| !environment.primary)
            .flat_map(|environment| environment.services.iter())
            .map(|service| owned_domain_key(&service.domain))
            .collect::<HashSet<_>>();
        let mut domains = self
            .domains
            .values()
            .filter(|domain| domain.project.as_deref() == Some(project_id))
            .filter(|domain| {
                !environment_domains.contains(&owned_domain_key(&domain.pattern.display_pattern()))
            })
            .collect::<Vec<_>>();
        domains.sort_by_key(|domain| domain.pattern.display_pattern());

        let mut used_ids = HashSet::new();
        domains
            .into_iter()
            .map(|domain| {
                let primary_domain = domain.pattern.display_pattern();
                let inferred_id = infer_service_id(&primary_domain, project_id);
                let id = unique_service_id(&inferred_id, &mut used_ids);

                ConfiguredService {
                    id: id.clone(),
                    name: id,
                    project_id: project_id.to_string(),
                    project_name: project.name.clone(),
                    project_enabled: project.enabled,
                    project_root_path: project.root_path.clone(),
                    primary_target: root_route_target(domain),
                    primary_domain: primary_domain.clone(),
                    domains: vec![primary_domain],
                    path: None,
                    command: None,
                    inferred: true,
                }
            })
            .collect()
    }

    fn registration_for_domain(&self, domain: &str) -> Option<&RegistrationDto> {
        let domain = domain.trim();
        if let Some(registration) = self.domains.get(domain) {
            return Some(registration);
        }

        if let Some(base_domain) = domain.strip_prefix("*.") {
            return self.domains.get(base_domain);
        }

        let wildcard_domain = format!("*.{domain}");
        self.domains.get(&wildcard_domain)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.daemon.validate().map_err(ConfigError::InvalidConfig)?;

        let mut service_domain_owners = HashMap::new();
        for (project_id, project) in &self.projects {
            let trimmed_name = project.name.trim();
            if trimmed_name.is_empty() {
                return Err(ConfigError::InvalidProject(
                    project_id.clone(),
                    "Project name cannot be empty".into(),
                ));
            }
            validate_project_services(project_id, project, &mut service_domain_owners)?;
            validate_project_environments(project_id, project, self, &mut service_domain_owners)?;
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

    /// The project of each exposable local domain (`docs/public-urls.md`,
    /// "Exposable local domain"): every domain a project registers, and every
    /// project's namespace `<project-id><domain suffix>`, which a wildcard URL
    /// targets in a monorepo where only `web.journal.localify` is registered.
    /// Domains outside any project, Docker's included, are never exposed. The
    /// CLI and the app check it before creating a URL, and the daemon before
    /// serving public traffic.
    pub fn exposable_domains(&self) -> HashMap<String, String> {
        let namespaces = self.projects.keys().map(|id| {
            let namespace = format!("{}{}", id.to_ascii_lowercase(), BRANDING.domain_suffix());
            (namespace, id.clone())
        });
        let registered = self.configured_domains().into_iter().filter_map(|domain| {
            let project_id = domain
                .project_id
                .filter(|id| self.projects.contains_key(id))?;
            let name = domain
                .registration
                .pattern()
                .base_domain()
                .as_str()
                .to_string();
            Some((name, project_id))
        });
        namespaces.chain(registered).collect()
    }

    /// The project a public URL on `local_domain` belongs to, if the domain
    /// is exposable (see `exposable_domains`).
    pub fn public_url_project(&self, local_domain: &str) -> Option<String> {
        let domain = local_domain
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        self.exposable_domains().remove(&domain)
    }

    fn project_is_enabled(&self, project_id: Option<&str>) -> bool {
        project_id
            .and_then(|id| self.projects.get(id))
            .map(|project| project.enabled)
            .unwrap_or(true)
    }
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

fn service_config_to_dto(service: ProjectServiceConfig) -> ProjectServiceDto {
    ProjectServiceDto {
        name: service.name,
        primary_domain: service.primary_domain,
        domains: service.domains,
        path: service.path,
        command: service.command,
    }
}

fn service_dto_to_config(service: &ProjectServiceDto) -> ProjectServiceConfig {
    ProjectServiceConfig {
        name: service.name.clone(),
        primary_domain: service.primary_domain.clone(),
        domains: service.domains.clone(),
        path: service.path.clone(),
        command: service.command.clone(),
    }
}

fn configured_service_to_config(service: &ConfiguredService) -> ProjectServiceConfig {
    ProjectServiceConfig {
        name: service.name.clone(),
        primary_domain: service.primary_domain.clone(),
        domains: service
            .domains
            .iter()
            .filter(|domain| domain.as_str() != service.primary_domain)
            .cloned()
            .collect(),
        path: service.path.clone(),
        command: service.command.clone(),
    }
}

fn environment_config_to_dto(environment: ProjectEnvironmentConfig) -> ProjectEnvironmentDto {
    ProjectEnvironmentDto {
        name: environment.name,
        slug: environment.slug,
        primary: environment.primary,
        root_path: environment.root_path,
        git_common_dir: environment.git_common_dir,
        git_branch: environment.git_branch,
        services: environment
            .services
            .into_iter()
            .map(|service| ProjectEnvironmentServiceDto {
                service: service.service_id,
                domain: service.domain,
            })
            .collect(),
    }
}

fn configured_environment(
    project_id: &str,
    project: &ProjectDto,
    environment: &ProjectEnvironmentDto,
) -> ConfiguredProjectEnvironment {
    ConfiguredProjectEnvironment {
        project_id: project_id.to_string(),
        project_name: project.name.clone(),
        name: environment.name.clone(),
        slug: environment.slug.clone(),
        primary: environment.primary,
        root_path: environment
            .root_path
            .clone()
            .or_else(|| project.root_path.clone()),
        git_common_dir: environment.git_common_dir.clone(),
        git_branch: environment.git_branch.clone(),
        services: environment
            .services
            .iter()
            .map(|service| ConfiguredEnvironmentService {
                service_id: service.service.clone(),
                domain: service.domain.clone(),
            })
            .collect(),
    }
}

fn implicit_primary_environment(
    project_id: &str,
    project: &ProjectDto,
    services: &[&ConfiguredService],
) -> ConfiguredProjectEnvironment {
    ConfiguredProjectEnvironment {
        project_id: project_id.to_string(),
        project_name: project.name.clone(),
        name: "Primary".to_string(),
        slug: "primary".to_string(),
        primary: true,
        root_path: project.root_path.clone(),
        git_common_dir: None,
        git_branch: None,
        services: services
            .iter()
            .map(|service| ConfiguredEnvironmentService {
                service_id: service.id.clone(),
                domain: service.primary_domain.clone(),
            })
            .collect(),
    }
}

fn validate_project_services(
    project_id: &str,
    project: &ProjectDto,
    service_domain_owners: &mut HashMap<String, String>,
) -> Result<(), ConfigError> {
    let mut service_ids = HashSet::new();

    for service in &project.services {
        let service_name = service.name.trim();
        if service_name.is_empty() {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                "Service name cannot be empty".into(),
            ));
        }

        let service_id = normalized_label(service_name).ok_or_else(|| {
            ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Service name '{service_name}' must contain letters or numbers"),
            )
        })?;
        if !service_ids.insert(service_id.clone()) {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Duplicate service '{service_id}'"),
            ));
        }

        if service.primary_domain.trim().is_empty() {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Service '{service_name}' primary domain cannot be empty"),
            ));
        }
        if service
            .domains
            .iter()
            .any(|domain| domain.trim().is_empty())
        {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Service '{service_name}' additional domains cannot be empty"),
            ));
        }

        if let Some(path) = &service.path
            && path.trim().is_empty()
        {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Service '{service_name}' path cannot be empty when set"),
            ));
        }

        if let Some(command) = &service.command {
            if command.is_empty() {
                return Err(ConfigError::InvalidProject(
                    project_id.to_string(),
                    format!("Service '{service_name}' command cannot be empty when set"),
                ));
            }

            if command.iter().any(|part| part.trim().is_empty()) {
                return Err(ConfigError::InvalidProject(
                    project_id.to_string(),
                    format!("Service '{service_name}' command cannot contain empty arguments"),
                ));
            }
        }

        let owner = format!("{project_id}/{service_id}");
        for domain in service_domains(&service.primary_domain, &service.domains) {
            validate_service_domain(project_id, service_name, &domain)?;
            let key = owned_domain_key(&domain);

            match service_domain_owners.get(&key) {
                Some(existing_owner) if existing_owner != &owner => {
                    return Err(ConfigError::InvalidProject(
                        project_id.to_string(),
                        format!(
                            "Local domain '{domain}' is already owned by service '{existing_owner}'"
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    service_domain_owners.insert(key, owner.clone());
                }
            }
        }
    }

    Ok(())
}

fn validate_project_environments(
    project_id: &str,
    project: &ProjectDto,
    config: &Config,
    service_domain_owners: &mut HashMap<String, String>,
) -> Result<(), ConfigError> {
    let mut environment_slugs = HashSet::new();
    let mut primary_count = 0usize;
    let service_ids = project_service_ids(config, project_id, project);

    for environment in &project.environments {
        let name = environment.name.trim();
        if name.is_empty() {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                "Environment name cannot be empty".into(),
            ));
        }

        let slug = environment.slug.trim();
        if slug.is_empty() {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Environment '{name}' slug cannot be empty"),
            ));
        }
        if normalized_label(slug).as_deref() != Some(slug) {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Environment '{name}' slug must be normalized"),
            ));
        }
        if !environment_slugs.insert(slug.to_string()) {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Duplicate environment '{slug}'"),
            ));
        }

        if environment.primary {
            primary_count += 1;
        }

        if let Some(root_path) = &environment.root_path
            && root_path.trim().is_empty()
        {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Environment '{name}' root path cannot be empty when set"),
            ));
        }

        for service in &environment.services {
            let service_id = service.service.trim();
            if !service_ids.contains(service_id) {
                return Err(ConfigError::InvalidProject(
                    project_id.to_string(),
                    format!("Environment '{name}' references unknown service '{service_id}'"),
                ));
            }

            validate_service_domain(project_id, service_id, &service.domain)?;
            let key = owned_domain_key(&service.domain);
            let owner = format!("{project_id}/{service_id}");
            match service_domain_owners.get(&key) {
                Some(existing_owner) if existing_owner != &owner => {
                    return Err(ConfigError::InvalidProject(
                        project_id.to_string(),
                        format!(
                            "Local domain '{}' is already owned by service '{existing_owner}'",
                            service.domain
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    service_domain_owners.insert(key, owner);
                }
            }
        }
    }

    if primary_count > 1 {
        return Err(ConfigError::InvalidProject(
            project_id.to_string(),
            "Only one primary environment can be stored per project".into(),
        ));
    }

    Ok(())
}

fn project_service_ids(config: &Config, project_id: &str, project: &ProjectDto) -> HashSet<String> {
    if project.services.is_empty() {
        return config
            .inferred_services_for_project(project_id, project)
            .into_iter()
            .map(|service| service.id)
            .collect();
    }

    project
        .services
        .iter()
        .enumerate()
        .map(|(index, service)| {
            normalized_label(&service.name).unwrap_or_else(|| format!("service-{}", index + 1))
        })
        .collect()
}

fn validate_service_domain(
    project_id: &str,
    service_name: &str,
    domain: &str,
) -> Result<(), ConfigError> {
    let domain = domain.trim();
    if domain.is_empty() {
        return Err(ConfigError::InvalidProject(
            project_id.to_string(),
            format!("Service '{service_name}' domain cannot be empty"),
        ));
    }

    let (base_domain, wildcard) = split_service_domain(domain);
    if base_domain.starts_with("*.") {
        return Err(ConfigError::InvalidProject(
            project_id.to_string(),
            format!("Service '{service_name}' domain '{domain}' cannot contain nested wildcards"),
        ));
    }

    if !base_domain.ends_with(BRANDING.domain_suffix()) {
        return Err(ConfigError::InvalidProject(
            project_id.to_string(),
            format!(
                "Service '{service_name}' domain '{domain}' must end with {}",
                BRANDING.domain_suffix()
            ),
        ));
    }

    DomainPattern::from_name(base_domain, wildcard).map_err(|error| {
        ConfigError::InvalidProject(
            project_id.to_string(),
            format!("Service '{service_name}' domain '{domain}' is invalid: {error}"),
        )
    })?;

    Ok(())
}

fn split_service_domain(domain: &str) -> (&str, bool) {
    match domain.strip_prefix("*.") {
        Some(base_domain) => (base_domain, true),
        None => (domain, false),
    }
}

fn root_route_target(registration: &RegistrationDto) -> Option<RouteTarget> {
    registration
        .routes
        .iter()
        .find(|route| route.path().to_string() == "/")
        .map(|route| route.target().clone())
}

fn service_domains(primary_domain: &str, additional_domains: &[String]) -> Vec<String> {
    let mut domains = Vec::new();
    push_unique_domain(&mut domains, primary_domain);
    for domain in additional_domains {
        push_unique_domain(&mut domains, domain);
    }
    domains
}

fn push_unique_domain(domains: &mut Vec<String>, domain: &str) {
    let domain = domain.trim();
    if domain.is_empty() || domains.iter().any(|existing| existing == domain) {
        return;
    }
    domains.push(domain.to_string());
}

fn owned_domain_key(domain: &str) -> String {
    let domain = domain.trim();
    let (base_domain, _) = split_service_domain(domain);
    base_domain.to_ascii_lowercase()
}

fn infer_service_id(primary_domain: &str, project_id: &str) -> String {
    let (domain, _) = split_service_domain(primary_domain.trim());
    let project_namespace = project_id.trim();
    let without_suffix = domain
        .strip_suffix(BRANDING.domain_suffix())
        .unwrap_or(domain)
        .trim_end_matches('.');

    if without_suffix == project_namespace {
        return "web".to_string();
    }

    let namespace_suffix = format!(".{project_namespace}");
    if let Some(service_label) = without_suffix.strip_suffix(&namespace_suffix)
        && !service_label.is_empty()
        && !service_label.contains('.')
        && let Some(service_id) = normalized_label(service_label)
    {
        return service_id;
    }

    let fallback_label = without_suffix.split('.').next().unwrap_or(without_suffix);
    normalized_label(fallback_label)
        .or_else(|| normalized_label(without_suffix))
        .unwrap_or_else(|| "service".to_string())
}

fn unique_service_id(base_id: &str, used_ids: &mut HashSet<String>) -> String {
    let base_id = if base_id.is_empty() {
        "service"
    } else {
        base_id
    };

    if used_ids.insert(base_id.to_string()) {
        return base_id.to_string();
    }

    let mut index = 2;
    loop {
        let candidate = format!("{base_id}-{index}");
        if used_ids.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn normalized_label(value: &str) -> Option<String> {
    let mut slug = String::new();
    let mut previous_was_dash = false;

    for ch in value.trim().chars() {
        let ch = ch.to_ascii_lowercase();
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
    if slug.is_empty() { None } else { Some(slug) }
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

    pub fn find_registration_for_domain(
        &self,
        domain: &str,
    ) -> Result<Option<DomainRegistration>, ConfigError> {
        let config = self.load()?;
        Ok(config
            .registration_for_domain(domain)
            .cloned()
            .map(DomainRegistration::from))
    }

    #[allow(dead_code)]
    pub fn domain_project_id(
        &self,
        pattern: &DomainPattern,
    ) -> Result<Option<String>, ConfigError> {
        let config = self.load()?;
        let key = pattern.display_pattern();
        Ok(config.domains.get(&key).and_then(|dto| dto.project.clone()))
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

        let mut dto = RegistrationDto::from(registration);
        dto.project = project_id;
        config.domains.remove(&current_key);
        config.domains.insert(new_key.clone(), dto);
        if current_key != new_key {
            rename_project_domain_references(&mut config, &current_key, &new_key);
        }
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
    pub fn list_services(&self) -> Result<Vec<ConfiguredService>, ConfigError> {
        Ok(self.load()?.configured_services())
    }

    #[allow(dead_code)]
    pub fn list_project_environments(
        &self,
        project: Option<&str>,
    ) -> Result<Vec<ConfiguredProjectEnvironment>, ConfigError> {
        Ok(self.load()?.configured_project_environments(project))
    }

    #[allow(dead_code)]
    pub fn upsert_project_environment(
        &self,
        project_id: &str,
        environment: ProjectEnvironmentConfig,
    ) -> Result<ConfiguredProjectEnvironment, ConfigError> {
        let mut config = self.load()?;
        let project = config
            .projects
            .get_mut(project_id)
            .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;

        let slug = environment.slug.clone();
        let dto = environment_config_to_dto(environment);
        if dto.primary {
            project
                .environments
                .retain(|existing| !existing.primary && existing.slug != slug);
        } else {
            project
                .environments
                .retain(|existing| existing.slug != slug);
        }
        project.environments.push(dto);

        config.validate()?;
        self.save(&config)?;

        self.list_project_environments(Some(project_id))?
            .into_iter()
            .find(|environment| environment.slug == slug)
            .ok_or_else(|| {
                ConfigError::InvalidProject(
                    project_id.to_string(),
                    format!("Environment '{slug}' was not persisted"),
                )
            })
    }

    #[allow(dead_code)]
    pub fn remove_project_environment(
        &self,
        project_id: &str,
        environment_slug: &str,
    ) -> Result<ConfiguredProjectEnvironment, ConfigError> {
        let mut config = self.load()?;
        let result = {
            let project = config
                .projects
                .get_mut(project_id)
                .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;
            let environment_index = project
                .environments
                .iter()
                .position(|environment| environment.slug == environment_slug)
                .ok_or_else(|| {
                    ConfigError::InvalidProject(
                        project_id.to_string(),
                        format!("Environment '{environment_slug}' was not found"),
                    )
                })?;

            if project.environments[environment_index].primary {
                return Err(ConfigError::InvalidProject(
                    project_id.to_string(),
                    "The primary environment cannot be removed".into(),
                ));
            }

            let removed = project.environments.remove(environment_index);
            configured_environment(project_id, project, &removed)
        };

        config.validate()?;
        self.save(&config)?;
        Ok(result)
    }

    #[allow(dead_code)]
    pub fn replace_project_environment_service_domain(
        &self,
        project_id: &str,
        update: ProjectEnvironmentServiceDomainUpdate,
    ) -> Result<(), ConfigError> {
        let mut config = self.load()?;
        let current_key = update.current_domain.trim().to_string();
        let new_key = update.new_registration.display_pattern();

        let existing_domain = config
            .domains
            .get(&current_key)
            .ok_or_else(|| ConfigError::DomainNotFound(current_key.clone()))?;
        if existing_domain.project.as_deref() != Some(project_id) {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Domain '{current_key}' is not attached to project '{project_id}'"),
            ));
        }
        if current_key != new_key && config.domains.contains_key(&new_key) {
            return Err(ConfigError::DomainExists(new_key));
        }

        let project = config
            .projects
            .get_mut(project_id)
            .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;
        let environment = project
            .environments
            .iter_mut()
            .find(|environment| environment.slug == update.environment_slug)
            .ok_or_else(|| {
                ConfigError::InvalidProject(
                    project_id.to_string(),
                    format!(
                        "Environment '{}' is not stored. Run 'localify project env ensure' first.",
                        update.environment_slug
                    ),
                )
            })?;
        let service = environment
            .services
            .iter_mut()
            .find(|service| service.service == update.service_id)
            .ok_or_else(|| {
                ConfigError::InvalidProject(
                    project_id.to_string(),
                    format!(
                        "Service '{}' is not available in environment '{}'",
                        update.service_id, update.environment_slug
                    ),
                )
            })?;
        if service.domain != current_key {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!(
                    "Environment '{}' service '{}' domain is '{}', not '{current_key}'",
                    update.environment_slug, update.service_id, service.domain
                ),
            ));
        }

        service.domain = new_key.clone();

        let mut dto = RegistrationDto::from(update.new_registration);
        dto.project = Some(project_id.to_string());
        config.domains.remove(&current_key);
        config.domains.insert(new_key, dto);

        config.validate()?;
        self.save(&config)
    }

    #[allow(dead_code)]
    pub fn set_project_services(
        &self,
        project_id: &str,
        services: Vec<ProjectServiceConfig>,
    ) -> Result<(), ConfigError> {
        let mut config = self.load()?;
        let project = config
            .projects
            .get_mut(project_id)
            .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;
        project.services = services.into_iter().map(service_config_to_dto).collect();
        config.validate()?;
        self.save(&config)
    }

    #[allow(dead_code)]
    pub fn add_project_service(
        &self,
        project_id: &str,
        service: ProjectServiceConfig,
    ) -> Result<(), ConfigError> {
        let mut config = self.load()?;
        let existing_services = {
            let project = config
                .projects
                .get(project_id)
                .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;
            if project.services.is_empty() {
                config
                    .inferred_services_for_project(project_id, project)
                    .into_iter()
                    .filter(|existing| existing.primary_domain != service.primary_domain)
                    .map(|existing| configured_service_to_config(&existing))
                    .collect::<Vec<_>>()
            } else {
                project
                    .services
                    .iter()
                    .map(service_dto_to_config)
                    .collect::<Vec<_>>()
            }
        };

        let project = config
            .projects
            .get_mut(project_id)
            .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;
        project.services = existing_services
            .into_iter()
            .chain(std::iter::once(service))
            .map(service_config_to_dto)
            .collect();
        config.validate()?;
        self.save(&config)
    }

    #[allow(dead_code)]
    pub fn replace_project_service_domain(
        &self,
        project_id: &str,
        service_id: &str,
        update: ProjectServiceDomainUpdate,
    ) -> Result<(), ConfigError> {
        let mut config = self.load()?;
        let current_key = update.current_domain.trim().to_string();
        let new_key = update.new_registration.display_pattern();

        let existing_domain = config
            .domains
            .get(&current_key)
            .ok_or_else(|| ConfigError::DomainNotFound(current_key.clone()))?;
        if existing_domain.project.as_deref() != Some(project_id) {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Domain '{current_key}' is not attached to project '{project_id}'"),
            ));
        }
        if current_key != new_key && config.domains.contains_key(&new_key) {
            return Err(ConfigError::DomainExists(new_key));
        }

        let mut services = {
            let project = config
                .projects
                .get(project_id)
                .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;
            if project.services.is_empty() {
                config
                    .inferred_services_for_project(project_id, project)
                    .into_iter()
                    .map(|service| configured_service_to_config(&service))
                    .collect::<Vec<_>>()
            } else {
                project
                    .services
                    .iter()
                    .map(service_dto_to_config)
                    .collect::<Vec<_>>()
            }
        };

        let Some(service) = services
            .iter_mut()
            .find(|service| normalized_label(&service.name).as_deref() == Some(service_id))
        else {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!("Service '{service_id}' was not found"),
            ));
        };
        if service.primary_domain != current_key {
            return Err(ConfigError::InvalidProject(
                project_id.to_string(),
                format!(
                    "Service '{service_id}' primary domain is '{}', not '{current_key}'",
                    service.primary_domain
                ),
            ));
        }

        service.primary_domain = new_key.clone();
        service
            .domains
            .retain(|domain| domain != &current_key && domain != &new_key);

        let mut dto = RegistrationDto::from(update.new_registration);
        dto.project = Some(project_id.to_string());
        config.domains.remove(&current_key);
        config.domains.insert(new_key, dto);

        let project = config
            .projects
            .get_mut(project_id)
            .ok_or_else(|| ConfigError::ProjectNotFound(project_id.to_string()))?;
        project.services = services.into_iter().map(service_config_to_dto).collect();

        config.validate()?;
        self.save(&config)
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

        config.projects.insert(
            project_id.clone(),
            ProjectDto {
                name: project_name.to_string(),
                enabled: true,
                root_path: root_path.clone(),
                services: Vec::new(),
                environments: Vec::new(),
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

/// Rewrites project references to a renamed domain key so services and
/// environment mappings keep pointing at the registration after a rename
/// (including wildcard toggles, which change the key).
fn rename_project_domain_references(config: &mut Config, current_key: &str, new_key: &str) {
    for project in config.projects.values_mut() {
        for service in &mut project.services {
            if service.primary_domain == current_key {
                service.primary_domain = new_key.to_string();
            }
            for domain in &mut service.domains {
                if domain == current_key {
                    *domain = new_key.to_string();
                }
            }
        }
        for environment in &mut project.environments {
            for service in &mut environment.services {
                if service.domain == current_key {
                    service.domain = new_key.to_string();
                }
            }
        }
    }
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
    fn load(&self) -> Result<(DaemonConfig, LocalifyPaths), ConfigLoadError> {
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
        let normalized = domain.replace(".localify", BRANDING.domain_suffix());
        let pattern = DomainPattern::from_name(&normalized, false).unwrap();
        let route = Route::parse(&format!("/={port}")).unwrap();
        DomainRegistration::new(pattern, vec![route])
    }

    fn wildcard_proxy_registration(domain: &str, port: u16) -> DomainRegistration {
        let normalized = domain.replace(".localify", BRANDING.domain_suffix());
        let pattern = DomainPattern::from_name(&normalized, true).unwrap();
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
            lan_access: false,
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
    fn a_public_url_belongs_to_the_project_that_registers_its_domain_or_owns_its_namespace() {
        let (_dir, store) = temp_store();
        let journal = store.create_project("Journal").unwrap();
        let shop = store.create_project("Shop").unwrap();
        store
            .add_domain_with_project(
                proxy_registration("web.journal.localify", 3000),
                Some(journal.id.clone()),
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration("shop.localify", 3001),
                Some(shop.id.clone()),
            )
            .unwrap();
        store
            .add_domain(proxy_registration("loose.localify", 3002))
            .unwrap();
        store
            .add_domain(proxy_registration("api.other.localify", 3003))
            .unwrap();
        let config = store.load().unwrap();
        let project = |domain: &str| {
            config.public_url_project(&domain.replace(".localify", BRANDING.domain_suffix()))
        };

        assert_eq!(project("web.journal.localify"), Some(journal.id.clone()));
        assert_eq!(project("journal.localify"), Some(journal.id.clone()));
        assert_eq!(project(" Journal.localify. "), Some(journal.id));
        assert_eq!(project("shop.localify"), Some(shop.id));
        assert_eq!(project("loose.localify"), None);
        assert_eq!(project("other.localify"), None);
        assert_eq!(project("api.journal.localify"), None);
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
                proxy_registration("app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain(proxy_registration("plain.localify", 4000))
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
                proxy_registration("app.localify", 3000),
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
    fn services_are_inferred_from_project_domains_without_explicit_services() {
        let (_dir, store) = temp_store();
        let project = store.create_project("ACME").unwrap();

        store
            .add_domain_with_project(
                proxy_registration("acme.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration("admin.acme.localify", 3001),
                Some(project.id.clone()),
            )
            .unwrap();

        let services = store.list_services().unwrap();

        assert_eq!(services.len(), 2);
        assert_eq!(services[0].id, "admin");
        assert_eq!(services[0].name, "admin");
        assert!(services[0].inferred);
        assert_eq!(
            services[0].primary_domain,
            format!("admin.acme{}", BRANDING.domain_suffix())
        );
        assert_eq!(services[1].id, "web");
        assert_eq!(services[1].name, "web");
        assert!(services[1].inferred);
        assert_eq!(
            services[1].primary_domain,
            format!("acme{}", BRANDING.domain_suffix())
        );
        assert!(matches!(
            services[1].primary_target,
            Some(RouteTarget::Proxy(_))
        ));
    }

    #[test]
    fn inferred_services_suffix_colliding_legacy_root_and_web_domains() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Demo").unwrap();

        store
            .add_domain_with_project(
                proxy_registration("demo.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration("web.demo.localify", 3001),
                Some(project.id.clone()),
            )
            .unwrap();

        let ids = store
            .list_services()
            .unwrap()
            .into_iter()
            .map(|service| service.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["web", "web-2"]);
    }

    #[test]
    fn inferred_services_ignore_non_primary_environment_domains() {
        let (_dir, store) = temp_store();
        let project = store.create_project("ACME").unwrap();
        let primary_domain = format!("acme{}", BRANDING.domain_suffix());
        let admin_domain = format!("admin.acme{}", BRANDING.domain_suffix());
        let env_domain = format!("test-visible-modif.acme{}", BRANDING.domain_suffix());
        let env_admin_domain =
            format!("admin--test-visible-modif.acme{}", BRANDING.domain_suffix());

        store
            .add_domain_with_project(
                proxy_registration(&primary_domain, 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration(&admin_domain, 3001),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "test/visible-modif".into(),
                    slug: "test-visible-modif".into(),
                    primary: false,
                    root_path: Some("/tmp/acme-test-visible-modif".into()),
                    git_common_dir: None,
                    git_branch: Some("test/visible-modif".into()),
                    services: vec![
                        ProjectEnvironmentServiceConfig {
                            service_id: "web".into(),
                            domain: env_domain.clone(),
                        },
                        ProjectEnvironmentServiceConfig {
                            service_id: "admin".into(),
                            domain: env_admin_domain.clone(),
                        },
                    ],
                },
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration(&env_domain, 4000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration(&env_admin_domain, 4001),
                Some(project.id.clone()),
            )
            .unwrap();

        let ids = store
            .list_services()
            .unwrap()
            .into_iter()
            .map(|service| service.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["admin", "web"]);
    }

    #[test]
    fn explicit_project_services_take_precedence_over_inference() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();

        store
            .add_domain_with_project(
                wildcard_proxy_registration("client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();

        let mut config = store.load().unwrap();
        config
            .projects
            .get_mut(&project.id)
            .unwrap()
            .services
            .push(ProjectServiceDto {
                name: "Frontend".to_string(),
                primary_domain: format!("client-app{}", BRANDING.domain_suffix()),
                domains: vec![format!("www.client-app{}", BRANDING.domain_suffix())],
                path: Some("apps/web".to_string()),
                command: Some(vec!["pnpm".to_string(), "dev".to_string()]),
            });
        store.save(&config).unwrap();

        let services = store.list_services().unwrap();

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "frontend");
        assert_eq!(services[0].name, "Frontend");
        assert!(!services[0].inferred);
        assert_eq!(
            services[0].domains,
            vec![
                format!("client-app{}", BRANDING.domain_suffix()),
                format!("www.client-app{}", BRANDING.domain_suffix())
            ]
        );
        assert_eq!(services[0].path.as_deref(), Some("apps/web"));
        assert_eq!(
            services[0].command.as_ref().unwrap(),
            &vec!["pnpm".to_string(), "dev".to_string()]
        );
        assert!(matches!(
            services[0].primary_target,
            Some(RouteTarget::Proxy(_))
        ));
    }

    #[test]
    fn add_project_service_preserves_legacy_inferred_services() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();

        store
            .add_domain_with_project(
                proxy_registration("client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration("admin.client-app.localify", 3001),
                Some(project.id.clone()),
            )
            .unwrap();

        store
            .add_project_service(
                &project.id,
                ProjectServiceConfig {
                    name: "api".into(),
                    primary_domain: format!("api.client-app{}", BRANDING.domain_suffix()),
                    domains: Vec::new(),
                    path: Some("apps/api".into()),
                    command: Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]),
                },
            )
            .unwrap();

        let services = store.list_services().unwrap();
        let ids = services
            .iter()
            .map(|service| service.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["admin", "api", "web"]);
        assert!(services.iter().all(|service| !service.inferred));
    }

    #[test]
    fn project_environments_include_implicit_primary_environment() {
        let (_dir, store) = temp_store();
        let project = store
            .create_project_with_root("Client App", Some("/tmp/client-app".into()))
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration("client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();

        let environments = store.list_project_environments(Some(&project.id)).unwrap();

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].slug, "primary");
        assert!(environments[0].primary);
        assert_eq!(
            environments[0].services[0].domain,
            format!("client-app{}", BRANDING.domain_suffix())
        );
        assert_eq!(
            environments[0].root_path.as_deref(),
            Some("/tmp/client-app")
        );
    }

    #[test]
    fn upsert_project_environment_persists_service_domains() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        store
            .add_domain_with_project(
                proxy_registration("client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();

        let environment = store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "feat/auth".into(),
                    slug: "feat-auth".into(),
                    primary: false,
                    root_path: Some("/tmp/client-app-feat-auth".into()),
                    git_common_dir: Some("/tmp/client-app/.git".into()),
                    git_branch: Some("feat/auth".into()),
                    services: vec![ProjectEnvironmentServiceConfig {
                        service_id: "web".into(),
                        domain: format!("feat-auth.client-app{}", BRANDING.domain_suffix()),
                    }],
                },
            )
            .unwrap();

        assert_eq!(environment.slug, "feat-auth");
        assert!(!environment.primary);
        assert_eq!(environment.services[0].service_id, "web");
        assert_eq!(
            environment.services[0].domain,
            format!("feat-auth.client-app{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn remove_project_environment_deletes_non_primary_environment() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        store
            .add_domain_with_project(
                proxy_registration("client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "feat/auth".into(),
                    slug: "feat-auth".into(),
                    primary: false,
                    root_path: Some("/tmp/client-app-feat-auth".into()),
                    git_common_dir: Some("/tmp/client-app/.git".into()),
                    git_branch: Some("feat/auth".into()),
                    services: vec![ProjectEnvironmentServiceConfig {
                        service_id: "web".into(),
                        domain: format!("feat-auth.client-app{}", BRANDING.domain_suffix()),
                    }],
                },
            )
            .unwrap();

        let removed = store
            .remove_project_environment(&project.id, "feat-auth")
            .unwrap();
        let environments = store.list_project_environments(Some(&project.id)).unwrap();

        assert_eq!(removed.slug, "feat-auth");
        assert!(
            environments
                .iter()
                .all(|environment| environment.slug != "feat-auth")
        );
    }

    #[test]
    fn remove_project_environment_rejects_primary_environment() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "Primary".into(),
                    slug: "primary".into(),
                    primary: true,
                    root_path: Some("/tmp/client-app".into()),
                    git_common_dir: None,
                    git_branch: None,
                    services: Vec::new(),
                },
            )
            .unwrap();

        let error = store
            .remove_project_environment(&project.id, "primary")
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("primary environment cannot be removed")
        );
    }

    #[test]
    fn replace_project_service_domain_updates_service_and_registration() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        let old_domain = format!("old.client-app{}", BRANDING.domain_suffix());

        store
            .add_domain_with_project(
                proxy_registration("old.client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_project_service(
                &project.id,
                ProjectServiceConfig {
                    name: "web".into(),
                    primary_domain: old_domain.clone(),
                    domains: Vec::new(),
                    path: Some("apps/web".into()),
                    command: None,
                },
            )
            .unwrap();

        store
            .replace_project_service_domain(
                &project.id,
                "web",
                ProjectServiceDomainUpdate {
                    current_domain: old_domain.clone(),
                    new_registration: proxy_registration("new.client-app.localify", 3001),
                },
            )
            .unwrap();

        let services = store.list_services().unwrap();
        assert_eq!(
            services[0].primary_domain,
            format!("new.client-app{}", BRANDING.domain_suffix())
        );
        assert!(
            store
                .get_domain(&DomainPattern::from_name(&old_domain, false).unwrap())
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_domain(
                    &DomainPattern::from_name(
                        &format!("new.client-app{}", BRANDING.domain_suffix()),
                        false
                    )
                    .unwrap()
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn replace_project_environment_service_domain_updates_mapping_and_registration() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        let primary_domain = format!("client-app{}", BRANDING.domain_suffix());
        let old_env_domain = format!("feat-auth.client-app{}", BRANDING.domain_suffix());

        store
            .add_domain_with_project(
                proxy_registration("client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .add_project_service(
                &project.id,
                ProjectServiceConfig {
                    name: "web".into(),
                    primary_domain,
                    domains: Vec::new(),
                    path: None,
                    command: None,
                },
            )
            .unwrap();
        store
            .add_domain_with_project(
                proxy_registration("feat-auth.client-app.localify", 3001),
                Some(project.id.clone()),
            )
            .unwrap();
        store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "feat/auth".into(),
                    slug: "feat-auth".into(),
                    primary: false,
                    root_path: None,
                    git_common_dir: None,
                    git_branch: None,
                    services: vec![ProjectEnvironmentServiceConfig {
                        service_id: "web".into(),
                        domain: old_env_domain.clone(),
                    }],
                },
            )
            .unwrap();

        store
            .replace_project_environment_service_domain(
                &project.id,
                ProjectEnvironmentServiceDomainUpdate {
                    environment_slug: "feat-auth".into(),
                    service_id: "web".into(),
                    current_domain: old_env_domain,
                    new_registration: proxy_registration("preview.client-app.localify", 3002),
                },
            )
            .unwrap();

        let environments = store.list_project_environments(Some(&project.id)).unwrap();
        let environment = environments
            .iter()
            .find(|environment| environment.slug == "feat-auth")
            .unwrap();
        assert_eq!(
            environment.services[0].domain,
            format!("preview.client-app{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn project_environment_rejects_unknown_service() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        store
            .add_domain_with_project(
                proxy_registration("client-app.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();

        let error = store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "feat-auth".into(),
                    slug: "feat-auth".into(),
                    primary: false,
                    root_path: None,
                    git_common_dir: None,
                    git_branch: None,
                    services: vec![ProjectEnvironmentServiceConfig {
                        service_id: "api".into(),
                        domain: format!("api--feat-auth.client-app{}", BRANDING.domain_suffix()),
                    }],
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("unknown service"));
    }

    #[test]
    fn explicit_services_cannot_share_the_same_domain_owner() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();

        let mut config = store.load().unwrap();
        config.projects.get_mut(&project.id).unwrap().services = vec![
            ProjectServiceDto {
                name: "Web".to_string(),
                primary_domain: format!("client-app{}", BRANDING.domain_suffix()),
                domains: Vec::new(),
                path: None,
                command: None,
            },
            ProjectServiceDto {
                name: "Docs".to_string(),
                primary_domain: format!("*.client-app{}", BRANDING.domain_suffix()),
                domains: Vec::new(),
                path: None,
                command: None,
            },
        ];

        let error = config.validate().unwrap_err();

        assert!(error.to_string().contains("already owned"));
    }

    #[test]
    fn explicit_service_domains_must_be_localify_domains() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();

        let mut config = store.load().unwrap();
        config
            .projects
            .get_mut(&project.id)
            .unwrap()
            .services
            .push(ProjectServiceDto {
                name: "Web".to_string(),
                primary_domain: "client-app.test".to_string(),
                domains: Vec::new(),
                path: None,
                command: None,
            });

        let error = config.validate().unwrap_err();

        assert!(error.to_string().contains("must end with"));
    }

    #[test]
    fn explicit_service_domains_reject_nested_wildcards() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();

        let mut config = store.load().unwrap();
        config
            .projects
            .get_mut(&project.id)
            .unwrap()
            .services
            .push(ProjectServiceDto {
                name: "Web".to_string(),
                primary_domain: format!("*.*.client-app{}", BRANDING.domain_suffix()),
                domains: Vec::new(),
                path: None,
                command: None,
            });

        let error = config.validate().unwrap_err();

        assert!(error.to_string().contains("nested wildcards"));
    }

    #[test]
    fn replace_domain_updates_key_and_preserves_project() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Client App").unwrap();
        let current = proxy_registration("old.localify", 3000);
        let current_pattern = current.pattern().clone();

        store
            .add_domain_with_project(current, Some(project.id.clone()))
            .unwrap();

        store
            .replace_domain_with_project(
                &current_pattern,
                proxy_registration("new.localify", 3001),
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
    fn replace_domain_renames_environment_service_references() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Store").unwrap();
        let primary = proxy_registration("store.localify", 61092);
        let primary_domain = primary.display_pattern();
        let current = proxy_registration("feat-inventory.store.localify", 55931);
        let current_pattern = current.pattern().clone();
        let current_domain = current.display_pattern();

        store
            .add_domain_with_project(primary, Some(project.id.clone()))
            .unwrap();
        store
            .add_domain_with_project(current, Some(project.id.clone()))
            .unwrap();
        store
            .set_project_services(
                &project.id,
                vec![ProjectServiceConfig {
                    name: "web".into(),
                    primary_domain,
                    domains: Vec::new(),
                    path: None,
                    command: None,
                }],
            )
            .unwrap();
        store
            .upsert_project_environment(
                &project.id,
                ProjectEnvironmentConfig {
                    name: "feat-inventory".into(),
                    slug: "feat-inventory".into(),
                    primary: false,
                    root_path: Some("/tmp/store-feat-inventory".into()),
                    git_common_dir: None,
                    git_branch: Some("feat-inventory".into()),
                    services: vec![ProjectEnvironmentServiceConfig {
                        service_id: "web".into(),
                        domain: current_domain,
                    }],
                },
            )
            .unwrap();

        store
            .replace_domain_with_project(
                &current_pattern,
                wildcard_proxy_registration("feat-inventory.store.localify", 55931),
                Some(project.id.clone()),
            )
            .unwrap();

        let environments = store.list_project_environments(Some(&project.id)).unwrap();
        let environment = environments
            .iter()
            .find(|environment| environment.slug == "feat-inventory")
            .unwrap();
        assert_eq!(
            environment.services[0].domain,
            format!("*.feat-inventory.store{}", BRANDING.domain_suffix())
        );
    }

    #[test]
    fn replace_domain_renames_project_service_references() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Store").unwrap();
        let current = proxy_registration("web.store.localify", 3000);
        let current_pattern = current.pattern().clone();
        let current_domain = current.display_pattern();

        store
            .add_domain_with_project(current, Some(project.id.clone()))
            .unwrap();
        store
            .set_project_services(
                &project.id,
                vec![ProjectServiceConfig {
                    name: "web".into(),
                    primary_domain: current_domain.clone(),
                    domains: vec![current_domain],
                    path: Some("apps/web".into()),
                    command: None,
                }],
            )
            .unwrap();

        store
            .replace_domain_with_project(
                &current_pattern,
                wildcard_proxy_registration("web.store.localify", 3000),
                Some(project.id.clone()),
            )
            .unwrap();

        let services = store.list_services().unwrap();
        let renamed = format!("*.web.store{}", BRANDING.domain_suffix());
        assert_eq!(services[0].primary_domain, renamed);
        assert_eq!(services[0].domains, vec![renamed]);
    }

    #[test]
    fn replace_domain_rejects_duplicate_without_removing_current() {
        let (_dir, store) = temp_store();
        let current = proxy_registration("old.localify", 3000);
        let current_pattern = current.pattern().clone();

        store.add_domain(current).unwrap();
        store
            .add_domain(proxy_registration("existing.localify", 3001))
            .unwrap();

        let error = store
            .replace_domain_with_project(
                &current_pattern,
                proxy_registration("existing.localify", 3002),
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
                proxy_registration("app.localify", 3000),
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
    fn domains_per_project_are_unlimited() {
        let (_dir, store) = temp_store();
        let project = store.create_project("Demo").unwrap();

        for index in 0..10 {
            store
                .add_domain_with_project(
                    proxy_registration(&format!("app{index}.localify"), 3000 + index),
                    Some(project.id.clone()),
                )
                .unwrap();
        }

        assert_eq!(store.load().unwrap().domains.len(), 10);
    }

    /// `config.toml` as written by a release that sold licenses: six projects
    /// (over the old free limit of five) and a full licensing section.
    fn write_config_from_licensed_release(dir: &tempfile::TempDir) {
        let projects = (1..=6)
            .map(|index| format!("[projects.project-{index}]\nname = \"Project {index}\"\n"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            dir.path().join("config.toml"),
            format!(
                r#"[daemon]
http_port = 8080

[licensing.keygen]
apiUrl = "https://licences.localify.dev"
account = "account_123"
product = "product_123"
publicKey = "public_key_123"

[licensing.license]
key = "TEST-KEY-1234"
licenseId = "lic_123"
plan = "Free"
maxProjects = 5

[licensing.offlineLicense]
certificate = "-----BEGIN LICENSE FILE-----"
algorithm = "aes-256-gcm+ed25519"

{projects}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn config_with_licensing_section_keeps_every_project() {
        let (dir, store) = temp_store();
        write_config_from_licensed_release(&dir);

        let config = store.load().unwrap();

        assert_eq!(config.daemon.http_port, 8080);
        assert_eq!(store.list_projects().unwrap().len(), 6);
    }

    #[test]
    fn config_with_licensing_section_accepts_more_projects() {
        let (dir, store) = temp_store();
        write_config_from_licensed_release(&dir);

        store.create_project("Seven").unwrap();
        store.create_project("Eight").unwrap();

        assert_eq!(store.list_projects().unwrap().len(), 8);
    }

    #[test]
    fn saving_config_drops_the_stale_licensing_section() {
        let (dir, store) = temp_store();
        write_config_from_licensed_release(&dir);

        store.create_project("Seven").unwrap();

        let content = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(!content.contains("licensing"));
        assert!(!content.contains("TEST-KEY-1234"));
    }

    #[test]
    fn config_loads_external_wildcard_domains() {
        let (dir, store) = temp_store();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[domains."*.store.example-app.io"]
pattern = "*.store.example-app.io"
https_enabled = true

[[domains."*.store.example-app.io".routes]]
path = "/"
target = "127.0.0.1:61092"
"#,
        )
        .unwrap();

        let config = store.load().unwrap();
        let registration = config.registrations().into_iter().next().unwrap();

        assert_eq!(
            registration.pattern().display_pattern(),
            "*.store.example-app.io"
        );
    }

    #[test]
    fn every_project_owns_its_namespace() {
        let toml = r#"
[projects.journal]
name = "Journal"

[projects.paused]
name = "Paused"
enabled = false
"#;
        let config: Config = toml::from_str(toml).unwrap();

        let exposable = config.exposable_domains();

        assert_eq!(
            exposable.get(&format!("journal{}", BRANDING.domain_suffix())),
            Some(&"journal".to_string())
        );
        // A paused project's URLs can be created; the daemon serves only
        // enabled projects' domains.
        assert_eq!(exposable.len(), 2);
    }
}
