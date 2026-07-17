use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use localify::desktop::ProjectRootStore;

use crate::branding::BRANDING;
use crate::infrastructure::{
    certs::CertificateService, config::ConfigStore, mgmt_client::MgmtSocketClient,
    paths::LocalifyPaths, pid::PidFile,
};

/// Composition root: wires infrastructure adapters once,
/// shared by all CLI commands that need them.
pub struct AppContext {
    pub config_path: PathBuf,
    pub config_store: ConfigStore,
    pub cert_service: CertificateService,
    pub pid_file: PidFile,
    pub mgmt_client: MgmtSocketClient,
}

impl AppContext {
    pub fn new(config_path: &Path, paths: &LocalifyPaths) -> Self {
        Self {
            config_path: config_path.to_path_buf(),
            config_store: ConfigStore::new(config_path.to_path_buf()),
            cert_service: CertificateService::new(paths),
            pid_file: PidFile::new(paths.pid_file.clone()),
            mgmt_client: MgmtSocketClient::new(&paths.socket_path),
        }
    }
}

/// Resolve the real project directory when a command is launched from an
/// agent-owned project cache instead of the user's checkout.
pub fn effective_current_dir() -> Result<PathBuf> {
    let cwd = env::current_dir().context("Failed to determine the current directory")?;
    match resolve_agent_project_cache_dir(&cwd)? {
        Some(path) => Ok(path),
        None => Ok(cwd),
    }
}

struct AgentProjectCacheMatch {
    provider_label: &'static str,
    cache_root: PathBuf,
    suffix: PathBuf,
}

struct AgentProjectCacheProvider {
    label: &'static str,
    projects_dir: PathBuf,
}

fn resolve_agent_project_cache_dir(cwd: &Path) -> Result<Option<PathBuf>> {
    let Some(agent_cache) = agent_project_cache_root(cwd)? else {
        return Ok(None);
    };

    if let Some(real_root) = real_project_root_from_store(&agent_cache.cache_root)? {
        return Ok(Some(real_root.join(agent_cache.suffix)));
    }

    if let Some(real_root) = decode_existing_hyphen_path_project_root(&agent_cache.cache_root)? {
        return Ok(Some(real_root.join(agent_cache.suffix)));
    }

    bail!(
        "This command is running from a {} agent project cache at '{}', not from the real project checkout.\n\
         Run it from the project folder, or initialize/link the project once with '{} init-project --yes'.",
        agent_cache.provider_label,
        agent_cache.cache_root.display(),
        BRANDING.cli_command()
    )
}

fn agent_project_cache_root(cwd: &Path) -> Result<Option<AgentProjectCacheMatch>> {
    let Some(home) = home_dir_from_env() else {
        return Ok(None);
    };

    for provider in agent_project_cache_providers(&home) {
        if !cwd.starts_with(&provider.projects_dir) {
            continue;
        }
        let relative = cwd.strip_prefix(&provider.projects_dir).with_context(|| {
            format!(
                "Failed to inspect {} agent project cache path '{}'",
                provider.label,
                cwd.display()
            )
        })?;
        let mut components = relative.components();
        let Some(first) = components.next() else {
            return Ok(None);
        };
        let encoded = PathBuf::from(first.as_os_str());
        return Ok(Some(AgentProjectCacheMatch {
            provider_label: provider.label,
            cache_root: provider.projects_dir.join(encoded),
            suffix: components.as_path().to_path_buf(),
        }));
    }

    Ok(None)
}

fn agent_project_cache_providers(home: &Path) -> Vec<AgentProjectCacheProvider> {
    vec![AgentProjectCacheProvider {
        label: "Claude",
        projects_dir: home.join(".claude").join("projects"),
    }]
}

fn real_project_root_from_store(agent_root: &Path) -> Result<Option<PathBuf>> {
    let roots = ProjectRootStore::new_default()?.load()?;
    let Some(agent_name) = agent_root.file_name() else {
        return Ok(None);
    };
    Ok(roots.into_values().map(PathBuf::from).find(|root| {
        hyphen_path_cache_name_for_root(root)
            .as_deref()
            .is_some_and(|cache_name| cache_name == agent_name)
    }))
}

fn hyphen_path_cache_name_for_root(root: &Path) -> Option<std::ffi::OsString> {
    let display = root.display().to_string();
    Some(std::ffi::OsString::from(display.replace('/', "-")))
}

fn decode_existing_hyphen_path_project_root(agent_root: &Path) -> Result<Option<PathBuf>> {
    let Some(encoded) = agent_root.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let parts = encoded
        .trim_start_matches('-')
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Ok(None);
    }

    Ok(resolve_existing_path_segments(Path::new("/"), &parts))
}

fn resolve_existing_path_segments(base: &Path, parts: &[&str]) -> Option<PathBuf> {
    if parts.is_empty() {
        return Some(base.to_path_buf());
    }

    for end in (1..=parts.len()).rev() {
        let candidate = base.join(parts[..end].join("-"));
        if !candidate.exists() {
            continue;
        }
        if let Some(path) = resolve_existing_path_segments(&candidate, &parts[end..]) {
            return Some(path);
        }
    }

    None
}

fn home_dir_from_env() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_agent_project_cache_with_hyphenated_folder_names() {
        let temp_dir = tempfile::tempdir().unwrap();
        let real_root = temp_dir.path().join("projects").join("dev-playground-2");
        std::fs::create_dir_all(&real_root).unwrap();
        let encoded = temp_dir
            .path()
            .join(real_root.display().to_string().replace('/', "-"));

        let decoded = decode_existing_hyphen_path_project_root(&encoded).unwrap();

        assert_eq!(decoded.as_deref(), Some(real_root.as_path()));
    }
}
